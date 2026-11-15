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

const THEME_INDEX: &str = "\
[Icon Theme]
Name=Hicolor
Directories=scalable/actions

[scalable/actions]
Size=16
MinSize=8
MaxSize=512
Type=Scalable
Context=Actions
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
    for (name, bytes) in icons {
        let path = icon_dir.join(format!("{name}.svg"));
        if std::fs::read(&path).ok().as_deref() != Some(bytes) {
            crate::paths::write_atomic(&path, bytes, 0o600).map_err(|e| format!("{name}: {e}"))?;
        }
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
