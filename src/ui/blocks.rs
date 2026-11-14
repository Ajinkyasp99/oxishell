//! Warp-style command blocks.
//!
//! Every command run at the shell prompt becomes a block: a full-width
//! separator line above it and a header row showing the folder and git branch
//! it ran in, then — once it finishes — ✓ or ✗ with the exit status and how
//! long it took. Failed blocks get a red accent along their left edge.
//!
//! VTE cannot draw this itself, so it is painted by a transparent widget laid
//! over the terminal. The shell leaves an empty row above each prompt
//! (`OXISHELL_BLOCKS=1`, see `shellint`) for the header. The end of a command is
//! seen through the prompt hook rewriting `$OXISHELL_ENV_FILE` (whose third line
//! is the exit status), so no extra process runs per command.
//!
//! Positions are kept as absolute VTE rows. The scroll adjustment counts from
//! the first *retained* scrollback row (`clear`, the scrollback limit and
//! programs that clear the scrollback with `ESC[3J` drop rows), so the offset
//! between the two is re-measured whenever the shell is idle at its prompt,
//! and re-anchored from the screen's text after any output (see
//! [`Blocks::resync`]), so headers never drift onto a running program's output.

use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, glib, graphene};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, SystemTime};
use vte4::prelude::*;

/// A block's position on screen (see `Blocks::placed`).
struct Placed {
    index: usize,
    top: f64,
    end: f64,
}

/// Blocks kept per pane (older ones scroll out of the scrollback anyway).
const MAX_BLOCKS: usize = 5000;
/// VTE draws the first cell this many pixels from the widget's top edge.
const PAD_Y: f64 = 1.0;

/// Where the prompt line is on the screen.
#[derive(Debug, PartialEq, Eq)]
pub struct PromptLine {
    /// First and last display row of the last non-blank line.
    pub start: usize,
    pub end: usize,
    /// The row just above it is empty (room for the block header).
    pub blank_above: bool,
}

/// Find the prompt line (the last non-blank line) in `screen` text. VTE joins
/// soft-wrapped rows, so each line takes as many rows as it wraps to.
pub fn prompt_line(screen: &str, cols: usize) -> Option<PromptLine> {
    let cols = cols.max(1);
    let mut row = 0;
    let mut found = None;
    let mut prev_blank_row: Option<usize> = None;
    for line in screen.split('\n') {
        let width: usize = line.chars().map(super::suggest::cell_width).sum();
        let rows = width.div_ceil(cols).max(1);
        if line.trim().is_empty() {
            prev_blank_row = Some(row + rows - 1);
        } else {
            found = Some(PromptLine {
                start: row,
                end: row + rows - 1,
                blank_above: row > 0 && prev_blank_row == Some(row - 1),
            });
            prev_blank_row = None;
        }
        row += rows;
    }
    found
}

/// "0.04s", "9.6s", "2m 05s", "1h 02m".
pub fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 1.0 {
        format!("{s:.2}s")
    } else if s < 60.0 {
        format!("{s:.1}s")
    } else if s < 3600.0 {
        let s = s as u64;
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        let s = s as u64;
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    }
}

/// Right side of the header: "✓ 0.04s" or "✗ exit 1 · 2.3s".
pub fn status_text(status: i32, d: Duration) -> String {
    if status == 0 {
        format!("✓ {}", fmt_duration(d))
    } else {
        format!("✗ exit {status} · {}", fmt_duration(d))
    }
}

struct Block {
    id: u64,
    /// Absolute row of the header (or of the prompt when there is no room).
    row: i64,
    has_header: bool,
    /// "~/proj  ⎇ main"
    place: String,
    /// (exit status, duration) once finished.
    result: Option<(i32, Duration)>,
}

pub struct Blocks {
    pub area: BlocksArea,
    term: glib::WeakRef<vte4::Terminal>,
    pid: Rc<Cell<Option<i32>>>,
    list: RefCell<Vec<Block>>,
    next_id: Cell<u64>,
    /// Absolute row of the first retained scrollback row.
    offset: Cell<Option<i64>>,
    /// Output arrived since `offset` was last confirmed: rows may have been
    /// dropped, so it may be stale.
    dirty: Cell<bool>,
    pub enabled: Cell<bool>,
    pub red: Cell<gdk::RGBA>,
    pub green: Cell<gdk::RGBA>,
}

impl Blocks {
    /// Create the overlay for `term`; add `blocks.area` to the pane's overlay.
    pub fn new(term: &vte4::Terminal, pid: Rc<Cell<Option<i32>>>) -> Rc<Blocks> {
        let area: BlocksArea = glib::Object::new();
        area.set_can_target(false);
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.add_css_class("blocks");
        let b = Rc::new(Blocks {
            area,
            term: term.downgrade(),
            pid,
            list: RefCell::new(Vec::new()),
            next_id: Cell::new(0),
            offset: Cell::new(None),
            dirty: Cell::new(false),
            enabled: Cell::new(true),
            red: Cell::new(gdk::RGBA::new(1.0, 0.42, 0.42, 1.0)),
            green: Cell::new(gdk::RGBA::new(0.49, 0.9, 0.53, 1.0)),
        });
        *b.area.imp().blocks.borrow_mut() = Rc::downgrade(&b);

        // Repaint when the text moves.
        let redraw = {
            let a = b.area.downgrade();
            move || {
                if let Some(a) = a.upgrade() {
                    a.queue_draw();
                }
            }
        };
        {
            let r = redraw.clone();
            let w = Rc::downgrade(&b);
            term.connect_contents_changed(move |_| {
                if let Some(b) = w.upgrade() {
                    b.dirty.set(true);
                }
                r();
            });
        }
        {
            let r = redraw.clone();
            term.connect_char_size_changed(move |_, _, _| r());
        }
        let hook_adj = {
            let r = redraw.clone();
            move |t: &vte4::Terminal| {
                if let Some(adj) = t.vadjustment() {
                    let r1 = r.clone();
                    adj.connect_value_changed(move |_| r1());
                    let r2 = r.clone();
                    adj.connect_changed(move |_| r2());
                }
            }
        };
        hook_adj(term);
        term.connect_notify_local(Some("vadjustment"), move |t, _| hook_adj(t));
        b
    }

    /// Status colours from the theme's palette (as the terminal shows it).
    pub fn apply_style(&self, cfg: &crate::config::Config, theme: &crate::theme::Theme) {
        self.enabled.set(cfg.terminal.command_blocks);
        let (normal, bright) = if cfg.terminal.vivid_colors {
            theme.vivid_palette()
        } else {
            (theme.normal, theme.bright)
        };
        let pal = if theme.is_dark() { bright } else { normal };
        let rgba = |c: crate::theme::Rgb| gdk::RGBA::new(c.0 as f32 / 255.0, c.1 as f32 / 255.0, c.2 as f32 / 255.0, 1.0);
        self.red.set(rgba(pal[1]));
        self.green.set(rgba(pal[2]));
        self.area.queue_draw();
    }

    fn running(&self, term: &vte4::Terminal) -> bool {
        let Some(pid) = self.pid.get() else { return false };
        let fd = term.pty().map(|p| std::os::fd::AsRawFd::as_raw_fd(&p.fd())).unwrap_or(-1);
        crate::meta::foreground_pgrp(fd).is_some_and(|pg| pg != pid)
    }

    fn at_bottom(term: &vte4::Terminal) -> bool {
        term.vadjustment().is_some_and(|a| a.value() + a.page_size() >= a.upper() - 0.5)
    }

    /// Re-measure the absolute↔scroll offset while the shell sits at its
    /// prompt (the cursor is then at the end of the last non-blank line).
    fn measure(&self, term: &vte4::Terminal) -> Option<PromptLine> {
        if self.running(term) || !Self::at_bottom(term) {
            return None;
        }
        let ch = term.char_height();
        let adj = term.vadjustment()?;
        let text = term.text_format(vte4::Format::Text)?;
        let line = prompt_line(&text, term.column_count() as usize)?;
        if ch <= 0 {
            return None;
        }
        let top = (adj.value() / ch as f64).round() as i64;
        let (_, cursor) = term.cursor_position();
        self.offset.set(Some(cursor - (top + line.end as i64)));
        self.dirty.set(false);
        let off = self.offset.get().unwrap_or(0);
        self.list.borrow_mut().retain(|b| b.row >= off);
        Some(line)
    }

    /// A command was submitted at the prompt of this pane. `on_done` gets its
    /// exit status and duration once the shell shows its next prompt (also
    /// when blocks are not drawn, for "command finished" notifications).
    pub fn on_submit(self: &Rc<Self>, cwd: Option<&Path>, branch: Option<&str>, env_file: PathBuf, on_done: Box<dyn Fn(i32, Duration)>) {
        let Some(term) = self.term.upgrade() else { return };
        if !self.enabled.get() {
            let id = self.next_id.get();
            self.next_id.set(id + 1);
            self.watch_finish(id, false, env_file, on_done);
            return;
        }
        let (_, cursor) = term.cursor_position();
        let (row, has_header) = match self.measure(&term) {
            Some(l) => {
                let start = cursor - (l.end - l.start) as i64;
                if l.blank_above {
                    (start - 1, true)
                } else {
                    (start, false)
                }
            }
            None => (cursor, false),
        };
        let mut place = cwd.map(crate::meta::display_path).unwrap_or_default();
        if let Some(b) = branch.filter(|b| !b.is_empty()) {
            place.push_str("   ⎇ ");
            place.push_str(b);
        }
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        {
            let mut list = self.list.borrow_mut();
            list.retain(|b| b.row < row); // re-run on the same rows (e.g. after Ctrl+L)
            list.push(Block {
                id,
                row,
                has_header,
                place,
                result: None,
            });
            let n = list.len();
            if n > MAX_BLOCKS {
                list.drain(..n - MAX_BLOCKS);
            }
        }
        self.area.queue_draw();
        self.watch_finish(id, true, env_file, on_done);
    }

    /// Wait for the prompt hook to rewrite the env file: the command ended.
    /// `drawn`: the command has a block in the list (it stops being watched if
    /// that block is dropped). A newer submission supersedes the watch too.
    fn watch_finish(self: &Rc<Self>, id: u64, drawn: bool, env_file: PathBuf, on_done: Box<dyn Fn(i32, Duration)>) {
        let started = SystemTime::now();
        // Compare with the previous prompt's write, not with `started`: file
        // times come from a coarse kernel clock (a few ms per tick), so a
        // quick command can finish "before" a fine-grained start time.
        let mtime = |f: &Path| std::fs::metadata(f).and_then(|m| m.modified()).ok();
        let before = mtime(&env_file);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(120), move || {
            let Some(b) = weak.upgrade() else { return glib::ControlFlow::Break };
            let superseded = b.next_id.get() != id + 1;
            if superseded || (drawn && !b.list.borrow().iter().any(|x| x.id == id)) {
                return glib::ControlFlow::Break;
            }
            let Some(t) = mtime(&env_file).filter(|t| Some(*t) != before) else {
                return glib::ControlFlow::Continue;
            };
            let status = std::fs::read_to_string(&env_file)
                .ok()
                .and_then(|s| crate::shellint::parse_status(&s))
                .unwrap_or(0);
            let took = t.duration_since(started).unwrap_or_default();
            if let Some(x) = b.list.borrow_mut().iter_mut().find(|x| x.id == id) {
                x.result = Some((status, took));
            }
            b.area.queue_draw();
            on_done(status, took);
            // Re-measure once the new prompt is on screen.
            let w = Rc::downgrade(&b);
            glib::timeout_add_local_once(Duration::from_millis(150), move || {
                if let Some(b) = w.upgrade() {
                    if let Some(t) = b.term.upgrade() {
                        b.measure(&t);
                    }
                    b.area.queue_draw();
                }
            });
            glib::ControlFlow::Break
        });
    }

    /// Confirm or correct `offset` from what is on screen, without needing a
    /// shell prompt (a program may be running). While scrolled to the bottom,
    /// the screen's top row is one of the `rows` rows ending at the cursor; the
    /// right one is where VTE's text for that range equals the visible text.
    /// Returns false when the offset cannot be confirmed now (scrolled up,
    /// ambiguous screen such as an empty one).
    fn resync(&self, term: &vte4::Terminal) -> bool {
        let Some(adj) = term.vadjustment() else { return false };
        let ch = term.char_height();
        if ch <= 0 || !Self::at_bottom(term) {
            return false;
        }
        let Some(visible) = term.text_format(vte4::Format::Text) else {
            return false;
        };
        let visible = visible.trim_end();
        let rows = term.row_count();
        let cols = term.column_count();
        let scrolled = (adj.value() / ch as f64).round() as i64;
        let (_, cursor) = term.cursor_position();
        let screen_starts_at = |top: i64| {
            top >= 0
                && term
                    .text_range_format(vte4::Format::Text, top as _, 0, (top + rows - 1) as _, cols as _)
                    .0
                    .is_some_and(|t| t.trim_end() == visible)
        };
        // Try the likeliest tops first and stop at the first match: the
        // current offset (usually nothing moved), then the cursor on the
        // screen's last row (where programs that print a lot keep it), then
        // higher. Another top could match too only on a screen that repeats
        // itself, where keeping the earlier choice is right.
        let current = self.offset.get().map(|off| off + scrolled);
        let lowest = cursor - rows + 1;
        let candidates = current
            .into_iter()
            .filter(|t| (lowest..=cursor).contains(t))
            .chain((lowest..=cursor).filter(|t| Some(*t) != current));
        for top in candidates {
            if screen_starts_at(top) {
                self.offset.set(Some(top - scrolled));
                self.dirty.set(false);
                return true;
            }
        }
        false
    }

    /// Header positions as they are drawn: (y in pixels, place text).
    pub fn visible_headers(&self) -> Vec<(f64, String)> {
        let Some(term) = self.term.upgrade() else { return Vec::new() };
        let list = self.list.borrow();
        self.placed(&term, self.area.height() as f64)
            .into_iter()
            .filter(|p| list[p.index].has_header)
            .map(|p| (p.top, list[p.index].place.clone()))
            .collect()
    }

    /// Where each block on screen is drawn, in pixels from the top.
    fn placed(&self, term: &vte4::Terminal, height: f64) -> Vec<Placed> {
        let Some(adj) = term.vadjustment() else {
            return Vec::new();
        };
        if self.offset.get().is_none() || !self.enabled.get() {
            return Vec::new();
        }
        // A full-screen program (vim, htop…) uses the alternate screen, which
        // has no scrollback: the blocks belong to the normal screen.
        if adj.upper() <= adj.page_size() + 0.5 && self.running(term) {
            return Vec::new();
        }
        let ch = term.char_height() as f64;
        if ch <= 0.0 {
            return Vec::new();
        }
        // After output the offset may be stale (rows dropped). Re-anchor it; if
        // that is not possible at the bottom, draw nothing rather than draw in
        // the wrong place. Scrolled up, the last known offset is kept.
        if self.dirty.get() && !self.resync(term) && Self::at_bottom(term) {
            return Vec::new();
        }
        let Some(offset) = self.offset.get() else {
            return Vec::new();
        };
        let value = adj.value();
        let y_of = |row: i64| (row - offset) as f64 * ch - value + PAD_Y;
        let (_, cursor) = term.cursor_position();
        let list = self.list.borrow();
        let mut out = Vec::new();
        for (index, b) in list.iter().enumerate() {
            let top = y_of(b.row);
            // The block runs until the next one (or the current prompt row).
            let end = match list.get(index + 1) {
                Some(n) => y_of(n.row),
                None if b.result.is_some() => y_of(cursor) - ch,
                None => y_of(cursor) + ch,
            };
            if end < 0.0 || top > height {
                continue;
            }
            out.push(Placed { index, top, end });
        }
        out
    }

    fn draw(&self, area: &BlocksArea, snap: &gtk4::Snapshot) {
        let Some(term) = self.term.upgrade() else { return };
        let placed = self.placed(&term, area.height() as f64);
        if placed.is_empty() {
            return;
        }
        let ch = term.char_height() as f64;
        let width = area.width() as f32;
        let fg = area.color();
        let with_alpha = |c: gdk::RGBA, a: f32| gdk::RGBA::new(c.red(), c.green(), c.blue(), a);
        let (red, green) = (self.red.get(), self.green.get());
        let list = self.list.borrow();
        for &Placed { index, top, end } in &placed {
            let b = &list[index];
            let failed = matches!(b.result, Some((s, _)) if s != 0);
            let rect = |x: f32, y: f64, w: f32, h: f64| graphene::Rect::new(x, y as f32, w, h as f32);
            if failed {
                snap.append_color(&with_alpha(red, 0.06), &rect(0.0, top, width, end - top));
                snap.append_color(&with_alpha(red, 0.75), &rect(0.0, top, 3.0, end - top));
            }
            let line = if failed { with_alpha(red, 0.55) } else { with_alpha(fg, 0.14) };
            snap.append_color(&line, &rect(0.0, top, width, 1.0));
            if !b.has_header {
                continue;
            }
            let text = |s: &str, x: f32, right: bool, color: gdk::RGBA| {
                let layout = area.create_pango_layout(Some(s));
                let (w, h) = layout.pixel_size();
                let x = if right { x - w as f32 } else { x };
                snap.save();
                snap.translate(&graphene::Point::new(x, (top + (ch - h as f64) / 2.0) as f32));
                snap.append_layout(&layout, &color);
                snap.restore();
            };
            if !b.place.is_empty() {
                text(&b.place, 10.0, false, with_alpha(fg, 0.5));
            }
            if let Some((status, took)) = b.result {
                let color = if status == 0 {
                    with_alpha(green, 0.85)
                } else {
                    with_alpha(red, 0.95)
                };
                text(&status_text(status, took), width - 18.0, true, color);
            }
        }
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct BlocksArea {
        pub blocks: RefCell<Weak<super::Blocks>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BlocksArea {
        const NAME: &'static str = "OxishellBlocksArea";
        type Type = super::BlocksArea;
        type ParentType = gtk4::Widget;
    }

    impl ObjectImpl for BlocksArea {}

    impl WidgetImpl for BlocksArea {
        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let blocks = self.blocks.borrow().upgrade();
            if let Some(b) = blocks {
                b.draw(&self.obj(), snapshot);
            }
        }
    }
}

glib::wrapper! {
    /// Transparent overlay that paints the command blocks.
    pub struct BlocksArea(ObjectSubclass<imp::BlocksArea>) @extends gtk4::Widget;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_prompt_line_and_header_room() {
        // Output, empty spacer row, prompt.
        assert_eq!(
            prompt_line("out\n\n❯ git status\n\n", 80),
            Some(PromptLine {
                start: 2,
                end: 2,
                blank_above: true
            })
        );
        // No spacer (output without a trailing newline, or blocks off).
        assert_eq!(
            prompt_line("out\n❯ ls", 80),
            Some(PromptLine {
                start: 1,
                end: 1,
                blank_above: false
            })
        );
        // A wrapped command takes several rows; a wrapped line above counts too.
        let screen = format!("{}\n\n❯ {}", "x".repeat(25), "y".repeat(15));
        assert_eq!(
            prompt_line(&screen, 10),
            Some(PromptLine {
                start: 4,
                end: 5,
                blank_above: true
            })
        );
        // Spacer at the very top (first prompt, or after `clear`).
        assert_eq!(
            prompt_line("\n❯ ", 80),
            Some(PromptLine {
                start: 1,
                end: 1,
                blank_above: true
            })
        );
        assert_eq!(prompt_line("\n\n", 80), None);
    }

    #[test]
    fn formats_durations_and_status() {
        assert_eq!(fmt_duration(Duration::from_millis(44)), "0.04s");
        assert_eq!(fmt_duration(Duration::from_millis(9597)), "9.6s");
        assert_eq!(fmt_duration(Duration::from_secs(125)), "2m 05s");
        assert_eq!(fmt_duration(Duration::from_secs(3720)), "1h 02m");
        assert_eq!(status_text(0, Duration::from_millis(36)), "✓ 0.04s");
        assert_eq!(status_text(1, Duration::from_millis(2300)), "✗ exit 1 · 2.3s");
    }
}
