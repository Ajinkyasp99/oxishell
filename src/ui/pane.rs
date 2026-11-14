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
    /// Program name of the pane's shell ("bash", "zsh", "fish", ...).
    pub shell: String,
    pub cwd: Option<PathBuf>,
    pub git: Option<GitInfo>,
    pub ports: Vec<u16>,
    /// Agent program detected in the foreground (process name only).
    pub fg_agent: Option<&'static str>,
    /// Last state reported by agent hooks: (agent, state, message).
    pub agent: Option<(String, AgentState, Option<String>)>,
    /// When the agent's current turn started (first Running report).
    pub agent_since: Option<Instant>,
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
    // Gradient/image themes and a see-through window paint the background
    // behind the terminal; the terminal stays transparent over it.
    let transparent = theme.transparent_terminal(cfg.appearance.opacity);
    let bg_alpha = if transparent { 0.0 } else { 1.0 };
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
    t.set_clear_background(!transparent);
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
        // A hidden SearchBar still reserves its entry's width; keep that small so
        // narrow panes are not forced wider than their share of the window.
        entry.set_width_chars(4);
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
        // Clip the chip row instead of letting it set the pane's minimum width.
        let chips_clip = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::External)
            .vscrollbar_policy(gtk4::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&chips.bar)
            .build();
        chips.bar.bind_property("visible", &chips_clip, "visible").sync_create().build();
        // Overlay so inline suggestions can be drawn over the terminal.
        let overlay = gtk4::Overlay::new();
        overlay.set_child(Some(&scroll));
        let suggest = super::suggest::Suggest::new(&overlay);
        super::suggest::attach(self, id, &term, &suggest);
        super::history_panel::attach(self, id, &term);
        let body = gtk4::Box::new(Orientation::Vertical, 0);
        body.append(&chips_clip);
        body.append(&overlay);
        // Close button in the pane's top-right corner, level with the chip row.
        // Same path as the context menu's "Close Pane": idle panes close at once,
        // a running program asks first.
        let close_btn = gtk4::Button::from_icon_name("window-close-symbolic");
        close_btn.add_css_class("flat");
        close_btn.add_css_class("circular");
        close_btn.add_css_class("pane-close");
        close_btn.set_tooltip_text(Some("Close pane"));
        close_btn.set_focus_on_click(false);
        close_btn.set_can_focus(false);
        close_btn.set_halign(gtk4::Align::End);
        close_btn.set_valign(gtk4::Align::Start);
        close_btn.set_action_name(Some("app.pane-close"));
        close_btn.set_action_target_value(Some(&id.to_variant()));
        // Drag handle left of the close button: drop it on another pane to swap
        // the two panes' places.
        let handle = gtk4::Image::from_icon_name(super::icons::pane_move());
        handle.add_css_class("pane-drag");
        handle.set_tooltip_text(Some("Drag onto another pane to swap places"));
        handle.set_cursor_from_name(Some("grab"));
        let actions = gtk4::Box::new(Orientation::Horizontal, 0);
        actions.set_halign(gtk4::Align::End);
        actions.set_valign(gtk4::Align::Start);
        actions.append(&handle);
        actions.append(&close_btn);
        // Fades in over the pane under the pointer while another pane is
        // dragged (CSS class "active"); invisible and click-through otherwise.
        let drop_hint = gtk4::Box::new(Orientation::Vertical, 0);
        drop_hint.add_css_class("pane-drop-hint");
        drop_hint.set_can_target(false);
        let hint_label = gtk4::Label::new(Some("Swap here"));
        hint_label.add_css_class("pane-drop-hint-label");
        hint_label.set_vexpand(true);
        hint_label.set_valign(gtk4::Align::Center);
        hint_label.set_halign(gtk4::Align::Center);
        drop_hint.append(&hint_label);
        let top = gtk4::Overlay::new();
        top.set_child(Some(&body));
        top.add_overlay(&drop_hint);
        top.add_overlay(&actions);
        frame.append(&top);
        attach_swap_dnd(self, id, &handle, &frame, &drop_hint);

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
        super::links::attach(self, id, &term, &menu);

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
            format!("OXISHELL_PANE_ID={id}"),
            format!("OXISHELL_WORKSPACE_ID={wid}"),
            format!("OXISHELL_SOCKET={}", crate::paths::socket_path().display()),
            "TERM_PROGRAM=oxishell".to_string(),
            format!("TERM_PROGRAM_VERSION={}", env!("CARGO_PKG_VERSION")),
        ];
        // Context chips: the shell reports its active Python/conda env to this file.
        let env_file = crate::shellint::env_file(id);
        if let Some(dir) = env_file.parent() {
            let _ = crate::paths::ensure_private_dir(dir);
        }
        let _ = std::fs::remove_file(&env_file);
        env.push(format!("OXISHELL_ENV_FILE={}", env_file.display()));
        if self.inner.borrow().config.terminal.compact_prompt {
            env.push("OXISHELL_COMPACT_PROMPT=1".to_string());
        }
        if self.inner.borrow().config.terminal.highlight_commands {
            env.push("OXISHELL_HIGHLIGHT_CMD=1".to_string());
        }
        if self.inner.borrow().config.terminal.command_blocks {
            env.push("OXISHELL_BLOCKS=1".to_string());
        }
        let shell_name = crate::shellint::shell_name(&argv[0]).to_string();
        if shell_name == "bash" {
            env.push(format!("PROMPT_COMMAND={}", crate::shellint::BASH_PROMPT_COMMAND));
        }
        // Make the bundled `oxi` CLI reachable for hooks when running from a build dir.
        if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
            if dir.join("oxi").is_file() {
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
                shell: shell_name,
                cwd: Some(cwd),
                git: None,
                ports: vec![],
                fg_agent: None,
                agent: None,
                agent_since: None,
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

/// Prefix of the string a pane drag carries. Drags go through the display
/// protocol even inside the app, so the payload must be a string (a raw
/// integer has no MIME type and would never match a drop target).
const PANE_DRAG_PREFIX: &str = "oxishell-pane:";

/// Drag-to-swap: `handle` starts a drag carrying pane `id`; `frame` accepts a
/// drop of any other pane and swaps the two. Workspace rows reject the pane
/// string (not a workspace id) and panes reject workspace drags.
fn attach_swap_dnd(app: &AppRc, id: PaneId, handle: &gtk4::Image, frame: &gtk4::Box, hint: &gtk4::Box) {
    let src = gtk4::DragSource::new();
    src.set_actions(gdk::DragAction::MOVE);
    {
        let w = app.weak();
        src.connect_prepare(move |_, _, _| {
            let a = w.upgrade()?;
            a.dragging_pane.set(Some(id));
            Some(gdk::ContentProvider::for_value(&format!("{PANE_DRAG_PREFIX}{id}").to_value()))
        });
    }
    {
        let w = app.weak();
        let f = frame.clone();
        src.connect_drag_begin(move |_, drag| {
            f.add_css_class("pane-dragging");
            let name = w
                .upgrade()
                .and_then(|a| {
                    let i = a.inner.borrow();
                    let cwd = i.panes.get(&id)?.cwd.clone()?;
                    Some(
                        cwd.file_name()
                            .map_or_else(|| cwd.display().to_string(), |n| n.to_string_lossy().into_owned()),
                    )
                })
                .unwrap_or_else(|| "Terminal".into());
            let label = gtk4::Label::new(Some(&name));
            label.add_css_class("pane-drag-icon");
            gtk4::DragIcon::for_drag(drag).set_child(Some(&label));
        });
    }
    {
        let w = app.weak();
        let f = frame.clone();
        let h = handle.clone();
        src.connect_drag_end(move |_, _, _| {
            f.remove_css_class("pane-dragging");
            h.remove_css_class("pressed");
            if let Some(a) = w.upgrade() {
                a.dragging_pane.set(None);
            }
        });
    }
    {
        let h = handle.clone();
        src.connect_begin(move |_, _| h.add_css_class("pressed"));
    }
    {
        // GTK ends the gesture as soon as the drag starts; stay pressed until
        // the drag itself ends (drag-end below).
        let w = app.weak();
        let h = handle.clone();
        src.connect_end(move |_, _| {
            if w.upgrade().and_then(|a| a.dragging_pane.get()) != Some(id) {
                h.remove_css_class("pressed");
            }
        });
    }
    handle.add_controller(src);

    let drop = gtk4::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
    // Capture: the frame decides before the terminal inside it sees the drop.
    drop.set_propagation_phase(gtk4::PropagationPhase::Capture);
    // Accept only another pane of this window; highlight while hovering.
    let accepts = {
        let w = app.weak();
        let hint = hint.clone();
        move || {
            let other = w.upgrade().and_then(|a| a.dragging_pane.get()).is_some_and(|d| d != id);
            set_class(&hint, "active", other);
            if other {
                gdk::DragAction::MOVE
            } else {
                gdk::DragAction::empty()
            }
        }
    };
    {
        let accepts = accepts.clone();
        drop.connect_enter(move |_, _, _| accepts());
    }
    drop.connect_motion(move |_, _, _| accepts());
    {
        let hint = hint.clone();
        drop.connect_leave(move |_| hint.remove_css_class("active"));
    }
    {
        let w = app.weak();
        let hint = hint.clone();
        drop.connect_drop(move |_, value, _, _| {
            hint.remove_css_class("active");
            let dragged = value
                .get::<String>()
                .ok()
                .and_then(|s| s.strip_prefix(PANE_DRAG_PREFIX)?.parse::<PaneId>().ok());
            let Some(dragged) = dragged else { return false };
            if dragged == id {
                return false;
            }
            // Defer: the swap rebuilds the widget tree this drop runs inside.
            let w = w.clone();
            glib::idle_add_local_once(move || {
                if let Some(a) = w.upgrade() {
                    a.report(a.swap_panes(dragged, id));
                }
            });
            true
        });
    }
    frame.add_controller(drop);
}

fn set_class(w: &impl IsA<gtk4::Widget>, class: &str, on: bool) {
    if on {
        w.add_css_class(class);
    } else {
        w.remove_css_class(class);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_regex_metacharacters() {
        assert_eq!(super::regex_escape("a.b*(c)"), "a\\.b\\*\\(c\\)");
    }
}
