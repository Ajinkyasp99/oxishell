//! Background metadata refresh (cwd, git, ports, foreground agent).
//!
//! Polling is cheap procfs reads every 3 s for the visible workspace and every
//! 15 s for the rest; `git status` runs only when a pane's directory changes
//! or its cached result is older than 5 s. Everything runs off the GTK thread.

use super::AppRc;
use crate::compose::{self, ComposeFile, ComposeScan, ServiceStatus};
use crate::meta::{self, GitInfo};
use crate::model::PaneId;
use gtk4::glib;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct Job {
    pub pane: PaneId,
    pub shell_pid: i32,
    pub pty_fd: i32,
    pub ports: bool,
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

pub struct Update {
    pane: PaneId,
    cwd: Option<PathBuf>,
    git: Option<GitInfo>,
    ports: Option<Vec<u16>>,
    fg_agent: Option<&'static str>,
    fg_program: Option<String>,
    python_env: Option<String>,
    busy: bool,
    compose: Option<ComposeScan>,
    ps: Option<Result<Vec<ServiceStatus>, String>>,
}

const GIT_TTL: Duration = Duration::from_secs(5);

/// Worker-side caches for the Compose launcher. Entries expire after `ttl`, so
/// a compose file added later (or docker installed later) shows up by itself.
struct ComposeCache {
    ttl: Duration,
    /// Pane folder -> repo root.
    roots: HashMap<PathBuf, (Instant, PathBuf)>,
    /// Repo root -> compose files.
    files: HashMap<PathBuf, (Instant, Vec<ComposeFile>)>,
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
            ttl: COMPOSE_TTL,
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

/// How long a Compose scan result is reused before scanning again.
const COMPOSE_TTL: Duration = Duration::from_secs(30);

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
        let ttl = self.ttl;
        let root = cached(&mut self.roots, &cwd.to_path_buf(), ttl, rescan, || compose::repo_root(cwd));
        let files = cached(&mut self.files, &root, ttl, rescan, || compose::scan(&root));
        let command = cached(&mut self.commands, &setting.to_string(), ttl, rescan, || {
            compose::detect_command(setting, compose::command_works)
        });
        let scan = ComposeScan { root, files, command };
        if !rescan && self.sent.get(&pane) == Some(&scan) {
            return None;
        }
        self.sent.insert(pane, scan.clone());
        Some(scan)
    }

    /// `ps` of a stack, narrowed to the services its file declares.
    fn ps(&mut self, cmd: &[String], file: &Path, ctx: Option<&compose::LaunchContext>) -> Result<Vec<ServiceStatus>, String> {
        if self.declared.len() > 256 {
            self.declared.clear();
        }
        let ps = compose_ps(cmd, file, ctx)?;
        let key = (file.to_path_buf(), std::fs::metadata(file).and_then(|m| m.modified()).ok());
        let declared = cached(&mut self.declared, &key, self.ttl, false, || declared_services(cmd, file, ctx));
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

pub fn spawn() -> (std::sync::mpsc::Sender<Job>, async_channel::Receiver<Update>) {
    let (tx, rx) = std::sync::mpsc::channel::<Job>();
    let (utx, urx) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("oxishell-meta".into())
        .spawn(move || {
            let mut git_cache: HashMap<PathBuf, (Instant, Option<GitInfo>)> = HashMap::new();
            let mut compose_cache = ComposeCache::default();
            while let Ok(first) = rx.recv() {
                // Coalesce queued jobs per pane.
                let mut jobs: HashMap<PaneId, Job> = HashMap::new();
                jobs.insert(first.pane, first);
                while let Ok(j) = rx.try_recv() {
                    jobs.insert(j.pane, j);
                }
                // One /proc scan per batch, only if some job wants ports.
                let table = jobs.values().any(|j| j.ports).then(meta::ppid_table);
                for job in jobs.into_values() {
                    if unsafe { libc::kill(job.shell_pid, 0) } != 0 {
                        continue;
                    }
                    let fg = meta::foreground_pgrp(job.pty_fd).filter(|p| *p > 0);
                    let cwd = fg.and_then(meta::cwd_of).or_else(|| meta::cwd_of(job.shell_pid));
                    let fg_agent = fg.and_then(|p| meta::comm(p).and_then(|c| meta::agent_for_process(&c, &meta::cmdline(p))));
                    let git = cwd.as_ref().and_then(|c| {
                        let fresh = git_cache.get(c).filter(|(t, _)| t.elapsed() < GIT_TTL).map(|(_, g)| g.clone());
                        fresh.unwrap_or_else(|| {
                            let g = meta::git_info(c);
                            git_cache.insert(c.clone(), (Instant::now(), g.clone()));
                            g
                        })
                    });
                    if git_cache.len() > 256 {
                        git_cache.retain(|_, (t, _)| t.elapsed() < GIT_TTL);
                    }
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
                    let ports = match (&table, job.ports) {
                        (Some(t), true) => Some(meta::listening_ports(&meta::process_tree_in(t, job.shell_pid))),
                        _ => None,
                    };
                    if utx
                        .send_blocking(Update {
                            pane: job.pane,
                            cwd,
                            git,
                            ports,
                            fg_agent,
                            fg_program,
                            python_env: crate::shellint::read_env(&job.env_file),
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

pub fn attach(app: &AppRc, rx: async_channel::Receiver<Update>) {
    let w = app.weak();
    glib::spawn_future_local(async move {
        while let Ok(u) = rx.recv().await {
            let Some(a) = w.upgrade() else { return };
            let changed = {
                let mut i = a.inner.borrow_mut();
                let Some(p) = i.panes.get_mut(&u.pane) else { continue };
                let mut changed = false;
                if u.cwd.is_some() && p.cwd != u.cwd {
                    p.cwd = u.cwd;
                    changed = true;
                }
                if p.git != u.git {
                    p.git = u.git;
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
                        p.compose.stack = None;
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
                a.refresh();
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
        a.request_meta(!tick.is_multiple_of(5));
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

    #[test]
    fn compose_files_added_later_are_found_without_a_rescan() {
        let dir = tmp("added-later");
        let mut c = ComposeCache {
            ttl: Duration::ZERO,
            ..Default::default()
        };
        let first = c.scan(1, &dir, "/bin/true compose", false).expect("first scan is sent");
        assert!(first.files.is_empty());
        std::fs::write(dir.join("docker-compose.yml"), "services:\n  api: {}\n").unwrap();
        let second = c.scan(1, &dir, "/bin/true compose", false).expect("the new file is reported");
        assert_eq!(second.files.len(), 1);
    }

    #[test]
    fn the_scan_cache_is_reused_within_its_lifetime() {
        let dir = tmp("cached");
        let mut c = ComposeCache::default();
        assert_eq!(c.ttl, Duration::from_secs(30));
        c.scan(1, &dir, "/bin/true compose", false).expect("first scan is sent");
        std::fs::write(dir.join("docker-compose.yml"), "services:\n  api: {}\n").unwrap();
        assert!(c.scan(1, &dir, "/bin/true compose", false).is_none(), "cached, unchanged");
        assert!(c.scan(1, &dir, "/bin/true compose", true).is_some(), "rescan bypasses the cache");
    }
}
