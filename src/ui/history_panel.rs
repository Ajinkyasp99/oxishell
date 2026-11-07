//! Warp-style history panel: press ↑ at the shell prompt to see recent
//! commands under the input line (newest at the bottom, with when they ran).
//!
//! ↑/↓ move, Enter runs the selected command, Tab puts it on the prompt
//! without running it, Esc closes. Typing keeps going to the shell and
//! filters the list live. ↓ past the newest entry closes the panel. When the
//! shell is not idle at its prompt (vim, an agent, a pager…) ↑ goes to the
//! program as usual.

use super::AppRc;
use crate::history::{now_secs, relative_time, Event};
use crate::model::PaneId;
use gtk4::prelude::*;
use gtk4::{gdk, glib};
use std::cell::RefCell;
use std::rc::Rc;
use vte4::prelude::*;

/// How many matching commands the panel lists (scrollable).
const MAX_ROWS: usize = 60;
/// Rows visible before the list scrolls.
const VISIBLE_ROWS: i32 = 10;

struct Panel {
    pop: gtk4::Popover,
    list: gtk4::ListBox,
    scroll: gtk4::ScrolledWindow,
    empty: gtk4::Label,
    /// Commands in display order (oldest first, newest last).
    items: RefCell<Vec<String>>,
}

pub fn attach(app: &AppRc, pane: PaneId, term: &vte4::Terminal) {
    let keys = gtk4::EventControllerKey::new();
    keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let w = app.weak();
    keys.connect_key_pressed(move |_, key, _, state| {
        let plain = (state & gtk4::accelerator_get_default_mod_mask()).is_empty();
        if !plain || !matches!(key, gdk::Key::Up | gdk::Key::KP_Up) {
            return glib::Propagation::Proceed;
        }
        match w.upgrade() {
            Some(a) if open(&a, pane) => glib::Propagation::Stop,
            _ => glib::Propagation::Proceed,
        }
    });
    term.add_controller(keys);
}

/// Open the panel for `pane` if the shell is idle at its prompt and there is
/// matching history. Returns false to let ↑ reach the shell instead.
fn open(app: &AppRc, pane: PaneId) -> bool {
    let (term, query, row) = {
        let Ok(i) = app.inner.try_borrow() else { return false };
        if !i.config.terminal.history_panel {
            return false;
        }
        let Some(p) = i.panes.get(&pane) else { return false };
        if p.pending_input.borrow().is_some() || p.has_running_child() || !super::suggest::at_bottom(&p.term) {
            return false;
        }
        let tracker = p.suggest.tracker.borrow();
        if !tracker.valid {
            return false; // the line was edited in ways we cannot follow
        }
        if i.history.search(&tracker.line, 1).is_empty() {
            return false;
        }
        let Some(row) = super::suggest::cursor_screen_row(&p.term) else {
            return false;
        };
        p.suggest.ghost.set_visible(false);
        (p.term.clone(), tracker.line.clone(), row)
    };

    let panel = Rc::new(build(&term));
    fill(app, &panel, &query);

    // Keys while open. Typing goes to the shell (so the prompt stays in sync)
    // and narrows the list.
    let keys = gtk4::EventControllerKey::new();
    keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
    {
        let w = app.weak();
        let panel = panel.clone();
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(a) = w.upgrade() else { return glib::Propagation::Proceed };
            let mods = state & gtk4::accelerator_get_default_mod_mask();
            let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
            match key {
                gdk::Key::Up | gdk::Key::KP_Up => {
                    move_sel(&panel, -1);
                }
                gdk::Key::Down | gdk::Key::KP_Down => {
                    if !move_sel(&panel, 1) {
                        panel.pop.popdown(); // past the newest: back to the prompt
                    }
                }
                gdk::Key::Page_Up => {
                    move_sel(&panel, -VISIBLE_ROWS);
                }
                gdk::Key::Page_Down => {
                    move_sel(&panel, VISIBLE_ROWS);
                }
                gdk::Key::Return | gdk::Key::KP_Enter => choose(&a, pane, &panel, true),
                gdk::Key::Tab | gdk::Key::Right => choose(&a, pane, &panel, false),
                gdk::Key::Escape => panel.pop.popdown(),
                gdk::Key::BackSpace => {
                    type_into(&a, pane, "\u{7f}");
                    refill(&a, pane, &panel);
                }
                _ if ctrl && matches!(key, gdk::Key::c | gdk::Key::C) => {
                    panel.pop.popdown();
                    type_into(&a, pane, "\u{3}");
                }
                _ => match key.to_unicode().filter(|c| !c.is_control()) {
                    Some(c) if !ctrl && !mods.contains(gdk::ModifierType::ALT_MASK) => {
                        type_into(&a, pane, &c.to_string());
                        refill(&a, pane, &panel);
                    }
                    _ => return glib::Propagation::Proceed,
                },
            }
            glib::Propagation::Stop
        });
    }
    panel.pop.add_controller(keys);
    {
        let w = app.weak();
        let panel_ref = Rc::downgrade(&panel);
        panel.list.connect_row_activated(move |_, _| {
            if let (Some(a), Some(p)) = (w.upgrade(), panel_ref.upgrade()) {
                choose(&a, pane, &p, true);
            }
        });
    }
    {
        let t = term.clone();
        let keep = RefCell::new(Some(panel.clone()));
        panel.pop.connect_closed(move |pop| {
            t.grab_focus();
            let pop = pop.clone();
            let keep = keep.take();
            // Unparent after the close animation/signal has finished.
            glib::idle_add_local_once(move || {
                pop.unparent();
                drop(keep);
            });
        });
    }

    // Span the terminal's width, right under the prompt line (GTK flips it
    // above the line when there is no room below).
    let ch = term.char_height() as i32;
    let width = term.width();
    panel.pop.set_size_request((width - 16).max(320), -1);
    panel.pop.set_pointing_to(Some(&gdk::Rectangle::new(0, row as i32 * ch, width, ch)));
    panel.pop.popup();
    true
}

fn build(term: &vte4::Terminal) -> Panel {
    let title = gtk4::Label::new(Some("History"));
    title.add_css_class("history-title");
    title.set_xalign(0.0);
    title.set_hexpand(true);
    let hint = gtk4::Label::new(Some("↑↓ navigate   ↵ run   Tab insert   Esc dismiss"));
    hint.add_css_class("history-hint");
    let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    header.add_css_class("history-header");
    header.append(&title);
    header.append(&hint);

    let list = gtk4::ListBox::new();
    list.add_css_class("history-list");
    list.set_selection_mode(gtk4::SelectionMode::Single);
    list.set_activate_on_single_click(false);
    list.set_can_focus(false);
    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .propagate_natural_height(true)
        .child(&list)
        .build();
    let empty = gtk4::Label::new(Some("No matching commands"));
    empty.add_css_class("history-empty");
    empty.set_visible(false);

    let col = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    col.append(&header);
    col.append(&scroll);
    col.append(&empty);

    let pop = gtk4::Popover::builder()
        .child(&col)
        .has_arrow(false)
        .position(gtk4::PositionType::Bottom)
        .build();
    pop.add_css_class("history-panel");
    pop.set_parent(term);
    Panel {
        pop,
        list,
        scroll,
        empty,
        items: RefCell::new(Vec::new()),
    }
}

fn row_widget(cmd: &str, when: Option<i64>, now: i64) -> gtk4::ListBoxRow {
    let mark = gtk4::Label::new(Some("›"));
    mark.add_css_class("history-mark");
    let text = gtk4::Label::builder()
        .label(cmd)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .build();
    text.add_css_class("history-cmd");
    let b = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    b.append(&mark);
    b.append(&text);
    if let Some(t) = when {
        let age = gtk4::Label::new(Some(&relative_time(t, now)));
        age.add_css_class("history-age");
        b.append(&age);
    }
    let row = gtk4::ListBoxRow::new();
    row.set_child(Some(&b));
    row.set_tooltip_text(Some(cmd));
    row
}

fn fill(app: &AppRc, panel: &Panel, query: &str) {
    while let Some(r) = panel.list.row_at_index(0) {
        panel.list.remove(&r);
    }
    let now = now_secs();
    let (items, rows): (Vec<String>, Vec<gtk4::ListBoxRow>) = {
        let i = app.inner.borrow();
        // search() is best match / newest first; show it bottom-up like a shell.
        let mut found: Vec<&str> = i.history.search(query, MAX_ROWS);
        found.reverse();
        found
            .into_iter()
            .map(|c| (c.to_string(), row_widget(c, i.history.time_of(c), now)))
            .unzip()
    };
    for r in &rows {
        panel.list.append(r);
    }
    panel.empty.set_visible(items.is_empty());
    panel.scroll.set_visible(!items.is_empty());
    // Height: up to VISIBLE_ROWS rows, then scroll.
    let row_h = rows.first().map(|r| r.preferred_size().1.height()).unwrap_or(24).max(1);
    panel.scroll.set_max_content_height(row_h * VISIBLE_ROWS);
    *panel.items.borrow_mut() = items;
    if let Some(last) = rows.last() {
        panel.list.select_row(Some(last));
        reveal(panel, last);
    }
}

fn refill(app: &AppRc, pane: PaneId, panel: &Panel) {
    let line = {
        let i = app.inner.borrow();
        let Some(p) = i.panes.get(&pane) else { return };
        let t = p.suggest.tracker.borrow();
        t.line.clone()
    };
    fill(app, panel, &line);
}

/// Move the selection by `delta` rows; false when it would leave the list
/// at the bottom.
fn move_sel(panel: &Panel, delta: i32) -> bool {
    let n = panel.items.borrow().len() as i32;
    if n == 0 {
        return delta < 0;
    }
    let cur = panel.list.selected_row().map(|r| r.index()).unwrap_or(n - 1);
    let next = cur + delta;
    if next >= n && delta == 1 {
        return false;
    }
    let next = next.clamp(0, n - 1);
    if let Some(r) = panel.list.row_at_index(next) {
        panel.list.select_row(Some(&r));
        reveal(panel, &r);
    }
    true
}

/// Scroll so `row` is visible (after layout, so sizes are known).
fn reveal(panel: &Panel, row: &gtk4::ListBoxRow) {
    let scroll = panel.scroll.clone();
    let list = panel.list.clone();
    let row = row.clone();
    glib::idle_add_local_once(move || {
        let Some(b) = row.compute_bounds(&list) else { return };
        let adj = scroll.vadjustment();
        let (top, bottom) = (b.y() as f64, (b.y() + b.height()) as f64);
        if top < adj.value() {
            adj.set_value(top);
        } else if bottom > adj.value() + adj.page_size() {
            adj.set_value(bottom - adj.page_size());
        }
    });
}

/// Send `text` to the shell as if typed, keeping the line tracker in sync.
fn type_into(app: &AppRc, pane: PaneId, text: &str) {
    if text.ends_with('\r') {
        app.command_submitted(pane); // before the shell sees Enter
    }
    let submitted = {
        let i = app.inner.borrow();
        let Some(p) = i.panes.get(&pane) else { return };
        p.send_input(text.as_bytes());
        let ev = p.suggest.tracker.borrow_mut().feed(text);
        match ev {
            Event::Submitted(Some(cmd)) => Some(cmd),
            _ => None,
        }
    };
    if let Some(cmd) = submitted {
        app.inner.borrow_mut().history.push(&cmd);
    }
}

/// Replace the prompt line with the selected command; `run` also submits it.
fn choose(app: &AppRc, pane: PaneId, panel: &Panel, run: bool) {
    let Some(idx) = panel.list.selected_row().map(|r| r.index()) else {
        panel.pop.popdown();
        return;
    };
    let Some(cmd) = panel.items.borrow().get(idx as usize).cloned() else {
        return;
    };
    panel.pop.popdown();
    // Ctrl+U clears what was typed (the cursor is at the end: the tracker
    // would be invalid otherwise), then the command is typed for the user.
    let mut text = format!("\u{15}{cmd}");
    if run {
        text.push('\r');
    }
    type_into(app, pane, &text);
}
