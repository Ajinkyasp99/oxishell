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
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Weak;
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
    /// Bar under the terminal while a stack started here runs.
    pub stack_bar: gtk4::Box,
    /// Line above the stack bar while `logs -f` runs in the pane.
    pub logs_bar: gtk4::Box,
    /// The two bars, each clipped so a long chip row or status line cannot
    /// raise the pane's minimum width (that would block resizing the pane).
    pub logs_clip: Clip,
    pub stack_clip: Clip,
    /// Compose files of the pane's repo and the compose program, from the meta worker.
    pub scan: Option<ComposeScan>,
    /// The stack started from this pane, while it runs.
    pub stack: Option<Stack>,
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
    /// Status glyph, coloured by the stack's [`compose::Tone`].
    dot: gtk4::Label,
    status: gtk4::Label,
    chips: gtk4::FlowBox,
    /// Services the chips were last drawn for.
    chips_key: RefCell<String>,
    pub rebuild_btn: gtk4::Button,
    bar_buttons: Vec<gtk4::Button>,
    logs_label: gtk4::Label,
    app: Weak<App>,
    id: PaneId,
}

impl PaneCompose {
    /// The button (prepended to `actions`, before the drag handle) and the two
    /// bars, which the caller places under the terminal.
    pub fn new(app: &AppRc, id: PaneId, actions: &gtk4::Box) -> Self {
        let button = attach_button(app, id, actions);

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
            let w = app.weak();
            rebuild_btn.connect_clicked(move |_| {
                if let Some(a) = w.upgrade() {
                    rebuild(&a, id);
                }
            });
        }
        for (b, action) in [(&restart_btn, Action::Restart), (&stop_btn, Action::Stop)] {
            let w = app.weak();
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
            let w = app.weak();
            stop_logs_btn.connect_clicked(move |_| {
                if let Some(a) = w.upgrade() {
                    stop_logs(&a, id);
                }
            });
        }
        logs_bar.append(&stop_logs_btn);

        let logs_clip = clip(&logs_bar);
        let stack_clip = clip(&stack_bar);
        Self {
            button,
            stack_bar,
            logs_bar,
            logs_clip,
            stack_clip,
            scan: None,
            stack: None,
            logs: None,
            rescan: false,
            pending: None,
            picker: RefCell::new(None),
            dot,
            status,
            chips,
            chips_key: RefCell::new(String::new()),
            rebuild_btn: rebuild_btn.clone(),
            bar_buttons: vec![rebuild_btn, restart_btn, stop_btn],
            logs_label,
            app: app.weak(),
            id,
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
    set_visible(&c.logs_bar, logs.is_some());
    if let Some(file) = logs {
        set_label(&c.logs_label, &format!("≡ logs -f  {} │ following", c.display(file)));
    }

    let stack = c.stack.as_ref().filter(|_| on);
    set_visible(&c.stack_bar, stack.is_some());
    let Some(stack) = stack else { return };
    let tone = compose::stack_tone(&stack.services, stack.stopping);
    set_label(&c.dot, &format!("{} ", tone_glyph(tone)));
    set_tone(&c.dot, tone);
    set_label(&c.status, &status_text(c, stack));
    for b in &c.bar_buttons {
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
    if *c.chips_key.borrow() != key {
        draw_chips(c, stack);
        *c.chips_key.borrow_mut() = key;
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

fn draw_chips(c: &PaneCompose, s: &Stack) {
    c.chips.remove_all();
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
        c.chips.append(&b);
    }
}

thread_local! {
    static CLOCK: Cell<bool> = const { Cell::new(false) };
}

/// Tick the uptime once a second while any stack bar is visible.
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
        for p in i.panes.values() {
            if let (Some(s), true) = (&p.compose.stack, p.compose.stack_bar.get_visible()) {
                set_label(&p.compose.status, &status_text(&p.compose, s));
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

/// Type `action` for `file` into the pane, and press Enter when
/// `compose.run_immediately` is on. Does nothing while the pane is busy, and
/// only says why while the prompt holds unfinished input: the command would be
/// appended to it (and Enter would run both).
pub fn run(app: &AppRc, id: PaneId, file: &Path, action: Action) {
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
        let Some(cmd) = p.compose.scan.as_ref().and_then(|s| s.command.clone()) else {
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
        }
        Action::Logs => c.logs = Some(file.to_path_buf()),
        Action::Down if c.stack.as_ref().is_some_and(|s| s.file == file) => c.stack = None,
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

/// The file picker under the pane button, drawn like terminal output.
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
    head.append(&root);
    body.append(&head);

    let filter_row = gtk4::Box::new(Orientation::Horizontal, 6);
    filter_row.add_css_class("compose-filter");
    filter_row.append(&label("/", &["compose-accent"]));
    let entry = gtk4::SearchEntry::new();
    entry.set_placeholder_text(Some("press / to filter"));
    entry.set_hexpand(true);
    filter_row.append(&entry);
    body.append(&filter_row);

    let list = gtk4::ListBox::new();
    list.add_css_class("compose-files");
    list.set_selection_mode(gtk4::SelectionMode::Single);
    // Grouped like a tree: the repo root's files first, then each folder as a
    // header with its files under it (the scan keeps each folder's files together).
    // `slots[row index]` is the file shown by that row, `None` for a header.
    let mut slots: Vec<Option<usize>> = Vec::new();
    let mut folder: Option<String> = None;
    for (n, f) in scan.files.iter().enumerate() {
        let dir = folder_of(f);
        if folder.as_ref() != Some(&dir) {
            list.append(&group_row(&dir));
            slots.push(None);
            folder = Some(dir);
        }
        list.append(&file_row(app, id, &pop, f));
        slots.push(Some(n));
    }
    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(320)
        .min_content_width(380)
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

    let keys = label(
        &format!("↑↓ move  ⏎ {default_action}  d detached  x down  l logs  / filter  r rescan  esc close"),
        &["compose-keys", "dim"],
    );
    keys.set_ellipsize(gtk4::pango::EllipsizeMode::None);
    keys.set_wrap(true);
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

    // Filter on the path.
    {
        let entry = entry.clone();
        let files = scan.files.clone();
        list.set_filter_func(move |row| {
            let q = entry.text().to_lowercase();
            let name = row.widget_name();
            let hit = |rel: &str| q.is_empty() || rel.to_lowercase().contains(&q);
            match name.strip_prefix(GROUP_PREFIX) {
                // A folder header shows while any of its files does.
                Some(dir) => files.iter().any(|f| folder_of(f) == dir && hit(&f.rel.to_string_lossy())),
                None => hit(&name),
            }
        });
    }
    {
        let list = list.clone();
        entry.connect_search_changed(move |_| {
            list.invalidate_filter();
            select_visible(&list, 0, false);
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
        let (w, pop, list, entry, rescan_btn) = (app.weak(), pop.clone(), list.clone(), entry.clone(), rescan_btn);
        keyc.connect_key_pressed(move |_, key, _, mods| {
            if mods.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | super::primary_mask()) {
                return glib::Propagation::Proceed;
            }
            let in_entry = entry.state_flags().contains(gtk4::StateFlags::FOCUS_WITHIN);
            let act = |action: Action| {
                if let (Some(a), Some(f)) = (w.upgrade(), chosen()) {
                    pop.popdown();
                    run(&a, id, &f.path, action);
                }
            };
            match key {
                // Our own arrow handling: GTK's list would get stuck on a folder
                // header (not selectable) instead of skipping it.
                gdk::Key::Down | gdk::Key::Up | gdk::Key::KP_Down | gdk::Key::KP_Up => {
                    let down = matches!(key, gdk::Key::Down | gdk::Key::KP_Down);
                    select_visible(&list, if down { 1 } else { -1 }, !in_entry);
                }
                gdk::Key::Return | gdk::Key::KP_Enter if in_entry => act(default.clone()),
                _ if in_entry => return glib::Propagation::Proceed,
                gdk::Key::j => select_visible(&list, 1, true),
                gdk::Key::k => select_visible(&list, -1, true),
                gdk::Key::u => act(default.clone()),
                gdk::Key::d => act(Action::UpDetached),
                gdk::Key::x => act(Action::Down),
                gdk::Key::l => act(Action::Logs),
                gdk::Key::r => rescan_btn.emit_clicked(),
                gdk::Key::slash => {
                    entry.grab_focus();
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

    // Move keyboard focus into the list once the popover is on screen.
    {
        let list = list.clone();
        pop.connect_map(move |_| {
            let list = list.clone();
            glib::idle_add_local_once(move || select_visible(&list, 0, true));
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
fn group_row(dir: &str) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();
    row.set_widget_name(&format!("{GROUP_PREFIX}{dir}"));
    row.set_selectable(false);
    row.set_activatable(false);
    row.set_can_focus(false);
    row.add_css_class("compose-group-row");
    row.set_child(Some(&label(dir, &["compose-group"])));
    row
}

fn file_row(app: &AppRc, id: PaneId, pop: &gtk4::Popover, f: &ComposeFile) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();
    row.set_widget_name(&f.rel.to_string_lossy());
    row.add_css_class("compose-file");
    let line = gtk4::Box::new(Orientation::Horizontal, 8);
    let text = gtk4::Box::new(Orientation::Vertical, 0);
    text.set_hexpand(true);
    let name = f.rel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    text.append(&label(&name, &["compose-name"]));
    let services = match &f.services {
        Some(s) if s.len() == 1 => format!("1 service · {}", s[0]),
        Some(s) => format!("{} services · {}", s.len(), s.join(", ")),
        None => "could not read services".into(),
    };
    text.append(&label(&services, &["dim"]));
    line.append(&text);
    for (key, word, tip, action) in [
        ("d", "-d", "Start detached (key d)", Action::UpDetached),
        ("x", "down", "Stop and remove (key x)", Action::Down),
        ("l", "logs", "Follow logs (key l)", Action::Logs),
    ] {
        let b = key_button(key, word, tip);
        b.set_valign(gtk4::Align::Center);
        let (w, pop, path) = (app.weak(), pop.clone(), f.path.clone());
        b.connect_clicked(move |_| {
            if let Some(a) = w.upgrade() {
                pop.popdown();
                run(&a, id, &path, action.clone());
            }
        });
        line.append(&b);
    }
    row.set_child(Some(&line));
    row
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

/// Wrap a bar so its width is clipped to the pane instead of setting the
/// pane's minimum width; it shows and hides with the bar.
fn clip(bar: &gtk4::Box) -> Clip {
    let c = Clip::new(bar);
    bar.bind_property("visible", &c, "visible").sync_create().build();
    c
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
    fn new(child: &impl IsA<gtk4::Widget>) -> Self {
        let c: Self = glib::Object::new();
        c.set_overflow(gtk4::Overflow::Hidden);
        child.set_parent(&c);
        c
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
