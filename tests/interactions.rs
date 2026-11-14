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
