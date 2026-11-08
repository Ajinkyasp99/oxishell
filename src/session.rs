//! Session persistence: workspace names, split layouts, pane working
//! directories, display-only scrollback, and agent session ids.
//!
//! Restoration never executes commands. Saved scrollback is sanitized to
//! plain text before it is shown, so replayed escape sequences cannot
//! reprogram the terminal. Agent resume commands are only offered, never run.

use crate::model::{AppState, PaneId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PaneSnapshot {
    pub cwd: Option<String>,
    /// Agent id and session id last reported by hooks, for explicit resume.
    pub agent: Option<String>,
    pub agent_session: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub state: AppState,
    #[serde(default)]
    pub panes: BTreeMap<PaneId, PaneSnapshot>,
}

pub fn session_path() -> PathBuf {
    crate::paths::state_dir().join("session.json")
}

pub fn scrollback_dir() -> PathBuf {
    crate::paths::state_dir().join("scrollback")
}

impl Snapshot {
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let data = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        crate::paths::write_atomic(path, &data, 0o600)
    }

    /// Load and validate. A corrupt file is moved aside (`.corrupt`) so the
    /// app starts fresh instead of failing repeatedly.
    pub fn load(path: &Path) -> Result<Option<Snapshot>, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        let parsed: Result<Snapshot, String> = serde_json::from_str(&text).map_err(|e| e.to_string()).and_then(|mut s: Snapshot| {
            if s.version != VERSION {
                return Err(format!("unsupported session version {}", s.version));
            }
            s.state.validate()?;
            s.state.normalize_ids();
            let live: std::collections::HashSet<_> = s.state.all_panes().into_iter().map(|(_, p)| p).collect();
            s.panes.retain(|p, _| live.contains(p));
            Ok(s)
        });
        match parsed {
            Ok(s) => Ok(Some(s)),
            Err(e) => {
                let _ = std::fs::rename(path, path.with_extension("json.corrupt"));
                Err(format!("session file was invalid and has been set aside: {e}"))
            }
        }
    }
}

/// Turn terminal text into inert plain text: drop escape sequences and
/// control characters (keep newlines and tabs), cap to the last `max_lines`.
pub fn sanitize_scrollback(text: &str, max_lines: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                // Skip CSI/OSC/other escape sequences.
                match chars.next() {
                    Some('[') => {
                        for n in chars.by_ref() {
                            if ('@'..='~').contains(&n) {
                                break;
                            }
                        }
                    }
                    Some(']') | Some('P') | Some('_') | Some('^') => {
                        while let Some(n) = chars.next() {
                            if n == '\u{7}' {
                                break;
                            }
                            if n == '\u{1b}' && chars.peek() == Some(&'\\') {
                                chars.next();
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    let trimmed = out.trim_end();
    let lines: Vec<&str> = trimmed.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

pub fn save_scrollback(pane: PaneId, text: &str, max_lines: usize) -> std::io::Result<()> {
    let dir = scrollback_dir();
    crate::paths::ensure_private_dir(&dir)?;
    crate::paths::write_atomic(
        &dir.join(format!("{pane}.txt")),
        sanitize_scrollback(text, max_lines).as_bytes(),
        0o600,
    )
}

pub fn load_scrollback(pane: PaneId, max_lines: usize) -> Option<String> {
    let text = std::fs::read_to_string(scrollback_dir().join(format!("{pane}.txt"))).ok()?;
    // Sanitize again on load: the file may have been edited.
    let t = sanitize_scrollback(&text, max_lines);
    (!t.is_empty()).then_some(t)
}

/// Remove scrollback files for panes that no longer exist.
pub fn prune_scrollback(live: &[PaneId]) {
    let Ok(rd) = std::fs::read_dir(scrollback_dir()) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let id = name.strip_suffix(".txt").and_then(|s| s.parse::<PaneId>().ok());
        if id.is_none_or(|id| !live.contains(&id)) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Orient;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oxishell-sess-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn roundtrip_layout_and_cwds() {
        let mut st = AppState::default();
        let (_, p1) = st.create_workspace(Some("Backend")).unwrap();
        let p2 = st.split(p1, Orient::Vertical).unwrap();
        let mut panes = BTreeMap::new();
        panes.insert(
            p2,
            PaneSnapshot {
                cwd: Some("/tmp".into()),
                agent: Some("claude".into()),
                agent_session: Some("s1".into()),
            },
        );
        panes.insert(999, PaneSnapshot::default()); // stale entry, dropped on load
        let snap = Snapshot {
            version: VERSION,
            state: st,
            panes,
        };
        let dir = tmp("rt");
        let path = dir.join("session.json");
        snap.save(&path).unwrap();
        let back = Snapshot::load(&path).unwrap().unwrap();
        assert_eq!(back.state.workspaces[0].layout, snap.state.workspaces[0].layout);
        assert_eq!(back.panes.len(), 1);
        assert_eq!(back.panes[&p2].cwd.as_deref(), Some("/tmp"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupt_and_truncated_files_are_set_aside() {
        let dir = tmp("corrupt");
        let path = dir.join("session.json");
        assert!(Snapshot::load(&path).unwrap().is_none());
        std::fs::write(&path, "{\"version\":1,\"state\":{\"workspa").unwrap(); // interrupted write
        assert!(Snapshot::load(&path).is_err());
        assert!(!path.exists());
        assert!(dir.join("session.json.corrupt").exists());
        // Structurally invalid (focused pane missing).
        std::fs::write(&path, r#"{"version":1,"state":{"workspaces":[{"id":1,"name":"a","layout":{"type":"leaf","pane":2},"focused":3}],"active":1,"next_id":3}}"#).unwrap();
        assert!(Snapshot::load(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn scrollback_sanitizer_strips_escapes() {
        let evil = "ok\u{1b}[31mred\u{1b}[0m\n\u{1b}]52;c;ZXZpbA==\u{7}clip\n\u{1b}]0;title\u{1b}\\x\r\nbell\u{7}";
        assert_eq!(sanitize_scrollback(evil, 100), "okred\nclip\nx\nbell");
        assert_eq!(sanitize_scrollback("1\n2\n3\n", 2), "2\n3");
    }
}
