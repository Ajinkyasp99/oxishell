//! Shared motion for panels and lists (the "subtle" level).
//!
//! There is one switch: GTK's `gtk-enable-animations`. GTK's own transitions
//! (Stack, Revealer, CSS animations) and libadwaita animations already follow
//! it, so nothing here checks it per widget. GNOME's "Reduce animation" reaches
//! it on Linux; on macOS [`follow_os`] mirrors System Settings' "Reduce motion"
//! into it, because GTK does not read that setting there.
//!
//! Nothing here runs while nothing moves: no timers, no idle frame ticks.

#[cfg(target_os = "macos")]
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
#[cfg(target_os = "macos")]
use std::cell::Cell;

/// Crossfades between views (workspace, tab, panel content).
pub const FADE_MS: u32 = 120;
/// Panels and rows sliding in or out.
pub const SLIDE_MS: u32 = 160;

// The "subtle" motion level: nothing slower than this.
const _: () = assert!(FADE_MS <= 150 && SLIDE_MS <= 200);

/// Fast start, gentle stop. `t` is clamped to 0..=1.
pub fn ease_out_cubic(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Switch views with a short crossfade.
pub fn crossfade(stack: &gtk4::Stack) {
    stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
    stack.set_transition_duration(FADE_MS);
}

/// A revealer that fades (`Crossfade`, [`FADE_MS`]) or slides (other kinds,
/// [`SLIDE_MS`]) its child in and out.
pub fn reveal(revealer: &gtk4::Revealer, kind: gtk4::RevealerTransitionType) {
    revealer.set_transition_type(kind);
    revealer.set_transition_duration(match kind {
        gtk4::RevealerTransitionType::Crossfade => FADE_MS,
        _ => SLIDE_MS,
    });
}

/// Fade and lift a newly added item into place (`.oxi-enter`, 150 ms), once.
/// Does nothing while `parent` is unmapped (startup, a hidden workspace), so
/// only additions the user sees animate. The class is dropped afterwards, so a
/// later reparent (reorder) does not replay it.
pub fn enter(widget: &impl IsA<gtk4::Widget>, parent: &impl IsA<gtk4::Widget>) {
    if parent.is_mapped() {
        once(widget, "oxi-enter", 150);
    }
}

/// Run the CSS animation that `class` starts on `widget` once: add the class,
/// then drop it after `ms` (the animation's length in style.css), so it can
/// run again later. Skipped while animations are off.
pub fn once(widget: &impl IsA<gtk4::Widget>, class: &'static str, ms: u64) {
    let widget = widget.as_ref();
    if !widget.settings().is_gtk_enable_animations() {
        return;
    }
    // Restart an animation that is still running.
    widget.remove_css_class(class);
    widget.add_css_class(class);
    let weak = widget.downgrade();
    glib::timeout_add_local_once(std::time::Duration::from_millis(ms + 50), move || {
        if let Some(w) = weak.upgrade() {
            w.remove_css_class(class);
        }
    });
}

#[cfg(target_os = "macos")]
thread_local! {
    /// Whether oxishell itself turned animations off for macOS "Reduce motion".
    static WE_DISABLED: Cell<bool> = const { Cell::new(false) };
}

/// Mirror the OS "reduce motion" request into `gtk-enable-animations`. Call at
/// startup and whenever the window becomes active. Linux: does nothing (GTK
/// already follows GNOME). macOS: reads the setting off the GTK thread.
pub fn follow_os() {
    #[cfg(target_os = "macos")]
    glib::spawn_future_local(async {
        let reduce = gio::spawn_blocking(crate::platform::reduce_motion).await.unwrap_or(false);
        let Some(settings) = gtk4::Settings::default() else { return };
        apply_os(&settings, reduce);
    });
}

/// Turn animations off while the OS asks to reduce motion, and back on only if
/// we were the ones who turned them off.
#[cfg(target_os = "macos")]
fn apply_os(settings: &gtk4::Settings, reduce: bool) {
    WE_DISABLED.with(|we| {
        if reduce && settings.is_gtk_enable_animations() {
            settings.set_gtk_enable_animations(false);
            we.set(true);
        } else if !reduce && we.replace(false) {
            settings.set_gtk_enable_animations(true);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_cubic_is_clamped_monotonic_and_front_loaded() {
        assert_eq!(ease_out_cubic(-1.0), 0.0);
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert_eq!(ease_out_cubic(2.0), 1.0);
        assert!(ease_out_cubic(0.5) > 0.5, "most of the distance is covered early");
        let mut last = 0.0;
        for i in 1..=20 {
            let v = ease_out_cubic(i as f64 / 20.0);
            assert!(v > last);
            last = v;
        }
    }
}
