//! Workspace sidebar: one row per workspace with its folder, a line per
//! AI-agent tab and an unread badge. Rows support click, drag-to-reorder and a
//! context menu.

use super::AppRc;
use crate::agents::AgentState;
use crate::model::{TabId, WorkspaceId};
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib, Orientation};
use std::collections::HashMap;

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

    // Double click renames. One gesture on the whole list (not one per row) so
    // a press on another row resets the press count: quickly clicking A, B, A
    // must switch workspaces three times, not open "rename A".
    let dbl = gtk4::GestureClick::new();
    let w = app.weak();
    let list = app.sidebar_list.clone();
    dbl.connect_pressed(move |_, n, _, y| {
        if n != 2 {
            return;
        }
        let (Some(a), Some(id)) = (w.upgrade(), list.row_at_y(y as i32).as_ref().and_then(row_id)) else {
            return;
        };
        super::dialogs::rename_workspace(&a, id);
    });
    app.sidebar_list.add_controller(dbl);
}

/// Keep row and button identity across metadata refreshes so an in-flight
/// click, double click, or drag is not cancelled by replacing its widget.
pub(super) struct Row {
    widget: gtk4::ListBoxRow,
    /// Colour bar at the left edge (hidden without a colour).
    stripe: gtk4::Box,
    icon: gtk4::Image,
    name: gtk4::Label,
    badge: gtk4::Label,
    context: gtk4::Label,
    /// One button per tab running an AI agent.
    sessions: gtk4::Box,
    session_lines: HashMap<TabId, SessionLine>,
    more: gtk4::Label,
    attention: gtk4::Label,
}

/// Most tab lines shown per workspace; the rest become "+N more".
const MAX_SESSION_LINES: usize = 8;

/// Widgets of one session line, kept across refreshes so a click in flight
/// is not dropped by replacing them.
struct SessionLine {
    button: gtk4::Button,
    dot: gtk4::Label,
    agent: gtk4::Label,
    summary: gtk4::Label,
    meta: gtk4::Label,
}

/// What one tab line shows.
pub(super) struct SessionInfo {
    pub tab: TabId,
    /// The agent running in the tab, if any.
    pub agent: Option<String>,
    /// The shown tab of its workspace.
    pub active: bool,
    pub state: Option<AgentState>,
    pub summary: String,
    pub model: Option<String>,
    pub ago: Option<String>,
    pub cwd: Option<String>,
}

impl SessionLine {
    fn new(app: &AppRc, tab: TabId) -> Self {
        let row = gtk4::Box::new(Orientation::Horizontal, 5);
        let dot = gtk4::Label::new(Some("●"));
        dot.add_css_class("ws-session-dot");
        let agent = gtk4::Label::new(None);
        agent.add_css_class("ws-session-agent");
        let summary = gtk4::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .build();
        summary.add_css_class("ws-session-summary");
        let meta = gtk4::Label::new(None);
        meta.add_css_class("ws-session-meta");
        row.append(&dot);
        row.append(&agent);
        row.append(&summary);
        row.append(&meta);
        let button = gtk4::Button::builder().child(&row).build();
        button.add_css_class("flat");
        button.add_css_class("ws-session");
        button.set_focus_on_click(false);
        let w = app.weak();
        button.connect_clicked(move |_| {
            let Some(a) = w.upgrade() else { return };
            // Deferred: selecting rebuilds the list this button is in.
            glib::idle_add_local_once(move || a.report(a.select_tab(tab)));
        });
        Self {
            button,
            dot,
            agent,
            summary,
            meta,
        }
    }

    fn update(&self, s: &SessionInfo) {
        for (class, on) in [
            ("dot-running", s.state == Some(AgentState::Running)),
            ("dot-waiting", s.state == Some(AgentState::Waiting)),
            ("dot-done", s.state == Some(AgentState::Idle)),
            ("dot-detected", s.agent.is_some() && s.state.is_none()),
            ("dot-none", s.agent.is_none()),
        ] {
            if on {
                self.dot.add_css_class(class);
            } else {
                self.dot.remove_css_class(class);
            }
        }
        set_optional_label(
            &self.agent,
            &s.agent.as_deref().map(crate::agent_info::short_agent).unwrap_or_default(),
        );
        if s.active {
            self.button.add_css_class("ws-session-active");
        } else {
            self.button.remove_css_class("ws-session-active");
        }
        set_optional_label(&self.summary, &s.summary);
        let meta = [s.model.as_deref(), s.ago.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("  ");
        set_optional_label(&self.meta, &meta);
        let mut tip = match &s.agent {
            Some(agent) => format!("{} · {}", crate::agents::display_name(agent), s.summary),
            None => s.summary.clone(),
        };
        if let Some(cwd) = &s.cwd {
            tip.push('\n');
            tip.push_str(cwd);
        }
        if self.button.tooltip_text().as_deref() != Some(tip.as_str()) {
            self.button.set_tooltip_text(Some(&tip));
        }
    }
}

/// One line per tab of a workspace running an AI agent, in tab order: the
/// agent and what it is doing (or the name the user gave the tab). Plain
/// shell tabs are only in the tab bar.
pub(super) fn session_infos(i: &super::Inner, ws: &crate::model::Workspace) -> Vec<SessionInfo> {
    let now = std::time::Instant::now();
    let infos: Vec<SessionInfo> = ws
        .tabs
        .iter()
        .map(|tab| {
            let p = i.panes.get(&super::tabs::primary_pane(i, tab));
            let agent = p.and_then(|p| p.agent_id()).map(str::to_string);
            SessionInfo {
                tab: tab.id,
                active: ws.tab().id == tab.id,
                state: p.and_then(|p| p.agent_state()),
                summary: match (&tab.name, &agent) {
                    (Some(name), _) => name.clone(),
                    (None, Some(_)) => p.and_then(|p| p.agent_summary()).unwrap_or_default(),
                    (None, None) => super::tabs::tab_label(i, tab),
                },
                model: agent
                    .as_ref()
                    .and(p.and_then(|p| p.agent_model.as_deref()))
                    .and_then(crate::agent_info::short_model),
                ago: agent
                    .as_ref()
                    .and(p.and_then(|p| p.agent_activity))
                    .map(|t| crate::agent_info::time_ago(now.duration_since(t))),
                cwd: p.and_then(|p| p.cwd.as_deref()).map(crate::meta::display_path),
                agent,
            }
        })
        .filter(|s: &SessionInfo| s.agent.is_some())
        .collect();
    infos
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
        let icon = gtk4::Image::new();
        icon.add_css_class("ws-icon");
        icon.set_visible(false);
        top.append(&icon);
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
        let sessions = gtk4::Box::new(Orientation::Vertical, 0);
        sessions.add_css_class("ws-sessions");
        v.append(&sessions);
        let more = label("ws-session-more", gtk4::pango::EllipsizeMode::None);
        let attention = label("ws-attention", gtk4::pango::EllipsizeMode::End);
        let stripe = gtk4::Box::new(Orientation::Vertical, 0);
        stripe.add_css_class("ws-stripe");
        stripe.set_visible(false);
        let outer = gtk4::Box::new(Orientation::Horizontal, 6);
        v.set_hexpand(true);
        outer.append(&stripe);
        outer.append(&v);
        widget.set_child(Some(&outer));
        attach_dnd(app, &widget, id);
        attach_menu(app, &widget, id);
        Self {
            widget,
            stripe,
            icon,
            name,
            badge,
            context,
            sessions,
            session_lines: HashMap::new(),
            more,
            attention,
        }
    }
}

/// Give `w` the class for `color` (`ws-color-red`, …), removing any other one.
fn set_color_class(w: &impl IsA<gtk4::Widget>, color: Option<&str>) {
    for c in crate::model::WORKSPACE_COLORS {
        let class = format!("ws-color-{c}");
        if Some(c) == color {
            w.add_css_class(&class);
        } else {
            w.remove_css_class(&class);
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
        let focused = i.panes.get(&ws.focused());
        set_optional_label(&row.name, &ws.name);
        set_color_class(&row.stripe, ws.color.as_deref());
        row.stripe.set_visible(ws.color.is_some());
        let icon = ws.icon.as_deref().and_then(crate::model::icon_name);
        if row.icon.icon_name().as_deref() != icon {
            row.icon.set_icon_name(icon);
        }
        set_color_class(&row.icon, ws.color.as_deref());
        row.icon.set_visible(icon.is_some());
        let unread = i.notes.unread_for_workspace(ws.id);
        set_optional_label(&row.badge, &if unread > 0 { unread.to_string() } else { String::new() });
        // Just the folder: branch and ports are in the pane chips and status bar.
        let folder = focused
            .and_then(|p| p.cwd.as_ref())
            .map(|c| crate::meta::display_path(c))
            .unwrap_or_default();
        set_optional_label(&row.context, &folder);
        let infos = session_infos(&i, ws);
        sync_sessions(app, row, &infos);
        let attention = i
            .notes
            .latest_non_agent_for_workspace(ws.id)
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
            ws.panes().len()
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

/// Show `infos` (at most [`MAX_SESSION_LINES`]) as session lines, reusing
/// existing line widgets and only reordering when the order changed.
fn sync_sessions(app: &AppRc, row: &mut Row, infos: &[SessionInfo]) {
    let shown = &infos[..infos.len().min(MAX_SESSION_LINES)];
    row.session_lines.retain(|tab, line| {
        let keep = shown.iter().any(|s| s.tab == *tab);
        if !keep {
            row.sessions.remove(&line.button);
        }
        keep
    });
    for s in shown {
        row.session_lines
            .entry(s.tab)
            .or_insert_with(|| SessionLine::new(app, s.tab))
            .update(s);
    }
    let mut current = Vec::new();
    let mut child = row.sessions.first_child();
    while let Some(c) = child {
        child = c.next_sibling();
        current.push(c);
    }
    let wanted: Vec<gtk4::Widget> = shown.iter().map(|s| row.session_lines[&s.tab].button.clone().upcast()).collect();
    if current != wanted {
        for c in &current {
            row.sessions.remove(c);
        }
        for w in &wanted {
            row.sessions.append(w);
        }
    }
    row.sessions.set_visible(!shown.is_empty());
    let hidden = infos.len() - shown.len();
    set_optional_label(&row.more, &if hidden > 0 { format!("+{hidden} more") } else { String::new() });
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
    // Colour and Icon submenus: targets are "<id>:<value>", empty value = none.
    let choice = |label: &str, action: &str, value: &str| {
        let it = gio::MenuItem::new(Some(label), None);
        it.set_action_and_target_value(Some(action), Some(&format!("{id}:{value}").to_variant()));
        it
    };
    let title = |s: &str| -> String {
        let mut c = s.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
    };
    let colors = gio::Menu::new();
    colors.append_item(&choice("None", "app.ws-color", ""));
    for c in crate::model::WORKSPACE_COLORS {
        colors.append_item(&choice(&title(c), "app.ws-color", c));
    }
    let icons = gio::Menu::new();
    icons.append_item(&choice("None", "app.ws-icon", ""));
    for (key, icon) in crate::model::WORKSPACE_ICONS {
        let it = choice(&title(key), "app.ws-icon", key);
        it.set_icon(&gio::ThemedIcon::new(icon));
        icons.append_item(&it);
    }
    let look = gio::Menu::new();
    look.append_submenu(Some("Colour"), &colors);
    look.append_submenu(Some("Icon"), &icons);
    menu.insert_section(1, None, &look);
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
}
