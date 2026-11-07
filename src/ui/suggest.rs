//! Inline command suggestions (Warp/fish style "ghost text").
//!
//! The shell owns the input line, so cmux-linux follows what the user types
//! (VTE's `commit` signal = bytes sent to the shell). While the line is being
//! followed reliably and the shell is idle at its prompt, the newest history
//! entry starting with the typed text is shown dimmed after the cursor.
//! → or End accepts it (the rest of the command is typed for the user).
//! Anything ambiguous (Tab, arrows, a running program, scrolled view) hides it.

use super::AppRc;
use crate::history::{Event, LineTracker};
use crate::model::PaneId;
use gtk4::prelude::*;
use gtk4::{gdk, glib};
use std::cell::RefCell;
use std::rc::Rc;
use vte4::prelude::*;

pub struct Suggest {
    pub ghost: gtk4::Label,
    pub tracker: Rc<RefCell<LineTracker>>,
    /// Text that would be appended on accept.
    pub suffix: Rc<RefCell<Option<String>>>,
}

impl Suggest {
    pub fn new(overlay: &gtk4::Overlay) -> Suggest {
        let ghost = gtk4::Label::builder()
            .halign(gtk4::Align::Start)
            .valign(gtk4::Align::Start)
            .can_target(false)
            .visible(false)
            .build();
        ghost.add_css_class("ghost");
        overlay.add_overlay(&ghost);
        Suggest {
            ghost,
            tracker: Rc::new(RefCell::new(LineTracker::default())),
            suffix: Rc::new(RefCell::new(None)),
        }
    }
}

/// Row of the cursor within the visible screen: the last non-blank line of the
/// screen. Valid at a shell prompt, where nothing is printed below the input line.
fn cursor_screen_row(term: &vte4::Terminal) -> Option<i64> {
    let text = term.text_format(vte4::Format::Text)?;
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, _)| i as i64)
        .last()
}

fn at_bottom(term: &vte4::Terminal) -> bool {
    term.vadjustment().is_none_or(|a| a.value() + a.page_size() >= a.upper() - 0.5)
}

/// Recompute the suggestion for `pane` and show/hide the ghost text.
pub fn update(app: &AppRc, pane: PaneId) {
    let Ok(i) = app.inner.try_borrow() else { return };
    let Some(p) = i.panes.get(&pane) else { return };
    let s = &p.suggest;
    let hide = || {
        s.ghost.set_visible(false);
        *s.suffix.borrow_mut() = None;
    };
    if !i.config.terminal.suggestions || p.has_running_child() || !at_bottom(&p.term) {
        return hide();
    }
    let tracker = s.tracker.borrow();
    if !tracker.valid || tracker.line.trim().is_empty() {
        return hide();
    }
    let Some(full) = i.history.suggest(&tracker.line) else {
        return hide();
    };
    let mut rest = full[tracker.line.len()..].to_string();
    let (col, _) = p.term.cursor_position();
    let Some(row) = cursor_screen_row(&p.term) else { return hide() };
    // Only what fits on the current line.
    let room = (p.term.column_count() - col).max(0) as usize;
    if room == 0 {
        return hide();
    }
    if rest.chars().count() > room {
        rest = rest.chars().take(room.saturating_sub(1)).collect::<String>() + "…";
    }
    let cw = p.term.char_width() as i32;
    let ch = p.term.char_height() as i32;
    s.ghost.set_margin_start(col as i32 * cw + GHOST_PAD_X);
    s.ghost.set_margin_top(row as i32 * ch + GHOST_PAD_Y);
    let mut fd = gtk4::pango::FontDescription::from_string(&i.config.appearance.font);
    if fd.size() > 0 {
        fd.set_size((fd.size() as f64 * i.font_scale) as i32);
    }
    let attrs = gtk4::pango::AttrList::new();
    attrs.insert(gtk4::pango::AttrFontDesc::new(&fd));
    s.ghost.set_attributes(Some(&attrs));
    s.ghost.set_label(&rest);
    s.ghost.set_visible(true);
    *s.suffix.borrow_mut() = Some(full[tracker.line.len()..].to_string());
}

/// VTE draws the first cell this many pixels from the widget's edge.
const GHOST_PAD_X: i32 = 1;
const GHOST_PAD_Y: i32 = 1;

pub fn attach(app: &AppRc, pane: PaneId, term: &vte4::Terminal, s: &Suggest) {
    // Follow input sent to the shell.
    {
        let w = app.weak();
        let tracker = s.tracker.clone();
        term.connect_commit(move |_, text, _| {
            let ev = tracker.borrow_mut().feed(text);
            if let Some(a) = w.upgrade() {
                if let Event::Submitted(Some(cmd)) = ev {
                    if let Ok(mut i) = a.inner.try_borrow_mut() {
                        i.history.push(&cmd);
                    }
                }
            }
        });
    }
    // Reposition after the shell echoes (cursor moves / screen changes).
    for kind in 0..2 {
        let w = app.weak();
        let cb = move |_: &vte4::Terminal| {
            let w = w.clone();
            glib::idle_add_local_once(move || {
                if let Some(a) = w.upgrade() {
                    update(&a, pane);
                }
            });
        };
        if kind == 0 {
            term.connect_cursor_moved(cb);
        } else {
            term.connect_contents_changed(cb);
        }
    }
    // → / End accept the suggestion.
    let keys = gtk4::EventControllerKey::new();
    keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let w = app.weak();
    keys.connect_key_pressed(move |_, key, _, state| {
        let plain = (state & gtk4::accelerator_get_default_mod_mask()).is_empty();
        if !plain || !matches!(key, gdk::Key::Right | gdk::Key::End | gdk::Key::KP_Right) {
            return glib::Propagation::Proceed;
        }
        let Some(a) = w.upgrade() else { return glib::Propagation::Proceed };
        let taken = {
            let Ok(i) = a.inner.try_borrow() else {
                return glib::Propagation::Proceed;
            };
            let Some(p) = i.panes.get(&pane) else {
                return glib::Propagation::Proceed;
            };
            if !p.suggest.ghost.get_visible() {
                return glib::Propagation::Proceed;
            }
            let Some(rest) = p.suggest.suffix.borrow_mut().take() else {
                return glib::Propagation::Proceed;
            };
            p.suggest.ghost.set_visible(false);
            let before = p.suggest.tracker.borrow().line.len();
            p.send_input(rest.as_bytes());
            // Keep the tracker in sync whether or not VTE reports our own input via `commit`.
            let mut t = p.suggest.tracker.borrow_mut();
            if t.line.len() == before {
                t.line.push_str(&rest);
            }
            true
        };
        if taken {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    term.add_controller(keys);
}
