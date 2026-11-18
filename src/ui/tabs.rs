//! Workspace tabs: one tab bar per workspace, above a stack holding each
//! tab's split layout. Bars are rebuilt with the workspace (pane frames are
//! only re-parented, never recreated); `sync` shows the active tab and
//! updates labels, state dots and unread marks from the model.

use super::{AppRc, Inner};
use crate::agents::AgentState;
use crate::model::{Tab, TabId, WorkspaceId};
use gtk4::prelude::*;
use gtk4::{gdk, glib, Orientation};

/// Longest tab label before it is ellipsized.
const TAB_CHARS: i32 = 28;

/// Drag payload of a tab (a pane-swap drag uses its own prefix).
const TAB_DRAG_PREFIX: &str = "oxishell-tab:";

pub(super) struct TabBar {
    tabs: Vec<(TabId, TabWidgets)>,
    stack: gtk4::Stack,
}

struct TabWidgets {
    widget: gtk4::Box,
    dot: gtk4::Label,
    label: gtk4::Label,
}

impl TabBar {
    pub fn stack(&self) -> &gtk4::Stack {
        &self.stack
    }
}

/// The pane a tab is "about": its focused pane if an agent runs there, else
/// its first pane with an agent, else its focused pane.
pub(super) fn primary_pane(i: &Inner, tab: &Tab) -> crate::model::PaneId {
    let has_agent = |p: &crate::model::PaneId| i.panes.get(p).is_some_and(|p| p.agent_id().is_some());
    if has_agent(&tab.focused) {
        return tab.focused;
    }
    tab.layout.panes().into_iter().find(has_agent).unwrap_or(tab.focused)
}

/// Label of a tab: the name the user gave it, else agent and summary, else
/// busy program, else folder.
pub(super) fn tab_label(i: &Inner, tab: &Tab) -> String {
    if let Some(name) = &tab.name {
        return name.clone();
    }
    let Some(p) = i.panes.get(&primary_pane(i, tab)) else {
        return "Terminal".into();
    };
    crate::agent_info::tab_text(
        p.agent_id(),
        p.agent_summary().as_deref(),
        p.fg_program.as_deref(),
        p.cwd.as_deref(),
    )
}

/// Build the bar for workspace `wid` with tabs `tabs`, and the stack its
/// tab layouts go in (the caller adds one page per tab, named by tab id).
pub(super) fn build_bar(app: &AppRc, wid: WorkspaceId, tabs: &[TabId]) -> (gtk4::Box, TabBar) {
    let bar = gtk4::Box::new(Orientation::Horizontal, 0);
    bar.add_css_class("tab-bar");
    let tabs_box = gtk4::Box::new(Orientation::Horizontal, 2);
    tabs_box.add_css_class("tab-list");
    // Double-clicking the empty bar after the tabs opens a new tab.
    let fill = gtk4::Box::new(Orientation::Horizontal, 0);
    fill.set_hexpand(true);
    fill.add_css_class("tab-fill");
    let content = gtk4::Box::new(Orientation::Horizontal, 0);
    content.append(&tabs_box);
    let new_btn = gtk4::Button::from_icon_name("list-add-symbolic");
    new_btn.add_css_class("flat");
    new_btn.add_css_class("tab-new");
    new_btn.set_tooltip_text(Some(&crate::config::with_shortcut("New tab", "new-tab")));
    new_btn.set_focus_on_click(false);
    new_btn.set_valign(gtk4::Align::Center);
    content.append(&new_btn);
    content.append(&fill);
    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::External)
        .vscrollbar_policy(gtk4::PolicyType::Never)
        .propagate_natural_height(true)
        .hexpand(true)
        .child(&content)
        .build();
    bar.append(&scroll);

    let stack = gtk4::Stack::new();
    stack.set_hhomogeneous(false);
    stack.set_vhomogeneous(false);
    stack.set_hexpand(true);
    stack.set_vexpand(true);

    let mut built = Vec::new();
    for &tab in tabs {
        let t = build_tab(app, tab);
        tabs_box.append(&t.widget);
        built.push((tab, t));
    }

    let w = app.weak();
    new_btn.connect_clicked(move |_| {
        if let Some(a) = w.upgrade() {
            glib::idle_add_local_once(move || a.report(a.new_tab(Some(wid), None).map(|_| ())));
        }
    });
    let dbl = gtk4::GestureClick::new();
    let w = app.weak();
    dbl.connect_pressed(move |_, n, _, _| {
        if n == 2 {
            if let Some(a) = w.upgrade() {
                glib::idle_add_local_once(move || a.report(a.new_tab(Some(wid), None).map(|_| ())));
            }
        }
    });
    fill.add_controller(dbl);
    attach_drop(app, &content, &tabs_box, tabs);
    (bar, TabBar { tabs: built, stack })
}

fn build_tab(app: &AppRc, tab: TabId) -> TabWidgets {
    let widget = gtk4::Box::new(Orientation::Horizontal, 5);
    widget.add_css_class("tab");
    widget.set_widget_name(&format!("tab-{tab}"));
    let dot = gtk4::Label::new(Some("●"));
    dot.add_css_class("ws-session-dot");
    dot.set_visible(false);
    let label = gtk4::Label::builder()
        .xalign(0.0)
        .max_width_chars(TAB_CHARS)
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .build();
    label.add_css_class("tab-label");
    let unread = gtk4::Label::new(Some("•"));
    unread.add_css_class("tab-unread-dot");
    let close = gtk4::Button::from_icon_name("window-close-symbolic");
    close.add_css_class("flat");
    close.add_css_class("circular");
    close.add_css_class("tab-close");
    close.set_tooltip_text(Some("Close tab"));
    close.set_focus_on_click(false);
    close.set_can_focus(false);
    close.set_valign(gtk4::Align::Center);
    let w = app.weak();
    close.connect_clicked(move |_| {
        if let Some(a) = w.upgrade() {
            glib::idle_add_local_once(move || a.report(a.close_tab(tab, false, true)));
        }
    });
    widget.append(&dot);
    widget.append(&label);
    widget.append(&unread);
    widget.append(&close);

    // Primary click shows the tab; middle click closes it.
    let click = gtk4::GestureClick::new();
    click.set_button(0);
    let w = app.weak();
    click.connect_released(move |g, _, _, _| {
        let Some(a) = w.upgrade() else { return };
        let button = g.current_button();
        glib::idle_add_local_once(move || match button {
            gdk::BUTTON_MIDDLE => a.report(a.close_tab(tab, false, true)),
            gdk::BUTTON_PRIMARY => a.report(a.select_tab(tab)),
            _ => {}
        });
    });
    widget.add_controller(click);

    // Double click renames.
    let dbl = gtk4::GestureClick::new();
    let w = app.weak();
    dbl.connect_pressed(move |_, n, _, _| {
        if n == 2 {
            if let Some(a) = w.upgrade() {
                glib::idle_add_local_once(move || super::dialogs::rename_tab(&a, tab));
            }
        }
    });
    widget.add_controller(dbl);

    // Right click: Rename… / Close Tab.
    let menu = gtk4::gio::Menu::new();
    for (label, action) in [("Rename…", "app.tab-rename"), ("Close Tab", "app.tab-close")] {
        let it = gtk4::gio::MenuItem::new(Some(label), None);
        it.set_action_and_target_value(Some(action), Some(&tab.to_variant()));
        menu.append_item(&it);
    }
    let ctx = gtk4::GestureClick::new();
    ctx.set_button(gdk::BUTTON_SECONDARY);
    ctx.connect_pressed(move |g, _, x, y| {
        let Some(t) = g.widget() else { return };
        let pop = gtk4::PopoverMenu::from_model(Some(&menu));
        pop.set_parent(&t);
        pop.set_has_arrow(false);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        // Unparent once closed (the tab may be rebuilt meanwhile).
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    });
    widget.add_controller(ctx);

    let src = gtk4::DragSource::new();
    src.set_actions(gdk::DragAction::MOVE);
    src.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&format!("{TAB_DRAG_PREFIX}{tab}").to_value())));
    // The tab comes from the controller: capturing it would keep the tab
    // (widget → controller → closure → widget) alive after every rebuild.
    src.connect_drag_begin(|src, _| {
        if let Some(t) = src.widget() {
            src.set_icon(Some(&gtk4::WidgetPaintable::new(Some(&t))), 0, 0);
        }
    });
    widget.add_controller(src);
    TabWidgets { widget, dot, label }
}

/// Where a tab dropped at `x` (in `area` coordinates) goes in `tabs_box`:
/// before the first tab whose middle is right of `x`, else at the end.
fn drop_index(area: &gtk4::Widget, tabs_box: &gtk4::Box, x: f64) -> usize {
    let mut index = 0;
    let mut child = tabs_box.first_child();
    while let Some(c) = child {
        if let Some(b) = c.compute_bounds(area) {
            if x < (b.x() + b.width() / 2.0) as f64 {
                return index;
            }
        }
        index += 1;
        child = c.next_sibling();
    }
    index
}

/// Tabs of this bar dropped on it move to the position under the pointer.
fn attach_drop(app: &AppRc, area: &gtk4::Box, tabs_box: &gtk4::Box, members: &[TabId]) {
    let drop = gtk4::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
    let w = app.weak();
    // `tabs_box` is a child of `area`, so holding it makes no cycle; `area`
    // itself comes from the controller.
    let tabs_box = tabs_box.clone();
    let members = members.to_vec();
    drop.connect_drop(move |drop, value, x, _| {
        let (Some(a), Some(area)) = (w.upgrade(), drop.widget()) else {
            return false;
        };
        let dragged = value
            .get::<String>()
            .ok()
            .and_then(|s| s.strip_prefix(TAB_DRAG_PREFIX)?.parse::<TabId>().ok());
        // Only this workspace's tabs; moving tabs between workspaces is not supported.
        let Some(k) = dragged.and_then(|d| members.iter().position(|t| *t == d)) else {
            return false;
        };
        let mut index = drop_index(&area, &tabs_box, x);
        // Positions count the dragged tab, which leaves its place first.
        if k < index {
            index -= 1;
        }
        let tab = members[k];
        // Defer: moving rebuilds the bar this drop runs inside.
        glib::idle_add_local_once(move || a.report(a.move_tab(tab, index)));
        true
    });
    area.add_controller(drop);
}

fn set_class(w: &impl IsA<gtk4::Widget>, class: &str, on: bool) {
    if on {
        w.add_css_class(class);
    } else {
        w.remove_css_class(class);
    }
}

/// Bring every tab bar in line with the model: shown tab, labels, state dots
/// and unread marks.
pub(super) fn sync(app: &AppRc) {
    let i = app.inner.borrow();
    let bars = app.tab_bars.borrow();
    for (wid, bar) in bars.iter() {
        let Some(ws) = i.state.workspace(*wid) else { continue };
        let shown = ws.tab().id.to_string();
        if bar.stack.visible_child_name().as_deref() != Some(shown.as_str()) && bar.stack.child_by_name(&shown).is_some() {
            bar.stack.set_visible_child_name(&shown);
        }
        for (id, t) in &bar.tabs {
            let Some(tab) = ws.tabs.iter().find(|x| x.id == *id) else {
                continue;
            };
            let active = *id == ws.tab().id;
            let unread = tab.layout.panes().iter().any(|p| i.notes.unread_for_pane(*p) > 0);
            set_class(&t.widget, "tab-active", active);
            set_class(&t.widget, "tab-unread", unread && !active);
            let text = tab_label(&i, tab);
            if t.label.label() != text {
                t.label.set_label(&text);
            }
            let primary = i.panes.get(&primary_pane(&i, tab));
            let tip = match primary.and_then(|p| Some((p.agent_id()?, p.agent_summary()?))) {
                Some((agent, s)) => format!("{} · {s}", crate::agents::display_name(agent)),
                None => primary
                    .and_then(|p| p.cwd.as_deref())
                    .map(crate::meta::display_path)
                    .unwrap_or_else(|| text.clone()),
            };
            if t.widget.tooltip_text().as_deref() != Some(tip.as_str()) {
                t.widget.set_tooltip_text(Some(&tip));
            }
            let agent = primary.and_then(|p| p.agent_id()).is_some();
            let state = primary.and_then(|p| p.agent_state());
            t.dot.set_visible(agent);
            set_class(&t.dot, "dot-running", state == Some(AgentState::Running));
            set_class(&t.dot, "dot-waiting", state == Some(AgentState::Waiting));
            set_class(&t.dot, "dot-done", state == Some(AgentState::Idle));
            set_class(&t.dot, "dot-detected", agent && state.is_none());
        }
    }
}
