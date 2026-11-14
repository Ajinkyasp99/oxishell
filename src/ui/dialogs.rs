//! Dialogs and popovers: confirm, rename, command palette, settings,
//! notifications panel, shortcuts help and about.

use super::AppRc;
use crate::config::DEFAULT_SHORTCUTS;
use crate::model::{PaneId, WorkspaceId};
use gtk4::prelude::*;
use gtk4::{glib, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::rc::Rc;

pub fn confirm(app: &AppRc, heading: &str, body: &str, verb: &str, on_yes: impl Fn() + 'static) {
    let d = adw::AlertDialog::new(Some(heading), Some(body));
    d.add_response("cancel", "Cancel");
    d.add_response("yes", verb);
    d.set_response_appearance("yes", adw::ResponseAppearance::Destructive);
    d.set_default_response(Some("cancel"));
    d.set_close_response("cancel");
    d.connect_response(None, move |_, r| {
        if r == "yes" {
            on_yes();
        }
    });
    d.present(Some(&app.window));
}

pub fn rename_workspace(app: &AppRc, wid: WorkspaceId) {
    let Some(current) = app.inner.borrow().state.workspace(wid).map(|w| w.name.clone()) else {
        return;
    };
    let d = adw::AlertDialog::new(Some("Rename Workspace"), None);
    let entry = gtk4::Entry::builder().text(&current).activates_default(true).build();
    d.set_extra_child(Some(&entry));
    d.add_response("cancel", "Cancel");
    d.add_response("ok", "Rename");
    d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    d.set_default_response(Some("ok"));
    d.set_close_response("cancel");
    let w = app.weak();
    let e = entry.clone();
    d.connect_response(None, move |_, r| {
        if r == "ok" {
            if let Some(a) = w.upgrade() {
                if let Err(err) = a.rename_workspace(wid, &e.text()) {
                    a.toast(&err);
                }
            }
        }
    });
    d.present(Some(&app.window));
    entry.grab_focus();
    entry.select_region(0, -1);
}

// ---- command palette --------------------------------------------------------

#[derive(Clone)]
enum Item {
    Action(String),
    Workspace(WorkspaceId),
    Command(String),
    Resume(PaneId),
    Hooks,
    Theme(String),
}

/// Case-insensitive subsequence match.
pub fn fuzzy_match(query: &str, text: &str) -> bool {
    let mut t = text.chars().flat_map(char::to_lowercase);
    query
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !c.is_whitespace())
        .all(|q| t.any(|c| c == q))
}

fn palette_items(app: &AppRc) -> Vec<(String, String, Item)> {
    let i = app.inner.borrow();
    let mut items = Vec::new();
    let accels: std::collections::HashMap<String, String> = i.config.effective_shortcuts().into_iter().collect();
    for (name, _, label) in DEFAULT_SHORTCUTS {
        if *name == "command-palette" {
            continue;
        }
        let accel = accels
            .get(*name)
            .and_then(gtk4::accelerator_parse)
            .map(|(k, m)| gtk4::accelerator_get_label(k, m).to_string())
            .unwrap_or_default();
        items.push((label.to_string(), accel, Item::Action(name.to_string())));
    }
    for w in &i.state.workspaces {
        items.push((format!("Switch to: {}", w.name), String::new(), Item::Workspace(w.id)));
    }
    for c in &i.config.commands {
        items.push((format!("Run: {}", c.name), c.command.clone(), Item::Command(c.command.clone())));
    }
    for (id, p) in &i.panes {
        if let Some((agent, _)) = &p.agent_session {
            let ws = i
                .state
                .workspace_of(*id)
                .and_then(|w| i.state.workspace(w))
                .map(|w| w.name.clone())
                .unwrap_or_default();
            items.push((
                format!("Resume {} session in {ws}", crate::agents::display_name(agent)),
                "types the resume command; press Enter to run".into(),
                Item::Resume(*id),
            ));
        }
    }
    let current = i.theme.id.clone();
    for t in crate::theme::all(i.theme.is_dark()) {
        let mark = if t.id == current { "current" } else { "" };
        items.push((format!("Theme: {}", t.name), mark.to_string(), Item::Theme(t.id)));
    }
    let agents = crate::agents::SUPPORTED
        .iter()
        .map(|a| crate::agents::display_name(a))
        .collect::<Vec<_>>()
        .join(", ");
    items.push(("Set up agent notifications (hooks)".to_string(), agents, Item::Hooks));
    items
}

fn run_item(app: &AppRc, item: Item) {
    match item {
        Item::Action(a) => app.run_action(&a),
        Item::Workspace(w) => app.select_workspace(w),
        Item::Command(cmd) => {
            // Explicit user choice: type the command into the focused pane and run it.
            let t = app.focused_pane().and_then(|p| {
                app.inner.borrow().panes.get(&p).map(|p| {
                    p.send_input(format!("{cmd}\n").as_bytes());
                    p.term.clone()
                })
            });
            match t {
                Some(t) => {
                    t.grab_focus();
                }
                None => app.toast("No focused pane"),
            }
        }
        Item::Resume(p) => app.resume_agent(p),
        Item::Theme(id) => app.set_theme(&id),
        Item::Hooks => super::settings::show(app, Some(super::settings::PAGE_NOTIFICATIONS)),
    }
}

pub fn command_palette(app: &AppRc) {
    let items = Rc::new(palette_items(app));
    let d = adw::Dialog::builder()
        .title("Command Palette")
        .content_width(560)
        .content_height(440)
        .build();
    let entry = gtk4::SearchEntry::builder()
        .placeholder_text("Type a command or workspace…")
        .margin_top(10)
        .margin_start(10)
        .margin_end(10)
        .build();
    let list = gtk4::ListBox::new();
    list.add_css_class("palette-list");
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk4::SelectionMode::Browse);
    for (idx, (label, sub, _)) in items.iter().enumerate() {
        let row = gtk4::ListBoxRow::new();
        row.set_widget_name(&idx.to_string());
        let h = gtk4::Box::new(Orientation::Horizontal, 8);
        h.append(
            &gtk4::Label::builder()
                .label(label)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk4::pango::EllipsizeMode::End)
                .build(),
        );
        if !sub.is_empty() {
            let s = gtk4::Label::builder()
                .label(sub)
                .ellipsize(gtk4::pango::EllipsizeMode::End)
                .max_width_chars(28)
                .build();
            s.add_css_class("palette-accel");
            h.append(&s);
        }
        row.set_child(Some(&h));
        list.append(&row);
    }
    {
        let e = entry.clone();
        let items = items.clone();
        list.set_filter_func(move |row| {
            let q = e.text();
            let idx: usize = row.widget_name().parse().unwrap_or(0);
            q.is_empty() || items.get(idx).is_some_and(|(l, s, _)| fuzzy_match(&q, l) || fuzzy_match(&q, s))
        });
    }
    let first_visible = |list: &gtk4::ListBox| {
        let mut c = list.first_child();
        while let Some(w) = c {
            if w.is_child_visible() {
                return w.downcast::<gtk4::ListBoxRow>().ok();
            }
            c = w.next_sibling();
        }
        None
    };
    {
        let l = list.clone();
        entry.connect_search_changed(move |_| {
            l.invalidate_filter();
            if let Some(r) = first_visible(&l) {
                l.select_row(Some(&r));
            }
        });
    }
    let activate = {
        let w = app.weak();
        let d = d.clone();
        let items = items.clone();
        Rc::new(move |row: &gtk4::ListBoxRow| {
            let idx: usize = row.widget_name().parse().unwrap_or(usize::MAX);
            let item = items.get(idx).map(|(_, _, it)| it.clone());
            d.close();
            if let (Some(a), Some(item)) = (w.upgrade(), item) {
                glib::idle_add_local_once(move || run_item(&a, item));
            }
        })
    };
    {
        let l = list.clone();
        let act = activate.clone();
        entry.connect_activate(move |_| {
            if let Some(r) = l.selected_row().filter(|r| r.is_child_visible()).or_else(|| first_visible(&l)) {
                act(&r);
            }
        });
    }
    {
        let act = activate.clone();
        list.connect_row_activated(move |_, r| act(r));
    }
    {
        // Down arrow moves from the entry into the list.
        let l = list.clone();
        let keys = gtk4::EventControllerKey::new();
        keys.connect_key_pressed(move |_, k, _, _| {
            if k == gtk4::gdk::Key::Down {
                if let Some(r) = l.selected_row().or_else(|| first_visible(&l)) {
                    r.grab_focus();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        entry.add_controller(keys);
    }
    if let Some(r) = first_visible(&list) {
        list.select_row(Some(&r));
    }
    let scroll = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .child(&list)
        .margin_start(10)
        .margin_end(10)
        .margin_bottom(10)
        .margin_top(6)
        .build();
    let v = gtk4::Box::new(Orientation::Vertical, 0);
    v.append(&entry);
    v.append(&scroll);
    d.set_child(Some(&v));
    {
        // The search entry consumes Escape (stop-search), so close the palette from there.
        let d = d.clone();
        entry.connect_stop_search(move |_| {
            d.close();
        });
    }
    d.present(Some(&app.window));
    entry.grab_focus();
}

// ---- notifications popover ---------------------------------------------------

pub fn connect_notifications_popover(app: &AppRc) {
    let pop = gtk4::Popover::new();
    let v = gtk4::Box::new(Orientation::Vertical, 6);
    v.set_margin_top(6);
    v.set_margin_bottom(6);
    v.set_margin_start(6);
    v.set_margin_end(6);
    let buttons = gtk4::Box::new(Orientation::Horizontal, 6);
    let jump = gtk4::Button::with_label("Jump to Latest");
    let read = gtk4::Button::with_label("Mark All Read");
    let clear = gtk4::Button::with_label("Clear");
    buttons.append(&jump);
    buttons.append(&read);
    buttons.append(&clear);
    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::None);
    list.add_css_class("boxed-list");
    let scroll = gtk4::ScrolledWindow::builder()
        .child(&list)
        .min_content_width(380)
        .min_content_height(120)
        .max_content_height(420)
        .propagate_natural_height(true)
        .build();
    v.append(&buttons);
    v.append(&scroll);
    pop.set_child(Some(&v));
    app.notif_button.set_popover(Some(&pop));

    let fill = {
        let w = app.weak();
        let list = list.clone();
        let pop = pop.clone();
        Rc::new(move || {
            let Some(a) = w.upgrade() else { return };
            // Remove rows only: popovers parented to the list are children too, and
            // ListBox::remove ignores them, so a first_child() loop would never end.
            while let Some(c) = list.row_at_index(0) {
                list.remove(&c);
            }
            let i = a.inner.borrow();
            let notes = i.notes.all();
            if notes.is_empty() {
                list.append(
                    &gtk4::Label::builder()
                        .label("No notifications")
                        .margin_top(12)
                        .margin_bottom(12)
                        .css_classes(["dim-label"])
                        .build(),
                );
            }
            for n in notes.iter().rev() {
                let row = gtk4::ListBoxRow::new();
                row.add_css_class("notif-row");
                if !n.read {
                    row.add_css_class("notif-unread");
                }
                let vb = gtk4::Box::new(Orientation::Vertical, 2);
                // App-wide notes (usage alerts, `oxi notify --global`) have no
                // workspace; "closed" is only for a workspace that is gone.
                let ws = n.workspace.map(|w| {
                    i.state
                        .workspace(w)
                        .map(|w| format!("{} · ", w.name))
                        .unwrap_or_else(|| "closed · ".into())
                });
                let t = gtk4::Label::builder()
                    .label(format!("{}{}", if n.read { "" } else { "● " }, n.title))
                    .xalign(0.0)
                    .ellipsize(gtk4::pango::EllipsizeMode::End)
                    .build();
                t.add_css_class("ws-name");
                vb.append(&t);
                let when = glib::DateTime::from_unix_local(n.time as i64)
                    .ok()
                    .and_then(|d| d.format("%H:%M:%S").ok())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let sub = gtk4::Label::builder()
                    .label(format!(
                        "{}{when} · {}{}",
                        ws.unwrap_or_default(),
                        n.source,
                        if n.body.is_empty() {
                            String::new()
                        } else {
                            format!(" — {}", n.body)
                        }
                    ))
                    .xalign(0.0)
                    .wrap(true)
                    .max_width_chars(48)
                    .build();
                sub.add_css_class("ws-sub");
                vb.append(&sub);
                row.set_child(Some(&vb));
                if n.pane.is_some() {
                    let id = n.id;
                    let w2 = a.weak();
                    let pop2 = pop.clone();
                    let g = gtk4::GestureClick::new();
                    g.connect_released(move |_, _, _, _| {
                        pop2.popdown();
                        if let Some(a) = w2.upgrade() {
                            glib::idle_add_local_once(move || a.open_notification(id));
                        }
                    });
                    row.add_controller(g);
                }
                list.append(&row);
            }
        })
    };
    {
        let f = fill.clone();
        pop.connect_show(move |_| f());
    }
    {
        let w = app.weak();
        let p = pop.clone();
        jump.connect_clicked(move |_| {
            p.popdown();
            if let Some(a) = w.upgrade() {
                a.run_action("jump-unread");
            }
        });
    }
    {
        let w = app.weak();
        let f = fill.clone();
        read.connect_clicked(move |_| {
            if let Some(a) = w.upgrade() {
                a.inner.borrow_mut().notes.mark_all_read();
                a.refresh();
                f();
            }
        });
    }
    {
        let w = app.weak();
        let f = fill;
        clear.connect_clicked(move |_| {
            if let Some(a) = w.upgrade() {
                a.inner.borrow_mut().notes.clear();
                a.refresh();
                f();
            }
        });
    }
}

pub fn shortcuts_help(app: &AppRc) {
    let d = adw::Dialog::builder()
        .title("Keyboard Shortcuts")
        .content_width(460)
        .content_height(560)
        .build();
    let list = gtk4::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk4::SelectionMode::None);
    let i = app.inner.borrow();
    let mut entries: Vec<(String, String)> = i
        .config
        .effective_shortcuts()
        .into_iter()
        .map(|(a, k)| {
            let label = DEFAULT_SHORTCUTS
                .iter()
                .find(|(x, _, _)| *x == a)
                .map(|(_, _, l)| l.to_string())
                .unwrap_or(a);
            let k = gtk4::accelerator_parse(&k)
                .map(|(k, m)| gtk4::accelerator_get_label(k, m).to_string())
                .unwrap_or_else(|| "—".into());
            (label, k)
        })
        .collect();
    entries.push(("Select workspace 1–9".into(), "Alt+1…9".into()));
    for (l, k) in entries {
        let row = adw::ActionRow::builder().title(l).build();
        let kl = gtk4::Label::new(Some(&k));
        kl.add_css_class("dim-label");
        row.add_suffix(&kl);
        list.append(&row);
    }
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(
        &gtk4::ScrolledWindow::builder()
            .child(&list)
            .margin_start(12)
            .margin_end(12)
            .margin_bottom(12)
            .build(),
    ));
    d.set_child(Some(&tv));
    d.present(Some(&app.window));
}

pub fn about(app: &AppRc) {
    let d = adw::AboutDialog::builder()
        .application_name("oxishell")
        .application_icon(crate::APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .comments("Native Linux terminal workspaces and split panes for AI coding agents. Inspired by cmux (manaflow-ai/cmux). Themes from warpdotdev/themes (Apache-2.0).")
        .license_type(gtk4::License::Gpl30)
        .website("https://github.com/Ajinkyasp99/oxishell")
        .issue_url("https://github.com/Ajinkyasp99/oxishell/issues")
        .developer_name("Ajinkya Patil")
        .build();
    d.present(Some(&app.window));
}

// ---- command history search ------------------------------------------------------

/// Search shell history (bash/zsh/fish + this session). Enter puts the command
/// on the prompt (replacing what was typed) for review; Ctrl+Enter also runs it.
pub fn command_history(app: &AppRc) {
    let Some(pane) = app.focused_pane() else { return };
    let prefill = app
        .inner
        .borrow()
        .panes
        .get(&pane)
        .map(|p| p.suggest.tracker.borrow().clone())
        .filter(|t| t.valid)
        .map(|t| t.line)
        .unwrap_or_default();
    let d = adw::Dialog::builder()
        .title("Command History")
        .content_width(640)
        .content_height(460)
        .build();
    let entry = gtk4::SearchEntry::builder()
        .placeholder_text("Search your command history…")
        .text(prefill.trim())
        .margin_top(10)
        .margin_start(10)
        .margin_end(10)
        .build();
    let hint = gtk4::Label::builder()
        .label("Enter: put on prompt   ·   Ctrl+Enter: run   ·   Esc: close")
        .margin_top(4)
        .build();
    hint.add_css_class("palette-accel");
    let list = gtk4::ListBox::new();
    list.add_css_class("palette-list");
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk4::SelectionMode::Browse);
    let fill = {
        let w = app.weak();
        let list = list.clone();
        Rc::new(move |q: &str| {
            let Some(a) = w.upgrade() else { return };
            while let Some(c) = list.row_at_index(0) {
                list.remove(&c);
            }
            let i = a.inner.borrow();
            for cmd in i.history.search(q, 200) {
                let row = gtk4::ListBoxRow::new();
                let l = gtk4::Label::builder()
                    .label(cmd)
                    .xalign(0.0)
                    .ellipsize(gtk4::pango::EllipsizeMode::End)
                    .build();
                l.add_css_class("monospace");
                row.set_child(Some(&l));
                row.set_tooltip_text(Some(cmd));
                list.append(&row);
            }
            if let Some(r) = list.row_at_index(0) {
                list.select_row(Some(&r));
            }
        })
    };
    fill(&prefill);
    {
        let f = fill.clone();
        entry.connect_search_changed(move |e| f(&e.text()));
    }
    let insert = {
        let w = app.weak();
        let d = d.clone();
        Rc::new(move |row: &gtk4::ListBoxRow, run: bool| {
            let cmd = row.tooltip_text().map(|t| t.to_string()).unwrap_or_default();
            d.close();
            let Some(a) = w.upgrade() else { return };
            if let Some(p) = a.inner.borrow().panes.get(&pane) {
                // Ctrl+U clears the current input line in bash, zsh and fish.
                let mut text = format!("\u{15}{cmd}");
                if run {
                    text.push('\r');
                }
                p.send_input(text.as_bytes());
                let mut t = p.suggest.tracker.borrow_mut();
                t.reset();
                if !run {
                    t.line = cmd.clone();
                }
            }
            a.focus_pane(pane);
        })
    };
    {
        let l = list.clone();
        let ins = insert.clone();
        entry.connect_activate(move |_| {
            if let Some(r) = l.selected_row() {
                ins(&r, false);
            }
        });
    }
    {
        let ins = insert.clone();
        list.connect_row_activated(move |_, r| ins(r, false));
    }
    {
        // Ctrl+Enter runs; Down moves into the list.
        let l = list.clone();
        let ins = insert;
        let keys = gtk4::EventControllerKey::new();
        keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
        keys.connect_key_pressed(move |_, k, _, state| {
            let ctrl = state.contains(gtk4::gdk::ModifierType::CONTROL_MASK);
            if ctrl && matches!(k, gtk4::gdk::Key::Return | gtk4::gdk::Key::KP_Enter) {
                if let Some(r) = l.selected_row() {
                    ins(&r, true);
                }
                return glib::Propagation::Stop;
            }
            if k == gtk4::gdk::Key::Down {
                let next = l.selected_row().and_then(|r| l.row_at_index(r.index() + 1));
                if let Some(n) = next {
                    l.select_row(Some(&n));
                    n.grab_focus();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        entry.add_controller(keys);
    }
    {
        let d = d.clone();
        entry.connect_stop_search(move |_| {
            d.close();
        });
    }
    let scroll = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .child(&list)
        .margin_start(10)
        .margin_end(10)
        .margin_bottom(10)
        .margin_top(6)
        .build();
    let v = gtk4::Box::new(Orientation::Vertical, 0);
    v.append(&entry);
    v.append(&hint);
    v.append(&scroll);
    d.set_child(Some(&v));
    d.present(Some(&app.window));
    entry.grab_focus();
    {
        // Keep the pre-filled text and continue typing after it (no select-all).
        let e = entry.clone();
        glib::idle_add_local_once(move || {
            e.select_region(-1, -1);
            e.set_position(-1);
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fuzzy() {
        assert!(super::fuzzy_match("spr", "Split right"));
        assert!(super::fuzzy_match("SPLIT", "Split right"));
        assert!(!super::fuzzy_match("xyz", "Split right"));
        assert!(super::fuzzy_match("", "anything"));
    }
}
