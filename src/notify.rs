//! Notification state: per-pane unread items, history, and agent status.

use crate::model::{PaneId, WorkspaceId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub id: u64,
    pub pane: Option<PaneId>,
    pub workspace: Option<WorkspaceId>,
    pub title: String,
    pub body: String,
    /// Origin, e.g. "cli", "bell", "agent:claude".
    pub source: String,
    /// Unix seconds.
    pub time: u64,
    pub read: bool,
}

#[derive(Debug, Default)]
pub struct NotificationCenter {
    items: Vec<Notification>,
    next: u64,
}

pub const MAX_HISTORY: usize = 200;
pub const MAX_TITLE: usize = 200;
pub const MAX_BODY: usize = 2000;

/// Strip control characters and cap length; notification text comes from
/// untrusted terminal programs.
pub fn sanitize(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl NotificationCenter {
    pub fn push(&mut self, pane: Option<PaneId>, workspace: Option<WorkspaceId>, title: &str, body: &str, source: &str) -> &Notification {
        self.next += 1;
        self.items.push(Notification {
            id: self.next,
            pane,
            workspace,
            title: sanitize(title, MAX_TITLE),
            body: sanitize(body, MAX_BODY),
            source: sanitize(source, 64),
            time: now(),
            read: false,
        });
        if self.items.len() > MAX_HISTORY {
            let drop = self.items.len() - MAX_HISTORY;
            self.items.drain(..drop);
        }
        self.items.last().unwrap()
    }

    pub fn all(&self) -> &[Notification] {
        &self.items
    }

    pub fn unread_for_pane(&self, pane: PaneId) -> usize {
        self.items.iter().filter(|n| !n.read && n.pane == Some(pane)).count()
    }

    pub fn unread_for_workspace(&self, ws: WorkspaceId) -> usize {
        self.items.iter().filter(|n| !n.read && n.workspace == Some(ws)).count()
    }

    pub fn latest_for_workspace(&self, ws: WorkspaceId) -> Option<&Notification> {
        self.items.iter().rev().find(|n| !n.read && n.workspace == Some(ws))
    }

    pub fn unread_total(&self) -> usize {
        self.items.iter().filter(|n| !n.read).count()
    }

    pub fn mark_pane_read(&mut self, pane: PaneId) -> bool {
        let mut changed = false;
        for n in self.items.iter_mut().filter(|n| n.pane == Some(pane) && !n.read) {
            n.read = true;
            changed = true;
        }
        changed
    }

    pub fn mark_all_read(&mut self) {
        self.items.iter_mut().for_each(|n| n.read = true);
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Most recent unread notification (target of jump-to-unread).
    pub fn latest_unread(&self) -> Option<&Notification> {
        self.items.iter().rev().find(|n| !n.read)
    }

    /// Drop pane association when a pane closes; keeps the history entry.
    pub fn forget_pane(&mut self, pane: PaneId) {
        for n in self.items.iter_mut().filter(|n| n.pane == Some(pane)) {
            n.pane = None;
            n.read = true;
        }
    }

    pub fn forget_workspace(&mut self, ws: WorkspaceId) {
        for n in self.items.iter_mut().filter(|n| n.workspace == Some(ws)) {
            n.workspace = None;
            n.pane = None;
            n.read = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unread_tracking_and_jump() {
        let mut c = NotificationCenter::default();
        c.push(Some(2), Some(1), "Build", "done", "cli");
        c.push(Some(3), Some(1), "Claude", "needs input", "agent:claude");
        assert_eq!(c.unread_for_workspace(1), 2);
        assert_eq!(c.unread_for_pane(3), 1);
        assert_eq!(c.latest_unread().unwrap().pane, Some(3));
        assert!(c.mark_pane_read(3));
        assert!(!c.mark_pane_read(3));
        assert_eq!(c.latest_unread().unwrap().pane, Some(2));
        c.forget_pane(2);
        assert_eq!(c.unread_total(), 0);
        assert_eq!(c.all().len(), 2);
    }

    #[test]
    fn sanitizes_and_caps() {
        let mut c = NotificationCenter::default();
        let n = c.push(None, None, "a\u{1b}]0;evil\u{7}b", &"x".repeat(5000), "cli");
        assert_eq!(n.title, "a]0;evilb");
        assert_eq!(n.body.len(), MAX_BODY);
        for _ in 0..(MAX_HISTORY + 10) {
            c.push(None, None, "t", "", "cli");
        }
        assert_eq!(c.all().len(), MAX_HISTORY);
    }
}
