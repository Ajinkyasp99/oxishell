//! Regression test for the VTE 0.76 GTK4 font-cache crash.
//!
//! VTE < 0.78 stores the PangoFont of each cached glyph without taking a
//! reference, yet unrefs it when the font cache is freed 30 s after a terminal
//! stops using a font (zoom, font change, pane close). That frees fonts still
//! in use and crashes in g_object_unref (SIGSEGV in
//! g_type_check_instance_is_fundamentally_a).
//!
//! The scenario runs in a child process with fatal criticals so any bad unref
//! aborts it. Needs a display:  xvfb-run -a cargo test --test vte_font_cache

use gtk4::glib;
use gtk4::prelude::*;
use std::process::Command;
use std::time::{Duration, Instant};
use vte4::TerminalExt;

const SCENARIO_ENV: &str = "CMUX_VTE_FONT_CACHE_SCENARIO";

fn pump_for(d: Duration) {
    let ctx = glib::MainContext::default();
    let end = Instant::now() + d;
    while Instant::now() < end {
        while ctx.iteration(false) {}
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Draw hundreds of distinct glyphs, as agent TUIs do over a session. VTE
/// caches one font pointer per distinct glyph, so each one is a stolen ref.
fn render_many_glyphs(term: &vte4::Terminal) {
    let ranges = [
        0x21..=0x7e,     // ASCII
        0xa1..=0x17f,    // Latin-1 Supplement, Latin Extended-A
        0x391..=0x3c9,   // Greek
        0x400..=0x45f,   // Cyrillic
        0x2500..=0x259f, // box drawing, block elements
    ];
    let chars: Vec<char> = ranges.into_iter().flatten().filter_map(char::from_u32).collect();
    for line in chars.chunks(60) {
        let mut s: String = line.iter().collect();
        s.push_str("\r\n");
        term.feed(s.as_bytes());
        // Draw every line while it is on screen.
        pump_for(Duration::from_millis(60));
    }
}

/// Render glyphs, drop the font cache by changing scale, outlive VTE's 30 s
/// cache timeout, then exit cleanly.
fn scenario(with_fix: bool) {
    if with_fix {
        let status = cmux_linux::ui::vte_fix::install();
        eprintln!("vte_fix status: {status:?}");
        assert!(
            !matches!(status, cmux_linux::ui::vte_fix::Status::Failed(_)),
            "workaround needed but not applied: {status:?}"
        );
    }
    gtk4::init().unwrap();
    let win = gtk4::Window::new();
    win.set_default_size(640, 400);
    let term = vte4::Terminal::new();
    win.set_child(Some(&term));
    win.present();
    pump_for(Duration::from_millis(500));
    let cycles: u32 = std::env::var("CMUX_VTE_CYCLES").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
    // Zoom away and back, like a user over a session. Each zoom level gets its
    // own VTE font cache; VTE frees an unused one 30 s later, and every cycle
    // re-caches glyphs of the same PangoFont.
    for cycle in 0..cycles {
        term.set_font_scale(1.0);
        render_many_glyphs(&term);
        term.set_font_scale(1.5);
        term.feed(b"\x1b[2J\x1b[H");
        pump_for(Duration::from_secs(32));
        eprintln!("cycle {cycle} done");
    }
    eprintln!("scenario completed");
}

fn run_child(with_fix: bool) -> std::process::Output {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "scenario_entry", "--nocapture", "--test-threads=1"])
        .env(SCENARIO_ENV, if with_fix { "fixed" } else { "unfixed" })
        .env("G_DEBUG", "fatal-criticals")
        .env("GSETTINGS_BACKEND", "memory")
        .env("GTK_A11Y", "none")
        // Scribble freed memory so use-after-free fails loudly.
        .env("MALLOC_PERTURB_", "165")
        .output()
        .unwrap()
}

fn have_display() -> bool {
    let ok = std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
    if !ok {
        eprintln!("SKIPPED: no display; run under `xvfb-run -a cargo test --test vte_font_cache`");
    }
    ok
}

/// Child-process entry; a no-op unless spawned by the tests below.
#[test]
fn scenario_entry() {
    match std::env::var(SCENARIO_ENV).as_deref() {
        Ok("fixed") => scenario(true),
        Ok("unfixed") => scenario(false),
        _ => {}
    }
}

#[test]
fn font_cache_expiry_does_not_crash() {
    if !have_display() {
        return;
    }
    let out = run_child(true);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stderr.contains("scenario completed"),
        "app crashed after VTE font cache expiry: {:?}\n{stderr}",
        out.status
    );
}
