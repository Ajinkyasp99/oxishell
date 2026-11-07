//! User configuration (`~/.config/cmux-linux/config.toml`).
//!
//! Unknown keys are rejected so typos are reported; an invalid file falls
//! back to defaults and the error is surfaced in the UI rather than crashing.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[derive(Default)]
pub struct Config {
    pub appearance: Appearance,
    pub terminal: Terminal,
    pub ipc: Ipc,
    pub session: Session,
    pub notifications: Notifications,
    /// Action name -> GTK accelerator string (e.g. "<Ctrl><Shift>e").
    pub shortcuts: BTreeMap<String, String>,
    /// Custom project commands shown in the command palette.
    pub commands: Vec<CustomCommand>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    /// "system", "light" or "dark".
    pub mode: String,
    /// Theme id (Warp theme file name without extension, or "default"); see [`crate::theme`].
    /// Unknown themes fall back to "default" with a warning.
    pub theme: String,
    /// Pango font description.
    pub font: String,
    pub sidebar_width: i32,
    pub sidebar_visible: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Terminal {
    /// Shell program; empty means $SHELL, then the passwd entry, then /bin/sh.
    pub shell: String,
    pub scrollback_lines: i64,
    pub confirm_close_running: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ipc {
    pub enabled: bool,
    /// Allow `cmux send` to type text into panes. Off by default.
    pub allow_send: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Session {
    pub restore: bool,
    /// Removed setting (older versions offered a tmux backend). Accepted and
    /// ignored so existing config files keep loading; never written back.
    #[serde(default, rename = "backend", skip_serializing)]
    pub legacy_backend: Option<toml::Value>,
    /// Lines of scrollback saved per pane for display-only restoration (0 = off).
    pub scrollback_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Notifications {
    pub desktop: bool,
    /// Treat the terminal bell as an attention request.
    pub bell_attention: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomCommand {
    pub name: String,
    pub command: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            mode: "system".into(),
            theme: "default".into(),
            font: "Monospace 11".into(),
            sidebar_width: 230,
            sidebar_visible: true,
        }
    }
}
impl Default for Terminal {
    fn default() -> Self {
        Self {
            shell: String::new(),
            scrollback_lines: 10_000,
            confirm_close_running: true,
        }
    }
}
impl Default for Ipc {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_send: false,
        }
    }
}
impl Default for Session {
    fn default() -> Self {
        Self {
            restore: true,
            legacy_backend: None,
            scrollback_lines: 500,
        }
    }
}
impl Default for Notifications {
    fn default() -> Self {
        Self {
            desktop: true,
            bell_attention: true,
        }
    }
}

/// Every bindable action with its default accelerator.
pub const DEFAULT_SHORTCUTS: &[(&str, &str, &str)] = &[
    ("new-workspace", "<Ctrl><Shift>n", "New workspace"),
    ("close-workspace", "<Ctrl><Shift>q", "Close workspace"),
    ("rename-workspace", "<Ctrl><Shift>r", "Rename workspace"),
    ("next-workspace", "<Ctrl>Page_Down", "Next workspace"),
    ("prev-workspace", "<Ctrl>Page_Up", "Previous workspace"),
    ("split-right", "<Ctrl><Shift>e", "Split right"),
    ("split-down", "<Ctrl><Shift>o", "Split down"),
    ("close-pane", "<Ctrl><Shift>w", "Close pane"),
    ("focus-left", "<Alt>Left", "Focus pane left"),
    ("focus-right", "<Alt>Right", "Focus pane right"),
    ("focus-up", "<Alt>Up", "Focus pane up"),
    ("focus-down", "<Alt>Down", "Focus pane down"),
    ("copy", "<Ctrl><Shift>c", "Copy"),
    ("paste", "<Ctrl><Shift>v", "Paste"),
    ("search", "<Ctrl><Shift>f", "Search scrollback"),
    ("command-palette", "<Ctrl><Shift>p", "Command palette"),
    ("toggle-sidebar", "<Ctrl><Shift>b", "Toggle sidebar"),
    ("jump-unread", "<Ctrl><Shift>u", "Jump to latest unread"),
    ("notifications", "<Ctrl><Shift>i", "Show notifications"),
    ("zoom-in", "<Ctrl>equal", "Increase font size"),
    ("zoom-out", "<Ctrl>minus", "Decrease font size"),
    ("zoom-reset", "<Ctrl>0", "Reset font size"),
    ("settings", "<Ctrl>comma", "Settings"),
];

impl Config {
    pub fn path() -> PathBuf {
        crate::paths::config_dir().join("config.toml")
    }

    /// Parse TOML text and validate semantic constraints.
    pub fn parse(text: &str) -> Result<Config, String> {
        let cfg: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !["system", "light", "dark"].contains(&self.appearance.mode.as_str()) {
            return Err(format!(
                "appearance.mode must be system, light or dark (got {:?})",
                self.appearance.mode
            ));
        }
        if !(120..=600).contains(&self.appearance.sidebar_width) {
            return Err("appearance.sidebar_width must be between 120 and 600".into());
        }
        if !(0..=1_000_000).contains(&self.terminal.scrollback_lines) {
            return Err("terminal.scrollback_lines must be between 0 and 1000000".into());
        }
        for k in self.shortcuts.keys() {
            if !DEFAULT_SHORTCUTS.iter().any(|(a, _, _)| a == k) {
                return Err(format!("unknown shortcut action {k:?}"));
            }
        }
        for c in &self.commands {
            if c.name.trim().is_empty() || c.command.trim().is_empty() {
                return Err("custom commands need a name and a command".into());
            }
        }
        Ok(())
    }

    /// Load from `path`. Missing file -> defaults. Invalid file -> defaults + error.
    pub fn load_from(path: &Path) -> (Config, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match Config::parse(&text) {
                Ok(c) => (c, None),
                Err(e) => (Config::default(), Some(format!("{}: {e}", path.display()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Config::default(), None),
            Err(e) => (Config::default(), Some(format!("{}: {e}", path.display()))),
        }
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        crate::paths::write_atomic(path, text.as_bytes(), 0o600)
    }

    /// Effective accelerator for every action (user overrides first).
    pub fn effective_shortcuts(&self) -> Vec<(String, String)> {
        DEFAULT_SHORTCUTS
            .iter()
            .map(|(a, d, _)| (a.to_string(), self.shortcuts.get(*a).cloned().unwrap_or_else(|| d.to_string())))
            .collect()
    }

    /// Accelerators bound to more than one action (empty strings ignored).
    pub fn shortcut_conflicts(&self) -> Vec<(String, Vec<String>)> {
        let mut by_accel: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (a, k) in self.effective_shortcuts() {
            if !k.is_empty() {
                by_accel.entry(k.to_lowercase()).or_default().push(a);
            }
        }
        by_accel.into_iter().filter(|(_, v)| v.len() > 1).collect()
    }

    pub fn shell(&self) -> String {
        if !self.terminal.shell.trim().is_empty() {
            return self.terminal.shell.clone();
        }
        if let Ok(s) = std::env::var("SHELL") {
            if !s.is_empty() {
                return s;
            }
        }
        passwd_shell().unwrap_or_else(|| "/bin/sh".into())
    }
}

fn passwd_shell() -> Option<String> {
    // SAFETY: getpwuid returns a pointer to static storage or null.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_shell.is_null() {
            return None;
        }
        Some(std::ffi::CStr::from_ptr((*pw).pw_shell).to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_roundtrip() {
        let c = Config::default();
        c.validate().unwrap();
        let text = toml::to_string_pretty(&c).unwrap();
        assert_eq!(Config::parse(&text).unwrap(), c);
        assert!(c.shortcut_conflicts().is_empty());
    }

    #[test]
    fn partial_file_uses_defaults() {
        let c = Config::parse("[appearance]\nmode = \"dark\"\n").unwrap();
        assert_eq!(c.appearance.mode, "dark");
        assert_eq!(c.terminal.scrollback_lines, 10_000);
    }

    #[test]
    fn legacy_backend_key_is_accepted_and_dropped() {
        let c = Config::parse("[session]\nrestore = true\nbackend = \"none\"\n").unwrap();
        assert!(!toml::to_string(&c).unwrap().contains("backend"));
    }

    #[test]
    fn invalid_values_rejected() {
        assert!(Config::parse("[appearance]\nmode = \"purple\"").is_err());
        assert!(Config::parse("[appearance]\nmodee = \"dark\"").is_err());
        assert!(Config::parse("[shortcuts]\nfly = \"<Ctrl>x\"").is_err());
        assert!(Config::parse("[session]\nrestored = true").is_err());
        assert!(Config::parse("not toml [[[").is_err());
    }

    #[test]
    fn load_missing_and_corrupt_files() {
        let dir = std::env::temp_dir().join(format!("cmux-cfg-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (c, err) = Config::load_from(&dir.join("missing.toml"));
        assert_eq!(c, Config::default());
        assert!(err.is_none());
        let bad = dir.join("bad.toml");
        std::fs::write(&bad, "[[[").unwrap();
        let (c, err) = Config::load_from(&bad);
        assert_eq!(c, Config::default());
        assert!(err.is_some());
        let good = dir.join("good.toml");
        let mut cfg = Config::default();
        cfg.shortcuts.insert("split-right".into(), "<Ctrl><Shift>d".into());
        cfg.commands.push(CustomCommand {
            name: "Test".into(),
            command: "cargo test".into(),
        });
        cfg.save_to(&good).unwrap();
        assert_eq!(Config::load_from(&good), (cfg, None));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detects_shortcut_conflicts() {
        let mut c = Config::default();
        c.shortcuts.insert("split-down".into(), "<Ctrl><Shift>E".into());
        let conflicts = c.shortcut_conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].1, vec!["split-right".to_string(), "split-down".to_string()]);
    }
}
