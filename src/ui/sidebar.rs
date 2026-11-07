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

pub fn rebuild(app: &AppRc) {
    let list = &app.sidebar_list;
    app.suppress_select.set(true);
    // Remove rows only: popovers parented to the list are children too, and
    // ListBox::remove ignores them, so a first_child() loop would never end.
    while let Some(c) = list.row_at_index(0) {
        list.remove(&c);
    }
    let i = app.inner.borrow();
    let mut active_row = None;
    for ws in &i.state.workspaces {
        let focused = i.panes.get(&ws.focused);
        let row = gtk4::ListBoxRow::new();
        row.set_widget_name(&ws.id.to_string());

        let v = gtk4::Box::new(Orientation::Vertical, 2);
        let top = gtk4::Box::new(Orientation::Horizontal, 6);
        let name = gtk4::Label::builder()
            .label(&ws.name)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .build();
        name.add_css_class("ws-name");
        top.append(&name);
        let unread = i.notes.unread_for_workspace(ws.id);
        if unread > 0 {
            let badge = gtk4::Label::new(Some(&unread.to_string()));
            badge.add_css_class("badge");
            badge.set_valign(gtk4::Align::Center);
            top.append(&badge);
        }
        v.append(&top);

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
        if !sub.is_empty() {
            let l = gtk4::Label::builder()
                .label(sub.join("  ·  "))
                .xalign(0.0)
                .ellipsize(gtk4::pango::EllipsizeMode::Middle)
                .build();
            l.add_css_class("ws-sub");
            v.append(&l);
        }
        let ports: Vec<u16> = {
            let mut p: Vec<u16> = ws
                .layout
                .panes()
                .iter()
                .filter_map(|id| i.panes.get(id))
                .flat_map(|p| p.ports.clone())
                .collect();
            p.sort_unstable();
            p.dedup();
            p
        };
        if !ports.is_empty() {
            let l = gtk4::Label::builder()
                .label(format!(
                    "ports {}",
                    ports.iter().map(|p| format!(":{p}")).collect::<Vec<_>>().join(" ")
                ))
                .xalign(0.0)
                .build();
            l.add_css_class("ws-sub");
            v.append(&l);
        }
        // Agent status for any pane in the workspace (first one with a label).
        if let Some(label) = ws
            .layout
            .panes()
            .iter()
            .filter_map(|id| i.panes.get(id))
            .find_map(|p| p.agent_label())
        {
            let l = gtk4::Label::builder()
                .label(label)
                .xalign(0.0)
                .ellipsize(gtk4::pango::EllipsizeMode::End)
                .build();
            l.add_css_class("ws-agent");
            v.append(&l);
        }
        if let Some(n) = i.notes.latest_for_workspace(ws.id) {
            let text = if n.body.is_empty() {
                n.title.clone()
            } else {
                format!("{}: {}", n.title, n.body)
            };
            let l = gtk4::Label::builder()
                .label(text.lines().next().unwrap_or(""))
                .xalign(0.0)
                .ellipsize(gtk4::pango::EllipsizeMode::End)
                .build();
            l.add_css_class("ws-attention");
            v.append(&l);
        }
        row.set_child(Some(&v));
        row.set_tooltip_text(Some(&format!("{} — {} pane(s)", ws.name, ws.layout.panes().len())));

        attach_dnd(app, &row, ws.id);
        attach_menu(app, &row, ws.id);
        list.append(&row);
        if i.state.active == Some(ws.id) {
            active_row = Some(row);
        }
    }
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
