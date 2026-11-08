//! Background metadata refresh (cwd, git, ports, foreground agent).
//!
//! Polling is cheap procfs reads every 3 s for the visible workspace and every
//! 15 s for the rest; `git status` runs only when a pane's directory changes
//! or its cached result is older than 5 s. Everything runs off the GTK thread.

use super::AppRc;
use crate::meta::{self, GitInfo};
use crate::model::PaneId;
use gtk4::glib;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct Job {
    pub pane: PaneId,
    pub shell_pid: i32,
    pub pty_fd: i32,
    pub ports: bool,
    /// File the shell writes its active Python/conda environment to.
    pub env_file: PathBuf,
}

pub struct Update {
    pane: PaneId,
    cwd: Option<PathBuf>,
    git: Option<GitInfo>,
    ports: Option<Vec<u16>>,
    fg_agent: Option<&'static str>,
    python_env: Option<String>,
}

const GIT_TTL: Duration = Duration::from_secs(5);

pub fn spawn() -> (std::sync::mpsc::Sender<Job>, async_channel::Receiver<Update>) {
    let (tx, rx) = std::sync::mpsc::channel::<Job>();
    let (utx, urx) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("oxishell-meta".into())
        .spawn(move || {
            let mut git_cache: HashMap<PathBuf, (Instant, Option<GitInfo>)> = HashMap::new();
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
                            python_env: crate::shellint::read_env(&job.env_file),
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
                    p.fg_agent = u.fg_agent;
                    changed = true;
                }
                if p.python_env != u.python_env {
                    p.python_env = u.python_env;
                    changed = true;
                }
                changed
            };
            if changed {
                a.refresh();
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
        glib::ControlFlow::Continue
    });
}
