//! Workspace sidebar: one row per workspace with git, cwd, ports, agent
//! status and unread badge. Rows support click, drag-to-reorder and a
//! context menu.

use super::AppRc;
use crate::model::WorkspaceId;
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib, Orientation};

fn row_id(row: &gtk4::ListBoxRow) -> Option<WorkspaceId> {
    row.widget_name().parse().ok()
}

pub fn connect(app: &AppRc) {
    let w = app.weak();
    app.sidebar_list.connect_row_selected(move |_, row| {
        let (Some(a), Some(row)) = (w.upgrade(), row) else { return };
        if a.suppress_select.get() {
            return;
        }
        if let Some(id) = row_id(row) {
            let w = a.weak();
            // Defer: selecting rebuilds the list we're inside of.
            glib::idle_add_local_once(move || {
                if let Some(a) = w.upgrade() {
                    if a.inner.borrow().state.active != Some(id) {
                        a.select_workspace(id);
                    }
                }
            });
        }
    });
}

/// Keep row and button identity across metadata refreshes so an in-flight
/// click, double click, or drag is not cancelled by replacing its widget.
pub(super) struct Row {
    widget: gtk4::ListBoxRow,
    name: gtk4::Label,
    badge: gtk4::Label,
    context: gtk4::Label,
    ports: gtk4::Label,
    agent: gtk4::Label,
    attention: gtk4::Label,
}

impl Row {
    fn new(app: &AppRc, id: WorkspaceId) -> Self {
        let widget = gtk4::ListBoxRow::new();
        widget.set_widget_name(&id.to_string());
        let v = gtk4::Box::new(Orientation::Vertical, 2);
        let top = gtk4::Box::new(Orientation::Horizontal, 6);
        let handle = gtk4::Image::from_icon_name("list-drag-handle-symbolic");
        handle.add_css_class("dim-label");
        handle.set_tooltip_text(Some("Drag to reorder workspace"));
        handle.set_cursor_from_name(Some("grab"));
        top.append(&handle);
        let name = gtk4::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .build();
        name.add_css_class("ws-name");
        top.append(&name);
        let badge = gtk4::Label::new(None);
        badge.add_css_class("badge");
        badge.set_valign(gtk4::Align::Center);
        top.append(&badge);
        let close = gtk4::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Close workspace")
            .valign(gtk4::Align::Center)
            .build();
        close.add_css_class("flat");
        close.add_css_class("circular");
        close.add_css_class("ws-close");
        let w = app.weak();
        close.connect_clicked(move |_| {
            if let Some(a) = w.upgrade() {
                glib::idle_add_local_once(move || {
                    let _ = a.close_workspace(id, false, true);
                });
            }
        });
        top.append(&close);
        v.append(&top);
        let label = |class, ellipsize| {
            let l = gtk4::Label::builder().xalign(0.0).ellipsize(ellipsize).build();
            l.add_css_class(class);
            v.append(&l);
            l
        };
        let context = label("ws-sub", gtk4::pango::EllipsizeMode::Middle);
        let ports = label("ws-sub", gtk4::pango::EllipsizeMode::None);
        let agent = label("ws-agent", gtk4::pango::EllipsizeMode::End);
        let attention = label("ws-attention", gtk4::pango::EllipsizeMode::End);
        widget.set_child(Some(&v));
        attach_dnd(app, &widget, id);
        attach_menu(app, &widget, id);
        Self {
            widget,
            name,
            badge,
            context,
            ports,
            agent,
            attention,
        }
    }
}

fn set_optional_label(label: &gtk4::Label, text: &str) {
    if label.text() != text {
        label.set_label(text);
    }
    label.set_visible(!text.is_empty());
}

pub fn rebuild(app: &AppRc) {
    let list = &app.sidebar_list;
    app.suppress_select.set(true);
    let i = app.inner.borrow();
    let mut rows = app.sidebar_rows.borrow_mut();
    rows.retain(|id, row| {
        if i.state.workspace(*id).is_some() {
            true
        } else {
            list.remove(&row.widget);
            false
        }
    });
    let mut active_row = None;
    for (index, ws) in i.state.workspaces.iter().enumerate() {
        let row = rows.entry(ws.id).or_insert_with(|| Row::new(app, ws.id));
        // Only additions and explicit reordering reparent rows.
        if list.row_at_index(index as i32).as_ref() != Some(&row.widget) {
            if row.widget.parent().is_some() {
                list.remove(&row.widget);
            }
            list.insert(&row.widget, index as i32);
        }
        let focused = i.panes.get(&ws.focused);
        set_optional_label(&row.name, &ws.name);
        let unread = i.notes.unread_for_workspace(ws.id);
        set_optional_label(&row.badge, &if unread > 0 { unread.to_string() } else { String::new() });
        let mut sub = Vec::new();
        if let Some(g) = focused.and_then(|p| p.git.as_ref()) {
            let mut s = format!("⎇ {}", g.branch);
            if g.changes > 0 {
                s.push_str(&format!(" ●{}", g.changes));
            }
            sub.push(s);
        }
        if let Some(c) = focused.and_then(|p| p.cwd.as_ref()) {
            sub.push(crate::meta::display_path(c));
        }
        set_optional_label(&row.context, &sub.join("  ·  "));
        let mut ports: Vec<u16> = ws
            .layout
            .panes()
            .iter()
            .filter_map(|id| i.panes.get(id))
            .flat_map(|p| p.ports.clone())
            .collect();
        ports.sort_unstable();
        ports.dedup();
        let ports = if ports.is_empty() {
            String::new()
        } else {
            format!("ports {}", ports.iter().map(|p| format!(":{p}")).collect::<Vec<_>>().join(" "))
        };
        set_optional_label(&row.ports, &ports);
        let agent = ws
            .layout
            .panes()
            .iter()
            .filter_map(|id| i.panes.get(id))
            .find_map(|p| p.agent_label())
            .unwrap_or_default();
        set_optional_label(&row.agent, &agent);
        let attention = i
            .notes
            .latest_for_workspace(ws.id)
            .map(|n| {
                if n.body.is_empty() {
                    n.title.clone()
                } else {
                    format!("{}: {}", n.title, n.body)
                }
            })
            .unwrap_or_default();
        set_optional_label(&row.attention, attention.lines().next().unwrap_or(""));
        row.widget.set_tooltip_text(Some(&format!(
            "{} — {} pane(s)\nClick to switch · Drag to reorder · Double-click to rename",
            ws.name,
            ws.layout.panes().len()
        )));
        if i.state.active == Some(ws.id) {
            active_row = Some(row.widget.clone());
        }
    }
    drop(rows);
    drop(i);
    if let Some(r) = active_row {
        list.select_row(Some(&r));
    }
    app.suppress_select.set(false);
}

fn attach_dnd(app: &AppRc, row: &gtk4::ListBoxRow, id: WorkspaceId) {
    let src = gtk4::DragSource::new();
    src.set_actions(gdk::DragAction::MOVE);
    src.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&id.to_string().to_value())));
    row.add_controller(src);

    let drop = gtk4::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
    let w = app.weak();
    drop.connect_drop(move |_, value, _, _| {
        let (Some(a), Ok(s)) = (w.upgrade(), value.get::<String>()) else {
            return false;
        };
        let Ok(dragged) = s.parse::<WorkspaceId>() else { return false };
        let to = a.inner.borrow().state.workspaces.iter().position(|w| w.id == id);
        if let Some(to) = to {
            let w = a.weak();
            glib::idle_add_local_once(move || {
                if let Some(a) = w.upgrade() {
                    let _ = a.move_workspace(dragged, to);
                }
            });
            return true;
        }
        false
    });
    row.add_controller(drop);
}

fn attach_menu(app: &AppRc, row: &gtk4::ListBoxRow, id: WorkspaceId) {
    let menu = gio::Menu::new();
    let t = Some(id.to_variant());
    for (label, action) in [
        ("Rename…", "app.ws-rename"),
        ("Move Up", "app.ws-move-up"),
        ("Move Down", "app.ws-move-down"),
        ("Close Workspace", "app.ws-close"),
    ] {
        let it = gio::MenuItem::new(Some(label), None);
        it.set_action_and_target_value(Some(action), t.as_ref());
        menu.append_item(&it);
    }
    let click = gtk4::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    let list = app.sidebar_list.clone();
    let row_w = row.clone();
    click.connect_pressed(move |_, _, x, y| {
        // Parent to the long-lived list (rows are rebuilt often) and drop on close.
        let pop = gtk4::PopoverMenu::from_model(Some(&menu));
        pop.set_parent(&list);
        pop.set_has_arrow(false);
        let (px, py) = row_w
            .compute_point(&list, &gtk4::graphene::Point::new(x as f32, y as f32))
            .map(|p| (p.x() as f64, p.y() as f64))
            .unwrap_or((x, y));
        pop.set_pointing_to(Some(&gdk::Rectangle::new(px as i32, py as i32, 1, 1)));
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    });
    row.add_controller(click);
    let dbl = gtk4::GestureClick::new();
    let w = app.weak();
    dbl.connect_pressed(move |_, n, _, _| {
        if n == 2 {
            if let Some(a) = w.upgrade() {
                super::dialogs::rename_workspace(&a, id);
            }
        }
    });
    row.add_controller(dbl);
}
