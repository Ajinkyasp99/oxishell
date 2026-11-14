//! Real pointer input against the GTK widgets. Run under Xvfb with xdotool.
use gtk4::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
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
        Self::with_history(name, &[])
    }

    /// Like `new`, with `history` (oldest first) as the shell's ~/.bash_history.
    fn with_history(name: &str, history: &[&str]) -> Self {
        let root = std::env::temp_dir().join(format!("oxishell-pointer-{}-{name}", std::process::id()));
        for dir in ["home", "config/oxishell", "state", "run"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        if !history.is_empty() {
            std::fs::write(root.join("home/.bash_history"), history.join("\n") + "\n").unwrap();
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
fn quick_clicks_across_rows_are_not_a_double_click_but_a_real_one_renames() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        use libadwaita::prelude::*;
        let h = Harness::new("dblclick");
        let first = h.app.inner.borrow().state.active.unwrap();
        let (second, _) = h.app.new_workspace(Some("Second"), Some(h.root.join("home"))).unwrap();
        pump(300);
        // Widen the double-click window so A, B, A, B clicks always land inside it.
        let settings = gtk4::Settings::default().unwrap();
        let saved = settings.gtk_double_click_time();
        settings.set_gtk_double_click_time(1500);
        for (index, expected) in [(0, first), (1, second), (0, first), (1, second)] {
            let row = h.app.sidebar_list.row_at_index(index).unwrap();
            h.point(&row);
            xdo(&["click", "1"]);
            pump(100);
            assert!(
                h.app.window.visible_dialog().is_none(),
                "clicking another row in between must reset the double-click count"
            );
            assert_eq!(
                h.app.inner.borrow().state.active,
                Some(expected),
                "one click must switch workspace {expected}"
            );
        }

        // A genuine double click on one row still opens the rename dialog.
        pump(1600);
        let row = h.app.sidebar_list.row_at_index(0).unwrap();
        h.point(&row);
        xdo(&["click", "--repeat", "2", "--delay", "60", "1"]);
        pump(200);
        let dialog = h.app.window.visible_dialog().expect("double click must open rename");
        dialog.force_close();
        // Wait out the closing animation: while a dialog is still visible,
        // closing the window (Harness::drop) only closes the dialog and leaves
        // this window and its terminals alive under the next test.
        let deadline = Instant::now() + Duration::from_secs(3);
        while h.app.window.visible_dialog().is_some() {
            assert!(Instant::now() < deadline, "rename dialog did not close");
            pump(20);
        }
        settings.set_gtk_double_click_time(saved);
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

/// VTE 0.76 does not invalidate its visible-row cache ("ringview") when the
/// view moves through the GtkAdjustment (scrollbar, wheel, kinetic scroll),
/// and check_hyperlink_at/check_match_at only rebuild a cache marked invalid.
/// Hit-testing (our link hover runs before VTE's own motion handler) between
/// such a scroll and the next redraw read outside the cache and crashed.
#[test]
fn link_hit_testing_right_after_an_adjustment_scroll_is_safe() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("hittest");
        let term = h.app.inner.borrow().panes.values().next().unwrap().term.clone();
        let output: String = (0..600).map(|i| format!("LINE-{i} https://example.com/{i}\r\n")).collect();
        term.feed(output.as_bytes());
        // Let VTE draw: its row cache now covers the bottom of the scrollback.
        pump(300);
        let adjustment = term.vadjustment().unwrap();
        let height = term.height() as f64;
        for pages in [3.0, 1.5, 6.0, 0.25] {
            // Scroll up as the scrollbar or wheel would, then hit-test before
            // any redraw, at the top, middle and bottom of the view.
            adjustment.set_value((adjustment.value() - pages * adjustment.page_size()).max(adjustment.lower()));
            for y in [2.0, height / 2.0, height - 3.0] {
                let _ = term.check_hyperlink_at(12.0, y);
                let _ = term.check_match_at(12.0, y);
            }
        }
        // The cache is correct again: the link under the pointer is found.
        let (link, _) = term.check_match_at(12.0 + 12.0 * term.char_width() as f64, 2.0);
        assert!(
            link.as_deref().is_some_and(|l| l.starts_with("https://example.com/")),
            "link under the pointer after scrolling: {link:?}"
        );
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

fn find_by_class(root: &gtk4::Widget, class: &str) -> Option<gtk4::Widget> {
    if root.has_css_class(class) {
        return Some(root.clone());
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = find_by_class(&c, class) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

#[test]
fn dragging_a_pane_handle_onto_another_pane_swaps_them() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("pane-swap");
        let wid = h.app.inner.borrow().state.active.unwrap();
        let first = h.app.focused_pane().unwrap();
        let second = h.app.split(Some(first), oxishell::model::Orient::Horizontal, None).unwrap();
        pump(400);
        let layout = || h.app.inner.borrow().state.workspace(wid).unwrap().layout.panes();
        assert_eq!(layout(), vec![first, second]);
        let (first_frame, second_frame, first_term) = {
            let i = h.app.inner.borrow();
            (
                i.panes[&first].frame.clone(),
                i.panes[&second].frame.clone(),
                i.panes[&first].term.clone(),
            )
        };
        let handle = find_by_class(first_frame.upcast_ref(), "pane-drag").expect("pane has a drag handle");
        // The bundled four-way move icon is installed and found by GTK.
        let icon = handle.downcast_ref::<gtk4::Image>().unwrap().icon_name();
        assert_eq!(icon.as_deref(), Some("oxishell-pane-move-symbolic"));
        assert!(gtk4::IconTheme::for_display(&handle.display()).has_icon("oxishell-pane-move-symbolic"));
        let center = |w: &gtk4::Widget| {
            let p = w
                .compute_point(
                    &h.app.window,
                    &gtk4::graphene::Point::new(w.width() as f32 / 2.0, w.height() as f32 / 2.0),
                )
                .unwrap();
            (p.x() as i32, p.y() as i32)
        };
        let to = |(x, y): (i32, i32)| {
            xdo(&["mousemove", "--window", &h.window, &x.to_string(), &y.to_string()]);
            pump(60);
        };
        // While dragging, move in small real steps: the drag only follows
        // pointer motion, not warps.
        let drag_to = |from: (i32, i32), (x, y): (i32, i32)| {
            let steps = 12;
            let (dx, dy) = ((x - from.0) / steps, (y - from.1) / steps);
            for _ in 0..steps {
                xdo(&["mousemove_relative", "--", &dx.to_string(), &dy.to_string()]);
                pump(25);
            }
            pump(100);
        };

        // Dropping a pane on itself changes nothing.
        to(center(&handle));
        xdo(&["mousedown", "1"]);
        pump(60);
        drag_to(center(&handle), center(first_frame.upcast_ref()));
        xdo(&["mouseup", "1"]);
        pump(200);
        assert_eq!(layout(), vec![first, second], "dropping on itself must not swap");

        // Drag the left pane's handle onto the right pane.
        to(center(&handle));
        xdo(&["mousedown", "1"]);
        pump(60);
        drag_to(center(&handle), center(second_frame.upcast_ref()));
        let hint = find_by_class(second_frame.upcast_ref(), "pane-drop-hint").unwrap();
        assert!(hint.has_css_class("active"), "the target pane shows the swap hint");
        assert!(handle.has_css_class("pressed"), "the held handle shows it is pressed");
        xdo(&["mouseup", "1"]);
        pump(400);

        assert_eq!(layout(), vec![second, first], "drop must swap the panes");
        assert!(!hint.has_css_class("active"), "hint hides after the drop");
        assert!(!handle.has_css_class("pressed"));
        assert!(first_frame.has_css_class("pane-landed"), "swapped panes play the settle animation");
        pump(500);
        assert!(!first_frame.has_css_class("pane-landed"), "settle class clears so it can replay");
        assert_eq!(h.app.focused_pane(), Some(first), "the dragged pane keeps focus");
        assert!(h.app.dragging_pane.get().is_none());
        // The same terminal (and its shell) moved; nothing was recreated.
        assert!(first_term.is_mapped());
        assert_eq!(h.app.inner.borrow().panes[&first].term, first_term);
        let (fx, _) = center(first_frame.upcast_ref());
        let (sx, _) = center(second_frame.upcast_ref());
        assert!(fx > sx, "the dragged pane is now on the right");
    });
}

fn paned_handle(p: &gtk4::Paned) -> gtk4::Widget {
    let mut child = p.first_child();
    while let Some(c) = child {
        if c.css_name() == "separator" {
            return c;
        }
        child = c.next_sibling();
    }
    panic!("paned without a handle");
}

#[test]
fn dragging_where_dividers_meet_resizes_both_directions() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("corner-resize");
        let wid = h.app.inner.borrow().state.active.unwrap();
        let first = h.app.focused_pane().unwrap();
        // ├ layout: first | (second over third).
        let second = h.app.split(Some(first), oxishell::model::Orient::Horizontal, None).unwrap();
        h.app.split(Some(second), oxishell::model::Orient::Vertical, None).unwrap();
        pump(700);
        let page = h.app.inner.borrow().pages[&wid].clone();
        let root = page.first_child().and_downcast::<gtk4::Paned>().unwrap();
        let column = root.end_child().and_downcast::<gtk4::Paned>().unwrap();
        let (root_handle, column_handle) = (paned_handle(&root), paned_handle(&column));
        // Exact X coordinates: include the window's client-side shadow, which
        // sits between the X window's origin and the GTK window's.
        let (sx, sy) = h.app.window.surface_transform();
        let at = |w: &gtk4::Widget, fx: f32, fy: f32| {
            let p = w
                .compute_point(
                    &h.app.window,
                    &gtk4::graphene::Point::new(w.width() as f32 * fx, w.height() as f32 * fy),
                )
                .unwrap();
            ((p.x() as f64 + sx).round() as i32, (p.y() as f64 + sy).round() as i32)
        };
        let to = |(x, y): (i32, i32)| {
            xdo(&["mousemove", "--window", &h.window, &x.to_string(), &y.to_string()]);
            pump(80);
        };
        let drag = |dx: i32, dy: i32| {
            xdo(&["mousedown", "1"]);
            pump(60);
            for _ in 0..10 {
                xdo(&["mousemove_relative", "--", &(dx / 10).to_string(), &(dy / 10).to_string()]);
                pump(25);
            }
            pump(100);
            xdo(&["mouseup", "1"]);
            pump(200);
        };
        let cursor = |w: &gtk4::Widget| w.cursor().and_then(|c| c.name()).map(|n| n.to_string());

        // The ├ junction: on the vertical divider, level with the horizontal one.
        let junction = (at(&root_handle, 0.5, 0.0).0, at(&column_handle, 0.0, 0.5).1);
        to(junction);
        assert_eq!(cursor(&root_handle).as_deref(), Some("move"), "junction shows the four-way cursor");

        let (root_before, column_before) = (root.position(), column.position());
        drag(-60, -40);
        assert!(
            (root.position() - (root_before - 60)).abs() <= 3,
            "vertical divider follows x: {root_before} -> {}",
            root.position()
        );
        assert!(
            (column.position() - (column_before - 40)).abs() <= 3,
            "horizontal divider follows y: {column_before} -> {}",
            column.position()
        );
        // Both new ratios are saved in the layout.
        let rects = h.app.inner.borrow().state.workspace(wid).unwrap().layout.rects();
        assert!(rects[0].1.w < 0.49, "first pane got narrower: {rects:?}");
        assert!(rects[1].1.h < 0.49, "top-right pane got shorter: {rects:?}");

        // Away from the junction a divider still moves in one direction only.
        let (root_before, column_before) = (root.position(), column.position());
        to(at(&root_handle, 0.5, 0.85));
        assert_ne!(cursor(&root_handle).as_deref(), Some("move"), "cursor restored off the junction");
        drag(30, 30);
        assert!((root.position() - (root_before + 30)).abs() <= 3, "plain divider drag still works");
        assert_eq!(column.position(), column_before, "the other divider stays put");
    });
}

fn find_descendant<T: IsA<gtk4::Widget>>(root: &gtk4::Widget) -> Option<T> {
    if let Some(w) = root.downcast_ref::<T>() {
        return Some(w.clone());
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = find_descendant::<T>(&c) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

#[test]
fn history_panel_shows_the_newest_command_in_full() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        // More commands than the panel shows at once, so it has to scroll.
        let cmds: Vec<String> = (1..=25).map(|n| format!("echo history-entry-{n:02}")).collect();
        let refs: Vec<&str> = cmds.iter().map(String::as_str).collect();
        let h = Harness::with_history("history-panel", &refs);
        let pane = h.app.focused_pane().unwrap();
        let term = h.app.inner.borrow().panes[&pane].term.clone();
        // Wait for the shell prompt, then open the panel with ↑.
        pump(2500);
        h.point(&term);
        xdo(&["click", "1"]);
        pump(200);
        xdo(&["key", "Up"]);
        pump(600);

        let mut pop = None;
        let mut child = term.first_child();
        while let Some(c) = child {
            if c.has_css_class("history-panel") {
                pop = Some(c.clone());
            }
            child = c.next_sibling();
        }
        let pop = pop.expect("↑ at the prompt opens the history panel");
        assert!(pop.is_visible());
        let scroll: gtk4::ScrolledWindow = find_descendant(&pop).unwrap();
        let list: gtk4::ListBox = find_descendant(scroll.upcast_ref()).unwrap();
        let newest = list.row_at_index(cmds.len() as i32 - 1).unwrap();
        assert_eq!(list.selected_row().as_ref(), Some(&newest), "the newest command is selected");
        let b = newest.compute_bounds(&scroll).unwrap();
        let (top, bottom, view) = (b.y(), b.y() + b.height(), scroll.height() as f32);
        assert!(
            top >= -0.5 && bottom <= view + 0.5,
            "newest row must be fully visible: row {top}..{bottom}, viewport 0..{view}"
        );
        // Moving up past the top of the visible rows scrolls each one fully in.
        for _ in 0..12 {
            xdo(&["key", "Up"]);
            pump(60);
        }
        pump(200);
        let selected = list.selected_row().unwrap();
        assert_eq!(selected.index(), cmds.len() as i32 - 13);
        let b = selected.compute_bounds(&scroll).unwrap();
        assert!(
            b.y() >= -0.5 && b.y() + b.height() <= scroll.height() as f32 + 0.5,
            "selected row must be fully visible after moving up: {}..{}",
            b.y(),
            b.y() + b.height()
        );
        xdo(&["key", "Escape"]);
        pump(200);
    });
}

/// Centre of terminal cell (`col`, `row`) in terminal widget coordinates.
fn cell(term: &vte4::Terminal, col: usize, row: usize) -> (f64, f64) {
    (
        (col as f64 + 0.5) * term.char_width() as f64,
        (row as f64 + 0.5) * term.char_height() as f64,
    )
}

#[test]
fn file_line_references_are_links_that_open_the_editor() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("file-links");
        // The file the compiler talks about.
        std::fs::create_dir_all(h.root.join("home/src")).unwrap();
        std::fs::write(h.root.join("home/src/model.rs"), "fn main() {}\n").unwrap();
        // A stand-in for VS Code that records how it was called.
        let bin = h.root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let record = h.root.join("editor-args");
        let code = bin.join("code");
        std::fs::write(&code, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", record.display())).unwrap();
        std::fs::set_permissions(&code, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        h.app.inner.borrow_mut().config.editor.command = code.display().to_string();

        // A workspace whose pane runs in that folder.
        let (_, pane) = h.app.new_workspace(Some("proj"), Some(h.root.join("home"))).unwrap();
        let term = h.app.inner.borrow().panes[&pane].term.clone();
        pump(1500);
        term.feed(
            concat!(
                "\x1b[2J\x1b[H",
                "  --> src/model.rs:230:9\r\n",
                "time 12:30:45 ip 10.0.0.1:8080\r\n",
                "  File \"app/main.py\", line 12, in <module>\r\n",
                "see https://example.com/a.rs:12\r\n",
            )
            .as_bytes(),
        );
        pump(300);

        let at = |col, row| {
            let (x, y) = cell(&term, col, row);
            term.check_match_at(x, y).0.map(|s| s.to_string())
        };
        assert_eq!(at(10, 0).as_deref(), Some("src/model.rs:230:9"));
        assert_eq!(at(7, 1), None, "a clock time is not a file reference");
        assert_eq!(at(20, 1), None, "an IP address and port is not a file reference");
        assert_eq!(at(8, 2).as_deref(), Some("File \"app/main.py\", line 12"));
        assert_eq!(at(10, 3).as_deref(), Some("https://example.com/a.rs:12"), "URLs stay URLs");

        // Ctrl+click opens the editor at the line and column.
        let (x, y) = cell(&term, 10, 0);
        let p = term
            .compute_point(&h.app.window, &gtk4::graphene::Point::new(x as f32, y as f32))
            .unwrap();
        xdo(&[
            "mousemove",
            "--window",
            &h.window,
            &(p.x() as i32).to_string(),
            &(p.y() as i32).to_string(),
        ]);
        pump(50);
        xdo(&["keydown", "ctrl"]);
        xdo(&["click", "1"]);
        xdo(&["keyup", "ctrl"]);
        let until = Instant::now() + Duration::from_secs(5);
        while !record.exists() && Instant::now() < until {
            pump(50);
        }
        let args = std::fs::read_to_string(&record).expect("the editor was started");
        let file = h.root.join("home/src/model.rs");
        assert_eq!(args, format!("-g\n{}:230:9\n", file.display()));
    });
}

#[test]
fn workspace_colour_and_icon_show_in_the_sidebar_row() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("ws-look");
        let wid = h.app.inner.borrow().state.workspaces[0].id;
        let row: gtk4::Widget = h.app.sidebar_list.row_at_index(0).unwrap().upcast();
        let stripe = find_by_class(&row, "ws-stripe").expect("row has a colour stripe");
        let icon = find_by_class(&row, "ws-icon")
            .and_then(|w| w.downcast::<gtk4::Image>().ok())
            .expect("row has an icon");
        assert!(!stripe.is_visible() && !icon.is_visible(), "nothing shown by default");

        h.app.set_workspace_look(wid, Some(Some("red")), Some(Some("server"))).unwrap();
        pump(50);
        assert!(stripe.is_visible() && stripe.has_css_class("ws-color-red"));
        assert!(icon.is_visible());
        assert_eq!(icon.icon_name().as_deref(), Some("network-server-symbolic"));
        assert!(icon.has_css_class("ws-color-red"), "the icon takes the colour too");

        // The context menu's "Colour" items run this action.
        h.app.app.activate_action("ws-color", Some(&format!("{wid}:green").to_variant()));
        pump(50);
        assert!(stripe.has_css_class("ws-color-green") && !stripe.has_css_class("ws-color-red"));
        h.app.app.activate_action("ws-icon", Some(&format!("{wid}:").to_variant()));
        h.app.app.activate_action("ws-color", Some(&format!("{wid}:").to_variant()));
        pump(50);
        assert!(!stripe.is_visible() && !icon.is_visible(), "cleared");
        assert_eq!(h.app.inner.borrow().state.workspaces[0].color, None);
    });
}

#[test]
fn bundled_and_theme_css_parse_without_errors() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let _ = gtk4::init();
        let errors = Rc::new(RefCell::new(Vec::new()));
        let check = |name: &str, css: &str| {
            let provider = gtk4::CssProvider::new();
            let e = errors.clone();
            let name = name.to_string();
            provider.connect_parsing_error(move |_, section, err| {
                e.borrow_mut().push(format!("{name}: {section}: {err}"));
            });
            provider.load_from_string(css);
        };
        // The check itself works: a broken rule is reported.
        check("broken", ".x { color: nope(1); }");
        assert_eq!(errors.borrow().len(), 1, "parse errors are caught");
        errors.borrow_mut().clear();

        check("style.css", include_str!("../src/ui/style.css"));
        for theme in oxishell::theme::all(true).into_iter().chain(oxishell::theme::all(false)) {
            check(&theme.id, &theme.css());
            check(&format!("{} at 80% opacity", theme.id), &theme.opacity_css(80));
        }
        assert!(errors.borrow().is_empty(), "{:#?}", errors.borrow());
    });
}

#[test]
fn background_opacity_setting_makes_only_the_background_see_through() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("opacity");
        assert!(h.app.window.has_css_class("oxishell-window"));
        // GTK prints loaded CSS back in its own expanded form: look for the
        // window rule, which only the opacity CSS contains.
        let css = || h.app.theme_css.to_str().to_string();
        assert!(!css().contains(".oxishell-window"), "opaque by default");

        h.app.inner.borrow_mut().config.appearance.opacity = 80;
        h.app.apply_config();
        pump(50);
        let text = css();
        assert!(text.contains(".oxishell-window"), "{text}");
        assert!(text.contains(",0.8)") || text.contains(", 0.8)"), "80% alpha: {text}");

        h.app.inner.borrow_mut().config.appearance.opacity = 100;
        h.app.apply_config();
        pump(50);
        assert!(!css().contains(".oxishell-window"), "back to opaque");
    });
}

/// A one-route HTTP server answering every request with `body`; returns its
/// URL and how many requests it has seen.
fn release_server(body: &'static str) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/latest", listener.local_addr().unwrap());
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (url, hits)
}

#[test]
fn update_check_is_off_by_default_and_announces_a_newer_release_once() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let (url, hits) = release_server(
            r#"{"tag_name":"v99.0.0","html_url":"https://github.com/Ajinkyasp99/oxishell/releases/tag/v99.0.0","draft":false,"prerelease":false}"#,
        );
        std::env::set_var("OXISHELL_UPDATE_URL", &url);
        let h = Harness::new("update-check");
        let count = || hits.load(std::sync::atomic::Ordering::SeqCst);
        let wait_for = |f: &dyn Fn() -> bool| {
            let until = Instant::now() + Duration::from_secs(5);
            while !f() && Instant::now() < until {
                pump(50);
            }
        };

        // Off by default: no request at all.
        h.app.check_for_updates();
        pump(500);
        assert_eq!(count(), 0, "no network request while the setting is off");
        assert!(h.app.update_notice.borrow().is_none());

        h.app.inner.borrow_mut().config.updates.check = true;
        h.app.check_for_updates();
        wait_for(&|| h.app.update_notice.borrow().is_some());
        assert_eq!(count(), 1);
        let title = h.app.update_notice.borrow().as_ref().unwrap().title();
        assert_eq!(title.as_deref(), Some("oxishell 99.0.0 is out"));

        // Within a day: no second request.
        h.app.check_for_updates();
        pump(500);
        assert_eq!(count(), 1, "at most one check a day");
        std::env::remove_var("OXISHELL_UPDATE_URL");
    });
}

// ---- Deep UI checks for the picked features (T1, A4, W4, L2, D4) ----------
//
// Real pointer and keyboard input through xdotool wherever GTK lets a test
// address the widget. Set OXISHELL_SCREENSHOTS=<dir> to save what each step
// looks like.

/// Render `w` to `<$OXISHELL_SCREENSHOTS>/<name>.png` (no-op without the variable).
fn screenshot(w: &impl IsA<gtk4::Widget>, name: &str) {
    let Some(dir) = std::env::var_os("OXISHELL_SCREENSHOTS") else {
        return;
    };
    let w = w.as_ref();
    let paintable = gtk4::WidgetPaintable::new(Some(w));
    let snapshot = gtk4::Snapshot::new();
    paintable.snapshot(&snapshot, w.width() as f64, w.height() as f64);
    let (Some(node), Some(renderer)) = (snapshot.to_node(), w.native().and_then(|n| n.renderer())) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    renderer
        .render_texture(&node, None)
        .save_to_png(PathBuf::from(dir).join(format!("{name}.png")))
        .unwrap();
}

/// `root` and every widget below it.
fn widgets(root: &impl IsA<gtk4::Widget>) -> Vec<gtk4::Widget> {
    fn walk(w: &gtk4::Widget, out: &mut Vec<gtk4::Widget>) {
        out.push(w.clone());
        let mut c = w.first_child();
        while let Some(x) = c {
            walk(&x, out);
            c = x.next_sibling();
        }
    }
    let mut out = Vec::new();
    walk(root.as_ref(), &mut out);
    out
}

/// Text of every visible label below `root`.
fn label_texts(root: &impl IsA<gtk4::Widget>) -> Vec<String> {
    widgets(root)
        .into_iter()
        .filter(|w| w.is_mapped())
        .filter_map(|w| w.downcast::<gtk4::Label>().ok())
        .map(|l| l.label().to_string())
        .collect()
}

/// A program at `<root>/bin/<name>` that writes its arguments, one per line, to `record`.
fn fake_program(h: &Harness, name: &str, record: &std::path::Path) -> PathBuf {
    let bin = h.root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let path = bin.join(name);
    std::fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", record.display())).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    path
}

/// Real click with `button` on terminal cell (`col`, `row`), optionally holding Ctrl.
fn click_cell(h: &Harness, term: &vte4::Terminal, col: usize, row: usize, button: u32, ctrl: bool) {
    let (x, y) = cell(term, col, row);
    let p = term
        .compute_point(&h.app.window, &gtk4::graphene::Point::new(x as f32, y as f32))
        .unwrap();
    xdo(&[
        "mousemove",
        "--window",
        &h.window,
        &(p.x() as i32).to_string(),
        &(p.y() as i32).to_string(),
    ]);
    pump(60);
    if ctrl {
        xdo(&["keydown", "ctrl"]);
    }
    xdo(&["click", &button.to_string()]);
    if ctrl {
        xdo(&["keyup", "ctrl"]);
    }
    pump(300);
}

/// Labels of the first section of the terminal's current context menu.
fn context_menu_head(term: &vte4::Terminal) -> Vec<String> {
    let model: gio::MenuModel = term.property::<Option<gio::MenuModel>>("context-menu-model").unwrap();
    let section = model.item_link(0, gio::MENU_LINK_SECTION).unwrap();
    (0..section.n_items())
        .filter_map(|i| {
            section
                .item_attribute_value(i, "label", Some(glib::VariantTy::STRING))
                .and_then(|v| v.get::<String>())
        })
        .collect()
}

/// Left-click a link cell until its popover is visible (at most 3 tries).
///
/// Under Xvfb the first click after a link popover closes is sometimes
/// swallowed. The same happens with the code before these features (checked
/// on a clean copy of the last commit), so it is not what these tests are
/// about.
fn open_link_popover(h: &Harness, term: &vte4::Terminal, col: usize, row: usize) -> gtk4::Widget {
    let popover = || find_by_class(term.upcast_ref(), "link-popover").expect("link popover");
    for _ in 0..3 {
        click_cell(h, term, col, row, 1, false);
        if popover().is_visible() {
            return popover();
        }
    }
    panic!("no popover after clicking cell {col},{row}");
}

fn wait_until(f: &dyn Fn() -> bool, secs: u64) -> bool {
    let until = Instant::now() + Duration::from_secs(secs);
    while !f() && Instant::now() < until {
        pump(50);
    }
    f()
}

#[test]
fn deep_t1_file_link_popover_menus_copy_and_open_keep_url_links_unchanged() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("t1-deep");
        std::fs::create_dir_all(h.root.join("home/src")).unwrap();
        std::fs::write(h.root.join("home/src/model.rs"), "").unwrap();
        let record = h.root.join("editor-args");
        let code = fake_program(&h, "code", &record);
        h.app.inner.borrow_mut().config.editor.command = code.display().to_string();
        let (_, pane) = h.app.new_workspace(Some("proj"), Some(h.root.join("home"))).unwrap();
        let term = h.app.inner.borrow().panes[&pane].term.clone();
        pump(1500);
        term.feed(b"\x1b[2J\x1b[H  --> src/model.rs:230:9\r\nsee https://example.com/docs\r\nmissing src/nope.rs:3 here\r\n");
        pump(300);
        let file = h.root.join("home/src/model.rs");
        let location = format!("{}:230:9", file.display());
        let popover = || find_by_class(term.upcast_ref(), "link-popover").expect("link popover");

        // Plain click on a file position: the popover offers the editor.
        let texts = label_texts(&open_link_popover(&h, &term, 10, 0));
        for want in [
            "Open in Editor",
            "Copy Path",
            location.as_str(),
            "Tip: Ctrl+Click opens the file directly",
        ] {
            assert!(texts.iter().any(|t| t == want), "missing {want:?} in {texts:?}");
        }
        screenshot(&h.app.window, "t1-1-file-link-popover");
        xdo(&["key", "Escape"]);
        pump(200);
        assert!(!popover().is_visible(), "Escape closes it");

        // URLs keep their own popover (unchanged behaviour).
        let texts = label_texts(&open_link_popover(&h, &term, 8, 1));
        for want in ["Open Link", "Copy Link", "https://example.com/docs"] {
            assert!(texts.iter().any(|t| t == want), "missing {want:?} in {texts:?}");
        }
        screenshot(&h.app.window, "t1-2-url-popover-unchanged");
        xdo(&["key", "Escape"]);
        pump(200);

        // A path to a file that does not exist stays plain text: no popover,
        // even after several clicks.
        for _ in 0..3 {
            click_cell(&h, &term, 12, 2, 1, false);
            assert!(!popover().is_visible(), "no popover for a missing file");
        }

        // Right-click menus put the matching actions on top.
        click_cell(&h, &term, 10, 0, 3, false);
        assert_eq!(context_menu_head(&term), ["Open in Editor", "Copy Path"]);
        screenshot(&h.app.window, "t1-3-file-link-context-menu");
        xdo(&["key", "Escape"]);
        pump(200);
        click_cell(&h, &term, 8, 1, 3, false);
        assert_eq!(context_menu_head(&term), ["Open Link", "Copy Link"]);
        xdo(&["key", "Escape"]);
        pump(200);
        click_cell(&h, &term, 2, 2, 3, false);
        assert_eq!(
            context_menu_head(&term),
            ["Copy", "Paste", "Search…"],
            "plain text: the normal menu"
        );
        xdo(&["key", "Escape"]);
        pump(200);

        // "Copy Path" (the menu's action) copies the full location.
        click_cell(&h, &term, 10, 0, 3, false);
        xdo(&["key", "Escape"]);
        pump(100);
        term.activate_action("link.copy", None).unwrap();
        let copied: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let c = copied.clone();
        term.clipboard().read_text_async(gio::Cancellable::NONE, move |r| {
            *c.borrow_mut() = r.ok().flatten().map(|s| s.to_string());
        });
        assert!(wait_until(&|| copied.borrow().is_some(), 3));
        assert_eq!(copied.borrow().as_deref(), Some(location.as_str()));

        // "Open in Editor" in the popover. The popover is its own X window,
        // which xdotool cannot address, so its button is clicked directly.
        let open = widgets(&open_link_popover(&h, &term, 10, 0))
            .into_iter()
            .filter_map(|w| w.downcast::<gtk4::Button>().ok())
            .find(|b| label_texts(b).iter().any(|t| t == "Open in Editor"))
            .unwrap();
        open.emit_clicked();
        assert!(wait_until(&|| record.exists(), 5), "the editor was started");
        assert_eq!(std::fs::read_to_string(&record).unwrap(), format!("-g\n{location}\n"));
        pump(200);
        assert!(!popover().is_visible(), "the popover closes after choosing");
    });
}

#[test]
fn deep_t1_terminal_editors_open_in_a_new_pane() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("t1-vim");
        std::fs::create_dir_all(h.root.join("home/src")).unwrap();
        std::fs::write(h.root.join("home/src/model.rs"), "").unwrap();
        let record = h.root.join("vim-args");
        let vim = fake_program(&h, "vim", &record);
        h.app.inner.borrow_mut().config.editor.command = vim.display().to_string();
        let (wid, pane) = h.app.new_workspace(Some("proj"), Some(h.root.join("home"))).unwrap();
        let term = h.app.inner.borrow().panes[&pane].term.clone();
        pump(1500);
        term.feed(b"\x1b[2J\x1b[Hsrc/model.rs:42\r\n");
        pump(300);
        let panes = || h.app.inner.borrow().state.workspace(wid).unwrap().layout.panes().len();
        assert_eq!(panes(), 1);
        click_cell(&h, &term, 3, 0, 1, true);
        assert!(wait_until(&|| panes() == 2, 3), "vim gets its own pane");
        assert!(wait_until(&|| record.exists(), 8), "the command ran in the new pane");
        let file = h.root.join("home/src/model.rs");
        assert_eq!(std::fs::read_to_string(&record).unwrap(), format!("+42\n{}\n", file.display()));
        screenshot(&h.app.window, "t1-4-terminal-editor-split");
    });
}

/// With a popover menu open, press Down until the item labelled `text` has
/// keyboard focus, then Return.
fn choose_menu_item(h: &Harness, text: &str) {
    for _ in 0..30 {
        let focused = gtk4::prelude::RootExt::focus(&h.app.window).map(|w| label_texts(&w));
        if focused.as_ref().is_some_and(|t| t.iter().any(|x| x == text)) {
            xdo(&["key", "Return"]);
            pump(300);
            return;
        }
        xdo(&["key", "Down"]);
        pump(60);
    }
    panic!("menu item {text:?} never got focus");
}

fn right_click_row(h: &Harness, index: i32) {
    let row = h.app.sidebar_list.row_at_index(index).unwrap();
    let p = row.compute_point(&h.app.window, &gtk4::graphene::Point::new(60.0, 12.0)).unwrap();
    xdo(&[
        "mousemove",
        "--window",
        &h.window,
        &(p.x() as i32).to_string(),
        &(p.y() as i32).to_string(),
    ]);
    pump(60);
    xdo(&["click", "3"]);
    pump(400);
}

#[test]
fn deep_w4_colour_and_icon_from_the_real_menu_survive_reordering() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("w4-deep");
        let (prod, _) = h.app.new_workspace(Some("prod"), None).unwrap();
        pump(300);
        let ws = |id| h.app.inner.borrow().state.workspace(id).unwrap().clone();

        right_click_row(&h, 1);
        screenshot(&h.app.window, "w4-1-workspace-menu");
        choose_menu_item(&h, "Colour");
        screenshot(&h.app.window, "w4-2-colour-submenu");
        choose_menu_item(&h, "Red");
        assert_eq!(ws(prod).color.as_deref(), Some("red"));

        right_click_row(&h, 1);
        choose_menu_item(&h, "Icon");
        choose_menu_item(&h, "Server");
        assert_eq!(ws(prod).icon.as_deref(), Some("server"));
        let row: gtk4::Widget = h.app.sidebar_list.row_at_index(1).unwrap().upcast();
        assert!(find_by_class(&row, "ws-stripe").unwrap().has_css_class("ws-color-red"));
        screenshot(&h.app.window, "w4-3-red-server-row");

        // Old behaviour: dragging a row reorders workspaces; the look moves with it.
        let center = |w: &gtk4::Widget| {
            let p = w
                .compute_point(&h.app.window, &gtk4::graphene::Point::new(20.0, w.height() as f32 / 2.0))
                .unwrap();
            (p.x() as i32, p.y() as i32)
        };
        let from = center(&row);
        let to = center(&h.app.sidebar_list.row_at_index(0).unwrap().upcast());
        xdo(&["mousemove", "--window", &h.window, &from.0.to_string(), &from.1.to_string()]);
        pump(60);
        xdo(&["mousedown", "1"]);
        let steps = 12;
        for _ in 0..steps {
            xdo(&["mousemove_relative", "--", "0", &((to.1 - from.1) / steps).to_string()]);
            pump(25);
        }
        pump(150);
        xdo(&["mouseup", "1"]);
        pump(500);
        let first = h.app.inner.borrow().state.workspaces[0].clone();
        assert_eq!(first.name, "prod", "dragged to the top");
        assert_eq!((first.color.as_deref(), first.icon.as_deref()), (Some("red"), Some("server")));
        let top: gtk4::Widget = h.app.sidebar_list.row_at_index(0).unwrap().upcast();
        assert!(find_by_class(&top, "ws-stripe").unwrap().has_css_class("ws-color-red"));
        screenshot(&h.app.window, "w4-4-after-reorder");

        // And "None" clears it from the menu.
        right_click_row(&h, 0);
        choose_menu_item(&h, "Colour");
        choose_menu_item(&h, "None");
        assert_eq!(ws(prod).color, None);
        assert!(!find_by_class(&top, "ws-stripe").unwrap().is_visible());
    });
}

/// The settings row titled `title`, on whatever page it is.
fn settings_row<T: IsA<gtk4::Widget>>(dialog: &adw::PreferencesDialog, title: &str) -> T {
    widgets(dialog)
        .into_iter()
        .filter(|w| w.downcast_ref::<adw::PreferencesRow>().is_some_and(|r| r.title() == title))
        .find_map(|w| w.downcast::<T>().ok())
        .unwrap_or_else(|| panic!("no settings row {title:?}"))
}

#[test]
fn deep_settings_rows_for_the_new_options_work_with_the_keyboard() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let (url, hits) =
            release_server(r#"{"tag_name":"v99.0.0","html_url":"https://github.com/Ajinkyasp99/oxishell/releases/tag/v99.0.0"}"#);
        std::env::set_var("OXISHELL_UPDATE_URL", &url);
        let h = Harness::new("settings-deep");
        let config_file = || std::fs::read_to_string(h.root.join("config/oxishell/config.toml")).unwrap();

        // Open Settings with its real shortcut.
        xdo(&["key", "ctrl+comma"]);
        assert!(wait_until(
            &|| find_descendant::<adw::PreferencesDialog>(h.app.window.upcast_ref()).is_some(),
            3
        ));
        let dialog: adw::PreferencesDialog = find_descendant(h.app.window.upcast_ref()).unwrap();

        // Every new option has its row, with the documented defaults.
        let editor: adw::EntryRow = settings_row(&dialog, "Editor command");
        let alerts: adw::SwitchRow = settings_row(&dialog, "Usage limit alerts");
        let updates: adw::SwitchRow = settings_row(&dialog, "Check for updates");
        let opacity: adw::SpinRow = settings_row(&dialog, "Background opacity (%)");
        assert_eq!(editor.text(), "");
        assert!(alerts.is_active());
        assert!(!updates.is_active());
        assert_eq!(opacity.value(), 100.0);
        let composited = gtk4::gdk::Display::default().unwrap().is_composited();
        assert_eq!(opacity.is_sensitive(), composited, "greyed out without a compositor");
        assert_eq!((opacity.adjustment().lower(), opacity.adjustment().upper()), (50.0, 100.0));
        screenshot(&dialog, "settings-1-appearance-opacity");

        // Editor command: type it and press Enter (the apply button).
        dialog.set_visible_page_name("terminal");
        pump(200);
        editor.grab_focus();
        pump(100);
        xdo(&["type", "--delay", "20", "nano"]);
        xdo(&["key", "Return"]);
        pump(300);
        assert_eq!(h.app.inner.borrow().config.editor.command, "nano");
        assert!(config_file().contains("command = \"nano\""), "{}", config_file());
        screenshot(&dialog, "settings-2-terminal-editor");

        // Usage limit alerts: Space toggles it off and it is saved.
        dialog.set_visible_page_name("notifications");
        pump(200);
        alerts.grab_focus();
        pump(100);
        xdo(&["key", "space"]);
        pump(300);
        assert!(!h.app.inner.borrow().config.notifications.usage_alerts);
        assert!(config_file().contains("usage_alerts = false"));

        // Check for updates: turning it on checks right away.
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0, "nothing requested while off");
        updates.grab_focus();
        pump(100);
        xdo(&["key", "space"]);
        assert!(
            wait_until(&|| h.app.update_notice.borrow().is_some(), 5),
            "notice after turning it on"
        );
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(config_file().contains("check = true"));
        screenshot(&dialog, "settings-3-notifications");

        // Opacity: a new value reaches the window CSS at once.
        opacity.set_value(80.0);
        pump(300);
        assert_eq!(h.app.inner.borrow().config.appearance.opacity, 80);
        assert!(h.app.theme_css.to_str().contains(".oxishell-window"));
        dialog.close();
        pump(200);
        std::env::remove_var("OXISHELL_UPDATE_URL");
    });
}

#[test]
fn deep_a4_and_d4_notices_show_in_the_window() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let (url, _) =
            release_server(r#"{"tag_name":"v99.0.0","html_url":"https://github.com/Ajinkyasp99/oxishell/releases/tag/v99.0.0"}"#);
        std::env::set_var("OXISHELL_UPDATE_URL", &url);
        let h = Harness::new("notices-deep");
        let pane = h.app.focused_pane().unwrap();

        // A4: a Claude report past 80% shows up in the notifications panel.
        let usage = oxishell::usage::Usage {
            five_hour: Some(oxishell::usage::Window {
                used_percent: 84.0,
                resets_at: Some(oxishell::usage::now() + 5400),
            }),
            weekly: None,
        };
        h.app.usage_footer.claude.borrow_mut().update(pane, usage);
        h.app.refresh();
        pump(200);
        xdo(&["key", "ctrl+shift+i"]);
        pump(500);
        let panel = h.app.notif_button.popover().unwrap();
        let texts = label_texts(&panel);
        assert!(
            texts.iter().any(|t| t.contains("Claude Code usage at 80%")),
            "notifications panel: {texts:?}"
        );
        assert!(texts.iter().any(|t| t.contains("84% of the 5-hour limit used")), "{texts:?}");
        assert!(
            !texts.iter().any(|t| t.starts_with("closed")),
            "an alert that belongs to no workspace must not say \"closed\": {texts:?}"
        );
        screenshot(&panel, "a4-notifications-panel");
        xdo(&["key", "Escape"]);
        pump(200);

        // D4: the update notice is a toast that stays, with Release notes and close.
        h.app.inner.borrow_mut().config.updates.check = true;
        h.app.check_for_updates();
        assert!(wait_until(&|| h.app.update_notice.borrow().is_some(), 5));
        pump(400);
        let texts = label_texts(&h.app.toasts);
        assert!(texts.iter().any(|t| t == "oxishell 99.0.0 is out"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "Release notes"), "{texts:?}");
        assert_eq!(h.app.update_notice.borrow().as_ref().unwrap().timeout(), 0, "stays until dismissed");
        screenshot(&h.app.window, "d4-update-toast");
        std::env::remove_var("OXISHELL_UPDATE_URL");
    });
}

/// Reproduces a block header drawn over a running program's own UI (seen with
/// Claude Code's input box): the program clears the scrollback (`ESC[3J`), the
/// rows the old blocks sat on are gone, but their headers were still drawn
/// using the row offset measured at the last shell prompt.
#[test]
fn command_block_headers_stay_put_when_a_running_program_clears_the_scrollback() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("blocks-clear");
        let pane = h.app.focused_pane().unwrap();
        let (term, blocks) = {
            let i = h.app.inner.borrow();
            (i.panes[&pane].term.clone(), i.panes[&pane].blocks.clone())
        };
        pump(2500);
        h.point(&term);
        xdo(&["click", "1"]);
        pump(200);
        xdo(&["type", "--delay", "15", "echo first-block"]);
        xdo(&["key", "Return"]);
        pump(1500);
        let height = term.height() as f64;
        let on_screen = || -> Vec<(f64, String)> {
            blocks
                .visible_headers()
                .into_iter()
                .filter(|(y, _)| *y >= 0.0 && *y < height)
                .collect()
        };
        assert!(!on_screen().is_empty(), "the echo block has a header on screen");

        // A program that fills the screen, clears the scrollback and keeps running.
        xdo(&["type", "--delay", "15", "seq 1 400; printf '\\033[3J'; echo PROGRAM-UI; sleep 30"]);
        xdo(&["key", "Return"]);
        pump(3000);
        assert!(term.text_format(vte4::Format::Text).unwrap().contains("PROGRAM-UI"));
        let headers = on_screen();
        assert!(
            headers.is_empty(),
            "headers of commands whose rows were cleared must not be drawn over the program: {headers:?}"
        );
        // Scrolling, redraws and the program's output keep it that way.
        term.feed(b"more program output\r\n");
        pump(300);
        assert!(on_screen().is_empty(), "{:?}", on_screen());
        xdo(&["key", "ctrl+c"]);
        pump(1500);

        // Back at the prompt, the next command's header sits right above it.
        xdo(&["type", "--delay", "15", "echo after-clear"]);
        xdo(&["key", "Return"]);
        pump(1500);
        let ch = term.char_height() as f64;
        let text = term.text_format(vte4::Format::Text).unwrap().to_string();
        let row = text
            .lines()
            .position(|l| l.contains("echo after-clear"))
            .expect("command on screen") as f64;
        let headers = on_screen();
        assert!(
            headers.iter().any(|(y, _)| (*y - (row - 1.0) * ch).abs() <= 2.0),
            "header on the row above the command (row {row}, {ch}px rows): {headers:?}"
        );
    });
}

/// Rows also drop when output passes the scrollback limit while a command is
/// still running (long builds, agents): the same drift must not happen.
#[test]
fn command_block_headers_stay_put_when_output_passes_the_scrollback_limit() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("blocks-limit");
        let pane = h.app.focused_pane().unwrap();
        let (term, blocks) = {
            let i = h.app.inner.borrow();
            (i.panes[&pane].term.clone(), i.panes[&pane].blocks.clone())
        };
        term.set_scrollback_lines(100);
        pump(2500);
        h.point(&term);
        xdo(&["click", "1"]);
        pump(200);
        // Fill the scrollback first, so the next block sits near its end, as
        // it does in any long session.
        xdo(&["type", "--delay", "15", "seq 1 150"]);
        xdo(&["key", "Return"]);
        pump(1200);
        xdo(&["type", "--delay", "15", "echo first-block"]);
        xdo(&["key", "Return"]);
        pump(1500);
        let height = term.height() as f64;
        let on_screen = || -> Vec<(f64, String)> {
            blocks
                .visible_headers()
                .into_iter()
                .filter(|(y, _)| *y >= 0.0 && *y < height)
                .collect()
        };
        assert!(!on_screen().is_empty());
        xdo(&["type", "--delay", "15", "seq 1 500; sleep 30"]);
        xdo(&["key", "Return"]);
        pump(3000);
        let headers = on_screen();
        assert!(headers.is_empty(), "rows past the scrollback limit are gone: {headers:?}");
        xdo(&["key", "ctrl+c"]);
        pump(800);
    });
}
