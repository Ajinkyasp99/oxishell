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

/// What a notification is about. Decides how loudly the desktop announces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An agent is blocked until the user answers (permission, question).
    AgentInput,
    /// An agent finished its turn.
    AgentDone,
    /// A long shell command exited with status 0.
    CommandDone,
    /// A long shell command failed.
    CommandFailed,
    /// A program rang the terminal bell.
    Bell,
    /// Anything else (`cmux notify`, scripts).
    Message,
}

/// Urgency levels of the freedesktop notification spec.
pub const URGENCY_LOW: u8 = 0;
pub const URGENCY_NORMAL: u8 = 1;
pub const URGENCY_CRITICAL: u8 = 2;

impl Kind {
    pub fn from_source(source: &str) -> Kind {
        match source {
            "bell" => Kind::Bell,
            _ => Kind::Message,
        }
    }

    /// Event name from the freedesktop sound theme (`sound-name` hint).
    pub fn sound(self) -> &'static str {
        match self {
            Kind::AgentInput => "message-new-instant",
            Kind::AgentDone | Kind::CommandDone => "complete",
            Kind::CommandFailed => "dialog-warning",
            Kind::Bell => "bell",
            Kind::Message => "message",
        }
    }

    /// Waiting for input is critical: it stays on screen (and passes Do Not
    /// Disturb) until handled, because nothing progresses until the user acts.
    /// Nothing is low: GNOME shows no banner for low-urgency notifications.
    pub fn urgency(self) -> u8 {
        match self {
            Kind::AgentInput => URGENCY_CRITICAL,
            _ => URGENCY_NORMAL,
        }
    }

    pub fn category(self) -> &'static str {
        match self {
            Kind::AgentInput => "im.received",
            Kind::AgentDone | Kind::CommandDone => "transfer.complete",
            Kind::CommandFailed => "transfer.error",
            Kind::Bell => "x-cmux.bell",
            Kind::Message => "x-cmux.message",
        }
    }
}

/// The command typed at the prompt starts a terminal agent (`claude`,
/// `FOO=1 codex …`). Its exit is not worth a "command finished" notification:
/// the agent's own hooks already report its turns.
pub fn is_agent_command(cmd: &str) -> bool {
    let Some(prog) = cmd.split_whitespace().find(|w| !w.contains('=')) else {
        return false;
    };
    let base = prog.rsplit('/').next().unwrap_or(prog);
    crate::meta::AGENTS.iter().any(|(_, names)| names.contains(&base))
}

/// Title and body for a finished shell command; `took` is already formatted.
pub fn command_text(cmd: &str, status: i32, took: &str) -> (String, String) {
    const MAX_CMD: usize = 80;
    let cmd = cmd.trim();
    let shown = if cmd.chars().count() > MAX_CMD {
        format!("{}…", cmd.chars().take(MAX_CMD - 1).collect::<String>())
    } else {
        cmd.to_string()
    };
    let title = if status == 0 {
        "Command finished".to_string()
    } else {
        format!("Command failed (exit {status})")
    };
    (title, format!("{shown} · {took}"))
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

    pub fn is_unread(&self, id: u64) -> bool {
        self.items.iter().any(|n| n.id == id && !n.read)
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
    fn is_unread_follows_read_state_and_history() {
        let mut c = NotificationCenter::default();
        let a = c.push(Some(1), Some(1), "a", "", "cli").id;
        let b = c.push(Some(2), Some(1), "b", "", "cli").id;
        assert!(c.is_unread(a) && c.is_unread(b));
        c.mark_pane_read(1);
        assert!(!c.is_unread(a) && c.is_unread(b));
        c.clear();
        assert!(!c.is_unread(b));
        assert!(!c.is_unread(999));
    }

    #[test]
    fn kinds_map_to_sound_urgency_and_category() {
        assert_eq!(Kind::AgentInput.urgency(), URGENCY_CRITICAL);
        assert_eq!(Kind::AgentInput.sound(), "message-new-instant");
        assert_eq!(Kind::AgentDone.urgency(), URGENCY_NORMAL);
        assert_eq!(Kind::CommandFailed.sound(), "dialog-warning");
        assert_eq!(Kind::CommandFailed.category(), "transfer.error");
        assert_eq!(Kind::from_source("bell"), Kind::Bell);
        assert_eq!(Kind::from_source("cli"), Kind::Message);
    }

    #[test]
    fn agent_commands_are_recognised() {
        assert!(is_agent_command("claude"));
        assert!(is_agent_command("  ANTHROPIC_LOG=debug /usr/local/bin/claude --resume"));
        assert!(is_agent_command("codex exec 'fix it'"));
        assert!(!is_agent_command("cargo build --release"));
        assert!(!is_agent_command("echo claude"));
        assert!(!is_agent_command("FOO=1"));
        assert!(!is_agent_command(""));
    }

    #[test]
    fn command_text_reports_status_and_truncates() {
        assert_eq!(
            command_text("cargo build ", 0, "2m 05s"),
            ("Command finished".to_string(), "cargo build · 2m 05s".to_string())
        );
        let (t, b) = command_text(&"x".repeat(200), 101, "12s");
        assert_eq!(t, "Command failed (exit 101)");
        assert_eq!(b.chars().count(), 80 + " · 12s".chars().count());
        assert!(b.starts_with(&format!("{}…", "x".repeat(79))));
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
