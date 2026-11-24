//! App-local symbolic icons that no system icon theme provides.
//!
//! Like the bundled fonts, they are written to the runtime dir. A hicolor
//! theme layout lets GTK recognize the `-symbolic` suffix and recolor them
//! with the widget's CSS colour instead of loading fixed-color images.

use gtk4::gdk;
use std::sync::OnceLock;

/// Pane drag handle: four-way move arrows.
const PANE_MOVE: &str = "oxishell-pane-move-symbolic";
/// Theme icon used when the bundled icons could not be installed.
const PANE_MOVE_FALLBACK: &str = "list-drag-handle-symbolic";
/// Docker Compose launcher button: stacked containers.
const COMPOSE: &str = "oxishell-compose-symbolic";
const COMPOSE_FALLBACK: &str = "system-run-symbolic";

/// Sidebar logos of AI agents (one colour, recoloured by CSS).
const AGENT_ICONS: [(&str, &str, &[u8]); 4] = [
    (
        "claude",
        "oxishell-agent-claude-symbolic",
        include_bytes!("../../data/icons/oxishell-agent-claude-symbolic.svg"),
    ),
    (
        "codex",
        "oxishell-agent-codex-symbolic",
        include_bytes!("../../data/icons/oxishell-agent-codex-symbolic.svg"),
    ),
    (
        "gemini",
        "oxishell-agent-gemini-symbolic",
        include_bytes!("../../data/icons/oxishell-agent-gemini-symbolic.svg"),
    ),
    (
        "",
        "oxishell-agent-generic-symbolic",
        include_bytes!("../../data/icons/oxishell-agent-generic-symbolic.svg"),
    ),
];
const AGENT_FALLBACK: &str = "utilities-terminal-symbolic";

const THEME_INDEX: &str = "\
[Icon Theme]
Name=Hicolor
Directories=scalable/actions,scalable/apps

[scalable/actions]
Size=16
MinSize=8
MaxSize=512
Type=Scalable
Context=Actions

[scalable/apps]
Size=128
MinSize=8
MaxSize=512
Type=Scalable
Context=Applications
";

static REGISTERED: OnceLock<bool> = OnceLock::new();

/// Install the bundled icons for `display`. Call once before building widgets.
pub fn register(display: &gdk::Display) -> Result<(), String> {
    let result = install(display);
    let _ = REGISTERED.set(result.is_ok());
    result
}

fn install(display: &gdk::Display) -> Result<(), String> {
    let dir = crate::paths::runtime_dir().join("icons");
    crate::paths::ensure_private_dir(&dir).map_err(|e| e.to_string())?;
    let theme_dir = dir.join("hicolor");
    let icon_dir = theme_dir.join("scalable/actions");
    crate::paths::ensure_private_dir(&icon_dir).map_err(|e| e.to_string())?;
    let icons: [(&str, &[u8]); 2] = [
        (PANE_MOVE, include_bytes!("../../data/icons/oxishell-pane-move-symbolic.svg")),
        (COMPOSE, include_bytes!("../../data/icons/oxishell-compose-symbolic.svg")),
    ];
    let agent_icons = AGENT_ICONS.iter().map(|(_, name, bytes)| (*name, *bytes));
    for (name, bytes) in icons.into_iter().chain(agent_icons) {
        let path = icon_dir.join(format!("{name}.svg"));
        if std::fs::read(&path).ok().as_deref() != Some(bytes) {
            crate::paths::write_atomic(&path, bytes, 0o600).map_err(|e| format!("{name}: {e}"))?;
        }
    }
    // The app icon too, for the About dialog where it is not installed
    // system-wide (Homebrew or a build directory).
    let apps_dir = theme_dir.join("scalable/apps");
    crate::paths::ensure_private_dir(&apps_dir).map_err(|e| e.to_string())?;
    let app_icon: &[u8] = include_bytes!("../../data/icons/dev.oxishell.Oxishell.svg");
    let path = apps_dir.join(format!("{}.svg", crate::APP_ID));
    if std::fs::read(&path).ok().as_deref() != Some(app_icon) {
        crate::paths::write_atomic(&path, app_icon, 0o600).map_err(|e| format!("app icon: {e}"))?;
    }
    let index = theme_dir.join("index.theme");
    if std::fs::read(&index).ok().as_deref() != Some(THEME_INDEX.as_bytes()) {
        crate::paths::write_atomic(&index, THEME_INDEX.as_bytes(), 0o600).map_err(|e| format!("index.theme: {e}"))?;
    }
    gtk4::IconTheme::for_display(display).add_search_path(&dir);
    Ok(())
}

/// Icon name for the pane drag handle.
pub fn pane_move() -> &'static str {
    if REGISTERED.get().copied().unwrap_or(false) {
        PANE_MOVE
    } else {
        PANE_MOVE_FALLBACK
    }
}

/// Icon name for the Docker Compose launcher button.
pub fn compose() -> &'static str {
    if REGISTERED.get().copied().unwrap_or(false) {
        COMPOSE
    } else {
        COMPOSE_FALLBACK
    }
}

/// Logo icon name for an agent id ("claude", "codex", ...). Agents without a
/// logo of their own get a generic terminal mark.
pub fn agent(id: &str) -> &'static str {
    if !REGISTERED.get().copied().unwrap_or(false) {
        return AGENT_FALLBACK;
    }
    AGENT_ICONS
        .iter()
        .find(|(agent, _, _)| *agent == id)
        .or_else(|| AGENT_ICONS.iter().find(|(agent, _, _)| agent.is_empty()))
        .map_or(AGENT_FALLBACK, |(_, name, _)| name)
}
