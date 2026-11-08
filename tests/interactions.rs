//! Real pointer input against the GTK widgets. Run under Xvfb with xdotool.
use gtk4::prelude::*;
use gtk4::{gio, glib};
use oxishell::ui::{App, AppRc};
use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vte4::prelude::*;

struct Harness {
    app: AppRc,
    root: PathBuf,
    window: String,
}

fn pump(ms: u64) {
    let context = glib::MainContext::default();
    let until = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < until {
        while context.pending() {
            context.iteration(false);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn xdo(args: &[&str]) -> String {
    let output = Command::new("xdotool").args(args).output().unwrap();
    assert!(output.status.success(), "xdotool {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn have_input() -> bool {
    let available = std::env::var_os("DISPLAY").is_some() && Command::new("xdotool").arg("version").output().is_ok();
    if !available {
        eprintln!("SKIPPED: pointer tests require X11 and xdotool; run under xvfb-run");
    }
    available
}

impl Harness {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("oxishell-pointer-{}-{name}", std::process::id()));
        for dir in ["home", "config/oxishell", "state", "run"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        for (key, dir) in [
            ("HOME", "home"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_RUNTIME_DIR", "run"),
        ] {
            std::env::set_var(key, root.join(dir));
        }
        std::env::remove_var("OXISHELL_SOCKET");
        std::env::set_var("GSETTINGS_BACKEND", "memory");
        std::fs::write(
            root.join("config/oxishell/config.toml"),
            "[terminal]\nshell = '/bin/bash'\nsuggestions = false\n[session]\nrestore = false\n[notifications]\ndesktop = false\n",
        )
        .unwrap();
        let application = libadwaita::Application::builder()
            .application_id(format!("dev.oxishell.PointerTest.{name}"))
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        application.register(gio::Cancellable::NONE).unwrap();
        let app = App::build(&application);
        pump(500);
        let windows = xdo(&["search", "--onlyvisible", "--pid", &std::process::id().to_string()]);
        let window = windows.lines().last().unwrap().to_string();
        xdo(&["windowfocus", "--sync", &window]);
        Self { app, root, window }
    }

    fn point(&self, widget: &impl IsA<gtk4::Widget>) {
        let p = widget
            .as_ref()
            .compute_point(&self.app.window, &gtk4::graphene::Point::new(20.0, 15.0))
            .unwrap();
        xdo(&[
            "mousemove",
            "--window",
            &self.window,
            &(p.x() as i32).to_string(),
            &(p.y() as i32).to_string(),
        ]);
        pump(30);
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.app.window.close();
        pump(100);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn workspace_switches_on_each_single_click_during_refresh() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("workspace");
        let first = h.app.inner.borrow().state.active.unwrap();
        let (second, _) = h.app.new_workspace(Some("Second"), Some(h.root.join("home"))).unwrap();
        pump(300);
        for (index, expected) in [(0, first), (1, second), (0, first), (1, second)] {
            let row = h.app.sidebar_list.row_at_index(index).unwrap();
            h.point(&row);
            xdo(&["mousedown", "1"]);
            pump(30);
            // Metadata/notification updates can arrive between press and release.
            h.app.refresh();
            assert_eq!(h.app.sidebar_list.row_at_index(index).as_ref(), Some(&row));
            pump(30);
            xdo(&["mouseup", "1"]);
            pump(100);
            assert_eq!(
                h.app.inner.borrow().state.active,
                Some(expected),
                "one click must switch workspace {expected}"
            );
            assert_eq!(h.app.stack.visible_child_name().as_deref(), Some(expected.to_string().as_str()));
        }

        // Holding and dragging a workspace must work through live refreshes.
        let (third, _) = h.app.new_workspace(Some("Third"), Some(h.root.join("home"))).unwrap();
        pump(300);
        let row = h.app.sidebar_list.row_at_index(2).unwrap();
        h.point(&row);
        xdo(&["mousedown", "1"]);
        pump(100);
        // Cross GTK's drag threshold before entering the destination row.
        xdo(&["mousemove_relative", "--", "12", "0"]);
        pump(100);
        let target = h.app.sidebar_list.row_at_index(0).unwrap();
        h.point(&target);
        xdo(&["mousemove_relative", "--", "2", "0"]);
        pump(200);
        h.app.refresh();
        pump(50);
        xdo(&["mouseup", "1"]);
        pump(200);
        let order = || h.app.inner.borrow().state.workspaces.iter().map(|w| w.id).collect::<Vec<_>>();
        assert_eq!(
            order(),
            vec![third, first, second],
            "dragging last workspace above first must reorder it"
        );
        h.app.refresh();
        assert_eq!(order(), vec![third, first, second], "refresh must preserve the dragged order");
        for (index, expected) in [third, first, second].into_iter().enumerate() {
            assert_eq!(
                h.app.sidebar_list.row_at_index(index as i32).unwrap().widget_name(),
                expected.to_string()
            );
        }
        h.app.inner.borrow_mut().config.session.restore = true;
        h.app.save_session(false);
        let saved = oxishell::session::Snapshot::load(&h.root.join("state/oxishell/session.json"))
            .unwrap()
            .unwrap();
        assert_eq!(saved.state.workspaces.iter().map(|w| w.id).collect::<Vec<_>>(), order());
    });
}

#[test]
fn scrollback_uses_pixels_and_preserves_terminal_app_wheel_input() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("scroll");
        let term = h.app.inner.borrow().panes.values().next().unwrap().term.clone();
        let output: String = (0..500).map(|i| format!("SCROLL-{i}\r\n")).collect();
        term.feed(output.as_bytes());
        pump(200);
        h.point(&term);
        let adjustment = term.vadjustment().unwrap();
        let bottom = adjustment.upper() - adjustment.page_size();
        adjustment.set_value(bottom);
        pump(50);
        xdo(&["click", "4"]);
        pump(150);
        let distance = bottom - adjustment.value();
        let expected = adjustment.page_size().powf(2.0 / 3.0);
        assert!(
            (distance - expected).abs() < 1.0,
            "wheel moved {distance}, expected GTK pixel step {expected}"
        );
        assert!((adjustment.page_size() / term.row_count() as f64 - term.char_height() as f64).abs() < 1.0);
        xdo(&["click", "5"]);
        pump(150);
        assert!((adjustment.value() - bottom).abs() < 1.0, "reverse scroll must return to bottom");

        // Sub-line movement must survive the shared adjustment.
        adjustment.set_value(bottom - 0.5 * term.char_height() as f64);
        pump(50);
        assert!((adjustment.value() - (bottom - 0.5 * term.char_height() as f64)).abs() < 0.1);
        adjustment.set_value(bottom);
        let committed = Rc::new(RefCell::new(String::new()));
        let input = committed.clone();
        term.connect_commit(move |_, text, _| input.borrow_mut().push_str(text));
        term.feed(b"\x1b[?1049h\x1b[?1000h\x1b[?1006h");
        pump(100);
        xdo(&["click", "4"]);
        pump(100);
        assert!(
            committed.borrow().contains("\x1b[<64;"),
            "mouse-aware app must receive wheel up: {committed:?}"
        );
        term.feed(b"\x1b[?1000l");
        pump(50);
        committed.borrow_mut().clear();
        xdo(&["click", "5"]);
        pump(100);
        assert!(
            committed.borrow().contains("\x1b[B"),
            "alternate screen must retain arrow-key fallback: {committed:?}"
        );
        term.feed(b"\x1b[?1049l");
    });
}

#[test]
fn mouse_wheel_scales_real_reports_to_a_mouse_reporting_pty_app() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("appwheel");
        let ready = h.root.join("ready");
        let captured = h.root.join("mouse-input");
        let script = h.root.join("mouse-app.py");
        std::fs::write(
            &script,
            r#"import os, pathlib, sys, tty
tty.setraw(0)
log = open(sys.argv[1], 'wb', buffering=0)
os.write(1, b'\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[2J\x1b[HMouse-reporting app')
pathlib.Path(sys.argv[2]).touch()
while True:
    log.write(os.read(0, 4096))
"#,
        )
        .unwrap();
        let term = {
            let i = h.app.inner.borrow();
            let pane = i.panes.values().next().unwrap();
            pane.send_input(format!("python3 {} {} {}\r", script.display(), captured.display(), ready.display()).as_bytes());
            pane.term.clone()
        };
        let start = Instant::now();
        while !ready.exists() {
            assert!(start.elapsed() < Duration::from_secs(5), "PTY mouse app did not start");
            pump(20);
        }
        pump(50);
        h.point(&term);
        xdo(&["click", "4"]);
        pump(100);
        let input = std::fs::read_to_string(&captured).unwrap();
        assert_eq!(
            input.matches("\x1b[<64;").count(),
            3,
            "one mouse notch should deliver three reports, received {input:?}"
        );
        let reports: Vec<_> = input.split_inclusive('M').collect();
        assert!(
            reports.iter().all(|report| *report == reports[0]),
            "coordinates and encoding must be preserved"
        );
        xdo(&["click", "5"]);
        xdo(&["click", "--repeat", "5", "--delay", "8", "4"]);
        pump(100);
        let input = std::fs::read_to_string(&captured).unwrap();
        assert_eq!(
            input.matches("\x1b[<64;").count(),
            18,
            "a burst must deliver a bounded number of reports"
        );
        assert_eq!(input.matches("\x1b[<65;").count(), 3, "downward scrolling must use the same speed");

        // IPC/synthetic input that looks like a wheel packet must stay literal.
        let before = input.len();
        let literal = "\x1b[<64;7;8M";
        term.feed_child(literal.as_bytes());
        xdo(&["key", "a"]);
        pump(100);
        let input = std::fs::read_to_string(&captured).unwrap();
        assert_eq!(&input[before..], format!("{literal}a"));

        let before = input.len();
        let scale = term.font_scale();
        xdo(&["keydown", "ctrl", "click", "4", "keyup", "ctrl"]);
        pump(100);
        assert!(term.font_scale() > scale, "Ctrl-wheel must still zoom");
        assert_eq!(
            std::fs::read_to_string(&captured).unwrap().len(),
            before,
            "Ctrl-wheel must not scroll the app"
        );
    });
}
