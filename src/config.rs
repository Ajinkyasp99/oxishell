//! User configuration (`~/.config/oxishell/config.toml`).
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
    pub editor: Editor,
    pub updates: Updates,
    pub compose: Compose,
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
    /// Show Warp-style context chips (environment, directory, git) above terminals.
    pub chips: bool,
    /// Faint workspace name centred behind the terminals.
    pub watermark: bool,
    /// Hide the header buttons (not the window controls) until the pointer
    /// reaches the header.
    pub autohide_buttons: bool,
    /// Window background opacity in percent (50–100). Text stays opaque.
    pub opacity: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Terminal {
    /// Shell program; empty means $SHELL, then the passwd entry, then /bin/sh.
    pub shell: String,
    pub scrollback_lines: i64,
    pub confirm_close_running: bool,
    /// Inline command suggestions from shell history (ghost text, accept with →).
    pub suggestions: bool,
    /// Replace the shell prompt with a compact `❯` inside oxishell panes
    /// (green, red after a failed command). Applies to new terminals.
    pub compact_prompt: bool,
    /// "block" (solid bar), "ibeam" (vertical line) or "underline" (horizontal line).
    pub cursor_shape: String,
    pub cursor_blink: bool,
    /// Turn pastel/near-white ANSI colours of the theme into vivid ones so
    /// coloured program output (git, ls, grep, compilers) stands out.
    pub vivid_colors: bool,
    /// Colour the command you type (and keep program output in the normal
    /// colour) with the compact prompt. Applies to new terminals.
    pub highlight_commands: bool,
    /// Pressing ↑ at the shell prompt opens a history panel (Warp style),
    /// filtered by what is already typed.
    pub history_panel: bool,
    /// Warp-style command blocks: a separator and a header (folder, branch,
    /// exit status, duration) above every command. Applies to new terminals.
    pub command_blocks: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ipc {
    pub enabled: bool,
    /// Allow `oxi send` to type text into panes. Off by default.
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
    /// Replay this many lines of each pane's previous output after a restart
    /// (display-only). 0 (default) = terminals start clean at the top and no
    /// terminal output is written to disk.
    pub replay_lines: usize,
    /// Removed setting (replaced by `replay_lines`, now off by default).
    /// Accepted and ignored so existing config files keep loading.
    #[serde(default, rename = "scrollback_lines", skip_serializing)]
    pub legacy_scrollback_lines: Option<toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Notifications {
    pub desktop: bool,
    /// Treat the terminal bell as an attention request.
    pub bell_attention: bool,
    /// Desktop notifications play a sound (from the system sound theme).
    pub sound: bool,
    /// Notify when a shell command that ran for a while finishes in a pane
    /// you are not looking at.
    pub command_finished: bool,
    /// How long (seconds) a command must run before its end is notified.
    pub command_min_seconds: u64,
    /// Notify once when an agent's 5-hour or weekly limit passes 80% and 95%.
    pub usage_alerts: bool,
}

/// Optional check for a newer release (the only network request oxishell can make).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Updates {
    /// Ask GitHub at most once a day whether a newer version is out. Off by default.
    pub check: bool,
}

/// Editor used for `path:line` links in terminal output.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Editor {
    /// Program and arguments, e.g. "code" or "nvim". Empty = `$VISUAL`,
    /// `$EDITOR`, then the first installed graphical editor.
    pub command: String,
}

/// Docker Compose launcher: a pane button that lists the repo's compose files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Compose {
    /// Show the button, the stack bar and the shortcut. Off skips the scan too.
    pub enabled: bool,
    /// Press Enter after typing the command (off: the user presses it).
    pub run_immediately: bool,
    /// Desktop notification when a service exits non-zero or turns unhealthy.
    pub notify_on_crash: bool,
    /// What choosing a file runs: "up", "up -d", "down" or "logs -f".
    pub default_action: String,
    /// Compose program and arguments, e.g. "docker compose". Empty = auto-detect.
    pub command: String,
}

/// Allowed values of `compose.default_action`.
pub const COMPOSE_ACTIONS: &[&str] = &["up", "up -d", "down", "logs -f"];

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
            font: "Hack 11".into(),
            sidebar_width: 230,
            sidebar_visible: true,
            chips: true,
            watermark: true,
            autohide_buttons: true,
            opacity: 100,
        }
    }
}
impl Default for Terminal {
    fn default() -> Self {
        Self {
            shell: String::new(),
            scrollback_lines: 10_000,
            confirm_close_running: true,
            suggestions: true,
            compact_prompt: true,
            cursor_shape: "block".into(),
            cursor_blink: true,
            vivid_colors: true,
            highlight_commands: true,
            history_panel: true,
            command_blocks: true,
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
            replay_lines: 0,
            legacy_scrollback_lines: None,
        }
    }
}
impl Default for Compose {
    fn default() -> Self {
        Self {
            enabled: true,
            run_immediately: true,
            notify_on_crash: true,
            default_action: "up".into(),
            command: String::new(),
        }
    }
}
impl Default for Notifications {
    fn default() -> Self {
        Self {
            desktop: true,
            bell_attention: true,
            sound: true,
            command_finished: true,
            command_min_seconds: 10,
            usage_alerts: true,
        }
    }
}

/// Every bindable action with its default accelerator (Linux: Ctrl-based).
#[cfg(not(target_os = "macos"))]
pub const DEFAULT_SHORTCUTS: &[(&str, &str, &str)] = &[
    ("new-workspace", "<Ctrl><Shift>n", "New workspace"),
    ("close-workspace", "<Ctrl><Shift>q", "Close workspace"),
    ("rename-workspace", "<Ctrl><Shift>r", "Rename workspace"),
    ("next-workspace", "<Ctrl>Page_Down", "Next workspace"),
    ("prev-workspace", "<Ctrl>Page_Up", "Previous workspace"),
    ("split-right", "<Ctrl><Shift>e", "Split right"),
    ("split-down", "<Ctrl><Shift>o", "Split down"),
    ("close-pane", "<Ctrl><Shift>w", "Close tab"),
    ("focus-left", "<Alt>Left", "Focus pane left"),
    ("focus-right", "<Alt>Right", "Focus pane right"),
    ("focus-up", "<Alt>Up", "Focus pane up"),
    ("focus-down", "<Alt>Down", "Focus pane down"),
    ("newline", "<Shift>Return", "New line in prompt"),
    ("newline-alt", "<Ctrl>Return", "New line in prompt (alternative)"),
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
    ("command-history", "<Ctrl><Shift>h", "Search command history"),
    ("settings", "<Ctrl>comma", "Settings"),
    ("compose-files", "<Ctrl><Shift>l", "Docker Compose files"),
    ("new-tab", "<Ctrl><Shift>t", "New tab"),
    ("next-tab", "<Ctrl>Tab", "Next tab"),
    ("prev-tab", "<Ctrl><Shift>Tab", "Previous tab"),
];

/// Every bindable action with its default accelerator (macOS: Command-based,
/// `<Meta>` in GTK, so Ctrl keys stay with the shell).
#[cfg(target_os = "macos")]
pub const DEFAULT_SHORTCUTS: &[(&str, &str, &str)] = &[
    ("new-workspace", "<Meta>t", "New workspace"),
    ("close-workspace", "<Meta><Shift>w", "Close workspace"),
    ("rename-workspace", "<Meta><Shift>r", "Rename workspace"),
    ("next-workspace", "<Meta><Shift>braceright", "Next workspace"),
    ("prev-workspace", "<Meta><Shift>braceleft", "Previous workspace"),
    ("split-right", "<Meta>d", "Split right"),
    ("split-down", "<Meta><Shift>d", "Split down"),
    ("close-pane", "<Meta>w", "Close tab"),
    ("focus-left", "<Meta><Alt>Left", "Focus pane left"),
    ("focus-right", "<Meta><Alt>Right", "Focus pane right"),
    ("focus-up", "<Meta><Alt>Up", "Focus pane up"),
    ("focus-down", "<Meta><Alt>Down", "Focus pane down"),
    ("newline", "<Shift>Return", "New line in prompt"),
    ("newline-alt", "<Ctrl>Return", "New line in prompt (alternative)"),
    ("copy", "<Meta>c", "Copy"),
    ("paste", "<Meta>v", "Paste"),
    ("search", "<Meta>f", "Search scrollback"),
    ("command-palette", "<Meta><Shift>p", "Command palette"),
    ("toggle-sidebar", "<Meta><Shift>b", "Toggle sidebar"),
    ("jump-unread", "<Meta><Shift>u", "Jump to latest unread"),
    ("notifications", "<Meta><Shift>i", "Show notifications"),
    ("zoom-in", "<Meta>equal", "Increase font size"),
    ("zoom-out", "<Meta>minus", "Decrease font size"),
    ("zoom-reset", "<Meta>0", "Reset font size"),
    ("command-history", "<Meta><Shift>h", "Search command history"),
    ("settings", "<Meta>comma", "Settings"),
    ("compose-files", "<Meta><Shift>l", "Docker Compose files"),
    ("new-tab", "<Meta><Shift>t", "New tab"),
    ("next-tab", "<Ctrl>Tab", "Next tab"),
    ("prev-tab", "<Ctrl><Shift>Tab", "Previous tab"),
];

/// The primary modifier as shown in help texts: Ctrl, or ⌘ on macOS.
pub const PRIMARY_KEY: &str = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl" };

/// The default accelerator of `action` (empty if there is none).
pub fn default_shortcut(action: &str) -> &'static str {
    DEFAULT_SHORTCUTS.iter().find(|(a, _, _)| *a == action).map_or("", |(_, k, _)| k)
}

/// `text (shortcut)` with the default shortcut of `action`, for tooltips.
pub fn with_shortcut(text: &str, action: &str) -> String {
    format!("{text} ({})", accel_label(default_shortcut(action)))
}

/// A GTK accelerator such as `<Ctrl><Shift>b` as people write it: `Ctrl+Shift+B`
/// on Linux, `⇧⌘B` on macOS (Control, Option, Shift, Command order).
pub fn accel_label(accel: &str) -> String {
    accel_label_for(accel, cfg!(target_os = "macos"))
}

fn accel_label_for(accel: &str, mac: bool) -> String {
    let mut mods = Vec::new();
    let mut rest = accel;
    while let Some(after) = rest.strip_prefix('<') {
        let Some((m, tail)) = after.split_once('>') else { break };
        mods.push(m.to_ascii_lowercase());
        rest = tail;
    }
    let key = match rest {
        "equal" => "=".to_string(),
        "minus" => "-".into(),
        "plus" => "+".into(),
        "comma" => ",".into(),
        "period" => ".".into(),
        "braceright" => "}".into(),
        "braceleft" => "{".into(),
        "bracketright" => "]".into(),
        "bracketleft" => "[".into(),
        "Return" if mac => "↩".into(),
        "Return" => "Enter".into(),
        "Left" if mac => "←".into(),
        "Right" if mac => "→".into(),
        "Up" if mac => "↑".into(),
        "Down" if mac => "↓".into(),
        k if k.chars().count() == 1 => k.to_uppercase(),
        k => k.replace('_', " "),
    };
    if mac {
        let order = [
            ("ctrl", "⌃"),
            ("control", "⌃"),
            ("alt", "⌥"),
            ("shift", "⇧"),
            ("meta", "⌘"),
            ("super", "⌘"),
        ];
        let mut out: String = order
            .iter()
            .filter(|(m, _)| mods.iter().any(|x| x == m))
            .map(|(_, sym)| *sym)
            .collect();
        out.push_str(&key);
        out
    } else {
        let name = |m: &str| match m {
            "ctrl" | "control" | "primary" => "Ctrl".to_string(),
            "shift" => "Shift".into(),
            "alt" => "Alt".into(),
            "super" => "Super".into(),
            "meta" => "Meta".into(),
            other => other.to_string(),
        };
        let mut parts: Vec<String> = mods.iter().map(|m| name(m)).collect();
        parts.push(key);
        parts.join("+")
    }
}

impl Config {
    pub fn path() -> PathBuf {
        crate::paths::config_dir().join("config.toml")
    }

    /// Parse TOML text and validate semantic constraints.
    pub fn parse(text: &str) -> Result<Config, String> {
        let mut cfg: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        // Move existing installations off the previous default typeface.
        if cfg.appearance.font == "Monospace 11" {
            cfg.appearance.font = Appearance::default().font;
        }
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !["block", "ibeam", "underline"].contains(&self.terminal.cursor_shape.as_str()) {
            return Err(format!(
                "terminal.cursor_shape must be block, ibeam or underline (got {:?})",
                self.terminal.cursor_shape
            ));
        }
        if !["system", "light", "dark"].contains(&self.appearance.mode.as_str()) {
            return Err(format!(
                "appearance.mode must be system, light or dark (got {:?})",
                self.appearance.mode
            ));
        }
        if !(120..=600).contains(&self.appearance.sidebar_width) {
            return Err("appearance.sidebar_width must be between 120 and 600".into());
        }
        if !(50..=100).contains(&self.appearance.opacity) {
            return Err("appearance.opacity must be between 50 and 100".into());
        }
        if !(0..=1_000_000).contains(&self.terminal.scrollback_lines) {
            return Err("terminal.scrollback_lines must be between 0 and 1000000".into());
        }
        if !COMPOSE_ACTIONS.contains(&self.compose.default_action.as_str()) {
            return Err(format!(
                "compose.default_action must be up, up -d, down or logs -f (got {:?})",
                self.compose.default_action
            ));
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
        login_shell()
    }
}

/// The user's own shell: `$SHELL`, else the passwd entry, else `/bin/sh`.
pub fn login_shell() -> String {
    if let Ok(s) = std::env::var("SHELL") {
        if !s.is_empty() {
            return s;
        }
    }
    passwd_shell().unwrap_or_else(|| "/bin/sh".into())
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
    fn accel_labels_read_naturally_on_each_platform() {
        assert_eq!(accel_label_for("<Ctrl><Shift>b", false), "Ctrl+Shift+B");
        assert_eq!(accel_label_for("<Ctrl>equal", false), "Ctrl+=");
        assert_eq!(accel_label_for("<Ctrl>Page_Down", false), "Ctrl+Page Down");
        assert_eq!(accel_label_for("<Alt>Left", false), "Alt+Left");
        assert_eq!(accel_label_for("<Meta><Shift>p", true), "⇧⌘P");
        assert_eq!(accel_label_for("<Shift><Meta><Alt>Left", true), "⌥⇧⌘←");
        assert_eq!(accel_label_for("<Meta>comma", true), "⌘,");
        assert_eq!(accel_label_for("<Ctrl>Return", true), "⌃↩");
    }

    #[test]
    fn every_action_has_a_unique_default_on_this_platform() {
        let mut seen = std::collections::HashSet::new();
        for (action, accel, _) in DEFAULT_SHORTCUTS {
            assert!(!accel.is_empty(), "{action}");
            assert!(seen.insert(accel.to_lowercase()), "{accel} bound twice");
        }
        assert_eq!(default_shortcut("command-palette"), DEFAULT_SHORTCUTS[17].1);
        assert!(with_shortcut("Split right", "split-right").starts_with("Split right ("));
    }

    #[test]
    fn defaults_are_valid_and_roundtrip() {
        let c = Config::default();
        c.validate().unwrap();
        let text = toml::to_string_pretty(&c).unwrap();
        assert_eq!(Config::parse(&text).unwrap(), c);
        assert!(c.shortcut_conflicts().is_empty());
    }

    #[test]
    fn editor_command_defaults_to_auto_detect_and_can_be_set() {
        assert_eq!(Config::default().editor.command, "");
        let c = Config::parse("[editor]\ncommand = \"code --new-window\"\n").unwrap();
        assert_eq!(c.editor.command, "code --new-window");
        assert!(Config::parse("[editor]\ncomand = \"zed\"").is_err());
    }

    #[test]
    fn usage_alerts_are_on_by_default_and_can_be_turned_off() {
        assert!(Config::default().notifications.usage_alerts);
        assert!(
            Config::parse("[notifications]\ndesktop = true\n")
                .unwrap()
                .notifications
                .usage_alerts
        );
        assert!(
            !Config::parse("[notifications]\nusage_alerts = false\n")
                .unwrap()
                .notifications
                .usage_alerts
        );
    }

    #[test]
    fn background_opacity_defaults_to_opaque_and_is_range_checked() {
        assert_eq!(Config::default().appearance.opacity, 100);
        assert_eq!(Config::parse("[appearance]\nopacity = 85").unwrap().appearance.opacity, 85);
        assert!(Config::parse("[appearance]\nopacity = 50").is_ok());
        assert!(Config::parse("[appearance]\nopacity = 49").is_err());
        assert!(Config::parse("[appearance]\nopacity = 101").is_err());
    }

    #[test]
    fn update_checks_are_off_unless_turned_on() {
        assert!(!Config::default().updates.check);
        assert!(!Config::parse("[appearance]\nmode = \"dark\"\n").unwrap().updates.check);
        assert!(Config::parse("[updates]\ncheck = true\n").unwrap().updates.check);
        assert!(Config::parse("[updates]\ncheck_daily = true\n").is_err());
    }

    #[test]
    fn compose_launcher_is_on_by_default_and_can_be_turned_off() {
        let d = Config::default().compose;
        assert!(d.enabled && d.run_immediately && d.notify_on_crash);
        assert_eq!(d.default_action, "up");
        assert_eq!(d.command, "");
        assert!(Config::parse("[appearance]\nmode = \"dark\"\n").unwrap().compose.enabled);
        assert!(!Config::parse("[compose]\nenabled = false\n").unwrap().compose.enabled);
        assert!(Config::parse("[compose]\nenable = false\n").is_err());
    }

    #[test]
    fn compose_default_action_is_checked() {
        for ok in ["up", "up -d", "down", "logs -f"] {
            assert!(Config::parse(&format!("[compose]\ndefault_action = \"{ok}\"\n")).is_ok(), "{ok}");
        }
        assert!(Config::parse("[compose]\ndefault_action = \"explode\"\n").is_err());
    }

    #[test]
    fn tab_shortcuts_are_bound_without_conflicts() {
        let c = Config::default();
        for action in ["new-tab", "next-tab", "prev-tab"] {
            let accel = default_shortcut(action);
            assert!(!accel.is_empty(), "{action} has a default shortcut");
            assert!(c.effective_shortcuts().contains(&(action.to_string(), accel.to_string())));
        }
        #[cfg(not(target_os = "macos"))]
        assert_eq!(default_shortcut("new-tab"), "<Ctrl><Shift>t");
        #[cfg(target_os = "macos")]
        assert_eq!(default_shortcut("new-tab"), "<Meta><Shift>t");
        assert_eq!(default_shortcut("next-tab"), "<Ctrl>Tab");
        assert_eq!(default_shortcut("prev-tab"), "<Ctrl><Shift>Tab");
        assert!(c.shortcut_conflicts().is_empty());
    }

    #[test]
    fn compose_shortcut_is_bound_without_conflicts() {
        let c = Config::default();
        let accel = default_shortcut("compose-files").to_string();
        assert!(!accel.is_empty());
        assert!(c.effective_shortcuts().contains(&("compose-files".into(), accel)));
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
        let c = Config::parse("[session]\nrestore = true\nbackend = \"none\"\nscrollback_lines = 500\n").unwrap();
        let out = toml::to_string(&c).unwrap();
        assert!(!out.contains("backend") && !out.contains("scrollback_lines = 500"));
        assert_eq!(c.session.replay_lines, 0, "old auto-written replay setting must not turn replay on");
    }

    #[test]
    fn invalid_values_rejected() {
        assert!(Config::parse("[appearance]\nmode = \"purple\"").is_err());
        assert!(Config::parse("[appearance]\nmodee = \"dark\"").is_err());
        assert!(Config::parse("[shortcuts]\nfly = \"<Ctrl>x\"").is_err());
        assert!(Config::parse("[session]\nrestored = true").is_err());
        assert!(Config::parse("[terminal]\ncursor_shape = \"triangle\"").is_err());
        assert_eq!(
            Config::parse("[terminal]\ncursor_shape = \"ibeam\"\ncursor_blink = false")
                .unwrap()
                .terminal
                .cursor_shape,
            "ibeam"
        );
        assert!(Config::parse("not toml [[[").is_err());
    }

    #[test]
    fn load_missing_and_corrupt_files() {
        let dir = std::env::temp_dir().join(format!("oxishell-cfg-test-{}", std::process::id()));
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
        // Same keys as split-right, written in another case.
        c.shortcuts
            .insert("split-down".into(), default_shortcut("split-right").to_uppercase());
        let conflicts = c.shortcut_conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].1, vec!["split-right".to_string(), "split-down".to_string()]);
    }

    #[test]
    fn hack_replaces_the_old_default_but_preserves_other_fonts() {
        assert_eq!(Config::default().appearance.font, "Hack 11");
        assert_eq!(
            Config::parse("[appearance]\nfont = 'Monospace 11'").unwrap().appearance.font,
            "Hack 11"
        );
        for font in ["JetBrains Mono 12", "Monospace 14"] {
            let text = format!("[appearance]\nfont = '{font}'");
            assert_eq!(Config::parse(&text).unwrap().appearance.font, font);
        }
    }
}
