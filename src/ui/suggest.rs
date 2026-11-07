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

/// Row of the cursor within the visible screen: the last display row of the
/// last non-blank line. Valid at a shell prompt, where nothing is printed
/// below the input line.
///
/// (The absolute cursor row can't be used: the scroll adjustment is relative
/// to the retained scrollback, and `clear` drops scrollback rows.)
pub(super) fn cursor_screen_row(term: &vte4::Terminal) -> Option<i64> {
    let text = term.text_format(vte4::Format::Text)?;
    last_line_row(&text, term.column_count().max(1) as usize).map(|r| r as i64)
}

/// Display row of the end of the last non-blank line of `screen` text, where
/// each line takes as many rows as it wraps to (VTE joins soft-wrapped rows
/// into one line of text).
pub(super) fn last_line_row(screen: &str, cols: usize) -> Option<usize> {
    let mut row = 0;
    let mut last = None;
    for line in screen.split('\n') {
        let width: usize = line.chars().map(cell_width).sum();
        let rows = width.div_ceil(cols).max(1);
        if !line.trim().is_empty() {
            last = Some(row + rows - 1);
        }
        row += rows;
    }
    last
}

/// Terminal cells taken by `c` (common wide and zero-width ranges).
fn cell_width(c: char) -> usize {
    match c as u32 {
        0x300..=0x36F | 0x200B..=0x200F | 0xFE00..=0xFE0F => 0,
        0x1100..=0x115F
        | 0x2E80..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

pub(super) fn at_bottom(term: &vte4::Terminal) -> bool {
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

#[cfg(test)]
mod tests {
    use super::last_line_row;

    #[test]
    fn prompt_row_counts_wrapped_lines() {
        // 10 columns: the 25-char line wraps to 3 rows (0..=2), prompt on row 3.
        let screen = format!("{}\n❯ git\n\n\n", "x".repeat(25));
        assert_eq!(last_line_row(&screen, 10), Some(3));
        // A line of exactly the width takes one row.
        assert_eq!(last_line_row("0123456789\n❯ ", 10), Some(1));
        // Blank lines in between count; trailing blanks don't.
        assert_eq!(last_line_row("a\n\n\n❯ ls\n\n", 80), Some(3));
        // Wide characters take two cells.
        assert_eq!(last_line_row("日本語日本\n❯", 8), Some(2));
        assert_eq!(last_line_row("\n\n", 80), None);
    }
}
