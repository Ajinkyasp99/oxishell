//! Keyboard aliases and Ctrl-wheel input for the existing font zoom actions.

use super::AppRc;
use crate::config::Config;
use gtk4::glib::translate::IntoGlib;
use gtk4::prelude::*;
use gtk4::{gdk, glib};
use std::collections::HashMap;

pub fn add_key_aliases(map: &mut HashMap<(u32, gdk::ModifierType), String>, config: &Config) {
    let ctrl = gdk::ModifierType::CONTROL_MASK;
    for (action, aliases) in [
        (
            "zoom-in",
            vec![
                (gdk::Key::plus, ctrl),
                (gdk::Key::plus, ctrl | gdk::ModifierType::SHIFT_MASK),
                (gdk::Key::KP_Add, ctrl),
            ],
        ),
        ("zoom-out", vec![(gdk::Key::KP_Subtract, ctrl)]),
    ] {
        // User bindings (including disabling zoom) take precedence over aliases.
        if config.shortcuts.get(action).is_some_and(|custom| {
            crate::config::DEFAULT_SHORTCUTS
                .iter()
                .any(|(name, default, _)| *name == action && custom != default)
        }) {
            continue;
        }
        for (key, mods) in aliases {
            map.entry((key.into_glib(), mods)).or_insert_with(|| action.into());
        }
    }
}

fn scroll_action(state: gdk::ModifierType, dy: f64) -> Option<&'static str> {
    if state != gdk::ModifierType::CONTROL_MASK || !dy.is_finite() || dy == 0.0 {
        return None;
    }
    Some(if dy < 0.0 { "zoom-in" } else { "zoom-out" })
}

pub fn attach_scroll(app: &AppRc, widget: &impl IsA<gtk4::Widget>) {
    // DISCRETE normalizes trackpad deltas as well as mouse-wheel notches.
    let controller =
        gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL | gtk4::EventControllerScrollFlags::DISCRETE);
    controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let w = app.weak();
    controller.connect_scroll(move |controller, _, dy| {
        let mods = controller.current_event_state() & gtk4::accelerator_get_default_mod_mask();
        let Some(action) = scroll_action(mods, dy) else {
            return glib::Propagation::Proceed;
        };
        let Some(app) = w.upgrade() else {
            return glib::Propagation::Proceed;
        };
        app.run_action(action);
        glib::Propagation::Stop
    });
    widget.add_controller(controller);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plus_and_keypad_aliases_preserve_custom_shortcuts() {
        let ctrl = gdk::ModifierType::CONTROL_MASK;
        let mut map = HashMap::new();
        add_key_aliases(&mut map, &Config::default());
        assert_eq!(map[&(gdk::Key::plus.into_glib(), ctrl)], "zoom-in");
        assert_eq!(map[&(gdk::Key::plus.into_glib(), ctrl | gdk::ModifierType::SHIFT_MASK)], "zoom-in");
        assert_eq!(map[&(gdk::Key::KP_Add.into_glib(), ctrl)], "zoom-in");
        assert_eq!(map[&(gdk::Key::KP_Subtract.into_glib(), ctrl)], "zoom-out");
        map.insert((gdk::Key::plus.into_glib(), ctrl), "new-workspace".into());
        add_key_aliases(&mut map, &Config::default());
        assert_eq!(map[&(gdk::Key::plus.into_glib(), ctrl)], "new-workspace");
        for binding in ["", "<Alt>z"] {
            let mut config = Config::default();
            config.shortcuts.insert("zoom-in".into(), binding.into());
            let mut map = HashMap::new();
            add_key_aliases(&mut map, &config);
            assert!(!map.contains_key(&(gdk::Key::plus.into_glib(), ctrl)));
        }
    }

    #[test]
    fn only_ctrl_vertical_scroll_zooms() {
        let ctrl = gdk::ModifierType::CONTROL_MASK;
        assert_eq!(scroll_action(ctrl, -1.0), Some("zoom-in"));
        assert_eq!(scroll_action(ctrl, 1.0), Some("zoom-out"));
        assert_eq!(scroll_action(gdk::ModifierType::empty(), -1.0), None);
        assert_eq!(scroll_action(ctrl | gdk::ModifierType::SHIFT_MASK, 1.0), None);
        assert_eq!(scroll_action(ctrl, 0.0), None);
        assert_eq!(scroll_action(ctrl, f64::NAN), None);
    }
}
