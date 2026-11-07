//! One terminal pane: a VTE terminal (which owns the PTY and child process),
//! a search bar, and process lifecycle helpers.

use super::{App, AppRc};
use crate::agents::AgentState;
use crate::config::Config;
use crate::meta::GitInfo;
use crate::model::{PaneId, WorkspaceId};
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib, Orientation};
use std::cell::Cell;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vte4::prelude::*;

pub struct Pane {
    pub id: PaneId,
    pub frame: gtk4::Box,
    pub term: vte4::Terminal,
    pub search: gtk4::SearchBar,
    pub pid: Rc<Cell<Option<i32>>>,
    pub cwd: Option<PathBuf>,
    pub git: Option<GitInfo>,
    pub ports: Vec<u16>,
    /// Agent program detected in the foreground (process name only).
    pub fg_agent: Option<&'static str>,
    /// Last state reported by agent hooks: (agent, state, message).
    pub agent: Option<(String, AgentState, Option<String>)>,
    /// (agent, session id) reported by hooks, offered for explicit resume.
    pub agent_session: Option<(String, String)>,
    pub last_bell: Cell<Option<Instant>>,
    /// Input sent before the shell finished spawning; flushed once it has.
    pub pending_input: Rc<std::cell::RefCell<Option<Vec<u8>>>>,
    /// Active Python/conda environment reported by shell integration.
    pub python_env: Option<String>,
    pub chips: super::chips::Chips,
    pub suggest: super::suggest::Suggest,
    pub blocks: Rc<super::blocks::Blocks>,
}

pub(super) const PCRE2_CASELESS: u32 = 0x0000_0008;
pub(super) const PCRE2_MULTILINE: u32 = 0x0000_0400;
pub(super) const PCRE2_UTF: u32 = 0x0008_0000;

fn regex_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        if "\\^$.|?*+()[]{}/".contains(c) {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

impl Pane {
    /// Type `bytes` into the pane, queueing them if the shell is still starting.
    pub fn send_input(&self, bytes: &[u8]) {
        let mut q = self.pending_input.borrow_mut();
        match q.as_mut() {
            Some(buf) => buf.extend_from_slice(bytes),
            None => self.term.feed_child(bytes),
        }
    }

    pub fn pty_fd(&self) -> i32 {
        self.term.pty().map(|p| p.fd().as_raw_fd()).unwrap_or(-1)
    }

    /// True when a program other than the shell holds the terminal's foreground.
    pub fn has_running_child(&self) -> bool {
        let Some(pid) = self.pid.get() else { return false };
        match crate::meta::foreground_pgrp(self.pty_fd()) {
            Some(pg) => pg != pid,
            None => false,
        }
    }

    pub fn agent_label(&self) -> Option<String> {
        if let Some((a, s, _)) = &self.agent {
            if *s != AgentState::Ended {
                return Some(format!("{} — {}", crate::agents::display_name(a), s.label()));
            }
        }
        self.fg_agent.map(|a| format!("{} (running)", crate::agents::display_name(a)))
    }

    pub fn hangup(&self) {
        if let Some(pid) = self.pid.get() {
            unsafe {
                libc::kill(-pid, libc::SIGHUP);
                libc::kill(pid, libc::SIGHUP);
            }
        }
    }

    /// End the pane's processes: SIGHUP the process group, then SIGKILL after
    /// 3 s if anything ignored the hangup.
    pub fn terminate(&self) {
        self.hangup();
        if let Some(pid) = self.pid.get() {
            glib::timeout_add_local_once(Duration::from_secs(3), move || unsafe {
                if libc::kill(pid, 0) == 0 {
                    libc::kill(-pid, libc::SIGKILL);
                    libc::kill(pid, libc::SIGKILL);
                }
            });
        }
    }

    /// The last `max_lines` lines of content (scrollback + screen), ignoring
    /// the empty rows below the cursor.
    pub fn scrollback_text(&self, max_lines: usize) -> String {
        // VTE's text ranges use absolute row numbers, while the scroll adjustment
        // is relative to retained scrollback. Anchor on the cursor's absolute row.
        let rows = self.term.row_count();
        let (_, cursor_row) = self.term.cursor_position();
        let end = cursor_row + rows; // covers full-screen programs drawing below the cursor
                                     // Rows older than the retained scrollback are clamped away by VTE.
        let start = (end - max_lines as i64 - 2 * rows).max(0);
        let (text, _) = self
            .term
            .text_range_format(vte4::Format::Text, start as _, 0, end as _, self.term.column_count() as _);
        let text = text.map(|t| t.to_string()).unwrap_or_default();
        let lines: Vec<&str> = text.trim_end().lines().collect();
        lines[lines.len().saturating_sub(max_lines)..].join("\n")
    }
}

pub fn detach(w: &impl IsA<gtk4::Widget>) {
    let w = w.as_ref();
    let Some(parent) = w.parent() else { return };
    if let Some(p) = parent.downcast_ref::<gtk4::Paned>() {
        if p.start_child().as_ref() == Some(w) {
            p.set_start_child(gtk4::Widget::NONE);
        } else {
            p.set_end_child(gtk4::Widget::NONE);
        }
    } else if let Some(b) = parent.downcast_ref::<gtk4::Box>() {
        b.remove(w);
    } else {
        w.unparent();
    }
}

pub fn style_terminal(t: &vte4::Terminal, cfg: &Config, scale: f64, theme: &crate::theme::Theme) {
    t.set_font_desc(Some(&gtk4::pango::FontDescription::from_string(&cfg.appearance.font)));
    t.set_font_scale(scale);
    t.set_scrollback_lines(cfg.terminal.scrollback_lines);
    let c = |c: crate::theme::Rgb, a: f32| gdk::RGBA::new(c.0 as f32 / 255.0, c.1 as f32 / 255.0, c.2 as f32 / 255.0, a);
    // Gradient/image themes paint the window background; the terminal stays transparent over it.
    let bg_alpha = if theme.layered_background() { 0.0 } else { 1.0 };
    let (normal, bright) = if cfg.terminal.vivid_colors {
        theme.vivid_palette()
    } else {
        (theme.normal, theme.bright)
    };
    let pal: Vec<gdk::RGBA> = normal.iter().chain(bright.iter()).map(|x| c(*x, 1.0)).collect();
    let refs: Vec<&gdk::RGBA> = pal.iter().collect();
    t.set_colors(
        Some(&c(theme.foreground, 1.0)),
        Some(&c(theme.background.average(), bg_alpha)),
        &refs,
    );
    // VTE paints its background unless told not to; let the window gradient show through.
    t.set_clear_background(!theme.layered_background());
    t.set_color_cursor(theme.cursor.map(|x| c(x, 1.0)).as_ref());
    // Programs like vim may still change the shape temporarily (DECSCUSR).
    t.set_cursor_shape(match cfg.terminal.cursor_shape.as_str() {
        "ibeam" => vte4::CursorShape::Ibeam,
        "underline" => vte4::CursorShape::Underline,
        _ => vte4::CursorShape::Block,
    });
    t.set_cursor_blink_mode(if cfg.terminal.cursor_blink {
        vte4::CursorBlinkMode::On
    } else {
        vte4::CursorBlinkMode::Off
    });
}

impl App {
    /// Create the widgets and process for pane `id`. `restored` is
    /// sanitized text from a previous session, shown read-only above the prompt.
    pub fn create_pane(self: &AppRc, wid: WorkspaceId, id: PaneId, cwd: Option<PathBuf>, restored: Option<String>) {
        let term = vte4::Terminal::new();
        term.set_hexpand(true);
        term.set_vexpand(true);
        term.set_allow_hyperlink(true);
        term.set_audible_bell(false);
        term.set_mouse_autohide(true);
        term.set_scroll_on_keystroke(true);
        term.set_scroll_on_output(false);
        // GtkScrolledWindow owns history scrolling and touchpad inertia. Its
        // adjustment must use pixels; VTE still handles application mouse input
        // and alternate-screen arrow scrolling before propagating history events.
        term.set_scroll_unit_is_pixels(true);
        term.set_enable_fallback_scrolling(false);
        super::scroll::attach(&term);
        {
            let i = self.inner.borrow();
            style_terminal(&term, &i.config, i.font_scale, &i.theme);
        }

        let scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .child(&term)
            .hexpand(true)
            .vexpand(true)
            .build();
        super::zoom::attach_scroll(self, &scroll);

        let entry = gtk4::SearchEntry::builder()
            .placeholder_text("Search scrollback")
            .hexpand(true)
            .build();
        let search = gtk4::SearchBar::builder().child(&entry).show_close_button(true).build();
        search.connect_entry(&entry);
        {
            let t = term.clone();
            entry.connect_search_changed(move |e| {
                let text = e.text();
                if text.is_empty() {
                    t.search_set_regex(None, 0);
                    return;
                }
                if let Ok(re) = vte4::Regex::for_search(&regex_escape(&text), PCRE2_UTF | PCRE2_MULTILINE | PCRE2_CASELESS) {
                    t.search_set_regex(Some(&re), 0);
                    t.search_set_wrap_around(true);
                    t.search_find_previous();
                }
            });
        }
        {
            let t = term.clone();
            entry.connect_activate(move |_| {
                t.search_find_previous();
            });
        }
        {
            let t = term.clone();
            entry.connect_next_match(move |_| {
                t.search_find_next();
            });
        }
        {
            let t = term.clone();
            entry.connect_previous_match(move |_| {
                t.search_find_previous();
            });
        }
        {
            let t = term.clone();
            let s = search.clone();
            entry.connect_stop_search(move |_| {
                s.set_search_mode(false);
                t.grab_focus();
            });
        }

        let frame = gtk4::Box::new(Orientation::Vertical, 0);
        frame.add_css_class("pane");
        frame.set_hexpand(true);
        frame.set_vexpand(true);
        let chips = super::chips::Chips::new();
        frame.append(&search);
        frame.append(&chips.bar);
        // Overlay so inline suggestions can be drawn over the terminal.
        let overlay = gtk4::Overlay::new();
        overlay.set_child(Some(&scroll));
        let suggest = super::suggest::Suggest::new(&overlay);
        super::suggest::attach(self, id, &term, &suggest);
        super::history_panel::attach(self, id, &term);
        frame.append(&overlay);

        // Pane context menu.
        let menu = gio::Menu::new();
        let target = Some(id.to_variant());
        let item = |label: &str, action: &str| {
            let it = gio::MenuItem::new(Some(label), None);
            it.set_action_and_target_value(Some(action), target.as_ref());
            it
        };
        let edit = gio::Menu::new();
        edit.append_item(&item("Copy", "app.pane-copy"));
        edit.append_item(&item("Paste", "app.pane-paste"));
        edit.append_item(&item("Search…", "app.pane-search"));
        let layout = gio::Menu::new();
        layout.append_item(&item("Split Right", "app.pane-split-right"));
        layout.append_item(&item("Split Down", "app.pane-split-down"));
        let agent = gio::Menu::new();
        agent.append_item(&item("Resume Agent Session", "app.pane-resume-agent"));
        let close = gio::Menu::new();
        close.append_item(&item("Close Pane", "app.pane-close"));
        menu.append_section(None, &edit);
        menu.append_section(None, &layout);
        menu.append_section(None, &agent);
        menu.append_section(None, &close);
        term.set_context_menu_model(Some(&menu));
        super::links::attach(self, &term, &menu);

        let pid_cell: Rc<Cell<Option<i32>>> = Rc::new(Cell::new(None));
        let pending: Rc<std::cell::RefCell<Option<Vec<u8>>>> = Rc::new(std::cell::RefCell::new(Some(Vec::new())));
        let blocks = super::blocks::Blocks::new(&term, pid_cell.clone());
        overlay.add_overlay(&blocks.area);
        {
            let i = self.inner.borrow();
            blocks.apply_style(&i.config, &i.theme);
        }

        // Signals: defer to idle so handlers never run while the model is borrowed.
        {
            let w = self.weak();
            term.connect_child_exited(move |_, _status| {
                let w = w.clone();
                glib::idle_add_local_once(move || {
                    if let Some(a) = w.upgrade() {
                        a.on_child_exited(id);
                    }
                });
            });
        }
        {
            let w = self.weak();
            let focus = gtk4::EventControllerFocus::new();
            focus.connect_enter(move |_| {
                let w = w.clone();
                glib::idle_add_local_once(move || {
                    if let Some(a) = w.upgrade() {
                        a.on_pane_focused(id);
                    }
                });
            });
            term.add_controller(focus);
        }
        {
            let w = self.weak();
            term.connect_bell(move |_| {
                let w = w.clone();
                glib::idle_add_local_once(move || {
                    let Some(a) = w.upgrade() else { return };
                    let allowed = {
                        let i = a.inner.borrow();
                        let Some(p) = i.panes.get(&id) else { return };
                        let recent = p.last_bell.get().is_some_and(|t| t.elapsed() < Duration::from_secs(5));
                        if !recent {
                            p.last_bell.set(Some(Instant::now()));
                        }
                        i.config.notifications.bell_attention && !recent
                    };
                    if allowed {
                        a.notify(Some(id), "Bell", "A program rang the terminal bell", "bell");
                    }
                });
            });
        }
        {
            let w = self.weak();
            term.connect_window_title_changed(move |_| {
                if let Some(a) = w.upgrade() {
                    super::sidebar::rebuild(&a);
                }
            });
        }

        // Restored output is display-only and already sanitized.
        if let Some(text) = restored {
            let mut buf = text.replace('\n', "\r\n").into_bytes();
            buf.extend_from_slice(b"\r\n\x1b[2m--- restored output above (not live) ---\x1b[0m\r\n");
            term.feed(&buf);
        }

        // Spawn the user's shell.
        let shell = self.inner.borrow().config.shell();
        let cwd = cwd.filter(|d| d.is_dir()).unwrap_or_else(crate::paths::home);
        let cwd_s = cwd.display().to_string();
        let argv: Vec<String> = vec![shell];
        let mut env = vec![
            format!("CMUX_PANE_ID={id}"),
            format!("CMUX_WORKSPACE_ID={wid}"),
            format!("CMUX_SOCKET={}", crate::paths::socket_path().display()),
            "TERM_PROGRAM=cmux-linux".to_string(),
            format!("TERM_PROGRAM_VERSION={}", env!("CARGO_PKG_VERSION")),
        ];
        // Context chips: the shell reports its active Python/conda env to this file.
        let env_file = crate::shellint::env_file(id);
        if let Some(dir) = env_file.parent() {
            let _ = crate::paths::ensure_private_dir(dir);
        }
        let _ = std::fs::remove_file(&env_file);
        env.push(format!("CMUX_ENV_FILE={}", env_file.display()));
        if self.inner.borrow().config.terminal.compact_prompt {
            env.push("CMUX_COMPACT_PROMPT=1".to_string());
        }
        if self.inner.borrow().config.terminal.highlight_commands {
            env.push("CMUX_HIGHLIGHT_CMD=1".to_string());
        }
        if self.inner.borrow().config.terminal.command_blocks {
            env.push("CMUX_BLOCKS=1".to_string());
        }
        if crate::shellint::shell_name(&argv[0]) == "bash" {
            env.push(format!("PROMPT_COMMAND={}", crate::shellint::BASH_PROMPT_COMMAND));
        }
        // Make the bundled `cmux` CLI reachable for hooks when running from a build dir.
        if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
            if dir.join("cmux").is_file() {
                let path = std::env::var("PATH").unwrap_or_default();
                env.push(format!("PATH={}:{path}", dir.display()));
            }
        }
        let argv_ref: Vec<&str> = argv.iter().map(String::as_str).collect();
        let env_ref: Vec<&str> = env.iter().map(String::as_str).collect();
        {
            let pid_cell = pid_cell.clone();
            let pending = pending.clone();
            let t = term.clone();
            let w = self.weak();
            term.spawn_async(
                vte4::PtyFlags::DEFAULT,
                Some(&cwd_s),
                &argv_ref,
                &env_ref,
                glib::SpawnFlags::SEARCH_PATH,
                || {},
                -1,
                gio::Cancellable::NONE,
                move |res| match res {
                    Ok(pid) => {
                        pid_cell.set(Some(pid.0));
                        // Shells can lose typeahead while startup files run, so queued
                        // input is delivered only when the shell is idle at its prompt:
                        // it has printed output, output has been quiet for 200 ms, and
                        // the line editor switched the tty out of canonical mode.
                        // After 5 s it is delivered regardless.
                        let started = Instant::now();
                        let last_out: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
                        let hid = {
                            let last_out = last_out.clone();
                            t.connect_contents_changed(move |_| last_out.set(Some(Instant::now())))
                        };
                        let hid = Cell::new(Some(hid));
                        let pending = pending.clone();
                        let t = t.clone();
                        glib::timeout_add_local(Duration::from_millis(40), move || {
                            let canonical = t.pty().is_some_and(|p| unsafe {
                                let mut tio: libc::termios = std::mem::zeroed();
                                libc::tcgetattr(p.fd().as_raw_fd(), &mut tio) != 0 || tio.c_lflag & libc::ICANON != 0
                            });
                            let quiet = last_out.get().is_some_and(|i| i.elapsed() >= Duration::from_millis(200));
                            if (quiet && !canonical) || started.elapsed() > Duration::from_secs(5) {
                                if let Some(id) = hid.take() {
                                    t.disconnect(id);
                                }
                                if let Some(buf) = pending.borrow_mut().take() {
                                    if !buf.is_empty() {
                                        t.feed_child(&buf);
                                    }
                                }
                                return glib::ControlFlow::Break;
                            }
                            glib::ControlFlow::Continue
                        });
                        if let Some(a) = w.upgrade() {
                            a.request_meta(true);
                        }
                    }
                    Err(e) => {
                        if let Some(a) = w.upgrade() {
                            a.toast(&format!("Failed to start shell: {e}"));
                        }
                    }
                },
            );
        }

        self.inner.borrow_mut().panes.insert(
            id,
            Pane {
                id,
                frame,
                term,
                search,
                pid: pid_cell,
                cwd: Some(cwd),
                git: None,
                ports: vec![],
                fg_agent: None,
                agent: None,
                agent_session: None,
                last_bell: Cell::new(None),
                pending_input: pending,
                python_env: None,
                chips,
                suggest,
                blocks,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_regex_metacharacters() {
        assert_eq!(super::regex_escape("a.b*(c)"), "a\\.b\\*\\(c\\)");
    }
}
