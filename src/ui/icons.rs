//! App-local symbolic icons that no system icon theme provides.
//!
//! Like the bundled fonts, they are written to the runtime dir and that folder
//! is added to the display's icon search path. The `-symbolic` suffix makes
//! GTK recolor them with the widget's CSS colour, like any theme icon.

use gtk4::gdk;
use std::sync::OnceLock;

/// Pane drag handle: four-way move arrows.
const PANE_MOVE: &str = "oxishell-pane-move-symbolic";
/// Theme icon used when the bundled icons could not be installed.
const PANE_MOVE_FALLBACK: &str = "list-drag-handle-symbolic";

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
    let bytes = include_bytes!("../../data/icons/oxishell-pane-move-symbolic.svg").as_slice();
    let path = dir.join(format!("{PANE_MOVE}.svg"));
    if std::fs::read(&path).ok().as_deref() != Some(bytes) {
        crate::paths::write_atomic(&path, bytes, 0o600).map_err(|e| format!("{PANE_MOVE}: {e}"))?;
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
