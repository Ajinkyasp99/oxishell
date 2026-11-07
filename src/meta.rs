//! Developer metadata from Linux procfs and the git CLI.
//!
//! All functions are blocking and are called from a worker thread, never the
//! GTK main loop.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn cwd_of(pid: i32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// Foreground process group of the terminal behind `fd` (a PTY master).
pub fn foreground_pgrp(fd: std::os::fd::RawFd) -> Option<i32> {
    let pg = unsafe { libc::tcgetpgrp(fd) };
    (pg > 0).then_some(pg)
}

pub fn comm(pid: i32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
}

pub fn cmdline(pid: i32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|b| {
            b.split(|c| *c == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Known terminal coding agents: (agent id, executable basenames).
pub const AGENTS: &[(&str, &[&str])] = &[
    ("claude", &["claude"]),
    ("codex", &["codex"]),
    ("opencode", &["opencode"]),
    ("gemini", &["gemini"]),
    ("qoder", &["qodercli"]),
    ("cursor", &["cursor-agent"]),
    ("amp", &["amp"]),
    ("aider", &["aider"]),
];

/// Identify an agent from a process's name and first argv entries. This only
/// says which program is in the foreground; it does not infer agent state.
pub fn agent_for_process(comm: &str, argv: &[String]) -> Option<&'static str> {
    let base = |s: &str| {
        Path::new(s)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let mut names = vec![comm.to_string()];
    names.extend(argv.iter().take(2).map(|a| base(a)));
    AGENTS
        .iter()
        .find(|(_, bins)| names.iter().any(|n| bins.contains(&n.as_str())))
        .map(|(id, _)| *id)
}

/// Map pid -> parent pid for every process.
pub fn ppid_table() -> HashMap<i32, i32> {
    let mut map = HashMap::new();
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for e in rd.flatten() {
            let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else {
                continue;
            };
            if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                // comm may contain spaces/parens; fields after the last ')' are fixed.
                if let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) {
                    if let Some(ppid) = rest.split_whitespace().nth(1).and_then(|s| s.parse().ok()) {
                        map.insert(pid, ppid);
                    }
                }
            }
        }
    }
    map
}

/// `root` and all its descendants.
pub fn process_tree(root: i32) -> Vec<i32> {
    process_tree_in(&ppid_table(), root)
}

/// Like [`process_tree`] but reuses a table from [`ppid_table`] (one /proc scan per batch).
pub fn process_tree_in(table: &HashMap<i32, i32>, root: i32) -> Vec<i32> {
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    for (p, pp) in table {
        children.entry(*pp).or_default().push(*p);
    }
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        if let Some(c) = children.get(&out[i]) {
            out.extend(c);
        }
        i += 1;
    }
    out
}

fn listening_inodes() -> HashMap<u64, u16> {
    let mut map = HashMap::new();
    for f in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(text) = std::fs::read_to_string(f) {
            map.extend(parse_proc_net_tcp(&text));
        }
    }
    map
}

/// Parse /proc/net/tcp{,6}: returns inode -> local port for LISTEN sockets.
pub fn parse_proc_net_tcp(text: &str) -> HashMap<u64, u16> {
    let mut map = HashMap::new();
    for line in text.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 10 || cols[3] != "0A" {
            continue;
        }
        let port = cols[1].rsplit(':').next().and_then(|p| u16::from_str_radix(p, 16).ok());
        if let (Some(port), Ok(inode)) = (port, cols[9].parse::<u64>()) {
            map.insert(inode, port);
        }
    }
    map
}

/// TCP ports in LISTEN state owned by any of `pids` (only our own processes are readable).
pub fn listening_ports(pids: &[i32]) -> Vec<u16> {
    let inodes = listening_inodes();
    if inodes.is_empty() {
        return vec![];
    }
    let mut ports = HashSet::new();
    for pid in pids {
        let Ok(rd) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        for e in rd.flatten() {
            if let Ok(link) = std::fs::read_link(e.path()) {
                let l = link.to_string_lossy();
                if let Some(ino) = l
                    .strip_prefix("socket:[")
                    .and_then(|s| s.strip_suffix(']'))
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    if let Some(p) = inodes.get(&ino) {
                        ports.insert(*p);
                    }
                }
            }
        }
    }
    let mut v: Vec<u16> = ports.into_iter().collect();
    v.sort_unstable();
    v
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GitInfo {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    /// Number of changed (staged, unstaged or untracked) entries.
    pub changes: u32,
}

/// Parse `git status --porcelain=v2 --branch` output.
pub fn parse_git_status(text: &str) -> GitInfo {
    let mut g = GitInfo::default();
    let mut oid = String::new();
    for line in text.lines() {
        if let Some(h) = line.strip_prefix("# branch.head ") {
            g.branch = h.to_string();
        } else if let Some(o) = line.strip_prefix("# branch.oid ") {
            oid = o.chars().take(7).collect();
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            for part in ab.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    g.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    g.behind = n.parse().unwrap_or(0);
                }
            }
        } else if !line.starts_with('#') && !line.is_empty() {
            g.changes += 1;
        }
    }
    if g.branch == "(detached)" && !oid.is_empty() {
        g.branch = format!("({oid})");
    }
    g
}

/// Git status for `dir`, or None when it is not inside a work tree or git is missing.
pub fn git_info(dir: &Path) -> Option<GitInfo> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["status", "--porcelain=v2", "--branch", "--untracked-files=normal"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_git_status(&String::from_utf8_lossy(&out.stdout)))
}

/// Shorten a path under $HOME to `~/...` for display.
pub fn display_path(p: &Path) -> String {
    let home = crate::paths::home();
    match p.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_git_porcelain() {
        let t = "# branch.oid abcdef1234\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n1 .M N... 100644 100644 100644 a b src/x.rs\n? new.txt\n";
        assert_eq!(
            parse_git_status(t),
            GitInfo {
                branch: "main".into(),
                ahead: 2,
                behind: 1,
                changes: 2
            }
        );
        let d = parse_git_status("# branch.oid abcdef1234\n# branch.head (detached)\n");
        assert_eq!(d.branch, "(abcdef1)");
    }

    #[test]
    fn git_info_on_real_repo_and_non_repo() {
        let dir = std::env::temp_dir().join(format!("cmux-git-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(git_info(&dir), None);
        let ok = Command::new("git")
            .args(["init", "-q", "-b", "trunk"])
            .current_dir(&dir)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        std::fs::write(dir.join("f"), "x").unwrap();
        let g = git_info(&dir).unwrap();
        assert_eq!(g.branch, "trunk");
        assert_eq!(g.changes, 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parses_proc_net_tcp_listen_only() {
        let t = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 12345 1\n   1: 0100007F:0016 0100007F:9999 01 00000000:00000000 00:00000000 00000000  1000        0 999 1\n";
        let m = parse_proc_net_tcp(t);
        assert_eq!(m.get(&12345), Some(&8080));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn detects_listening_port_of_own_process() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let me = std::process::id() as i32;
        assert!(listening_ports(&[me]).contains(&port));
    }

    #[test]
    fn process_tree_includes_children() {
        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        let me = std::process::id() as i32;
        assert!(process_tree(me).contains(&(child.id() as i32)));
        assert_eq!(comm(child.id() as i32).as_deref(), Some("sleep"));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn agent_identification() {
        assert_eq!(agent_for_process("claude", &[]), Some("claude"));
        assert_eq!(
            agent_for_process("node", &["node".into(), "/usr/lib/node_modules/.bin/codex".into()]),
            Some("codex")
        );
        assert_eq!(agent_for_process("bash", &["bash".into()]), None);
    }

    #[test]
    fn cwd_of_self() {
        assert_eq!(cwd_of(std::process::id() as i32), std::env::current_dir().ok());
    }
}
