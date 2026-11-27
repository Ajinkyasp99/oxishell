//! Background metadata refresh (cwd, git, ports, foreground agent).
//!
//! Polling is cheap procfs reads every 3 s for the visible workspace and every
//! 15 s for the rest. Listening ports are looked up once per batch for the
//! panes that are due ([`PortSchedule`]: a process started or exited, a forced
//! check, or a 9-30 s back-off). `git status` runs when a pane's directory
//! changes, a prompt was drawn since the last job, or its cached result has
//! expired ([`GitBackoff`]: 5-30 s, fixed 5 s without shell integration).
//! While the window is hidden (minimized or suspended) the 3 s tick only
//! refreshes visible-workspace panes running a Compose stack, without ports or
//! git ([`hidden_jobs`]); the 15 s full pass keeps running.
//! Everything runs off the GTK thread.

use super::AppRc;
use crate::compose::{self, ComposeFile, ComposeScan, ServiceStatus};
use crate::meta::{self, GitInfo};
use crate::meta_schedule::{GitBackoff, PortSchedule, PromptWatch};
use crate::model::{AppState, PaneId};
use gtk4::glib;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct Job {
    pub pane: PaneId,
    pub shell_pid: i32,
    pub pty_fd: i32,
    /// Ports may be checked (the worker's schedule decides whether they are due).
    pub ports: bool,
    /// Check ports now, whatever the schedule says.
    pub force_ports: bool,
    /// Only the Compose state is wanted: no ports and no git lookup.
    pub compose_only: bool,
    /// File the shell writes its active Python/conda environment to.
    pub env_file: PathBuf,
    /// `Some(compose.command setting)` when the Compose launcher is on.
    pub compose_cmd: Option<String>,
    /// Scan the repo again instead of using the cache.
    pub rescan: bool,
    /// Stack whose `ps` to report (a stack runs from this pane): its compose
    /// program, file, and the environment and folder it was started with.
    pub ps_of: Option<(Vec<String>, PathBuf, Option<compose::LaunchContext>)>,
}

/// Coalesce two queued jobs of one pane: the newest wins, but flags the older
/// one asked for are not lost.
fn merge_job(old: Job, new: Job) -> Job {
    Job {
        ports: new.ports || old.ports,
        force_ports: new.force_ports || old.force_ports,
        rescan: new.rescan || old.rescan,
        compose_only: new.compose_only && old.compose_only,
        ..new
    }
}

pub struct Update {
    pane: PaneId,
    cwd: Option<PathBuf>,
    /// `None`: not looked up (keep what the pane has).
    git: Option<Option<GitInfo>>,
    ports: Option<Vec<u16>>,
    fg_agent: Option<&'static str>,
    fg_program: Option<String>,
    python_env: Option<String>,
    /// Commands the shell ran since the last update (from its history log).
    commands: Vec<String>,
    busy: bool,
    compose: Option<ComposeScan>,
    ps: Option<Result<Vec<ServiceStatus>, String>>,
}

/// Lifetimes of the Compose caches. A compose file added later (or docker
/// installed later) shows up by itself once its entry expires.
#[derive(Debug, Clone, Copy)]
pub struct ComposeTtls {
    /// Detected compose program.
    pub commands: Duration,
    /// Pane folder -> repo root.
    pub roots: Duration,
    /// Compose-file scan, right after a change (and the start of the back-off).
    pub files_base: Duration,
    /// Cap of the scan's back-off: it doubles while the result is unchanged.
    pub files_max: Duration,
}

impl Default for ComposeTtls {
    fn default() -> Self {
        Self {
            commands: Duration::from_secs(600),
            roots: Duration::from_secs(300),
            files_base: Duration::from_secs(30),
            files_max: Duration::from_secs(300),
        }
    }
}

impl ComposeTtls {
    /// Everything expires at once (tests).
    #[cfg(test)]
    pub fn zero() -> Self {
        Self {
            commands: Duration::ZERO,
            roots: Duration::ZERO,
            files_base: Duration::ZERO,
            files_max: Duration::ZERO,
        }
    }
}

/// A cached compose-file scan of one root.
struct FilesEntry {
    at: Instant,
    ttl: Duration,
    files: Vec<ComposeFile>,
    /// A full-depth scan was done (a manual rescan); `$HOME` and `/` then keep
    /// scanning deep instead of going back to the shallow automatic scan.
    deep: bool,
}

/// Worker-side caches for the Compose launcher.
struct ComposeCache {
    ttls: ComposeTtls,
    /// The home folder: scanned shallow automatically, like `/`.
    home: PathBuf,
    /// Pane folder -> repo root.
    roots: HashMap<PathBuf, (Instant, PathBuf)>,
    /// Repo root -> compose files.
    files: HashMap<PathBuf, FilesEntry>,
    /// `compose.command` setting -> detected program.
    commands: HashMap<String, (Instant, Option<Vec<String>>)>,
    /// Stack file and its modification time -> the services it declares
    /// (`None` when unknown). Keyed by mtime so an edited file counts at once.
    declared: HashMap<FileVersion, (Instant, Option<Vec<String>>)>,
    /// Last scan sent for each pane.
    sent: HashMap<PaneId, ComposeScan>,
}

impl Default for ComposeCache {
    fn default() -> Self {
        Self {
            ttls: ComposeTtls::default(),
            home: crate::paths::home(),
            roots: HashMap::new(),
            files: HashMap::new(),
            commands: HashMap::new(),
            declared: HashMap::new(),
            sent: HashMap::new(),
        }
    }
}

/// A file and its modification time.
type FileVersion = (PathBuf, Option<std::time::SystemTime>);

/// The cached value for `key`, or a fresh one from `make` when missing, older
/// than `ttl`, or `force`d.
fn cached<K: std::hash::Hash + Eq + Clone, V: Clone>(
    map: &mut HashMap<K, (Instant, V)>,
    key: &K,
    ttl: Duration,
    force: bool,
    make: impl FnOnce() -> V,
) -> V {
    match map.get(key) {
        Some((at, v)) if !force && at.elapsed() < ttl => v.clone(),
        _ => {
            let v = make();
            map.insert(key.clone(), (Instant::now(), v.clone()));
            v
        }
    }
}

impl ComposeCache {
    /// The pane's scan when it differs from the last one sent, or on rescan.
    fn scan(&mut self, pane: PaneId, cwd: &Path, setting: &str, rescan: bool) -> Option<ComposeScan> {
        if self.roots.len() > 256 {
            self.roots.clear();
        }
        let ttls = self.ttls;
        let root = cached(&mut self.roots, &cwd.to_path_buf(), ttls.roots, rescan, || compose::repo_root(cwd));
        let files = self.files_of(&root, rescan);
        let command = cached(&mut self.commands, &setting.to_string(), ttls.commands, rescan, || {
            compose::detect_command(setting, compose::command_works)
        });
        let scan = ComposeScan { root, files, command };
        if !rescan && self.sent.get(&pane) == Some(&scan) {
            return None;
        }
        self.sent.insert(pane, scan.clone());
        Some(scan)
    }

    /// Compose files under `root`. `$HOME` and `/` get a shallow automatic scan
    /// (a deep one would walk the whole disk); a rescan searches deep. An
    /// unchanged result is re-checked less often, up to `files_max`.
    fn files_of(&mut self, root: &Path, rescan: bool) -> Vec<ComposeFile> {
        if self.files.len() > 256 {
            self.files.clear();
        }
        let prev = self.files.get(root);
        if let Some(e) = prev {
            if !rescan && e.at.elapsed() < e.ttl {
                return e.files.clone();
            }
        }
        let shallow_root = root == self.home || root == Path::new("/");
        let deep = rescan || prev.is_some_and(|e| e.deep);
        let limits = if shallow_root && !deep {
            compose::Limits {
                max_depth: 1,
                ..Default::default()
            }
        } else {
            compose::Limits::default()
        };
        let found = compose::scan_limited(root, &limits);
        let ttl = match prev {
            Some(e) if !rescan && e.files == found => (e.ttl * 2).min(self.ttls.files_max),
            _ => self.ttls.files_base,
        };
        self.files.insert(
            root.to_path_buf(),
            FilesEntry {
                at: Instant::now(),
                ttl,
                files: found.clone(),
                deep,
            },
        );
        found
    }

    /// `ps` of a stack, narrowed to the services its file declares.
    fn ps(&mut self, cmd: &[String], file: &Path, ctx: Option<&compose::LaunchContext>) -> Result<Vec<ServiceStatus>, String> {
        if self.declared.len() > 256 {
            self.declared.clear();
        }
        let ps = compose_ps(cmd, file, ctx)?;
        let key = (file.to_path_buf(), std::fs::metadata(file).and_then(|m| m.modified()).ok());
        let declared = cached(&mut self.declared, &key, self.ttls.files_base, false, || {
            declared_services(cmd, file, ctx)
        });
        Ok(compose::only_declared(ps, declared.as_deref()))
    }
}

/// `argv` as a command run in the stack's context (or the worker's own).
fn command_in(argv: &[String], ctx: Option<&compose::LaunchContext>) -> std::process::Command {
    let mut c = match ctx {
        Some(ctx) => ctx.command(argv),
        None => {
            let mut c = std::process::Command::new(&argv[0]);
            c.args(&argv[1..]);
            c
        }
    };
    c.stdin(std::process::Stdio::null());
    c
}

/// The services `file` declares: Compose's own answer (it resolves `include:`,
/// `extends` and profiles), else the file's top-level `services:` keys, else
/// `None` when neither can be read.
fn declared_services(cmd: &[String], file: &Path, ctx: Option<&compose::LaunchContext>) -> Option<Vec<String>> {
    match command_in(&compose::services_argv(cmd, file), ctx).output() {
        Ok(o) if o.status.success() => compose::parse_services(&String::from_utf8_lossy(&o.stdout)),
        _ => None,
    }
    .or_else(|| compose::read_service_names(file))
}

/// `docker compose ps` of `file` with `cmd`, run in the stack's context.
fn compose_ps(cmd: &[String], file: &Path, ctx: Option<&compose::LaunchContext>) -> Result<Vec<ServiceStatus>, String> {
    match command_in(&compose::ps_argv(cmd, file), ctx).output() {
        Ok(o) if o.status.success() => compose::parse_ps(&String::from_utf8_lossy(&o.stdout)),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            Err(err.lines().last().unwrap_or("compose ps failed").to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Ports of the panes that are due, with one lookup for all their pids.
/// `trees`: (pane, its process tree sorted ascending, forced). Panes that are
/// not due are left out (their last result stands).
fn due_ports(
    sched: &mut PortSchedule,
    trees: Vec<(PaneId, Vec<i32>, bool)>,
    now: Instant,
    lookup: impl FnOnce(&[i32]) -> HashMap<i32, Vec<u16>>,
) -> HashMap<PaneId, Vec<u16>> {
    let due: Vec<(PaneId, Vec<i32>)> = trees
        .into_iter()
        .filter(|(pane, tree, force)| sched.due(*pane, tree, *force, now))
        .map(|(pane, tree, _)| (pane, tree))
        .collect();
    if due.is_empty() {
        return HashMap::new();
    }
    let mut pids: Vec<i32> = due.iter().flat_map(|(_, t)| t.iter().copied()).collect();
    pids.sort_unstable();
    pids.dedup();
    let by_pid = lookup(&pids);
    due.into_iter()
        .map(|(pane, tree)| {
            let mut ports: Vec<u16> = tree.iter().filter_map(|p| by_pid.get(p)).flatten().copied().collect();
            ports.sort_unstable();
            ports.dedup();
            sched.record(pane, &ports, now);
            (pane, ports)
        })
        .collect()
}

pub fn spawn() -> (std::sync::mpsc::Sender<Job>, async_channel::Receiver<Update>) {
    let (tx, rx) = std::sync::mpsc::channel::<Job>();
    let (utx, urx) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("oxishell-meta".into())
        .spawn(move || {
            let mut git_backoff: GitBackoff<Option<GitInfo>> = GitBackoff::new();
            let mut ports_sched = PortSchedule::default();
            let mut prompt_watch = PromptWatch::default();
            let mut compose_cache = ComposeCache::default();
            // How much of each pane's history log has been read.
            let mut hist_offsets: HashMap<PaneId, u64> = HashMap::new();
            while let Ok(first) = rx.recv() {
                // Coalesce queued jobs per pane.
                let mut jobs: HashMap<PaneId, Job> = HashMap::new();
                let mut queue = std::iter::once(first).chain(std::iter::from_fn(|| rx.try_recv().ok()));
                for j in &mut queue {
                    match jobs.remove(&j.pane) {
                        Some(old) => jobs.insert(j.pane, merge_job(old, j)),
                        None => jobs.insert(j.pane, j),
                    };
                }
                let now = Instant::now();
                let jobs: Vec<Job> = jobs
                    .into_values()
                    .filter(|job| {
                        let alive = unsafe { libc::kill(job.shell_pid, 0) } == 0;
                        if !alive {
                            ports_sched.forget(job.pane);
                            prompt_watch.forget(job.pane);
                            hist_offsets.remove(&job.pane);
                        }
                        alive
                    })
                    .collect();
                // Closed panes are never sent again: start over now and then.
                if ports_sched.len() > 512 {
                    ports_sched = PortSchedule::default();
                    prompt_watch = PromptWatch::default();
                }
                // One process-table snapshot per batch, only if some job wants
                // ports, and one port lookup for all panes that are due.
                let wants_ports = |j: &Job| j.ports && !j.compose_only;
                let mut due = match jobs.iter().any(wants_ports).then(meta::ppid_table) {
                    Some(table) => {
                        let trees = jobs
                            .iter()
                            .filter(|j| wants_ports(j))
                            .map(|j| {
                                let mut tree = meta::process_tree_in(&table, j.shell_pid);
                                tree.sort_unstable();
                                (j.pane, tree, j.force_ports)
                            })
                            .collect();
                        due_ports(&mut ports_sched, trees, now, meta::listening_ports_by_pid)
                    }
                    None => HashMap::new(),
                };
                // A prompt in any pane may mean a repo changed: observe all
                // prompt signals first and, if any pane drew one, stop every
                // cached git entry from staying backed off (cross-pane lag).
                // Compose-only jobs leave the prompt signal for the next
                // ordinary job.
                let signals: Vec<Option<(bool, bool)>> = jobs
                    .iter()
                    .map(|job| {
                        (!job.compose_only).then(|| {
                            let mtime = std::fs::metadata(&job.env_file).and_then(|m| m.modified()).ok();
                            prompt_watch.observe(job.pane, mtime)
                        })
                    })
                    .collect();
                if signals.iter().flatten().any(|(_, prompted)| *prompted) {
                    git_backoff.reset_ttls();
                }
                for (job, signal) in jobs.into_iter().zip(signals) {
                    let fg = meta::foreground_pgrp(job.pty_fd).filter(|p| *p > 0);
                    let cwd = fg.and_then(meta::cwd_of).or_else(|| meta::cwd_of(job.shell_pid));
                    let fg_agent = fg.and_then(|p| meta::comm(p).and_then(|c| meta::agent_for_process(&c, &meta::cmdline(p))));
                    let git = signal.map(|(integrated, prompted)| {
                        cwd.as_ref()
                            .and_then(|c| git_backoff.get(c, !integrated, prompted, now, || meta::git_info(c)))
                    });
                    let busy = fg.is_some_and(|p| p != job.shell_pid);
                    let fg_program = fg.filter(|p| *p != job.shell_pid).and_then(meta::comm);
                    let (compose_scan, ps) = match (&job.compose_cmd, &cwd) {
                        (Some(setting), Some(c)) => {
                            let scan = compose_cache.scan(job.pane, c, setting, job.rescan);
                            let ps = job.ps_of.as_ref().map(|(cmd, f, ctx)| compose_cache.ps(cmd, f, ctx.as_ref()));
                            (scan, ps)
                        }
                        _ => {
                            compose_cache.sent.remove(&job.pane);
                            (None, None)
                        }
                    };
                    let ports = due.remove(&job.pane);
                    let commands = crate::shellint::read_new_commands(
                        &crate::shellint::hist_file(job.pane),
                        hist_offsets.entry(job.pane).or_default(),
                    );
                    if utx
                        .send_blocking(Update {
                            pane: job.pane,
                            cwd,
                            git,
                            ports,
                            fg_agent,
                            fg_program,
                            python_env: crate::shellint::read_env(&job.env_file),
                            commands,
                            busy,
                            compose: compose_scan,
                            ps,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }
        })
        .expect("spawn meta thread");
    (tx, urx)
}

/// Panes the 3 s tick still refreshes while the window is hidden: those of the
/// visible workspace that run a Compose stack (crash notifications). Everything
/// else waits for the 15 s full pass.
pub(super) fn hidden_jobs(state: &AppState, panes_with_stack: &HashSet<PaneId>) -> Vec<PaneId> {
    state
        .active_workspace()
        .map(|w| w.panes().into_iter().filter(|p| panes_with_stack.contains(p)).collect())
        .unwrap_or_default()
}

pub fn attach(app: &AppRc, rx: async_channel::Receiver<Update>) {
    let w = app.weak();
    let refresh = super::once_per_idle({
        let w = w.clone();
        move || {
            if let Some(a) = w.upgrade() {
                a.refresh();
            }
        }
    });
    glib::spawn_future_local(async move {
        while let Ok(u) = rx.recv().await {
            let Some(a) = w.upgrade() else { return };
            let changed = {
                let mut i = a.inner.borrow_mut();
                if !u.commands.is_empty() && i.panes.contains_key(&u.pane) {
                    // What the shell itself ran: exact, whatever keys built the line.
                    for cmd in &u.commands {
                        i.history.push(cmd);
                    }
                }
                let Some(p) = i.panes.get_mut(&u.pane) else { continue };
                let mut changed = false;
                if u.cwd.is_some() && p.cwd != u.cwd {
                    p.cwd = u.cwd;
                    changed = true;
                }
                if let Some(git) = u.git.filter(|g| p.git != *g) {
                    p.git = git;
                    changed = true;
                }
                if let Some(ports) = u.ports {
                    if p.ports != ports {
                        p.ports = ports;
                        changed = true;
                    }
                }
                if p.fg_agent != u.fg_agent {
                    let had_agent = p.agent_id().is_some();
                    p.fg_agent = u.fg_agent;
                    match (had_agent, p.agent_id().is_some()) {
                        (false, true) => p.agent_started(),
                        (true, false) => p.clear_agent_info(),
                        _ => {}
                    }
                    changed = true;
                }
                if p.fg_program != u.fg_program {
                    p.fg_program = u.fg_program;
                    changed = true;
                }
                if p.python_env != u.python_env {
                    p.python_env = u.python_env;
                    changed = true;
                }
                // Launcher state is not part of the saved session.
                let mut shown = false;
                if p.busy != u.busy {
                    p.busy = u.busy;
                    if !u.busy {
                        // Whatever held the pane (e.g. `logs -f`) has ended.
                        p.compose.logs = None;
                        // An attached `up` ended (Ctrl+C, or it stopped by itself).
                        if let Some(s) = p.compose.stack.as_mut().filter(|s| s.attached) {
                            s.stopping = true;
                        }
                    }
                    shown = true;
                }
                if let Some(scan) = u.compose {
                    p.compose.scan = Some(scan);
                    shown = true;
                }
                let mut crashed = vec![];
                let file = p.compose.stack.as_ref().map(|s| p.compose.display(&s.file)).unwrap_or_default();
                if let (Some(ps), Some(stack)) = (u.ps, p.compose.stack.as_mut()) {
                    if let Ok(now) = &ps {
                        crashed = compose::crashes(&stack.services, now, stack.stopping)
                            .into_iter()
                            .map(|c| (stack.id, file.clone(), c))
                            .collect();
                    }
                    if !stack.apply_ps(ps, p.busy) {
                        p.compose.end_stack();
                    }
                    shown = true;
                }
                let notify = i.config.compose.notify_on_crash;
                (changed, shown, if notify { crashed } else { vec![] })
            };
            for (stack, file, crash) in &changed.2 {
                a.notify_compose_crash(u.pane, *stack, file, crash);
            }
            if changed.0 || changed.1 {
                refresh();
            }
            if changed.0 {
                a.schedule_save();
            }
        }
    });
    let w = app.weak();
    let mut tick = 0u32;
    glib::timeout_add_seconds_local(3, move || {
        let Some(a) = w.upgrade() else { return glib::ControlFlow::Break };
        tick += 1;
        // Every 5th tick is the full pass; it runs even while the window is hidden.
        let active_only_tick = !tick.is_multiple_of(5);
        if a.is_hidden() && active_only_tick {
            a.request_hidden_meta();
        } else {
            a.request_meta(active_only_tick, false);
        }
        // Keep the sidebar's "time since activity" current.
        if tick.is_multiple_of(10) {
            super::sidebar::rebuild(&a);
        }
        glib::ControlFlow::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oxishell-meta-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn job(pane: PaneId) -> Job {
        Job {
            pane,
            shell_pid: 1,
            pty_fd: -1,
            ports: false,
            force_ports: false,
            compose_only: true,
            env_file: PathBuf::from("/x"),
            compose_cmd: None,
            rescan: false,
            ps_of: None,
        }
    }

    #[test]
    fn coalescing_keeps_flags_of_the_older_job() {
        let id = PaneId::default();
        let mut old = job(id);
        old.force_ports = true;
        old.rescan = true;
        old.ports = true;
        let mut new = job(id);
        new.shell_pid = 7;
        let m = merge_job(old, new);
        assert!(m.force_ports && m.rescan && m.ports);
        assert!(m.compose_only);
        assert_eq!(m.shell_pid, 7);
        // An ordinary job anywhere in the pair makes the merged job ordinary.
        let mut ordinary = job(id);
        ordinary.compose_only = false;
        assert!(!merge_job(ordinary, job(id)).compose_only);
    }

    #[test]
    fn hidden_tick_only_refreshes_running_stacks() {
        use crate::model::Orient;
        let mut state = AppState::default();
        let (_, other) = state.create_workspace(None).unwrap();
        let (_, p1) = state.create_workspace(None).unwrap();
        let p2 = state.split(p1, Orient::Horizontal).unwrap();
        let _p3 = state.split(p2, Orient::Vertical).unwrap();
        assert_eq!(hidden_jobs(&state, &HashSet::from([p2])), vec![p2]);
        assert_eq!(hidden_jobs(&state, &HashSet::new()), Vec::<PaneId>::new());
        // A stack in a workspace that is not visible waits for the 15 s full pass.
        assert_eq!(hidden_jobs(&state, &HashSet::from([other])), Vec::<PaneId>::new());
    }

    #[test]
    fn due_panes_share_one_port_lookup_and_get_their_own_union() {
        let mut sched = PortSchedule::default();
        let t0 = Instant::now();
        let (a, b): (PaneId, PaneId) = (1, 2);
        let mut calls = vec![];
        let got = due_ports(&mut sched, vec![(a, vec![10, 11], false), (b, vec![20], false)], t0, |pids| {
            calls.push(pids.to_vec());
            HashMap::from([(10, vec![8080]), (11, vec![3000, 8080]), (20, vec![5432])])
        });
        assert_eq!(calls, vec![vec![10, 11, 20]], "one lookup for every due pid");
        assert_eq!(got, HashMap::from([(a, vec![3000, 8080]), (b, vec![5432])]));
        // Nothing is due a second later: no lookup at all, no result.
        let got = due_ports(&mut sched, vec![(a, vec![10, 11], false), (b, vec![20], false)], t0, |_| {
            panic!("no pane is due")
        });
        assert!(got.is_empty());
        // A tree change or a forced check makes only that pane due.
        let mut calls = vec![];
        let got = due_ports(&mut sched, vec![(a, vec![10, 11, 12], false), (b, vec![20], true)], t0, |pids| {
            calls.push(pids.to_vec());
            HashMap::from([(12, vec![9000])])
        });
        assert_eq!(calls, vec![vec![10, 11, 12, 20]]);
        assert_eq!(got, HashMap::from([(a, vec![9000]), (b, vec![])]));
    }

    const CMD: &str = "/bin/true compose";

    fn cache(ttls: ComposeTtls, home: &Path) -> ComposeCache {
        ComposeCache {
            ttls,
            home: home.to_path_buf(),
            ..Default::default()
        }
    }

    fn deep_compose(dir: &Path) {
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/b/docker-compose.yml"), "services:\n  api: {}\n").unwrap();
    }

    #[test]
    fn compose_files_added_later_are_found_without_a_rescan() {
        let dir = tmp("added-later");
        let mut c = cache(ComposeTtls::zero(), Path::new("/nonexistent-home"));
        let first = c.scan(1, &dir, CMD, false).expect("first scan is sent");
        assert!(first.files.is_empty());
        std::fs::write(dir.join("docker-compose.yml"), "services:\n  api: {}\n").unwrap();
        let second = c.scan(1, &dir, CMD, false).expect("the new file is reported");
        assert_eq!(second.files.len(), 1);
    }

    #[test]
    fn the_scan_cache_is_reused_within_its_lifetime() {
        let dir = tmp("cached");
        let mut c = cache(ComposeTtls::default(), Path::new("/nonexistent-home"));
        c.scan(1, &dir, CMD, false).expect("first scan is sent");
        std::fs::write(dir.join("docker-compose.yml"), "services:\n  api: {}\n").unwrap();
        assert!(c.scan(1, &dir, CMD, false).is_none(), "cached, unchanged");
        assert!(c.scan(1, &dir, CMD, true).is_some(), "rescan bypasses the cache");
    }

    #[test]
    fn files_ttl_doubles_while_unchanged_and_resets_on_change() {
        let dir = tmp("ttl-backoff");
        let ms = Duration::from_millis;
        let ttls = ComposeTtls {
            files_base: ms(1),
            files_max: ms(4),
            ..ComposeTtls::default()
        };
        let mut c = cache(ttls, Path::new("/nonexistent-home"));
        let step = |c: &mut ComposeCache| {
            std::thread::sleep(ms(10)); // let the entry expire
            c.files_of(&dir, false);
            c.files[&dir].ttl
        };
        c.files_of(&dir, false);
        assert_eq!(c.files[&dir].ttl, ms(1));
        let got: Vec<_> = (0..3).map(|_| step(&mut c)).collect();
        assert_eq!(got, vec![ms(2), ms(4), ms(4)]);
        std::fs::write(dir.join("docker-compose.yml"), "services:\n  api: {}\n").unwrap();
        assert_eq!(step(&mut c), ms(1), "a change resets the back-off");
    }

    #[test]
    fn home_is_scanned_shallow_until_a_rescan() {
        let home = tmp("home-shallow");
        deep_compose(&home);
        let mut c = cache(ComposeTtls::zero(), &home);
        assert!(c.files_of(&home, false).is_empty(), "automatic scan of $HOME is shallow");
        assert_eq!(c.files_of(&home, true).len(), 1, "a rescan searches deep");
        assert_eq!(c.files_of(&home, false).len(), 1, "later automatic scans stay deep");
    }

    #[test]
    fn other_folders_are_scanned_deep() {
        let dir = tmp("not-home");
        deep_compose(&dir);
        let mut c = cache(ComposeTtls::zero(), Path::new("/nonexistent-home"));
        assert_eq!(c.files_of(&dir, false).len(), 1);
    }

    #[test]
    fn command_detection_is_cached_for_ten_minutes() {
        assert_eq!(ComposeTtls::default().commands, Duration::from_secs(600));
        assert_eq!(ComposeTtls::default().roots, Duration::from_secs(300));
        assert_eq!(ComposeTtls::default().files_base, Duration::from_secs(30));
        assert_eq!(ComposeTtls::default().files_max, Duration::from_secs(300));
    }
}
