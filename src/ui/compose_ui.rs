//! Docker Compose launcher in a pane: the button next to the drag handle, the
//! file picker, the stack bar and the logs line. The logic lives in
//! `crate::compose`; this module keeps per-pane state and the widgets.

use super::pane::Pane;
use super::{App, AppRc};
use crate::compose::{self, Action, ComposeFile, ComposeScan, ServiceStatus, State};
use crate::config::Config;
use crate::model::PaneId;
use gtk4::prelude::*;
use gtk4::{gdk, glib, Orientation};
use std::cell::{Cell, Ref, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

pub fn tooltip() -> String {
    crate::config::with_shortcut("Compose files in this repo", "compose-files")
}
pub const TOOLTIP_BUSY: &str = "Busy: wait for the command to finish";

/// A new stack is never hidden this soon, so a `ps` that runs before the
/// shell has even started `docker compose up` cannot remove its bar.
const STACK_GRACE: Duration = Duration::from_secs(5);

/// Launcher state and widgets of one pane.
pub struct PaneCompose {
    /// The pane button; hidden unless the repo has compose files.
    pub button: gtk4::Button,
    /// Where the logs bar and the stack bar go under the terminal. Each clip
    /// stays empty and hidden until the pane first needs its bars (see
    /// [`PaneCompose::bars`]), and then keeps a long chip row or status line
    /// from raising the pane's minimum width (that would block resizing it).
    pub logs_clip: Clip,
    pub stack_clip: Clip,
    /// Holds the one-line "run again" strip under the terminal.
    pub rerun_clip: Clip,
    /// Compose files of the pane's repo and the compose program, from the meta worker.
    pub scan: Option<ComposeScan>,
    /// The stack started from this pane, while it runs.
    pub stack: Option<Stack>,
    /// The pane's last stack after it stopped: the "run again" strip shows it
    /// for the next [`compose::RERUN_KEEP_COMMANDS`] shell commands.
    pub stopped: Option<compose::Stopped>,
    /// File whose `logs -f` runs in the pane.
    pub logs: Option<PathBuf>,
    /// Ask the meta worker to scan again on its next job.
    pub rescan: bool,
    /// A command the launcher typed without pressing Enter (`run_immediately`
    /// off): the stack starts when the user presses Enter in this pane.
    pub pending: Option<(PathBuf, Action, Vec<String>)>,
    /// The last picker. A closed picker stays attached to the button until the
    /// next one opens: removing a popover right after it closes, while the
    /// pointer is still over it, crashes GTK (`gdk_surface_request_motion`).
    picker: RefCell<Option<gtk4::Popover>>,
    /// The stack bar and logs bar, built the first time a stack or `logs`
    /// shows in this pane; most panes never need them.
    bars: RefCell<Option<Bars>>,
    app: Weak<App>,
    id: PaneId,
}

/// The widgets under the terminal: the stack bar and the logs bar.
pub struct Bars {
    /// Bar under the terminal while a stack started here runs.
    pub stack_bar: gtk4::Box,
    /// Line above the stack bar while `logs -f` runs in the pane.
    pub logs_bar: gtk4::Box,
    /// One line after the pane's stack stopped: `[run up] [up -d] [×]`.
    pub rerun_bar: gtk4::Box,
    rerun_label: gtk4::Label,
    /// `[run up]` and `[up -d]`: greyed out while a program holds the pane.
    rerun_buttons: Vec<gtk4::Button>,
    /// Status glyph, coloured by the stack's [`compose::Tone`].
    dot: gtk4::Label,
    status: gtk4::Label,
    chips: gtk4::FlowBox,
    /// Services the chips were last drawn for.
    chips_key: RefCell<String>,
    bar_buttons: Vec<gtk4::Button>,
    logs_label: gtk4::Label,
}

impl PaneCompose {
    /// The button (prepended to `actions`, before the drag handle) and the two
    /// empty clips, which the caller places under the terminal.
    pub fn new(app: &AppRc, id: PaneId, actions: &gtk4::Box) -> Self {
        Self {
            button: attach_button(app, id, actions),
            logs_clip: Clip::new(),
            stack_clip: Clip::new(),
            rerun_clip: Clip::new(),
            scan: None,
            stack: None,
            stopped: None,
            logs: None,
            rescan: false,
            pending: None,
            picker: RefCell::new(None),
            bars: RefCell::new(None),
            app: app.weak(),
            id,
        }
    }

    /// The pane's bars, built and put into their clips on the first call.
    pub fn bars(&self) -> Ref<'_, Bars> {
        if self.bars.borrow().is_none() {
            let b = Bars::new(&self.app, self.id);
            fill_clip(&self.logs_clip, &b.logs_bar);
            fill_clip(&self.stack_clip, &b.stack_bar);
            fill_clip(&self.rerun_clip, &b.rerun_bar);
            *self.bars.borrow_mut() = Some(b);
        }
        Ref::map(self.bars.borrow(), |b| b.as_ref().expect("bars were just built"))
    }

    /// The pane's stack has stopped (or gone `down`): drop its bar and offer to
    /// run it again in the one-line strip.
    pub fn end_stack(&mut self) {
        if let Some(s) = self.stack.take() {
            self.stopped = Some(compose::Stopped::new(&s.file, s.cmd, s.services.len(), crate::history::now_secs()));
        }
    }

    /// Path of `file` as shown in the bars: relative to the repo root.
    pub fn display(&self, file: &Path) -> String {
        self.scan
            .as_ref()
            .and_then(|s| file.strip_prefix(&s.root).ok())
            .unwrap_or(file)
            .to_string_lossy()
            .into_owned()
    }
}

impl Bars {
    /// Both bars, hidden; their buttons act on pane `id`.
    fn new(app: &Weak<App>, id: PaneId) -> Self {
        let stack_bar = gtk4::Box::new(Orientation::Vertical, 2);
        stack_bar.add_css_class("compose-stack");
        stack_bar.set_visible(false);
        let top = gtk4::Box::new(Orientation::Horizontal, 8);
        let dot = label("", &["compose-status", "compose-dot"]);
        let status = label("", &["compose-status"]);
        status.set_hexpand(true);
        let line = gtk4::Box::new(Orientation::Horizontal, 0);
        line.set_hexpand(true);
        line.append(&dot);
        line.append(&status);
        top.append(&line);
        let rebuild_btn = bracket_button("rebuild", "Stop, then run up --build in this pane");
        let restart_btn = bracket_button("restart", "docker compose restart");
        let stop_btn = bracket_button("stop", "docker compose stop");
        stop_btn.add_css_class("compose-stop");
        rebuild_btn.add_css_class("compose-rebuild");
        restart_btn.add_css_class("compose-restart");
        for b in [&rebuild_btn, &restart_btn, &stop_btn] {
            top.append(b);
        }
        {
            let w = app.clone();
            rebuild_btn.connect_clicked(move |_| {
                if let Some(a) = w.upgrade() {
                    rebuild(&a, id);
                }
            });
        }
        for (b, action) in [(&restart_btn, Action::Restart), (&stop_btn, Action::Stop)] {
            let w = app.clone();
            b.connect_clicked(move |_| {
                if let Some(a) = w.upgrade() {
                    run_background(&a, id, action.clone());
                }
            });
        }
        stack_bar.append(&top);
        // Chips wrap onto more lines when the pane is narrow, like text.
        let chips = gtk4::FlowBox::new();
        chips.add_css_class("compose-chips");
        chips.set_selection_mode(gtk4::SelectionMode::None);
        chips.set_homogeneous(false);
        chips.set_min_children_per_line(1);
        chips.set_max_children_per_line(256);
        chips.set_column_spacing(12);
        chips.set_row_spacing(0);
        stack_bar.append(&chips);

        let logs_bar = gtk4::Box::new(Orientation::Horizontal, 8);
        logs_bar.add_css_class("compose-logs");
        logs_bar.set_visible(false);
        let logs_label = label("", &["compose-status"]);
        logs_label.set_hexpand(true);
        logs_bar.append(&logs_label);
        let stop_logs_btn = bracket_button(
            "stop logs",
            "Send Ctrl+C to the pane. Stops following the logs; the containers keep running.",
        );
        stop_logs_btn.add_css_class("compose-stop");
        {
            let w = app.clone();
            stop_logs_btn.connect_clicked(move |_| {
                if let Some(a) = w.upgrade() {
                    stop_logs(&a, id);
                }
            });
        }
        logs_bar.append(&stop_logs_btn);

        let rerun_bar = gtk4::Box::new(Orientation::Horizontal, 8);
        rerun_bar.add_css_class("compose-rerun");
        rerun_bar.set_visible(false);
        rerun_bar.append(&label("↺", &["compose-status", "compose-dot", "compose-idle"]));
        let rerun_label = label("", &["compose-status", "compose-rerun-text"]);
        rerun_label.set_hexpand(true);
        rerun_bar.append(&rerun_label);
        let up_btn = bracket_button("run up", "Run docker compose up again in this pane");
        let up_d_btn = bracket_button("up -d", "Run docker compose up -d again in this pane");
        let hide_btn = bracket_button("×", "Hide");
        up_btn.add_css_class("compose-rebuild");
        up_d_btn.add_css_class("compose-rebuild");
        hide_btn.add_css_class("compose-hide");
        for (b, action) in [(&up_btn, Action::Up), (&up_d_btn, Action::UpDetached)] {
            let w = app.clone();
            b.connect_clicked(move |_| {
                if let Some(a) = w.upgrade() {
                    rerun(&a, id, action.clone());
                }
            });
        }
        {
            let w = app.clone();
            hide_btn.connect_clicked(move |_| {
                if let Some(a) = w.upgrade() {
                    if let Some(p) = a.inner.borrow_mut().panes.get_mut(&id) {
                        p.compose.stopped = None;
                    }
                    a.refresh();
                }
            });
        }
        for b in [&up_btn, &up_d_btn, &hide_btn] {
            rerun_bar.append(b);
        }

        Self {
            stack_bar,
            logs_bar,
            rerun_bar,
            rerun_label,
            rerun_buttons: vec![up_btn, up_d_btn],
            dot,
            status,
            chips,
            chips_key: RefCell::new(String::new()),
            bar_buttons: vec![rebuild_btn, restart_btn, stop_btn],
            logs_label,
        }
    }
}

/// A stack started from the launcher.
pub struct Stack {
    /// Unique per launch: a notification about an older stack in the same pane
    /// must not act on this one.
    pub id: u64,
    pub file: PathBuf,
    /// The compose program that started the stack (a later scan may find
    /// another one, e.g. after the setting changed).
    pub cmd: Vec<String>,
    /// Environment and folder of the process that started the stack; status
    /// and bar actions run in it. None until captured (see `capture_context`).
    pub context: Option<compose::LaunchContext>,
    /// Still waiting to capture `context`: no `ps` meanwhile.
    pub capturing: bool,
    /// Started with `up` in the pane (true) or `up -d` (false).
    pub attached: bool,
    pub started: Instant,
    pub services: Vec<ServiceStatus>,
    /// Some service has been seen running since the stack started.
    pub seen_running: bool,
    /// Progress or error text of the last bar action, shown in place of the counts.
    pub note: Option<String>,
    /// A bar action is running; the bar buttons are disabled meanwhile.
    pub working: bool,
    /// Why the last `ps` failed, if it did.
    pub ps_error: Option<String>,
    /// The user is stopping the stack ([stop], Rebuild, or the attached `up`
    /// ended): signal exits are not crashes, and the bar may go away.
    pub stopping: bool,
}

impl Stack {
    fn new(file: &Path, attached: bool, cmd: Vec<String>) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            file: file.to_path_buf(),
            cmd,
            context: None,
            capturing: true,
            attached,
            started: Instant::now(),
            services: vec![],
            seen_running: false,
            note: None,
            working: false,
            ps_error: None,
            stopping: false,
        }
    }

    /// Apply a `ps` result. Returns false when the stack should go away.
    pub fn apply_ps(&mut self, ps: Result<Vec<ServiceStatus>, String>, pane_busy: bool) -> bool {
        match ps {
            Ok(services) => {
                self.ps_error = None;
                self.seen_running |= services.iter().any(ServiceStatus::is_running);
                self.services = services;
                self.started.elapsed() < STACK_GRACE
                    || !compose::stack_should_hide(self.seen_running, &self.services, pane_busy, self.stopping)
            }
            Err(e) => {
                self.ps_error = Some(format!("status unavailable: {e}"));
                true
            }
        }
    }
}

/// The (hidden) launcher button in the pane's top-right `actions` box.
fn attach_button(app: &AppRc, id: PaneId, actions: &gtk4::Box) -> gtk4::Button {
    let b = gtk4::Button::from_icon_name(super::icons::compose());
    for c in ["flat", "circular", "pane-compose"] {
        b.add_css_class(c);
    }
    b.set_tooltip_text(Some(&tooltip()));
    b.set_focus_on_click(false);
    // The picker popover is a child of this button. `can-focus = false` would
    // stop every widget in it (the filter box, the rows) from taking focus, so
    // typed text would never reach the filter.
    b.set_visible(false);
    actions.prepend(&b);
    let w = app.weak();
    b.connect_clicked(move |_| {
        if let Some(a) = w.upgrade() {
            open_picker(&a, id);
        }
    });
    b
}

/// Show, hide and fill the pane's launcher widgets from its state.
pub fn sync(p: &Pane, cfg: &Config) {
    let c = &p.compose;
    let on = cfg.compose.enabled;
    let has_files = c.scan.as_ref().is_some_and(|s| s.command.is_some() && !s.files.is_empty());
    set_visible(&c.button, on && has_files);
    if c.button.is_sensitive() == p.busy {
        c.button.set_sensitive(!p.busy);
        let idle = tooltip();
        c.button.set_tooltip_text(Some(if p.busy { TOOLTIP_BUSY } else { &idle }));
    }

    let logs = c.logs.as_ref().filter(|_| on && p.busy);
    let stack = c.stack.as_ref().filter(|_| on);
    let stopped = c.stopped.as_ref().filter(|_| on && stack.is_none());
    // Nothing to show and nothing shown: leave the bars unbuilt.
    if logs.is_none() && stack.is_none() && stopped.is_none() && c.bars.borrow().is_none() {
        return;
    }
    let bars = c.bars();
    set_visible(&bars.logs_bar, logs.is_some());
    if let Some(file) = logs {
        set_label(&bars.logs_label, &format!("≡ logs -f  {} │ following", c.display(file)));
    }

    set_visible(&bars.rerun_bar, stopped.is_some());
    if let Some(s) = stopped {
        set_label(&bars.rerun_label, &s.text(&c.display(&s.file), crate::history::now_secs()));
        for b in &bars.rerun_buttons {
            if b.is_sensitive() == p.busy {
                b.set_sensitive(!p.busy);
            }
        }
        ensure_clock(&c.app);
    }

    set_visible(&bars.stack_bar, stack.is_some());
    let Some(stack) = stack else { return };
    let tone = compose::stack_tone(&stack.services, stack.stopping);
    set_label(&bars.dot, &format!("{} ", tone_glyph(tone)));
    set_tone(&bars.dot, tone);
    set_label(&bars.status, &status_text(c, stack));
    for b in &bars.bar_buttons {
        if b.is_sensitive() == stack.working {
            b.set_sensitive(!stack.working);
        }
    }
    let key: String = stack
        .services
        .iter()
        .map(|s| format!("{}:{:?}:{}:{}:{:?};", s.service, s.state, s.health, s.exit_code, s.ports))
        .collect::<String>()
        + if stack.working { "w" } else { "" }
        + if stack.stopping { "s" } else { "" };
    if *bars.chips_key.borrow() != key {
        draw_chips(c, &bars.chips, stack);
        *bars.chips_key.borrow_mut() = key;
    }
    ensure_clock(&c.app);
}

fn set_visible(w: &impl IsA<gtk4::Widget>, on: bool) {
    if w.as_ref().get_visible() != on {
        w.as_ref().set_visible(on);
    }
}

fn set_label(l: &gtk4::Label, text: &str) {
    if l.label() != text {
        l.set_label(text);
    }
}

fn glyph(s: &ServiceStatus) -> &'static str {
    match s.state {
        State::Running => "●",
        State::Restarting => "↻",
        State::Exited => "✗",
        State::Other => "○",
    }
}

/// The stack's status glyph: `●` up, `↻` starting, `✗` crashed, `○` idle.
fn tone_glyph(t: compose::Tone) -> &'static str {
    match t {
        compose::Tone::Up => "●",
        compose::Tone::Pending => "↻",
        compose::Tone::Down => "✗",
        compose::Tone::Idle => "○",
    }
}

/// Give `w` the CSS class of `tone` (green, yellow, red or dim) and no other.
fn set_tone(w: &impl IsA<gtk4::Widget>, tone: compose::Tone) {
    for t in compose::Tone::ALL {
        let class = t.css_class();
        if t == tone {
            w.as_ref().add_css_class(class);
        } else if w.as_ref().has_css_class(class) {
            w.as_ref().remove_css_class(class);
        }
    }
}

/// `docker-compose.yml  4/4 up │ 02:13 │ attached` (the glyph before it is its own label).
fn status_text(c: &PaneCompose, s: &Stack) -> String {
    let up = s.services.iter().filter(|x| x.state == State::Running).count();
    let detail = if let Some(n) = s.note.as_ref().or(s.ps_error.as_ref()) {
        n.clone()
    } else if s.services.is_empty() {
        "starting…".into()
    } else {
        let exited = s.services.iter().filter(|x| x.state == State::Exited).count();
        let secs = s.started.elapsed().as_secs();
        format!(
            "{up}/{} up{} │ {:02}:{:02} │ {}",
            s.services.len(),
            if exited > 0 {
                format!(" │ {exited} exited")
            } else {
                String::new()
            },
            secs / 60,
            secs % 60,
            if s.attached { "attached" } else { "detached" }
        )
    };
    format!("{}  {detail}", c.display(&s.file))
}

fn draw_chips(c: &PaneCompose, chips: &gtk4::FlowBox, s: &Stack) {
    chips.remove_all();
    for svc in &s.services {
        let ports: String = svc.ports.iter().map(|p| format!(" :{p}")).collect();
        let b = gtk4::Button::with_label(&format!("{} {}{ports}", glyph(svc), svc.service));
        b.add_css_class("flat");
        b.add_css_class("compose-chip");
        // Line chips up on the left, like terminal text.
        b.set_halign(gtk4::Align::Start);
        set_tone(&b, compose::tone(svc, s.stopping));
        b.set_focus_on_click(false);
        b.set_sensitive(!s.working);
        b.set_tooltip_text(Some(&match svc.state {
            State::Exited => format!("{} exited with code {}. Click to restart it.", svc.service, svc.exit_code),
            _ => format!("Restart only {}", svc.service),
        }));
        let (w, id, name) = (c.app.clone(), c.id, svc.service.clone());
        b.connect_clicked(move |_| {
            if let Some(a) = w.upgrade() {
                run_background(&a, id, Action::RestartService(name.clone()));
            }
        });
        chips.append(&b);
    }
}

thread_local! {
    static CLOCK: Cell<bool> = const { Cell::new(false) };
}

/// Tick the uptime (and the "stopped … ago" of the run-again strip) once a
/// second while any stack bar or strip is visible.
fn ensure_clock(app: &Weak<App>) {
    if CLOCK.with(|c| c.replace(true)) {
        return;
    }
    let w = app.clone();
    glib::timeout_add_seconds_local(1, move || {
        let Some(a) = w.upgrade() else {
            CLOCK.with(|c| c.set(false));
            return glib::ControlFlow::Break;
        };
        let i = a.inner.borrow();
        let mut any = false;
        let now = crate::history::now_secs();
        for p in i.panes.values() {
            let c = &p.compose;
            let bars = c.bars.borrow();
            if let (Some(s), Some(b)) = (&c.stack, bars.as_ref().filter(|b| b.stack_bar.get_visible())) {
                set_label(&b.status, &status_text(c, s));
                any = true;
            }
            if let (Some(s), Some(b)) = (&c.stopped, bars.as_ref().filter(|b| b.rerun_bar.get_visible())) {
                set_label(&b.rerun_label, &s.text(&c.display(&s.file), now));
                any = true;
            }
        }
        if !any {
            CLOCK.with(|c| c.set(false));
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

/// Run `action` for the pane's stack as a separate process, never through the
/// pane's input. Progress and failures show in the stack bar.
pub fn run_background(app: &AppRc, id: PaneId, action: Action) {
    let argv = {
        let mut i = app.inner.borrow_mut();
        let Some(p) = i.panes.get_mut(&id) else { return };
        // Is the launcher's `up` still pulling or building in the pane, before any
        // container has run (a first start)? Only then does [stop] interrupt it;
        // with containers running, [stop] is the regular `docker compose stop`.
        let first_pull = p.has_running_child() && p.compose.stack.as_ref().is_some_and(|s| !s.seen_running);
        let term = p.term.clone();
        let tracker = p.suggest.tracker.clone();
        let Some(stack) = p.compose.stack.as_mut() else { return };
        if stack.working {
            return;
        }
        if action == Action::Stop && first_pull {
            // `docker compose stop` cannot stop an `up` that is still pulling or
            // building (no containers yet): interrupt it, as Ctrl+C would.
            // Compose then stops whatever it started.
            stack.stopping = true;
            stack.note = Some("Stopping…".into());
            let attached = stack.attached;
            vte4::prelude::TerminalExt::feed_child(&term, b"\x03");
            // The line ends here, as with a typed Ctrl+C.
            tracker.borrow_mut().skip("\u{3}");
            if attached {
                drop(i);
                app.refresh();
                return;
            }
        }
        stack.working = true;
        if action == Action::Stop {
            stack.stopping = true;
        }
        stack.note = Some(match &action {
            Action::Stop => "Stopping…".to_string(),
            Action::Restart => "Restarting all services…".to_string(),
            Action::RestartService(s) => format!("Restarting {s}…"),
            other => format!("{}…", other.args().join(" ")),
        });
        // Without a captured context (process unreadable): the app's environment,
        // from the compose file's folder.
        let context = stack.context.clone().unwrap_or_else(|| compose::LaunchContext {
            env: std::env::vars_os().collect(),
            cwd: stack.file.parent().map(Path::to_path_buf),
        });
        (compose::argv(&stack.cmd, &stack.file, &action), context)
    };
    let (argv, context) = argv;
    let (tx, rx) = async_channel::bounded::<Result<(), String>>(1);
    std::thread::spawn(move || {
        let out = context
            .command(&argv)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .output();
        let res = match out {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let code = o.status.code().unwrap_or(-1);
                Err(format!("(exit {code}): {}", err.lines().last().unwrap_or("")))
            }
            Err(e) => Err(format!(": {e}")),
        };
        let _ = tx.send_blocking(res);
    });
    let w = app.weak();
    let what = action.args().join(" ");
    glib::spawn_future_local(async move {
        let Ok(res) = rx.recv().await else { return };
        let Some(a) = w.upgrade() else { return };
        if let Some(stack) = a.inner.borrow_mut().panes.get_mut(&id).and_then(|p| p.compose.stack.as_mut()) {
            stack.working = false;
            stack.note = res.err().map(|e| format!("{what} failed {e}"));
        }
        a.refresh();
    });
    app.refresh();
}

/// How long Rebuild waits for the pane to return to the prompt after Ctrl+C.
const REBUILD_WAIT: Duration = Duration::from_secs(30);

/// Rebuild in the foreground, without `-d`: stop the launcher's attached `up`
/// (or `logs -f`) with Ctrl+C, wait for the prompt, then run `up --build` in
/// the pane. Another program in the pane gets left alone: the rebuild opens in
/// a new split instead.
pub fn rebuild(app: &AppRc, id: PaneId) {
    let (file, ours, busy, scan, cwd) = {
        let i = app.inner.borrow();
        let Some(p) = i.panes.get(&id) else { return };
        let Some(stack) = &p.compose.stack else { return };
        if stack.working {
            return;
        }
        let ours = stack.attached || p.compose.logs.is_some();
        (
            stack.file.clone(),
            ours,
            p.has_running_child(),
            p.compose.scan.clone(),
            p.cwd.clone(),
        )
    };
    if !busy {
        run(app, id, &file, Action::UpBuild);
        return;
    }
    if !ours {
        let Ok(new) = app.split(Some(id), crate::model::Orient::Vertical, cwd) else {
            return;
        };
        {
            let mut i = app.inner.borrow_mut();
            if let Some(p) = i.panes.get_mut(&id) {
                p.compose.stack = None;
            }
            if let Some(p) = i.panes.get_mut(&new) {
                p.compose.scan = scan;
            }
        }
        run(app, new, &file, Action::UpBuild);
        return;
    }
    {
        let mut i = app.inner.borrow_mut();
        let Some(p) = i.panes.get_mut(&id) else { return };
        if let Some(s) = p.compose.stack.as_mut() {
            s.working = true;
            s.note = Some("Rebuild: stopping…".into());
            s.stopping = true;
        }
        p.compose.logs = None;
        p.type_input("\u{3}");
    }
    app.refresh();
    let w = app.weak();
    let since = Instant::now();
    glib::timeout_add_local(Duration::from_millis(200), move || {
        let Some(a) = w.upgrade() else { return glib::ControlFlow::Break };
        // Back at the prompt: the child ended and the line editor reads again.
        let busy = a
            .inner
            .borrow()
            .panes
            .get(&id)
            .map(|p| p.has_running_child() || !p.prompt_is_empty());
        match busy {
            None => glib::ControlFlow::Break,
            Some(false) => {
                run(&a, id, &file, Action::UpBuild);
                glib::ControlFlow::Break
            }
            Some(true) if since.elapsed() > REBUILD_WAIT => {
                if let Some(s) = a.inner.borrow_mut().panes.get_mut(&id).and_then(|p| p.compose.stack.as_mut()) {
                    s.working = false;
                    s.note = Some("Rebuild: the pane did not return to the prompt".into());
                }
                a.refresh();
                glib::ControlFlow::Break
            }
            Some(true) => glib::ControlFlow::Continue,
        }
    });
}

/// Stop following logs: Ctrl+C to the pane. The containers keep running.
pub fn stop_logs(app: &AppRc, id: PaneId) {
    {
        let mut i = app.inner.borrow_mut();
        let Some(p) = i.panes.get_mut(&id) else { return };
        if p.compose.logs.take().is_none() {
            return;
        }
        p.type_input("\u{3}");
    }
    app.refresh();
}

/// Start the pane's stopped stack again with `action` (`up` or `up -d`), from
/// the run-again strip.
pub fn rerun(app: &AppRc, id: PaneId, action: Action) {
    let Some(s) = app.inner.borrow().panes.get(&id).and_then(|p| p.compose.stopped.clone()) else {
        return;
    };
    run_with(app, id, &s.file, action, Some(s.cmd));
}

/// The user ran a shell command in pane `id` (a non-empty line at the prompt):
/// the run-again strip goes away after [`compose::RERUN_KEEP_COMMANDS`] of them.
pub fn command_ran(app: &AppRc, id: PaneId) {
    // Called from the terminal's key handling: change the app state on idle.
    let w = app.weak();
    glib::idle_add_local_once(move || {
        let Some(a) = w.upgrade() else { return };
        let gone = {
            let mut i = a.inner.borrow_mut();
            let Some(c) = i.panes.get_mut(&id).map(|p| &mut p.compose) else {
                return;
            };
            let gone = c.stopped.as_mut().is_some_and(|s| !s.command_ran());
            if gone {
                c.stopped = None;
            }
            gone
        };
        if gone {
            a.refresh();
        }
    });
}

/// Type `action` for `file` into the pane, and press Enter when
/// `compose.run_immediately` is on. Does nothing while the pane is busy, and
/// only says why while the prompt holds unfinished input: the command would be
/// appended to it (and Enter would run both).
pub fn run(app: &AppRc, id: PaneId, file: &Path, action: Action) {
    run_with(app, id, file, action, None);
}

/// [`run`], with the compose program to use when the pane's current folder has
/// no scan (the shell left the repo since the stack ran).
fn run_with(app: &AppRc, id: PaneId, file: &Path, action: Action, fallback: Option<Vec<String>>) {
    let immediate = {
        let i = app.inner.borrow();
        let Some(p) = i.panes.get(&id) else { return };
        if p.has_running_child() {
            return;
        }
        if !p.prompt_is_empty() {
            drop(i);
            app.toast("The prompt has unfinished input: clear it (Ctrl+U) or run it first");
            return;
        }
        let Some(cmd) = p.compose.scan.as_ref().and_then(|s| s.command.clone()).or(fallback) else {
            return;
        };
        // The shell's folder right now: `p.cwd` can be a few seconds old, and a
        // path relative to an old folder could name another compose file.
        let cwd = p
            .pid
            .get()
            .and_then(crate::meta::cwd_of)
            .or_else(|| p.cwd.clone())
            .unwrap_or_default();
        let text = compose::typed_command(&cmd, file, &cwd, &action);
        let immediate = i.config.compose.run_immediately;
        // `up` and `up -d` feed the picker's "last starts" row.
        let mut starts = compose::load_starts();
        if compose::record_start(&mut starts, file, &action, crate::history::now_secs()) {
            compose::save_starts(&starts);
        }
        p.type_input(&text);
        if immediate {
            p.type_input("\n");
        }
        (immediate, cmd)
    };
    let (immediate, cmd) = immediate;
    {
        let mut i = app.inner.borrow_mut();
        if let Some(p) = i.panes.get_mut(&id) {
            if immediate {
                apply_started(p, file, &action, cmd);
            } else {
                p.compose.pending = Some((file.to_path_buf(), action, cmd));
            }
        }
    }
    if immediate {
        capture_context(app, id);
    }
    app.refresh();
}

/// How long the command the launcher started may take to appear in the pane
/// (input typed while the shell starts waits for its first prompt).
const CAPTURE_WAIT: Duration = Duration::from_secs(10);

/// Remember the environment and folder of the command the shell runs for the
/// pane's new stack: its exported variables pick the project and the daemon.
/// Watches the pane's foreground process until it has started the program
/// (its command line differs from the shell's, so it is no longer the forked
/// shell). If none shows up in time (`up -d` ended within one check), fall
/// back to the shell's environment and folder.
fn capture_context(app: &AppRc, id: PaneId) {
    let Some(stack) = app
        .inner
        .borrow()
        .panes
        .get(&id)
        .and_then(|p| p.compose.stack.as_ref().map(|s| s.id))
    else {
        return;
    };
    let cmdline = |pid: i32| Some(crate::meta::cmdline(pid)).filter(|c| !c.is_empty());
    let w = app.weak();
    let since = Instant::now();
    glib::timeout_add_local(Duration::from_millis(5), move || {
        let Some(a) = w.upgrade() else { return glib::ControlFlow::Break };
        let mut i = a.inner.borrow_mut();
        let Some(p) = i.panes.get_mut(&id) else {
            return glib::ControlFlow::Break;
        };
        let Some(shell) = p.pid.get() else {
            return glib::ControlFlow::Continue;
        };
        let fg = crate::meta::foreground_pgrp(p.pty_fd()).filter(|&pg| pg != shell);
        let context = match fg {
            Some(pg) if cmdline(pg).is_some_and(|c| Some(c) != cmdline(shell)) => compose::LaunchContext::of_process(pg),
            _ if since.elapsed() > CAPTURE_WAIT => compose::LaunchContext::of_process(shell),
            _ => return glib::ControlFlow::Continue,
        };
        if let Some(s) = p.compose.stack.as_mut().filter(|s| s.id == stack) {
            s.context = context;
            s.capturing = false;
        }
        glib::ControlFlow::Break
    });
}

/// Follow the user's keys in `term` for a command the launcher only typed:
/// Enter runs it (start its stack bar), Ctrl+C or Ctrl+U throws it away.
pub fn watch_input(app: &AppRc, id: PaneId, term: &vte4::Terminal) {
    use vte4::prelude::*;
    let w = app.weak();
    term.connect_commit(move |_, text, _| {
        let cancel = text.contains(['\u{3}', '\u{15}']);
        let enter = text.contains(['\r', '\n']);
        if !cancel && !enter {
            return;
        }
        // The terminal may be mid key-handling: change the app state on idle.
        let w = w.clone();
        glib::idle_add_local_once(move || {
            let Some(a) = w.upgrade() else { return };
            let started = {
                let mut i = a.inner.borrow_mut();
                let Some(p) = i.panes.get_mut(&id) else { return };
                let Some((file, action, cmd)) = p.compose.pending.take() else {
                    return;
                };
                if cancel {
                    return;
                }
                apply_started(p, &file, &action, cmd);
                true
            };
            if started {
                capture_context(&a, id);
                a.refresh();
            }
        });
    });
}

/// Pane state right after the launcher ran `action` for `file`.
fn apply_started(p: &mut Pane, file: &Path, action: &Action, cmd: Vec<String>) {
    if action.holds_pane() {
        // Grey the button out now instead of at the next meta tick.
        p.busy = true;
    }
    let c = &mut p.compose;
    match action {
        Action::Up | Action::UpBuild | Action::UpDetached => {
            c.stack = Some(Stack::new(file, *action != Action::UpDetached, cmd));
            c.stopped = None;
        }
        Action::Logs => c.logs = Some(file.to_path_buf()),
        Action::Down if c.stack.as_ref().is_some_and(|s| s.file == file) => c.end_stack(),
        _ => {}
    }
}

/// Scan the pane's repo again now.
pub fn rescan(app: &AppRc, id: PaneId) {
    if let Some(p) = app.inner.borrow_mut().panes.get_mut(&id) {
        p.compose.rescan = true;
    }
    if let Some(job) = app.meta_job(id, false) {
        let _ = app.meta_tx.send(job);
    }
}

/// Width of the picker's list in pixels; longer lines are cut with an ellipsis.
const PICKER_WIDTH: i32 = 540;

/// The actions a file row offers, in the order Tab walks them: label, action and
/// the style class that tells safe starts from stopping ones.
fn chip_actions() -> [(&'static str, Action, &'static str); 5] {
    [
        ("up -d", Action::UpDetached, "compose-chip-go"),
        ("up", Action::Up, "compose-chip-go"),
        ("down", Action::Down, "compose-chip-warn"),
        ("logs", Action::Logs, "compose-chip-info"),
        ("restart", Action::Restart, "compose-chip-warn"),
    ]
}

/// Shortcut text for the picker's own chords: `^X` on Linux, `⌘X` on macOS (Ctrl
/// stays with the shell there, as everywhere else in oxishell).
fn chord(key: &str) -> String {
    let m = if cfg!(target_os = "macos") { "⌘" } else { "^" };
    format!("{m}{}", key.to_uppercase())
}

/// A file row's second line: its service count and names, the part matching the
/// filter in bold and underlined.
fn services_markup(f: &ComposeFile, query: &str) -> String {
    let esc = |s: &str| glib::markup_escape_text(s).to_string();
    let Some(s) = &f.services else {
        return esc("could not read services");
    };
    let head = if s.len() == 1 {
        "1 service · ".to_string()
    } else {
        format!("{} services · ", s.len())
    };
    let mut out = esc(&head);
    for (n, name) in s.iter().enumerate() {
        if n > 0 {
            out.push_str(", ");
        }
        for (piece, hit) in compose::split_matches(name, query) {
            if hit {
                out.push_str(&format!("<span weight=\"bold\" underline=\"single\">{}</span>", esc(&piece)));
            } else {
                out.push_str(&esc(&piece));
            }
        }
    }
    out
}

/// A folder header's text, with how many of its files match while filtering.
fn group_text(dir: &str, shown: usize, total: usize, filtering: bool) -> String {
    if filtering {
        format!("▾ {dir}  · {shown} of {total} match")
    } else {
        format!("▾ {dir}")
    }
}

/// The file picker under the pane button, drawn like terminal output: a filter
/// on top (it always has the keyboard), the last starts, then the files grouped
/// by folder. Only the selected file shows its action chips.
pub fn open_picker(app: &AppRc, id: PaneId) {
    let (button, scan, default_action) = {
        let i = app.inner.borrow();
        let Some(p) = i.panes.get(&id) else { return };
        let b = &p.compose.button;
        let Some(scan) = p.compose.scan.clone() else { return };
        if !i.config.compose.enabled || !b.get_visible() || !b.is_sensitive() {
            return;
        }
        (b.clone(), scan, i.config.compose.default_action.clone())
    };
    let default = Action::from_setting(&default_action).unwrap_or(Action::Up);
    let actions = chip_actions();

    let pop = gtk4::Popover::new();
    pop.add_css_class("compose-picker");
    pop.set_has_arrow(false);
    pop.set_position(gtk4::PositionType::Bottom);
    let body = gtk4::Box::new(Orientation::Vertical, 0);
    pop.set_child(Some(&body));

    let head = gtk4::Box::new(Orientation::Horizontal, 8);
    head.add_css_class("compose-head");
    head.append(&label("compose files", &[]));
    let root = label(&crate::meta::display_path(&scan.root), &["dim"]);
    root.set_hexpand(true);
    root.set_xalign(1.0);
    root.set_max_width_chars(34);
    head.append(&root);
    body.append(&head);

    let filter_row = gtk4::Box::new(Orientation::Horizontal, 6);
    filter_row.add_css_class("compose-filter");
    filter_row.append(&label("›", &["compose-accent"]));
    let entry = gtk4::SearchEntry::new();
    entry.set_placeholder_text(Some("type to filter files and services"));
    entry.set_hexpand(true);
    filter_row.append(&entry);
    body.append(&filter_row);

    // The last three starts (`up` / `up -d`) of files in this repo.
    let recent: Vec<compose::Start> = compose::recent_starts(&compose::load_starts(), &scan.files, 3)
        .into_iter()
        .cloned()
        .collect();
    if !recent.is_empty() {
        let recent_box = gtk4::Box::new(Orientation::Vertical, 2);
        recent_box.add_css_class("compose-recent");
        let m = chord("1").chars().next().map(String::from).unwrap_or_default();
        recent_box.append(&label(
            &format!("last starts · {m}1 {m}2 {m}3 to repeat"),
            &["dim", "compose-section"],
        ));
        let pills = gtk4::FlowBox::new();
        pills.set_selection_mode(gtk4::SelectionMode::None);
        pills.set_homogeneous(false);
        pills.set_max_children_per_line(3);
        pills.set_halign(gtk4::Align::Start);
        pills.set_column_spacing(6);
        pills.set_row_spacing(4);
        for (n, s) in recent.iter().enumerate() {
            let name = s
                .path
                .file_name()
                .map(|f| compose::short_name(&f.to_string_lossy()))
                .unwrap_or_default();
            let how = if s.detached { "up -d" } else { "up" };
            let text = format!("↺ {} {name} · {how}", n + 1);
            let pill = label(&text, &[]);
            pill.set_max_width_chars(30);
            let b = gtk4::Button::builder()
                .child(&pill)
                .tooltip_text(format!("Run {how} again ({})", chord(&(n + 1).to_string())))
                .build();
            b.add_css_class("flat");
            b.add_css_class("compose-pill");
            b.set_focus_on_click(false);
            let (w, pop, s) = (app.weak(), pop.clone(), s.clone());
            b.connect_clicked(move |_| {
                if let Some(a) = w.upgrade() {
                    pop.popdown();
                    run(&a, id, &s.path, s.action());
                }
            });
            pills.append(&b);
        }
        // Pills sit side by side and wrap to the next row when a row is full. The
        // scroller keeps their natural width (all three in one row) from widening
        // the picker past `PICKER_WIDTH`.
        let pill_scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Never)
            .propagate_natural_height(true)
            .min_content_width(PICKER_WIDTH - 16)
            .max_content_width(PICKER_WIDTH - 16)
            .child(&pills)
            .build();
        recent_box.append(&pill_scroll);
        body.append(&recent_box);
    }

    let list = gtk4::ListBox::new();
    list.add_css_class("compose-files");
    list.set_selection_mode(gtk4::SelectionMode::Single);
    // Grouped like a tree: the repo root's files first, then each folder as a
    // header with its files under it (the scan keeps each folder's files together).
    // `slots[row index]` is the file shown by that row, `None` for a header.
    let mut slots: Vec<Option<usize>> = Vec::new();
    let mut folder: Option<String> = None;
    // Header labels with the number of files under them, and each file row with
    // its action chips and service line, to update while filtering.
    let mut groups: Vec<(String, gtk4::Label, usize)> = Vec::new();
    let mut rows: Vec<FileRow> = Vec::new();
    for (n, f) in scan.files.iter().enumerate() {
        let dir = folder_of(f);
        if folder.as_ref() != Some(&dir) {
            let (row, text) = group_row(&dir);
            list.append(&row);
            slots.push(None);
            groups.push((dir.clone(), text, 0));
            folder = Some(dir);
        }
        if let Some(g) = groups.last_mut() {
            g.2 += 1;
        }
        let fr = file_row(app, id, &pop, f, &default, &actions);
        list.append(&fr.row);
        slots.push(Some(n));
        rows.push(fr);
    }
    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(360)
        .min_content_width(PICKER_WIDTH)
        .max_content_width(PICKER_WIDTH)
        .child(&list)
        .build();
    body.append(&scroll);

    // Custom selection bypasses GTK's cursor scrolling. Reveal the whole row
    // even when focus stays in the filter. Retry after layout settles, as in
    // the history panel, and discard reveals superseded by another selection.
    {
        let scroll = scroll.downgrade();
        list.connect_row_selected(move |list, row| {
            let (Some(scroll), Some(row)) = (scroll.upgrade(), row) else {
                return;
            };
            let list = list.downgrade();
            let row = row.clone();
            let frames = Cell::new(0);
            scroll.add_tick_callback(move |scroll, _| {
                if list.upgrade().and_then(|l| l.selected_row()).as_ref() != Some(&row) || !row.is_child_visible() {
                    return glib::ControlFlow::Break;
                }
                if let Some(bounds) = scroll.child().and_then(|view| row.compute_bounds(&view)) {
                    let adj = scroll.vadjustment();
                    let (top, bottom) = (bounds.y() as f64, (bounds.y() + bounds.height()) as f64);
                    if top < 0.0 {
                        adj.set_value(adj.value() + top);
                    } else if bottom > adj.page_size() {
                        adj.set_value(adj.value() + bottom - adj.page_size());
                    }
                }
                frames.set(frames.get() + 1);
                if frames.get() >= 3 {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
        });
    }

    // The chip Tab has moved to (None: Enter runs the default action). Only the
    // selected file shows its chips, and the chosen one is lit.
    let chip: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
    let rows = Rc::new(rows);
    let show_chips = {
        let (list, rows, chip) = (list.clone(), rows.clone(), chip.clone());
        move || {
            let selected = list.selected_row().filter(|r| r.is_child_visible());
            for fr in rows.iter() {
                let on = selected.as_ref() == Some(&fr.row);
                fr.chips.set_visible(on);
                let mut child = fr.chips.first_child();
                let mut n = 0;
                while let Some(c) = child {
                    if on && chip.get() == Some(n) {
                        c.add_css_class("compose-chip-on");
                    } else {
                        c.remove_css_class("compose-chip-on");
                    }
                    n += 1;
                    child = c.next_sibling();
                }
            }
        }
    };
    {
        let (chip, show_chips) = (chip.clone(), show_chips.clone());
        list.connect_row_selected(move |_, _| {
            chip.set(None);
            show_chips();
        });
    }

    let (rescan_key, down_key, logs_key) = (chord("r"), chord("x"), chord("l"));
    let keys = label(
        &format!("↑↓ move  ⏎ {default_action}  tab actions  {down_key} down  {logs_key} logs  {rescan_key} rescan  esc close"),
        &["compose-keys", "dim"],
    );
    keys.set_ellipsize(gtk4::pango::EllipsizeMode::None);
    keys.set_wrap(true);
    keys.set_max_width_chars(62);
    body.append(&keys);

    let foot = gtk4::Box::new(Orientation::Horizontal, 8);
    foot.add_css_class("compose-foot");
    let count = scan.files.len();
    let summary = label(&format!("{count} file{}", if count == 1 { "" } else { "s" }), &["dim"]);
    summary.set_hexpand(true);
    foot.append(&summary);
    let rescan_btn = key_button("r", "rescan", "Scan the repo again");
    foot.append(&rescan_btn);
    body.append(&foot);

    // Filter on the path and on the service names.
    {
        let (entry, files) = (entry.clone(), scan.files.clone());
        list.set_filter_func(move |row| {
            let q = entry.text();
            let name = row.widget_name();
            match name.strip_prefix(GROUP_PREFIX) {
                // A folder header shows while any of its files does.
                Some(dir) => files.iter().any(|f| folder_of(f) == dir && compose::matches_query(f, &q)),
                None => files
                    .iter()
                    .find(|f| f.rel.to_string_lossy() == name.as_str())
                    .is_some_and(|f| compose::matches_query(f, &q)),
            }
        });
    }
    {
        let (list, entry, files, rows, show_chips) = (list.clone(), entry.clone(), scan.files.clone(), rows.clone(), show_chips.clone());
        entry.connect_search_changed(move |e| {
            let q = e.text();
            list.invalidate_filter();
            for (f, fr) in files.iter().zip(rows.iter()) {
                fr.services.set_markup(&services_markup(f, &q));
            }
            let filtering = !q.trim().is_empty();
            for (dir, text, total) in &groups {
                let shown = files
                    .iter()
                    .filter(|f| folder_of(f) == *dir && compose::matches_query(f, &q))
                    .count();
                text.set_text(&group_text(dir, shown, *total, filtering));
            }
            select_visible(&list, 0, false);
            show_chips();
        });
    }

    let files = scan.files.clone();
    let chosen = {
        let list = list.clone();
        move || {
            // Only a file the filter still shows can run.
            let row = list.selected_row().filter(|r| r.is_child_visible())?;
            let n = slots.get(row.index() as usize).copied().flatten()?;
            files.get(n).cloned()
        }
    };
    {
        let (w, pop, chosen, default) = (app.weak(), pop.clone(), chosen.clone(), default.clone());
        list.connect_row_activated(move |_, _| {
            if let (Some(a), Some(f)) = (w.upgrade(), chosen()) {
                pop.popdown();
                run(&a, id, &f.path, default.clone());
            }
        });
    }
    {
        let (w, pop) = (app.weak(), pop.clone());
        rescan_btn.connect_clicked(move |_| {
            pop.popdown();
            if let Some(a) = w.upgrade() {
                rescan(&a, id);
            }
        });
    }

    let keyc = gtk4::EventControllerKey::new();
    keyc.set_propagation_phase(gtk4::PropagationPhase::Capture);
    {
        let (w, pop, list, rescan_btn) = (app.weak(), pop.clone(), list.clone(), rescan_btn);
        let (chip, show_chips) = (chip.clone(), show_chips.clone());
        keyc.connect_key_pressed(move |_, key, _, mods| {
            let primary = super::primary_mask();
            let others = (gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::META_MASK).difference(primary);
            // Other chords (Ctrl+Shift+L, Alt+...) stay with the app.
            if mods.intersects(others) {
                return glib::Propagation::Proceed;
            }
            let act = |action: Action| {
                if let (Some(a), Some(f)) = (w.upgrade(), chosen()) {
                    pop.popdown();
                    run(&a, id, &f.path, action);
                }
            };
            if mods.contains(primary) {
                // The picker's own chords: the filter has the keyboard, so single
                // letters are for typing. Shift is left to the app's shortcuts.
                if mods.contains(gdk::ModifierType::SHIFT_MASK) {
                    return glib::Propagation::Proceed;
                }
                match key {
                    gdk::Key::x => act(Action::Down),
                    gdk::Key::l => act(Action::Logs),
                    gdk::Key::r => rescan_btn.emit_clicked(),
                    gdk::Key::_1 | gdk::Key::_2 | gdk::Key::_3 => {
                        let n = match key {
                            gdk::Key::_1 => 0,
                            gdk::Key::_2 => 1,
                            _ => 2,
                        };
                        if let (Some(a), Some(s)) = (w.upgrade(), recent.get(n)) {
                            pop.popdown();
                            run(&a, id, &s.path, s.action());
                        }
                    }
                    _ => return glib::Propagation::Proceed,
                }
                return glib::Propagation::Stop;
            }
            let last = actions.len() - 1;
            match key {
                // Our own arrow handling: GTK's list would get stuck on a folder
                // header (not selectable) instead of skipping it.
                gdk::Key::Down | gdk::Key::Up | gdk::Key::KP_Down | gdk::Key::KP_Up => {
                    let down = matches!(key, gdk::Key::Down | gdk::Key::KP_Down);
                    select_visible(&list, if down { 1 } else { -1 }, false);
                }
                gdk::Key::Return | gdk::Key::KP_Enter => act(chip.get().map(|n| actions[n].1.clone()).unwrap_or_else(|| default.clone())),
                // Tab walks the action chips of the selected file, then back to
                // the default; Shift+Tab walks the other way.
                gdk::Key::Tab | gdk::Key::ISO_Left_Tab => {
                    let back = key == gdk::Key::ISO_Left_Tab || mods.contains(gdk::ModifierType::SHIFT_MASK);
                    chip.set(match (chip.get(), back) {
                        (None, false) => Some(0),
                        (None, true) => Some(last),
                        (Some(n), false) if n < last => Some(n + 1),
                        (Some(n), true) if n > 0 => Some(n - 1),
                        _ => None,
                    });
                    show_chips();
                }
                gdk::Key::Right if chip.get().is_some() => {
                    chip.set(chip.get().map(|n| (n + 1).min(last)));
                    show_chips();
                }
                gdk::Key::Left if chip.get().is_some() => {
                    chip.set(chip.get().map(|n| n.saturating_sub(1)));
                    show_chips();
                }
                // Esc steps back out of the chips, then closes the picker.
                gdk::Key::Escape => {
                    if chip.get().is_some() {
                        chip.set(None);
                        show_chips();
                    } else {
                        pop.popdown();
                    }
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
    }
    pop.add_controller(keyc);
    {
        // Typing goes back to the pane's terminal however the picker closes.
        let w = app.weak();
        pop.connect_closed(move |p| {
            let (p, w) = (p.clone(), w.clone());
            glib::idle_add_local_once(move || {
                let _ = p; // removed when the next picker opens, see `PaneCompose::picker`
                let Some(a) = w.upgrade() else { return };
                let term = a.inner.borrow().panes.get(&id).map(|p| p.term.clone());
                if let Some(t) = term {
                    t.grab_focus();
                }
            });
        });
    }

    // The filter has the keyboard from the start: typing narrows the list.
    {
        let (list, entry) = (list.clone(), entry.clone());
        pop.connect_map(move |_| {
            let (list, entry) = (list.clone(), entry.clone());
            glib::idle_add_local_once(move || {
                select_visible(&list, 0, false);
                entry.grab_focus();
            });
        });
    }
    let old = app
        .inner
        .borrow()
        .panes
        .get(&id)
        .and_then(|p| p.compose.picker.replace(Some(pop.clone())));
    if let Some(old) = old {
        old.unparent();
    }
    pop.set_parent(&button);
    pop.popup();
    select_visible(&list, 0, false);
    show_chips();
}

/// Move the selection `step` rows among the visible ones (0 = first visible).
fn select_visible(list: &gtk4::ListBox, step: i32, focus: bool) {
    let mut rows = Vec::new();
    let mut child = list.first_child();
    while let Some(c) = child {
        if let Ok(r) = c.clone().downcast::<gtk4::ListBoxRow>() {
            if r.is_child_visible() && r.is_selectable() {
                rows.push(r);
            }
        }
        child = c.next_sibling();
    }
    if rows.is_empty() {
        // Nothing matches the filter: nothing stays selected (or runs on Enter).
        list.unselect_all();
        return;
    }
    let cur = list.selected_row().and_then(|s| rows.iter().position(|r| *r == s));
    let next = match (cur, step) {
        (Some(i), s) if s != 0 => (i as i32 + s).rem_euclid(rows.len() as i32) as usize,
        _ => 0,
    };
    list.select_row(Some(&rows[next]));
    if focus {
        rows[next].grab_focus();
    }
}

/// Widget name prefix of a folder header row in the picker.
const GROUP_PREFIX: &str = "group:";

/// Folder header text of a file: "repo root" or "path/to/folder/".
fn folder_of(f: &ComposeFile) -> String {
    match f.rel.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(p) => format!("{}/", p.to_string_lossy()),
        None => "repo root".into(),
    }
}

/// A folder header in the picker: not selectable, skipped by the arrow keys.
/// Returns the row and its text label (the label shows match counts).
fn group_row(dir: &str) -> (gtk4::ListBoxRow, gtk4::Label) {
    let row = gtk4::ListBoxRow::new();
    row.set_widget_name(&format!("{GROUP_PREFIX}{dir}"));
    row.set_selectable(false);
    row.set_activatable(false);
    row.set_can_focus(false);
    row.add_css_class("compose-group-row");
    let text = label(&group_text(dir, 0, 0, false), &["compose-group"]);
    row.set_child(Some(&text));
    (row, text)
}

/// A file row and the parts of it that change while the picker is open.
struct FileRow {
    row: gtk4::ListBoxRow,
    /// The action chips, shown only while this row is selected.
    chips: gtk4::Box,
    /// The service line, which highlights what the filter matches.
    services: gtk4::Label,
}

fn file_row(
    app: &AppRc,
    id: PaneId,
    pop: &gtk4::Popover,
    f: &ComposeFile,
    default: &Action,
    actions: &[(&'static str, Action, &'static str); 5],
) -> FileRow {
    let row = gtk4::ListBoxRow::new();
    row.set_widget_name(&f.rel.to_string_lossy());
    row.add_css_class("compose-file");
    let text = gtk4::Box::new(Orientation::Vertical, 2);
    let name = f.rel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    text.append(&label(&name, &["compose-name"]));
    let services = label("", &["dim"]);
    services.set_use_markup(true);
    services.set_markup(&services_markup(f, ""));
    services.set_max_width_chars(58);
    text.append(&services);
    // Only the selected row shows these, so the list stays quiet.
    let chips = gtk4::Box::new(Orientation::Horizontal, 6);
    chips.add_css_class("compose-chips");
    chips.set_visible(false);
    for (word, action, tone) in actions {
        let mut shown = word.to_string();
        if action == default {
            shown = format!("⏎ {shown}");
        }
        match action {
            Action::Down => shown = format!("{shown} {}", chord("x")),
            Action::Logs => shown = format!("{shown} {}", chord("l")),
            _ => {}
        }
        let b = gtk4::Button::builder().child(&label(&shown, &[])).build();
        b.add_css_class("flat");
        b.add_css_class("compose-chip-btn");
        b.add_css_class(tone);
        b.set_tooltip_text(Some(&if action == default {
            format!("{word} (Enter)")
        } else {
            word.to_string()
        }));
        b.set_focus_on_click(false);
        let (w, pop, path, action) = (app.weak(), pop.clone(), f.path.clone(), action.clone());
        b.connect_clicked(move |_| {
            if let Some(a) = w.upgrade() {
                pop.popdown();
                run(&a, id, &path, action.clone());
            }
        });
        chips.append(&b);
    }
    text.append(&chips);
    row.set_child(Some(&text));
    FileRow { row, chips, services }
}

/// A flat button reading `<key> <word>`, the key highlighted.
fn key_button(key: &str, word: &str, tip: &str) -> gtk4::Button {
    let l = gtk4::Label::new(None);
    l.set_markup(&format!(
        "<b>{}</b> {}",
        glib::markup_escape_text(key),
        glib::markup_escape_text(word)
    ));
    let b = gtk4::Button::builder().child(&l).tooltip_text(tip).build();
    b.add_css_class("flat");
    b.add_css_class("compose-key-btn");
    b.set_focus_on_click(false);
    b
}

/// Put a bar into its clip, so its width is clipped to the pane instead of
/// setting the pane's minimum width; the clip shows and hides with the bar.
fn fill_clip(c: &Clip, bar: &gtk4::Box) {
    c.set_child(bar.upcast_ref());
    bar.bind_property("visible", c, "visible").sync_create().build();
}

mod imp {
    use gtk4::prelude::*;
    use gtk4::subclass::prelude::*;
    use gtk4::{glib, Orientation};

    #[derive(Default)]
    pub struct Clip;

    #[glib::object_subclass]
    impl ObjectSubclass for Clip {
        const NAME: &'static str = "OxishellComposeClip";
        type Type = super::Clip;
        type ParentType = gtk4::Widget;
    }

    impl ObjectImpl for Clip {
        fn dispose(&self) {
            while let Some(c) = self.obj().first_child() {
                c.unparent();
            }
        }
    }

    impl WidgetImpl for Clip {
        fn request_mode(&self) -> gtk4::SizeRequestMode {
            gtk4::SizeRequestMode::HeightForWidth
        }

        /// Minimum width 0 (the pane can always shrink); height is the child's
        /// height at the width it will really get.
        fn measure(&self, orientation: Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.obj().first_child() else {
                return (0, 0, -1, -1);
            };
            if orientation == Orientation::Horizontal {
                let (_, nat, _, _) = child.measure(orientation, -1);
                return (0, nat, -1, -1);
            }
            let (min_w, nat_w, _, _) = child.measure(Orientation::Horizontal, -1);
            // Without a width, assume the natural one (one line), not the
            // narrowest (one chip per line), so the pane's minimum height stays small.
            let width = if for_size < 0 { nat_w } else { for_size.max(min_w) };
            let (min, nat, _, _) = child.measure(orientation, width);
            (min, nat, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.obj().first_child() {
                let w = width.max(child.measure(Orientation::Horizontal, -1).0);
                let h = height.max(child.measure(Orientation::Vertical, w).0);
                child.allocate(w, h, baseline, None);
            }
        }
    }
}

glib::wrapper! {
    /// Holds one bar under the terminal. It never asks for more width than the
    /// pane has (anything wider is cut off), and asks for exactly the height
    /// the bar needs at the pane's width, so wrapped chip rows all show.
    pub struct Clip(ObjectSubclass<imp::Clip>) @extends gtk4::Widget;
}

impl Clip {
    /// An empty, hidden clip; [`Clip::set_child`] gives it its bar.
    fn new() -> Self {
        let c: Self = glib::Object::new();
        c.set_overflow(gtk4::Overflow::Hidden);
        c.set_visible(false);
        c
    }

    /// Make `child` the clip's only child.
    pub fn set_child(&self, child: &gtk4::Widget) {
        while let Some(old) = self.first_child() {
            old.unparent();
        }
        child.set_parent(self);
    }
}

/// A flat `[word]` button, drawn like terminal text.
fn bracket_button(word: &str, tip: &str) -> gtk4::Button {
    let b = gtk4::Button::with_label(&format!("[{word}]"));
    b.add_css_class("flat");
    b.add_css_class("compose-bar-btn");
    b.set_tooltip_text(Some(tip));
    b.set_focus_on_click(false);
    b
}

fn label(text: &str, classes: &[&str]) -> gtk4::Label {
    let l = gtk4::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    for c in classes {
        l.add_css_class(c);
    }
    l
}
