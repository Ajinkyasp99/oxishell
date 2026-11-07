//! Mouse-wheel speed for mouse-reporting terminal applications.
//!
//! VTE consumes these events before GtkScrolledWindow sees them. Keep its
//! protocol encoding and coordinates, but use three reports per wheel step.

use gtk4::prelude::*;
use gtk4::{gdk, glib};
use std::cell::Cell;
use vte4::prelude::*;

const APP_WHEEL_MULTIPLIER: usize = 3;

fn is_wheel_report(text: &str) -> bool {
    // SGR mouse mode (Codex, tmux, vim, etc.). Modified buttons and horizontal
    // wheels are deliberately excluded, as are releases and ordinary clicks.
    if let Some(body) = text.strip_prefix("\x1b[<").and_then(|s| s.strip_suffix('M')) {
        let mut fields = body.split(';');
        return matches!(fields.next(), Some("64" | "65"))
            && fields.next().is_some_and(is_coordinate)
            && fields.next().is_some_and(is_coordinate)
            && fields.next().is_none();
    }
    // VTE's legacy X10 encoding uses button + 32 and one byte per coordinate.
    matches!(text.as_bytes(), [0x1b, b'[', b'M', 96 | 97, x, y] if *x > 32 && *y > 32)
}

fn is_coordinate(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) && text.parse::<u32>().is_ok_and(|n| n > 0)
}

pub fn attach(term: &vte4::Terminal) {
    let replaying = Cell::new(false);
    term.connect_commit(move |term, text, _| {
        if replaying.get() || !is_wheel_report(text) {
            return;
        }
        // Only amplify a report emitted while VTE is dispatching a real wheel
        // event. Pasted text, IPC input and touchpad surface deltas pass through.
        // Inspect public controller state without depending on VTE's private
        // controller names or reimplementing its terminal mouse-mode tracking.
        let wheel = term
            .observe_controllers()
            .iter::<glib::Object>()
            .filter_map(Result::ok)
            .any(|object| {
                object
                    .downcast::<gtk4::EventControllerScroll>()
                    .ok()
                    .and_then(|controller| controller.current_event())
                    .and_then(|event| event.downcast::<gdk::ScrollEvent>().ok())
                    .is_some_and(|event| event.unit() == gdk::ScrollUnit::Wheel)
            });
        if wheel {
            replaying.set(true);
            for _ in 1..APP_WHEEL_MULTIPLIER {
                term.feed_child(text.as_bytes());
            }
            replaying.set(false);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_unmodified_vertical_wheel_reports() {
        for text in ["\x1b[<64;2;1M", "\x1b[<65;120;40M", "\x1b[M`!!", "\x1b[Ma!!"] {
            assert!(is_wheel_report(text), "{text:?}");
        }
        for text in [
            "\x1b[<0;2;1M",
            "\x1b[<64;2;1m",
            "\x1b[<68;2;1M",
            "\x1b[<80;2;1M",
            "\x1b[<66;2;1M",
            "\x1b[<64;0;1M",
            "\x1b[<64;2;1Mextra",
            "\x1b[<64;2;1;3M",
            "\x1b[<64;2;M",
            "\x1b[<64;+2;1M",
            "\x1b[<64;4294967296;1M",
            "\x1b[B",
            "hello",
            "\x1b[M !!",
        ] {
            assert!(!is_wheel_report(text), "{text:?}");
        }
    }
}
