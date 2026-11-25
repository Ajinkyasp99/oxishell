//! Real pointer input against the GTK widgets. Run under Xvfb with xdotool.
// Linux only: needs X11/Xvfb, xdotool and /proc.
#![cfg(target_os = "linux")]
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
        // As the app does at startup: without it VTE 0.76 crashes ~30 s after
        // a terminal closes, i.e. in whichever test runs next.
        oxishell::ui::vte_fix::install();
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
        let layout = || h.app.inner.borrow().state.workspace(wid).unwrap().tab().layout.panes();
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
        // The page is the tab bar over the shown tab's split tree.
        let root = find_descendant::<gtk4::Paned>(page.upcast_ref()).unwrap();
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
        let rects = h.app.inner.borrow().state.workspace(wid).unwrap().tab().layout.rects();
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

#[test]
fn history_panel_survives_dismissal_under_the_pointer() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::with_history("history-dismiss", &["echo first", "echo second"]);
        let pane = h.app.focused_pane().unwrap();
        let term = h.app.inner.borrow().panes[&pane].term.clone();
        pump(2500);
        h.point(&term);
        xdo(&["click", "1", "key", "Up"]);
        pump(300);

        let pop = find_by_class(term.upcast_ref(), "history-panel")
            .and_downcast::<gtk4::Popover>()
            .expect("Up opens the history panel");
        assert!(pop.is_mapped());
        let (x, y) = window_point(&h, pop.upcast_ref());
        xdo(&["mousemove", "--window", &h.window, &x.to_string(), &y.to_string()]);
        pump(100);
        let pointer = pop.display().default_seat().unwrap().pointer().unwrap();
        assert_eq!(pointer.surface_at_position().0, pop.surface(), "pointer is over the popup surface");
        xdo(&["key", "Escape"]);
        h.app.window.queue_allocate();
        pump(200);

        assert!(!pop.is_visible());
        assert_eq!(pop.parent().as_ref(), Some(term.upcast_ref()));
        assert!(pop.surface().is_some(), "GTK can still address the last pointer target");
        assert!(term.has_focus());
        xdo(&["key", "Up"]);
        pump(200);
        assert!(pop.is_mapped(), "Up reopens the same popover");
        xdo(&["key", "Tab"]);
        pump(200);
        assert!(!pop.is_visible());
        assert!(term.has_focus());
        assert_eq!(h.app.inner.borrow().panes[&pane].suggest.tracker.borrow().line, "echo second");
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
fn pane_action_icons_are_symbolic() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("symbolic-icons");
        let theme = gtk4::IconTheme::for_display(&gtk4::prelude::WidgetExt::display(&h.app.window));
        for name in ["oxishell-compose-symbolic", "oxishell-pane-move-symbolic", "window-close-symbolic"] {
            assert!(theme.has_icon(name), "pane action icon exists: {name}");
            let icon = theme.lookup_icon(name, &[], 12, 1, gtk4::TextDirection::Ltr, gtk4::IconLookupFlags::empty());
            assert!(
                icon.is_symbolic(),
                "{name} must follow the widget's foreground color; SVG icons require an SVG pixbuf loader (librsvg2-common on Ubuntu)"
            );
        }
    });
}

#[test]
fn system_status_trend_strip_updates_and_fits_a_narrow_window() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("system-status");
        let root = find_by_class(h.app.window.upcast_ref(), "system-status").unwrap();
        let cpu = find_by_class(&root, "system-cpu-value").unwrap().downcast::<gtk4::Label>().unwrap();
        let ram = find_by_class(&root, "system-memory-value")
            .unwrap()
            .downcast::<gtk4::Label>()
            .unwrap();
        let chart = find_by_class(&root, "system-cpu-history").unwrap();
        let capacity = find_by_class(&root, "system-memory-capacity")
            .unwrap()
            .downcast::<gtk4::Label>()
            .unwrap();
        let bar = find_by_class(&root, "system-memory-bar")
            .unwrap()
            .downcast::<gtk4::ProgressBar>()
            .unwrap();
        wait_for(6_000, || cpu.text().ends_with('%'));
        let percent = |label: &gtk4::Label| label.text().trim_end_matches('%').parse::<f64>().unwrap();
        assert!((0.0..=100.0).contains(&percent(&cpu)));
        assert!((0.0..=100.0).contains(&percent(&ram)));
        assert!((percent(&ram) / 100.0 - bar.fraction()).abs() <= 0.0051);
        assert!(capacity.text().ends_with(" GiB"));
        assert!(chart.is_visible());
        assert_eq!(chart.width(), 76);
        assert!(bar.width() <= 54, "RAM meter stays compact: {}", bar.width());
        let status = find_by_class(h.app.window.upcast_ref(), "statusbar").unwrap();
        for (theme, dark) in [("warp_dark", true), ("snowy", false)] {
            h.app.set_theme(theme);
            pump(150);
            assert_eq!(h.app.inner.borrow().theme.is_dark(), dark);
            screenshot(&status, &format!("system-status-{theme}"));
            screenshot(&h.app.window, &format!("system-status-window-{theme}"));
        }
        h.app.window.set_default_size(480, 400);
        pump(300);
        // GTK's client-side decorations can inset the content below 480px.
        assert!(h.app.window.width() <= 480, "the strip must not raise the minimum window width");
        let bounds = root.compute_bounds(&h.app.window).unwrap();
        assert!(
            bounds.x() >= 0.0 && bounds.x() + bounds.width() <= h.app.window.width() as f32,
            "the resource strip fits at minimum window width"
        );
        screenshot(&status, "system-status-narrow");
        screenshot(&h.app.window, "system-status-window-narrow");
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
        let panes = || h.app.inner.borrow().state.workspace(wid).unwrap().tab().layout.panes().len();
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

/// A block header scrolled partly above the terminal was drawn over the chip
/// row above it (the overlay is not clipped by GTK).
#[test]
fn command_block_header_scrolled_above_the_terminal_is_not_drawn_outside_it() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("blocks-overflow");
        let pane = h.app.focused_pane().unwrap();
        let (term, blocks) = {
            let i = h.app.inner.borrow();
            (i.panes[&pane].term.clone(), i.panes[&pane].blocks.clone())
        };
        pump(2500);
        h.point(&term);
        xdo(&["click", "1"]);
        pump(200);
        for cmd in ["echo one", "echo two", "seq 1 200"] {
            xdo(&["type", "--delay", "15", cmd]);
            xdo(&["key", "Return"]);
            pump(1500);
        }
        let adj = term.vadjustment().unwrap();
        let ch = term.char_height() as f64;
        adj.set_value(adj.lower());
        pump(300);
        let top = blocks
            .visible_headers()
            .into_iter()
            .map(|(y, _)| y)
            .find(|y| *y > 0.0)
            .expect("a header below the top of the scrollback");
        // Scroll so that header sits half a row above the terminal.
        adj.set_value(adj.lower() + top + ch / 2.0);
        pump(300);
        let headers = blocks.visible_headers();
        assert!(
            headers.iter().any(|(y, _)| *y < 0.0 && *y > -ch),
            "a header partly above the terminal: {headers:?}"
        );
        let bounds = blocks.drawn_bounds().expect("headers drawn");
        assert!(
            bounds.y() >= 0.0 && bounds.y() + bounds.height() <= blocks.area.height() as f32,
            "drawing stays inside the terminal ({} px high): {bounds:?}",
            blocks.area.height()
        );
    });
}

#[test]
fn hidden_input_and_program_input_never_reach_the_command_history() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("history-secrets");
        let pane = h.app.focused_pane().unwrap();
        let term = h.app.inner.borrow().panes[&pane].term.clone();
        let screen = || term.text_format(vte4::Format::Text).unwrap().to_string();
        // A substring also matches the command as the shell echoes it, before
        // the read/program has finished. Only a complete output line counts.
        let output_line = |text: &str| screen().lines().any(|line| line == text);
        let shell_ready = || {
            let i = h.app.inner.borrow();
            let p = &i.panes[&pane];
            p.pid
                .get()
                .is_some_and(|pid| oxishell::meta::foreground_pgrp(p.pty_fd()) == Some(pid))
                && p.shell_reads_commands()
        };
        let typed = |text: &str| {
            xdo(&["type", "--delay", "15", text]);
            xdo(&["key", "Return"]);
        };
        wait_for(5000, shell_ready);
        h.point(&term);
        xdo(&["click", "1"]);
        pump(200);

        // A shell builtin reading with echo off: no child process owns the terminal.
        typed("read -s SECRET; echo READ-DONE");
        wait_for(5000, || {
            let i = h.app.inner.borrow();
            let p = &i.panes[&pane];
            !p.has_running_child() && oxishell::meta::canonical_mode(p.pty_fd()) == Some(true)
        });
        typed("builtin-secret-123");
        wait_for(5000, || output_line("READ-DONE"));
        wait_for(5000, shell_ready);
        // A program asking for a password (the foreground is the program).
        typed("python3 -c 'import getpass; getpass.getpass(); print(\"PY-DONE\")'");
        wait_for(5000, || screen().lines().any(|line| line.trim_end() == "Password:"));
        typed("child-secret-456");
        wait_for(5000, || output_line("PY-DONE"));
        wait_for(5000, shell_ready);
        // Visible input to a program is not a shell command either.
        typed("python3 -c 'input(\"Name: \"); print(\"IN-DONE\")'");
        wait_for(5000, || screen().lines().any(|line| line.trim_end() == "Name:"));
        typed("program-answer-789");
        wait_for(5000, || output_line("IN-DONE"));
        // Back at the prompt (the line editor has the terminal again), ordinary
        // commands are still recorded.
        wait_for(5000, shell_ready);
        typed("echo visible-command");
        wait_for(5000, || output_line("visible-command"));
        pump(300);

        let i = h.app.inner.borrow();
        for secret in ["builtin-secret-123", "child-secret-456", "program-answer-789"] {
            assert!(i.history.search(secret, 5).is_empty(), "{secret} leaked into history");
        }
        let recent = i.history.search("", 10);
        assert!(recent.contains(&"echo visible-command"), "{recent:?}");
        assert!(recent.contains(&"read -s SECRET; echo READ-DONE"), "{recent:?}");
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

/// Pump until `f` is true; panic after `ms` milliseconds.
#[track_caller]
fn wait_for(ms: u64, f: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_millis(ms);
    while !f() {
        assert!(Instant::now() < until, "condition not met within {ms} ms");
        pump(50);
    }
}

/// A `docker` stand-in: records its arguments, answers `compose version`, prints
/// `ps.json` for `ps`, and stays in the foreground for `up`, `up --build` and
/// `logs -f` until Ctrl+C. Returns (script, record file, ps.json).
fn fake_docker(h: &Harness) -> (PathBuf, PathBuf, PathBuf) {
    let bin = h.root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let record = h.root.join("docker-args");
    let ps = h.root.join("ps.json");
    std::fs::write(&ps, "").unwrap();
    let docker = bin.join("docker");
    std::fs::write(
        &docker,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{rec}'\necho \"$* | P=${{COMPOSE_PROJECT_NAME-unset}} | D=$(pwd)\" >> '{rec}.env'\ncase \"$*\" in\n  'compose version') exit 0;;\n  *' ps --all --format json') cat '{ps}'; exit 0;;\n  *' up -d') sleep 0.3;;\n  *' up'|*' up --build'|*' logs -f') trap 'exit 130' INT; while :; do sleep 0.1; done;;\nesac\nexit 0\n",
            rec = record.display(),
            ps = ps.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&docker, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    h.app.inner.borrow_mut().config.compose.command = format!("{} compose", docker.display());
    (docker, record, ps)
}

#[test]
fn panes_learn_the_repos_compose_files_and_busy_state() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-scan");
        fake_docker(&h);
        std::fs::write(h.root.join("home/docker-compose.yml"), "services:\n  api: {}\n  web: {}\n").unwrap();
        let (_, pane) = h.app.new_workspace(Some("shop"), Some(h.root.join("home"))).unwrap();
        wait_for(8_000, || h.app.inner.borrow().panes[&pane].compose.scan.is_some());
        {
            let i = h.app.inner.borrow();
            let scan = i.panes[&pane].compose.scan.as_ref().unwrap();
            assert_eq!(scan.files.len(), 1);
            assert_eq!(scan.files[0].services, Some(vec!["api".into(), "web".into()]));
            assert!(scan.command.is_some());
        }
        assert!(!h.app.inner.borrow().panes[&pane].busy);
        let term = h.app.inner.borrow().panes[&pane].term.clone();
        term.feed_child(b"sleep 30\n");
        wait_for(8_000, || h.app.inner.borrow().panes[&pane].busy);
    });
}

#[test]
fn choosing_a_compose_file_runs_it_in_the_pane_and_disables_the_button() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-run");
        let (_, record, _) = fake_docker(&h);
        std::fs::write(h.root.join("home/docker-compose.yml"), "services:\n  api: {}\n").unwrap();
        let (_, pane) = h.app.new_workspace(Some("shop"), Some(h.root.join("home"))).unwrap();
        wait_for(8_000, || h.app.inner.borrow().panes[&pane].compose.button.is_visible());
        let button = h.app.inner.borrow().panes[&pane].compose.button.clone();
        assert!(button.is_sensitive());
        assert_eq!(button.tooltip_text().as_deref(), Some("Compose files in this repo (Ctrl+Shift+L)"));

        let file = h.root.join("home/docker-compose.yml");
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::Up);
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        wait_for(8_000, || ran().contains(" up"));
        assert!(ran().contains("compose -f docker-compose.yml up"), "{}", ran());
        wait_for(8_000, || !button.is_sensitive());
        assert_eq!(button.tooltip_text().as_deref(), Some("Busy: wait for the command to finish"));
    });
}

#[test]
fn the_launcher_never_runs_together_with_unfinished_prompt_input() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-prompt");
        let (_, record, _) = fake_docker(&h);
        std::fs::write(h.root.join("home/docker-compose.yml"), "services:\n  api: {}\n").unwrap();
        let (_, pane) = h.app.new_workspace(Some("shop"), Some(h.root.join("home"))).unwrap();
        wait_for(8_000, || h.app.inner.borrow().panes[&pane].compose.button.is_visible());
        let term = h.app.inner.borrow().panes[&pane].term.clone();
        let screen = || term.text_format(vte4::Format::Text).unwrap().to_string();
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        let file = h.root.join("home/docker-compose.yml");
        pump(2500);
        h.point(&term);
        xdo(&["click", "1"]);
        pump(200);

        // Unfinished input: running the launcher now would execute it too.
        let marker = h.root.join("prefix-ran");
        xdo(&["type", "--delay", "15", &format!("touch {}; ", marker.display())]);
        pump(300);
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::Up);
        pump(1500);
        assert!(!marker.exists(), "the unfinished prefix was executed");
        assert!(!ran().contains(" up"), "{}", ran());
        assert!(
            !screen().contains("compose -f"),
            "nothing appended to the user's input:\n{}",
            screen()
        );
        assert!(screen().contains("prefix-ran; "), "the user's input stays as it was");

        // Once the prompt is empty, it runs.
        xdo(&["key", "ctrl+u"]);
        pump(300);
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::Up);
        wait_for(8_000, || ran().contains(" up"));
        assert!(!marker.exists());
    });
}

#[test]
fn a_typed_but_unsent_launch_is_not_typed_twice() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-twice");
        let (_, record, _) = fake_docker(&h);
        h.app.inner.borrow_mut().config.compose.run_immediately = false;
        std::fs::write(h.root.join("home/docker-compose.yml"), "services:\n  api: {}\n").unwrap();
        let (_, pane) = h.app.new_workspace(Some("shop"), Some(h.root.join("home"))).unwrap();
        wait_for(8_000, || h.app.inner.borrow().panes[&pane].compose.button.is_visible());
        let term = h.app.inner.borrow().panes[&pane].term.clone();
        let file = h.root.join("home/docker-compose.yml");
        pump(2500);
        let up = oxishell::compose::Action::Up;
        oxishell::ui::compose_ui::run(&h.app, pane, &file, up.clone());
        pump(500);
        oxishell::ui::compose_ui::run(&h.app, pane, &file, up);
        pump(500);
        let text = term.text_format(vte4::Format::Text).unwrap().to_string();
        assert_eq!(text.matches("compose -f").count(), 1, "{text}");
        h.point(&term);
        xdo(&["click", "1"]);
        pump(200);
        xdo(&["key", "Return"]);
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        wait_for(8_000, || ran().contains(" up"));
        assert!(ran().contains("compose -f docker-compose.yml up\n"), "{}", ran());
    });
}

#[test]
fn no_compose_files_means_no_button() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-none");
        fake_docker(&h);
        let (_, pane) = h.app.new_workspace(Some("empty"), Some(h.root.join("home"))).unwrap();
        wait_for(8_000, || h.app.inner.borrow().panes[&pane].compose.scan.is_some());
        pump(300);
        assert!(!h.app.inner.borrow().panes[&pane].compose.button.is_visible());
        h.app.run_action("compose-files");
        pump(200);
        assert!(find_by_class(h.app.window.upcast_ref(), "compose-picker").is_none());
    });
}

#[test]
fn compose_picker_opens_from_the_shortcut_action_and_works_from_the_keyboard() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-picker");
        let (_, record, _) = fake_docker(&h);
        std::fs::create_dir_all(h.root.join("home/proj/services/pay")).unwrap();
        std::fs::write(h.root.join("home/proj/docker-compose.yml"), "services:\n  api: {}\n  web: {}\n").unwrap();
        std::fs::write(h.root.join("home/proj/services/pay/compose.yaml"), "services:\n  pay: {}\n").unwrap();
        let (_, pane) = h.app.new_workspace(Some("shop"), Some(h.root.join("home/proj"))).unwrap();
        wait_for(8_000, || h.app.inner.borrow().panes[&pane].compose.button.is_visible());
        let button = h.app.inner.borrow().panes[&pane].compose.button.clone();

        h.app.run_action("compose-files");
        pump(400);
        let pop = find_by_class(button.upcast_ref(), "compose-picker").expect("the picker opened");
        assert!(pop.is_visible());
        let list: gtk4::ListBox = find_descendant(&pop).unwrap();
        // Folder headers: the repo root's files first, then each folder's files under it.
        let groups: Vec<String> = widgets(&pop)
            .into_iter()
            .filter(|w| w.has_css_class("compose-group"))
            .filter_map(|w| w.downcast::<gtk4::Label>().ok())
            .map(|l| l.label().to_string())
            .collect();
        assert_eq!(groups, ["▾ repo root", "▾ services/pay/"]);
        let selected = || list.selected_row().map(|r| r.widget_name().to_string());
        assert_eq!(selected().as_deref(), Some("docker-compose.yml"), "the first file is selected");
        screenshot(&pop, "compose-picker");
        screenshot(&h.app.window, "compose-pane");

        // Filtering hides folders with no matching file.
        let entry: gtk4::SearchEntry = find_descendant(&pop).unwrap();
        let shown_groups = || {
            widgets(&list)
                .into_iter()
                .filter_map(|w| w.downcast::<gtk4::ListBoxRow>().ok())
                .filter(|r| r.is_child_visible() && r.widget_name().starts_with("group:"))
                .map(|r| r.widget_name().to_string())
                .collect::<Vec<_>>()
        };
        entry.set_text("pay");
        pump(400);
        assert_eq!(shown_groups(), ["group:services/pay/"]);
        assert_eq!(selected().as_deref(), Some("services/pay/compose.yaml"));
        entry.set_text("");
        pump(400);
        assert_eq!(shown_groups().len(), 2);
        assert_eq!(selected().as_deref(), Some("docker-compose.yml"));
        assert!(
            entry.state_flags().contains(gtk4::StateFlags::FOCUS_WITHIN),
            "the filter has the keyboard from the start"
        );

        xdo(&["key", "--window", &h.window, "Down"]);
        pump(200);
        assert_eq!(
            selected().as_deref(),
            Some("services/pay/compose.yaml"),
            "Down moves to the next file, skipping the folder header"
        );
        // Tab moves to the first action chip (up -d); Enter runs it.
        xdo(&["key", "--window", &h.window, "Tab"]);
        pump(200);
        xdo(&["key", "--window", &h.window, "Return"]);
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        wait_for(8_000, || ran().contains(" up -d"));
        assert!(ran().contains("compose -f services/pay/compose.yaml up -d"), "{}", ran());
        assert!(!pop.is_visible(), "the picker closes after an action");
    });
}

#[test]
fn compose_picker_keeps_keyboard_selection_fully_visible() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-picker-scroll");
        fake_docker(&h);
        let files: Vec<String> = (0..24).map(|n| format!("group-{}/compose-{n:02}.yaml", n / 8)).collect();
        for rel in &files {
            let file = h.root.join("home").join(rel);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "services:\n  api: {}\n").unwrap();
        }
        let (_, pane) = h.app.new_workspace(Some("shop"), Some(h.root.join("home"))).unwrap();
        wait_for(8_000, || h.app.inner.borrow().panes[&pane].compose.button.is_visible());
        let button = h.app.inner.borrow().panes[&pane].compose.button.clone();
        h.app.run_action("compose-files");
        pump(400);
        let pop = find_by_class(button.upcast_ref(), "compose-picker").unwrap();
        let scroll: gtk4::ScrolledWindow = find_descendant(&pop).unwrap();
        let list: gtk4::ListBox = find_descendant(scroll.upcast_ref()).unwrap();
        let entry: gtk4::SearchEntry = find_descendant(&pop).unwrap();
        assert!(
            scroll.vadjustment().upper() > scroll.vadjustment().page_size(),
            "the file list must overflow"
        );
        let visible = |index: usize| {
            let row = list.selected_row().unwrap();
            assert_eq!(row.widget_name().as_str(), files[index]);
            let bounds = row.compute_bounds(&scroll.child().unwrap()).unwrap();
            let bottom = bounds.y() + bounds.height();
            let height = scroll.vadjustment().page_size() as f32;
            assert!(
                bounds.y() >= -0.5 && bottom <= height + 0.5,
                "selected file {} must be fully visible: {}..{bottom}, viewport 0..{height}",
                files[index],
                bounds.y()
            );
        };
        visible(0);
        // Arrows skip headers, scroll in both directions and wrap at either end.
        // They also work while typing a filter, without taking its keyboard focus.
        for in_filter in [true] {
            for (key, indices) in [
                ("Down", (1..files.len()).collect::<Vec<_>>()),
                ("Up", (0..files.len() - 1).rev().collect()),
                ("Up", vec![files.len() - 1]),
                ("Down", vec![0]),
            ] {
                for index in indices {
                    xdo(&["key", key]);
                    pump(80);
                    visible(index);
                    assert_eq!(entry.state_flags().contains(gtk4::StateFlags::FOCUS_WITHIN), in_filter);
                }
            }
        }
        // Filtering to a file near the end, then clearing, must reveal the new selection.
        entry.set_text("compose-23");
        pump(400);
        visible(23);
        entry.set_text("");
        pump(400);
        visible(0);
        for (key, index) in [("Up", 23), ("Down", 0), ("KP_Up", 23), ("KP_Down", 0)] {
            xdo(&["key", key]);
            pump(80);
            visible(index);
        }
        xdo(&["key", "Escape"]);
        pump(100);
    });
}

/// A compose file declaring `service-number-0` .. `service-number-{n-1}`.
fn declare_services(n: usize) -> String {
    (0..n).fold("services:\n".to_string(), |y, i| y + &format!("  service-number-{i}: {{}}\n"))
}

const PS_API_RUNNING: &str = r#"{"Service":"api","State":"running","Health":"","ExitCode":0,"Publishers":[{"URL":"0.0.0.0","TargetPort":8000,"PublishedPort":8000,"Protocol":"tcp"}]}"#;

/// A workspace in `home` with one compose file whose launcher button is showing.
fn compose_workspace(h: &Harness, name: &str) -> (oxishell::model::PaneId, PathBuf) {
    let file = h.root.join("home/docker-compose.yml");
    std::fs::write(&file, "services:\n  api: {}\n").unwrap();
    let (_, pane) = h.app.new_workspace(Some(name), Some(h.root.join("home"))).unwrap();
    wait_for(8_000, || h.app.inner.borrow().panes[&pane].compose.button.is_visible());
    (pane, file)
}

#[test]
fn an_old_crash_notifications_restart_never_acts_on_a_newer_stack() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-old-crash");
        let (_, record, ps) = fake_docker(&h);
        std::fs::create_dir_all(h.root.join("home/other")).unwrap();
        let file_b = h.root.join("home/other/compose.yaml");
        std::fs::write(&file_b, "services:\n  api: {}\n").unwrap();
        let (pane, file_a) = compose_workspace(&h, "shop");
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        let ready = || h.app.inner.borrow().panes[&pane].prompt_is_empty();
        let stack_file = || h.app.inner.borrow().panes[&pane].compose.stack.as_ref().map(|s| s.file.clone());
        let seen_running = || {
            h.app.inner.borrow().panes[&pane]
                .compose
                .stack
                .as_ref()
                .is_some_and(|s| s.seen_running)
        };
        let crash_notes = || -> Vec<u64> {
            let i = h.app.inner.borrow();
            i.notes
                .all()
                .iter()
                .filter(|n| n.title == "api exited with code 1")
                .map(|n| n.id)
                .collect()
        };
        let exited = r#"{"Service":"api","State":"exited","Health":"","ExitCode":1,"Publishers":[]}"#;
        let detached = oxishell::compose::Action::UpDetached;

        // Stack A crashes and its notification offers "Restart api".
        wait_for(8_000, ready);
        oxishell::ui::compose_ui::run(&h.app, pane, &file_a, detached.clone());
        std::fs::write(&ps, PS_API_RUNNING).unwrap();
        wait_for(8_000, seen_running);
        std::fs::write(&ps, exited).unwrap();
        wait_for(8_000, || crash_notes().len() == 1);
        let old = crash_notes()[0];

        // Stack B now runs from the same pane.
        std::fs::write(&ps, PS_API_RUNNING).unwrap();
        wait_for(8_000, ready);
        oxishell::ui::compose_ui::run(&h.app, pane, &file_b, detached);
        wait_for(8_000, || stack_file().as_deref() == Some(file_b.as_path()));
        wait_for(8_000, seen_running);

        // A's old button must not restart B's service.
        h.app.compose_notification_action(old, "restart");
        pump(2_000);
        assert!(!ran().contains(" restart"), "{}", ran());

        // B's own crash notification still restarts B.
        std::fs::write(&ps, exited).unwrap();
        wait_for(8_000, || crash_notes().len() == 2);
        let new = *crash_notes().iter().find(|&&n| n != old).unwrap();
        h.app.compose_notification_action(new, "restart");
        wait_for(8_000, || ran().contains(" restart"));
        let b_restart = format!("-f {} restart api", file_b.display());
        assert!(ran().contains(&b_restart), "{}", ran());
    });
}

#[test]
fn status_and_bar_actions_use_the_environment_and_folder_the_stack_was_started_with() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-context");
        let (_, record, ps) = fake_docker(&h);
        let env_log = PathBuf::from(format!("{}.env", record.display()));
        let (pane, file) = compose_workspace(&h, "shop");
        let sub = h.root.join("home/sub");
        std::fs::create_dir_all(&sub).unwrap();
        let term = h.app.inner.borrow().panes[&pane].term.clone();
        let ready = || h.app.inner.borrow().panes[&pane].prompt_is_empty();
        let log = || std::fs::read_to_string(&env_log).unwrap_or_default();
        pump(2500);
        h.point(&term);
        xdo(&["click", "1"]);
        pump(200);
        // Only the pane's shell has this variable and this folder.
        xdo(&["type", "--delay", "15", "export COMPOSE_PROJECT_NAME=review-stack; cd sub"]);
        xdo(&["key", "Return"]);
        pump(800);
        wait_for(8_000, ready);

        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::UpDetached);
        std::fs::write(&ps, PS_API_RUNNING).unwrap();
        wait_for(10_000, || {
            h.app.inner.borrow().panes[&pane]
                .compose
                .stack
                .as_ref()
                .is_some_and(|s| s.seen_running)
        });
        oxishell::ui::compose_ui::run_background(&h.app, pane, oxishell::compose::Action::Stop);
        wait_for(8_000, || log().contains(" stop |"));

        let context = format!("P=review-stack | D={}", sub.display());
        let log_text = log();
        let up = log_text.lines().find(|l| l.contains(" up -d |")).expect("up -d ran");
        assert!(up.ends_with(&context), "{log_text}");
        let ps_lines: Vec<&str> = log_text.lines().filter(|l| l.contains(" ps --all ")).collect();
        assert!(!ps_lines.is_empty());
        for l in &ps_lines {
            assert!(l.ends_with(&context), "status polled outside the stack's context: {l}\n{log_text}");
        }
        let stop = log_text.lines().find(|l| l.contains(" stop |")).unwrap();
        assert!(stop.ends_with(&context), "{log_text}");

        // A compose program found by a later scan does not replace the one that started the stack.
        let idle = || h.app.inner.borrow().panes[&pane].compose.stack.as_ref().is_some_and(|s| !s.working);
        wait_for(8_000, idle);
        h.app.inner.borrow_mut().config.compose.command = "/nonexistent/docker compose".into();
        oxishell::ui::compose_ui::rescan(&h.app, pane);
        pump(1_000);
        oxishell::ui::compose_ui::run_background(&h.app, pane, oxishell::compose::Action::Restart);
        wait_for(8_000, || log().contains(" restart |"));
        let restart = log().lines().find(|l| l.contains(" restart |")).unwrap().to_string();
        assert!(restart.ends_with(&context), "{restart}");
    });
}

#[test]
fn stack_bar_follows_ps_and_stop_runs_in_the_background() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-bar");
        let (_, record, ps) = fake_docker(&h);
        let (pane, file) = compose_workspace(&h, "shop");
        let bar = h.app.inner.borrow().panes[&pane].compose.bars().stack_bar.clone();
        assert!(!bar.is_visible());

        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::Up);
        pump(4_000);
        assert!(bar.is_visible(), "no containers yet but the pane is busy: the bar stays");

        std::fs::write(&ps, PS_API_RUNNING).unwrap();
        wait_for(8_000, || {
            h.app.inner.borrow().panes[&pane]
                .compose
                .stack
                .as_ref()
                .is_some_and(|s| s.seen_running)
        });
        screenshot(&h.app.window, "compose-stack-bar");

        oxishell::ui::compose_ui::run_background(&h.app, pane, oxishell::compose::Action::Stop);
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        wait_for(8_000, || ran().contains(" stop"));
        assert!(h.app.inner.borrow().panes[&pane].busy, "stop does not type into the pane");

        std::fs::write(&ps, "").unwrap();
        wait_for(8_000, || !bar.is_visible());
    });
}

#[test]
fn stop_logs_sends_ctrl_c_and_hides_the_logs_line() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-logs");
        let (_, record, _) = fake_docker(&h);
        let (pane, file) = compose_workspace(&h, "shop");
        let logs_bar = h.app.inner.borrow().panes[&pane].compose.bars().logs_bar.clone();

        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::Logs);
        wait_for(8_000, || logs_bar.is_visible());
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        wait_for(8_000, || ran().contains(" logs -f"));
        pump(500);

        oxishell::ui::compose_ui::stop_logs(&h.app, pane);
        wait_for(8_000, || !h.app.inner.borrow().panes[&pane].busy);
        wait_for(8_000, || !logs_bar.is_visible());
        assert!(!ran().contains(" stop") && !ran().contains(" down"), "{}", ran());
    });
}

#[test]
fn a_crashing_service_is_reported_once() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-crash");
        let (_, _, ps) = fake_docker(&h);
        let (pane, file) = compose_workspace(&h, "shop");
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::Up);
        std::fs::write(&ps, PS_API_RUNNING).unwrap();
        wait_for(8_000, || {
            h.app.inner.borrow().panes[&pane]
                .compose
                .stack
                .as_ref()
                .is_some_and(|s| s.seen_running)
        });
        let crashed = || {
            h.app
                .inner
                .borrow()
                .notes
                .all()
                .iter()
                .filter(|n| n.title == "api exited with code 1")
                .count()
        };
        let exited = r#"{"Service":"api","State":"exited","Health":"","ExitCode":1,"Publishers":[]}"#;
        std::fs::write(&ps, exited).unwrap();
        wait_for(8_000, || crashed() == 1);
        pump(7_000);
        assert_eq!(crashed(), 1, "one notification per crash");

        h.app.inner.borrow_mut().config.compose.notify_on_crash = false;
        std::fs::write(&ps, PS_API_RUNNING).unwrap();
        pump(4_000);
        std::fs::write(&ps, exited).unwrap();
        pump(7_000);
        assert_eq!(crashed(), 1, "notify_on_crash = false sends nothing");
    });
}

#[test]
fn compose_bars_are_built_only_when_a_stack_starts() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-lazy-bars");
        let (_, _, ps) = fake_docker(&h);
        let (pane, file) = compose_workspace(&h, "shop");
        let clips = || {
            let i = h.app.inner.borrow();
            let c = &i.panes[&pane].compose;
            (c.stack_clip.clone(), c.logs_clip.clone())
        };
        let (stack_clip, logs_clip) = clips();
        assert!(stack_clip.first_child().is_none(), "a fresh pane builds no stack bar");
        assert!(logs_clip.first_child().is_none(), "a fresh pane builds no logs bar");
        assert!(!stack_clip.is_visible() && !logs_clip.is_visible());

        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::Up);
        std::fs::write(&ps, PS_API_RUNNING).unwrap();
        wait_for(8_000, || {
            h.app.inner.borrow().panes[&pane]
                .compose
                .stack
                .as_ref()
                .is_some_and(|s| s.seen_running)
        });
        wait_for(8_000, || stack_clip.first_child().is_some_and(|b| b.is_visible()));
        assert!(stack_clip.is_visible(), "the clip shows with its bar");
        assert!(
            stack_clip.first_child().unwrap().has_css_class("compose-stack"),
            "the stack clip holds the stack bar"
        );
        assert!(
            logs_clip.first_child().is_some_and(|b| b.has_css_class("compose-logs")),
            "the logs bar is built with the stack bar"
        );
        assert!(!logs_clip.is_visible(), "no logs are followed, so the logs bar stays hidden");
    });
}

#[test]
fn rebuild_stops_the_attached_up_then_runs_up_build_in_the_pane() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-rebuild");
        let (_, record, _) = fake_docker(&h);
        let (pane, file) = compose_workspace(&h, "shop");
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::Up);
        wait_for(8_000, || ran().ends_with(" up\n"));
        pump(500);

        oxishell::ui::compose_ui::rebuild(&h.app, pane);
        wait_for(15_000, || ran().contains(" up --build"));
        let log = ran();
        assert!(
            log.find(" up\n").unwrap() < log.find(" up --build").unwrap(),
            "up --build runs after the attached up: {log}"
        );
        wait_for(8_000, || h.app.inner.borrow().panes[&pane].busy);
        let i = h.app.inner.borrow();
        let stack = i.panes[&pane].compose.stack.as_ref().expect("the stack bar stays");
        assert!(stack.attached);
    });
}

#[test]
fn rebuild_with_another_program_in_the_pane_opens_a_split() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-rebuild-split");
        let (_, record, ps) = fake_docker(&h);
        let (pane, file) = compose_workspace(&h, "shop");
        std::fs::write(&ps, PS_API_RUNNING).unwrap();
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::UpDetached);
        wait_for(8_000, || {
            h.app.inner.borrow().panes[&pane]
                .compose
                .stack
                .as_ref()
                .is_some_and(|s| s.seen_running && !s.attached)
        });
        let term = h.app.inner.borrow().panes[&pane].term.clone();
        term.feed_child(b"sleep 30\n");
        wait_for(8_000, || h.app.inner.borrow().panes[&pane].busy);
        let before = h.app.inner.borrow().panes.len();

        oxishell::ui::compose_ui::rebuild(&h.app, pane);
        wait_for(8_000, || h.app.inner.borrow().panes.len() == before + 1);
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        wait_for(15_000, || ran().contains(" up --build"));
        assert!(h.app.inner.borrow().panes[&pane].busy, "sleep still runs in the first pane");
    });
}

#[test]
fn turning_the_launcher_off_hides_everything_and_stops_nothing() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-off");
        let (_, record, ps) = fake_docker(&h);
        let (pane, file) = compose_workspace(&h, "shop");
        std::fs::write(&ps, PS_API_RUNNING).unwrap();
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::UpDetached);
        let (button, bar) = {
            let i = h.app.inner.borrow();
            let bar = i.panes[&pane].compose.bars().stack_bar.clone();
            (i.panes[&pane].compose.button.clone(), bar)
        };
        wait_for(8_000, || bar.is_visible() && button.is_sensitive());

        h.app.inner.borrow_mut().config.compose.enabled = false;
        h.app.apply_config();
        pump(300);
        assert!(!button.is_visible() && !bar.is_visible());
        h.app.run_action("compose-files");
        pump(200);
        assert!(find_by_class(button.upcast_ref(), "compose-picker").is_none());
        pump(3_500);
        let ran = std::fs::read_to_string(&record).unwrap_or_default();
        assert!(!ran.contains(" stop") && !ran.contains(" down"), "{ran}");

        h.app.inner.borrow_mut().config.compose.enabled = true;
        h.app.apply_config();
        pump(300);
        assert!(button.is_visible() && bar.is_visible());
    });
}

#[test]
fn compose_bars_do_not_widen_the_pane() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-resize");
        let (_, _, ps) = fake_docker(&h);
        let (pane, file) = compose_workspace(&h, "shop");
        let frame = h.app.inner.borrow().panes[&pane].frame.clone();
        let min_w = || frame.measure(gtk4::Orientation::Horizontal, -1).0;
        let min_h = || frame.measure(gtk4::Orientation::Vertical, -1).0;
        let (w0, h0) = (min_w(), min_h());

        // Many services with ports make the chip row and status line long.
        let many: String = (0..12)
            .map(|i| format!(r#"{{"Service":"service-number-{i}","State":"running","Health":"","ExitCode":0,"Publishers":[{{"URL":"0.0.0.0","TargetPort":80,"PublishedPort":{},"Protocol":"tcp"}}]}}"#, 8000 + i))
            .collect::<Vec<_>>()
            .join("\n");
        // The bar shows only the services the file declares.
        std::fs::write(&file, declare_services(12)).unwrap();
        std::fs::write(&ps, many).unwrap();
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::UpDetached);
        let (bar, logs_bar) = {
            let i = h.app.inner.borrow();
            let bars = i.panes[&pane].compose.bars();
            (bars.stack_bar.clone(), bars.logs_bar.clone())
        };
        wait_for(8_000, || {
            h.app.inner.borrow().panes[&pane]
                .compose
                .stack
                .as_ref()
                .is_some_and(|s| s.services.len() == 12)
        });
        wait_for(8_000, || {
            !h.app.inner.borrow().panes[&pane].term.pty().is_none() && !h.app.inner.borrow().panes[&pane].has_running_child()
        });
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::Logs);
        wait_for(8_000, || logs_bar.is_visible());
        pump(300);
        assert!(bar.is_visible());
        let (w1, h1) = (min_w(), min_h());
        eprintln!("pane minimum: before {w0}x{h0}, with bars {w1}x{h1}");
        assert!(w1 <= w0, "the bars must not raise the pane's minimum width ({w0} -> {w1})");
        // Height depends on how the chips wrap at the pane's real width; the
        // wrap test checks the bar takes only what its rows need.
        let _ = (h0, h1);
    });
}

#[test]
fn compose_stack_bar_wraps_its_chips_and_keeps_its_buttons_in_view_in_a_narrow_pane() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-wrap");
        let (_, _, ps) = fake_docker(&h);
        let (pane, file) = compose_workspace(&h, "shop");
        // Two panes side by side: the launcher pane is about half the window.
        h.app.split(Some(pane), oxishell::model::Orient::Horizontal, None).unwrap();
        let many: String = (0..12)
            .map(|i| format!(r#"{{"Service":"service-number-{i}","State":"running","Health":"","ExitCode":0,"Publishers":[{{"URL":"0.0.0.0","TargetPort":80,"PublishedPort":{},"Protocol":"tcp"}}]}}"#, 8000 + i))
            .collect::<Vec<_>>()
            .join("\n");
        // The bar shows only the services the file declares.
        std::fs::write(&file, declare_services(12)).unwrap();
        std::fs::write(&ps, many).unwrap();
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::UpDetached);
        wait_for(8_000, || {
            h.app.inner.borrow().panes[&pane]
                .compose
                .stack
                .as_ref()
                .is_some_and(|s| s.services.len() == 12)
        });
        pump(500);
        let (clip, frame, bar) = {
            let i = h.app.inner.borrow();
            let c = &i.panes[&pane].compose;
            let bar = c.bars().stack_bar.clone();
            (c.stack_clip.clone(), i.panes[&pane].frame.clone(), bar)
        };
        pump(800);
        screenshot(&h.app.window, "compose-stack-bar-narrow");
        let check = |label: &str| {
            let width = clip.width() as f32;
            let inside = |w: &gtk4::Widget| {
                let b = w.compute_bounds(&clip).unwrap();
                b.x() >= 0.0 && b.x() + b.width() <= width + 1.0
            };
            let all = widgets(&clip);
            let chips: Vec<_> = all.iter().filter(|w| w.has_css_class("compose-chip")).collect();
            assert_eq!(chips.len(), 12, "{label}");
            for c in &chips {
                assert!(inside(c), "{label}: chip {:?} is cut off", c.compute_bounds(&clip));
            }
            let stop = all.iter().find(|w| w.has_css_class("compose-stop")).expect("stop button");
            assert!(inside(stop), "{label}: [stop] is out of view: {:?}", stop.compute_bounds(&clip));
            let rows: std::collections::BTreeSet<i32> = chips.iter().map(|c| c.compute_bounds(&clip).unwrap().y() as i32).collect();
            assert!(rows.len() > 1, "{label}: chips wrap onto more lines");
            let needed = bar.measure(gtk4::Orientation::Vertical, clip.width()).1;
            assert!(
                clip.height() <= needed + 4,
                "{label}: the bar takes only the height its wrapped rows need ({} > {needed})",
                clip.height()
            );
            width
        };
        let half = check("half width");
        assert!(half < frame.root().unwrap().width() as f32 * 0.75, "pane is narrow: {half}");

        // Narrower still: split the launcher pane again (about a quarter of the window).
        h.app.split(Some(pane), oxishell::model::Orient::Horizontal, None).unwrap();
        pump(800);
        screenshot(&h.app.window, "compose-stack-bar-quarter");
        let quarter = check("quarter width");
        assert!(quarter < half * 0.75, "pane got narrower: {half} -> {quarter}");
    });
}

#[test]
fn typing_a_filter_in_the_compose_picker_narrows_the_list() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-filter-typed");
        let (_, record, _) = fake_docker(&h);
        std::fs::create_dir_all(h.root.join("home/proj/services/pay")).unwrap();
        std::fs::write(h.root.join("home/proj/docker-compose.yml"), "services:\n  api: {}\n").unwrap();
        std::fs::write(h.root.join("home/proj/services/pay/compose.yaml"), "services:\n  pay: {}\n").unwrap();
        let (_, pane) = h.app.new_workspace(Some("shop"), Some(h.root.join("home/proj"))).unwrap();
        wait_for(8_000, || h.app.inner.borrow().panes[&pane].compose.button.is_visible());
        let button = h.app.inner.borrow().panes[&pane].compose.button.clone();
        h.app.run_action("compose-files");
        pump(400);
        let pop = find_by_class(button.upcast_ref(), "compose-picker").expect("the picker opened");
        let list: gtk4::ListBox = find_descendant(&pop).unwrap();
        let entry: gtk4::SearchEntry = find_descendant(&pop).unwrap();
        let visible_files = || {
            widgets(&list)
                .into_iter()
                .filter_map(|w| w.downcast::<gtk4::ListBoxRow>().ok())
                .filter(|r| r.is_child_visible() && !r.widget_name().starts_with("group:"))
                .map(|r| r.widget_name().to_string())
                .collect::<Vec<_>>()
        };

        // Real keys, as a user types them: the filter has the keyboard already.
        xdo(&["type", "--window", &h.window, "--delay", "40", "pay"]);
        pump(600);
        assert_eq!(entry.text().as_str(), "pay", "the letters go into the filter");
        assert_eq!(visible_files(), ["services/pay/compose.yaml"]);
        assert!(pop.is_visible(), "typing letters does not run actions");
        assert!(!std::fs::read_to_string(&record).unwrap_or_default().contains(" logs"));

        xdo(&["key", "--window", &h.window, "Return"]);
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        wait_for(8_000, || ran().contains(" up"));
        assert!(ran().contains("compose -f services/pay/compose.yaml up"), "{}", ran());
        pump(300);
        let term = h.app.inner.borrow().panes[&pane].term.clone();
        assert!(term.has_focus(), "typing goes back to the terminal after the picker closes");
    });
}

#[test]
fn compose_picker_shows_last_starts_service_matches_and_chips_for_the_selected_file() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-picker-history");
        let (_, record, _) = fake_docker(&h);
        std::fs::create_dir_all(h.root.join("home/proj/services/pay")).unwrap();
        std::fs::write(h.root.join("home/proj/docker-compose.yml"), "services:\n  api: {}\n  redis: {}\n").unwrap();
        std::fs::write(h.root.join("home/proj/services/pay/compose.yaml"), "services:\n  pay: {}\n").unwrap();
        let (_, pane) = h.app.new_workspace(Some("shop"), Some(h.root.join("home/proj"))).unwrap();
        wait_for(8_000, || h.app.inner.borrow().panes[&pane].compose.button.is_visible());
        wait_for(8_000, || h.app.inner.borrow().panes[&pane].compose.scan.is_some());
        let button = h.app.inner.borrow().panes[&pane].compose.button.clone();

        // One earlier start of the root file, one of a file in another repo.
        let root_file = h.app.inner.borrow().panes[&pane].compose.scan.as_ref().unwrap().files[0]
            .path
            .clone();
        let now = oxishell::history::now_secs();
        let mut starts = Vec::new();
        oxishell::compose::record_start(
            &mut starts,
            std::path::Path::new("/elsewhere/compose.yml"),
            &oxishell::compose::Action::Up,
            now - 50,
        );
        oxishell::compose::record_start(&mut starts, &root_file, &oxishell::compose::Action::UpDetached, now - 240);
        oxishell::compose::save_starts(&starts);

        h.app.run_action("compose-files");
        pump(400);
        let pop = find_by_class(button.upcast_ref(), "compose-picker").expect("the picker opened");
        let list: gtk4::ListBox = find_descendant(&pop).unwrap();
        let entry: gtk4::SearchEntry = find_descendant(&pop).unwrap();

        // Last starts: only this repo's, with the action it ran.
        let pills: Vec<String> = widgets(&pop)
            .into_iter()
            .filter(|w| w.has_css_class("compose-pill"))
            .flat_map(|b| widgets(&b))
            .filter_map(|w| w.downcast::<gtk4::Label>().ok())
            .map(|l| l.label().to_string())
            .collect();
        assert_eq!(pills.len(), 1, "{pills:?}");
        assert!(pills[0].contains("docker-compose") && pills[0].contains("up -d"), "{pills:?}");

        // Only the selected file shows its action chips.
        let shown_chips = || {
            widgets(&list)
                .into_iter()
                .filter(|w| w.has_css_class("compose-chips") && w.is_child_visible() && w.is_visible())
                .count()
        };
        assert_eq!(shown_chips(), 1);

        // The filter finds a file by one of its service names.
        xdo(&["type", "--window", &h.window, "--delay", "40", "redis"]);
        pump(600);
        assert_eq!(entry.text().as_str(), "redis");
        let visible_files = widgets(&list)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk4::ListBoxRow>().ok())
            .filter(|r| r.is_child_visible() && !r.widget_name().starts_with("group:"))
            .map(|r| r.widget_name().to_string())
            .collect::<Vec<_>>();
        assert_eq!(visible_files, ["docker-compose.yml"]);

        // The picker's own chord runs `down` for the selected file.
        xdo(&["key", "--window", &h.window, "ctrl+x"]);
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        wait_for(8_000, || ran().contains(" down"));
        assert!(ran().contains("compose -f docker-compose.yml down"), "{}", ran());
        assert!(!pop.is_visible(), "the picker closes after an action");
        // `down` is not a start: the saved list is unchanged.
        assert_eq!(oxishell::compose::load_starts().len(), 2);
    });
}

#[test]
fn the_typed_compose_path_follows_the_shells_real_folder() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-stale-cwd");
        let (_, record, _) = fake_docker(&h);
        std::fs::write(h.root.join("home/docker-compose.yml"), "services:\n  api: {}\n").unwrap();
        std::fs::create_dir_all(h.root.join("home/services/pay")).unwrap();
        let file = h.root.join("home/services/pay/compose.yaml");
        std::fs::write(&file, "services:\n  pay: {}\n").unwrap();
        let (_, pane) = h.app.new_workspace(Some("shop"), Some(h.root.join("home"))).unwrap();
        wait_for(8_000, || h.app.inner.borrow().panes[&pane].compose.button.is_visible());

        // The shell is in `home`, but the pane's recorded folder is stale (the
        // meta worker refreshes it only every few seconds).
        h.app.inner.borrow_mut().panes.get_mut(&pane).unwrap().cwd = Some(h.root.join("home/services/pay"));
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::UpDetached);
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        wait_for(8_000, || ran().contains(" up -d"));
        assert!(ran().contains("compose -f services/pay/compose.yaml up -d"), "{}", ran());
    });
}

/// Where the middle of `w` is in the main X window (the coordinates xdotool's
/// `--window` uses): includes the window's client-side shadow, and works for a
/// widget inside a popover (a separate surface placed relative to the window).
fn window_point(h: &Harness, w: &gtk4::Widget) -> (i32, i32) {
    let mid = gtk4::graphene::Point::new(w.width() as f32 / 2.0, w.height() as f32 / 2.0);
    let native = w.native().expect("widget is shown");
    let native_widget: gtk4::Widget = native.clone().upcast();
    let p = w.compute_point(&native_widget, &mid).expect("widget is in its native");
    let (tx, ty) = native.surface_transform();
    let (mut x, mut y) = (p.x() as f64 + tx, p.y() as f64 + ty);
    if native_widget != *h.app.window.upcast_ref::<gtk4::Widget>() {
        let popup = native
            .surface()
            .unwrap()
            .dynamic_cast::<gtk4::gdk::Popup>()
            .expect("a popup surface");
        x += popup.position_x() as f64;
        y += popup.position_y() as f64;
    }
    (x.round() as i32, y.round() as i32)
}

/// Click the middle of `w` with the real pointer.
fn click_widget(h: &Harness, w: &impl IsA<gtk4::Widget>) {
    let (x, y) = window_point(h, w.as_ref());
    xdo(&["mousemove", "--window", &h.window, &x.to_string(), &y.to_string()]);
    pump(80);
    xdo(&["click", "1"]);
    pump(150);
}

/// The first widget under `root` with CSS class `class` whose label contains `text`.
fn button_with(root: &impl IsA<gtk4::Widget>, class: &str, text: &str) -> Option<gtk4::Button> {
    widgets(root)
        .into_iter()
        .filter(|w| w.has_css_class(class))
        .filter_map(|w| w.downcast::<gtk4::Button>().ok())
        .find(|b| b.label().is_some_and(|l| l.contains(text)))
}

#[test]
fn a_single_service_crash_keeps_its_red_chip_and_clicking_it_restarts_the_service() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-crash-chip");
        let (_, record, ps) = fake_docker(&h);
        let (pane, file) = compose_workspace(&h, "shop");
        std::fs::write(&ps, PS_API_RUNNING).unwrap();
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::UpDetached);
        let bar = h.app.inner.borrow().panes[&pane].compose.bars().stack_bar.clone();
        wait_for(8_000, || button_with(&bar, "compose-chip", "api").is_some());
        assert!(
            button_with(&bar, "compose-chip", "api").unwrap().has_css_class("compose-up"),
            "a running service's chip is green"
        );

        std::fs::write(
            &ps,
            r#"{"Service":"api","State":"exited","Health":"","ExitCode":1,"Publishers":[]}"#,
        )
        .unwrap();
        wait_for(8_000, || {
            button_with(&bar, "compose-chip", "api").is_some_and(|c| c.has_css_class("compose-down"))
        });
        pump(7_000);
        assert!(bar.is_visible(), "the bar stays after the only service crashed");
        let chip = button_with(&bar, "compose-chip", "api").expect("the red api chip");
        assert!(chip.label().unwrap().starts_with('✗'));

        click_widget(&h, &chip);
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        wait_for(8_000, || ran().contains(" restart api"));
    });
}

#[test]
fn the_stack_bar_shows_only_the_services_its_file_declares() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-own-services");
        let (_, _, ps) = fake_docker(&h);
        let (pane, file) = compose_workspace(&h, "shop");
        // `ps` lists the whole project: another compose file in the same
        // folder started `postgres`, which this file does not declare.
        let postgres = r#"{"Service":"postgres","State":"exited","Health":"","ExitCode":0,"Publishers":[]}"#;
        std::fs::write(&ps, format!("{PS_API_RUNNING}\n{postgres}\n")).unwrap();
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::UpDetached);
        let bar = h.app.inner.borrow().panes[&pane].compose.bars().stack_bar.clone();
        wait_for(8_000, || button_with(&bar, "compose-chip", "api").is_some());
        pump(500);
        assert!(
            button_with(&bar, "compose-chip", "postgres").is_none(),
            "postgres is another file's service"
        );
        assert_eq!(h.app.inner.borrow().panes[&pane].compose.stack.as_ref().unwrap().services.len(), 1);
    });
}

#[test]
fn clicking_stop_sends_no_crash_alert_for_killed_containers_and_removes_the_bar() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-stop-137");
        let (_, record, ps) = fake_docker(&h);
        let (pane, file) = compose_workspace(&h, "shop");
        std::fs::write(&ps, PS_API_RUNNING).unwrap();
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::UpDetached);
        let bar = h.app.inner.borrow().panes[&pane].compose.bars().stack_bar.clone();
        wait_for(8_000, || button_with(&bar, "compose-chip", "api").is_some());
        pump(5_500); // past the start-up grace period

        let stop = button_with(&bar, "compose-bar-btn", "[stop]").expect("[stop]");
        click_widget(&h, &stop);
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        wait_for(8_000, || ran().contains(" stop"));
        // The service ignored SIGTERM and was killed: exit 137.
        std::fs::write(
            &ps,
            r#"{"Service":"api","State":"exited","Health":"","ExitCode":137,"Publishers":[]}"#,
        )
        .unwrap();
        wait_for(8_000, || !bar.is_visible());
        let alerts = h
            .app
            .inner
            .borrow()
            .notes
            .all()
            .iter()
            .filter(|n| n.title.contains("exited"))
            .count();
        assert_eq!(alerts, 0, "a stop the user asked for is not a crash");
    });
}

#[test]
fn a_stopped_stack_leaves_a_run_again_strip_for_two_commands() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-rerun");
        let (_, record, ps) = fake_docker(&h);
        let (pane, file) = compose_workspace(&h, "shop");
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        let starts = || ran().lines().filter(|l| l.ends_with(" up -d")).count();
        let exited = r#"{"Service":"api","State":"exited","Health":"","ExitCode":137,"Publishers":[]}"#;
        let (term, bar, strip) = {
            let i = h.app.inner.borrow();
            let term = i.panes[&pane].term.clone();
            let bars = i.panes[&pane].compose.bars();
            let (bar, strip) = (bars.stack_bar.clone(), bars.rerun_bar.clone());
            drop(bars);
            (term, bar, strip)
        };
        let strip_text = || {
            widgets(&strip)
                .into_iter()
                .filter(|w| w.has_css_class("compose-rerun-text"))
                .filter_map(|w| w.downcast::<gtk4::Label>().ok())
                .map(|l| l.label().to_string())
                .next()
                .unwrap_or_default()
        };
        // Wait on state, not on fixed pauses or on effects of an earlier round:
        // a slow runner (CI) must not see the previous round's ` stop`, click a
        // button before it can act, or type before the shell reads commands.
        let ready = || h.app.inner.borrow().panes[&pane].prompt_is_empty();
        let seen_running = || {
            h.app.inner.borrow().panes[&pane]
                .compose
                .stack
                .as_ref()
                .is_some_and(|s| s.seen_running && !s.capturing)
        };
        let stops = || ran().lines().filter(|l| l.ends_with(" stop")).count();
        let commands = || h.app.inner.borrow().panes[&pane].compose.stopped.as_ref().map(|s| s.commands);
        let stop_and_wait_for_strip = |round: usize| {
            std::fs::write(&ps, PS_API_RUNNING).unwrap();
            wait_for(8_000, seen_running);
            wait_for(8_000, || button_with(&bar, "compose-chip", "api").is_some());
            pump(5_500); // past the start-up grace period
            let stop = button_with(&bar, "compose-bar-btn", "[stop]").expect("[stop]");
            wait_for(2_000, || stop.is_mapped() && stop.is_sensitive());
            click_widget(&h, &stop);
            wait_for(8_000, || stops() == round);
            std::fs::write(&ps, exited).unwrap();
            wait_for(8_000, || !bar.is_visible());
            wait_for(2_000, || strip.is_visible());
        };

        wait_for(8_000, ready);
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::UpDetached);
        stop_and_wait_for_strip(1);
        assert_eq!(strip_text(), "docker-compose.yml  stopped just now · 1 service");
        screenshot(&h.app.window, "compose-rerun-strip");

        // [up -d] starts the same stack again: the strip makes way for the bar.
        let up_d = button_with(&strip, "compose-bar-btn", "[up -d]").expect("[up -d]");
        wait_for(8_000, ready);
        wait_for(2_000, || up_d.is_mapped() && up_d.is_sensitive() && up_d.width() > 0);
        click_widget(&h, &up_d);
        wait_for(8_000, || starts() == 2);
        assert!(!strip.is_visible(), "the stack runs again: no strip");
        assert!(h.app.inner.borrow().panes[&pane].compose.stopped.is_none());

        stop_and_wait_for_strip(2);
        assert_eq!(commands(), Some(0));
        wait_for(8_000, ready);
        h.point(&term);
        xdo(&["click", "1"]);
        wait_for(2_000, || term.has_focus());
        xdo(&["type", "--delay", "15", "echo one"]);
        xdo(&["key", "Return"]);
        wait_for(4_000, || commands() == Some(1));
        wait_for(8_000, ready);
        xdo(&["key", "Return"]); // an empty line is not a command
        pump(500);
        wait_for(8_000, ready);
        assert_eq!(commands(), Some(1), "an empty line is not a command");
        assert!(strip.is_visible(), "one command run: the strip stays");
        xdo(&["type", "--delay", "15", "echo two"]);
        xdo(&["key", "Return"]);
        wait_for(4_000, || !strip.is_visible());
        assert!(h.app.inner.borrow().panes[&pane].compose.stopped.is_none());
        assert_eq!(starts(), 2, "typing commands starts nothing");
    });
}

#[test]
fn with_run_immediately_off_pressing_enter_starts_the_stack_bar() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("compose-no-auto-run");
        let (_, record, ps) = fake_docker(&h);
        let (pane, file) = compose_workspace(&h, "shop");
        h.app.inner.borrow_mut().config.compose.run_immediately = false;
        std::fs::write(&ps, PS_API_RUNNING).unwrap();
        let (term, bar) = {
            let i = h.app.inner.borrow();
            let bar = i.panes[&pane].compose.bars().stack_bar.clone();
            (i.panes[&pane].term.clone(), bar)
        };
        term.grab_focus();
        pump(200);
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();

        // Typed, then cancelled with Ctrl+C: nothing starts.
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::UpDetached);
        pump(300);
        assert!(!ran().contains(" up"), "only typed, not run");
        xdo(&["key", "--window", &h.window, "ctrl+c"]);
        pump(200);
        xdo(&["key", "--window", &h.window, "Return"]);
        pump(1_500);
        assert!(h.app.inner.borrow().panes[&pane].compose.stack.is_none(), "cancelled: no bar");

        // Typed, then Enter pressed by the user: it runs and the bar appears.
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::UpDetached);
        pump(300);
        xdo(&["key", "--window", &h.window, "Return"]);
        wait_for(8_000, || ran().contains(" up -d"));
        wait_for(8_000, || bar.is_visible());
        wait_for(8_000, || button_with(&bar, "compose-chip", "api").is_some());
    });
}

// ---- Compose launcher, driven only by real mouse clicks and key presses ----
//
// These tests use the launcher the way a person does: they never call the
// launcher's functions directly. A bug that only shows with real input (focus,
// key routing, layout) must fail here.

/// A workspace in `home/proj` (a project folder: the automatic scan of `$HOME` itself is shallow) with the given compose files (relative path, YAML),
/// once its launcher button shows. Returns the pane and the button.
fn ui_launcher_setup(h: &Harness, files: &[(&str, &str)]) -> (oxishell::model::PaneId, gtk4::Button) {
    for (rel, yaml) in files {
        let p = h.root.join("home/proj").join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, yaml).unwrap();
    }
    let (_, pane) = h.app.new_workspace(Some("shop"), Some(h.root.join("home/proj"))).unwrap();
    wait_for(8_000, || h.app.inner.borrow().panes[&pane].compose.button.is_visible());
    let (button, term) = {
        let i = h.app.inner.borrow();
        (i.panes[&pane].compose.button.clone(), i.panes[&pane].term.clone())
    };
    term.grab_focus();
    pump(200);
    (pane, button)
}

fn press(h: &Harness, key: &str) {
    xdo(&["key", "--window", &h.window, key]);
    pump(200);
}

fn type_keys(h: &Harness, text: &str) {
    xdo(&["type", "--window", &h.window, "--delay", "30", text]);
    pump(300);
}

/// The open picker under `button`, if it is showing.
fn open_picker(button: &gtk4::Button) -> Option<gtk4::Widget> {
    find_by_class(button.upcast_ref(), "compose-picker").filter(|p| p.is_visible())
}

/// Names (relative paths) of the file rows the picker shows.
fn picker_files(pop: &gtk4::Widget) -> Vec<String> {
    widgets(pop)
        .into_iter()
        .filter_map(|w| w.downcast::<gtk4::ListBoxRow>().ok())
        .filter(|r| r.is_child_visible() && !r.widget_name().starts_with("group:"))
        .map(|r| r.widget_name().to_string())
        .collect()
}

fn picker_selected(pop: &gtk4::Widget) -> Option<String> {
    let list: gtk4::ListBox = find_descendant(pop).unwrap();
    list.selected_row().map(|r| r.widget_name().to_string())
}

const UI_FILES: &[(&str, &str)] = &[
    ("docker-compose.yml", "services:\n  api: {}\n  web: {}\n"),
    ("docker-compose-dev.yml", "services:\n  api: {}\n"),
    ("services/pay/compose.yaml", "services:\n  pay: {}\n"),
];

#[test]
fn ui_launcher_clicking_the_button_opens_the_picker_and_esc_gives_typing_back_to_the_shell() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("ui-launcher-click-esc");
        fake_docker(&h);
        let (pane, button) = ui_launcher_setup(&h, UI_FILES);
        click_widget(&h, &button);
        pump(300);
        let pop = open_picker(&button).expect("a click opens the picker");
        assert_eq!(
            picker_selected(&pop).as_deref(),
            Some("docker-compose-dev.yml"),
            "first file selected"
        );
        screenshot(&pop, "ui-launcher-picker");

        press(&h, "Escape");
        assert!(open_picker(&button).is_none(), "Esc closes the picker");
        type_keys(&h, "echo typed-after-esc");
        press(&h, "Return");
        let term = h.app.inner.borrow().panes[&pane].term.clone();
        wait_for(5_000, || {
            term.text_format(vte4::Format::Text).unwrap().matches("typed-after-esc").count() >= 2
        });
        // Open and close it again and again (mouse and keyboard), then type.
        for _ in 0..3 {
            // A real hand always moves a little between clicks; synthetic
            // input with zero motion does not reach the window again.
            xdo(&["mousemove_relative", "--", "40", "40"]);
            pump(100);
            click_widget(&h, &button);
            assert!(open_picker(&button).is_some(), "reopens");
            press(&h, "Escape");
            press(&h, "ctrl+shift+l");
            assert!(open_picker(&button).is_some(), "reopens from the shortcut");
            press(&h, "Escape");
        }
        type_keys(&h, "echo still-typing");
        press(&h, "Return");
        wait_for(5_000, || {
            term.text_format(vte4::Format::Text).unwrap().matches("still-typing").count() >= 2
        });
    });
}

#[test]
fn ui_launcher_shortcut_opens_the_picker_and_the_arrow_keys_skip_folder_headers() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("ui-launcher-keys");
        fake_docker(&h);
        let (_, button) = ui_launcher_setup(&h, UI_FILES);
        press(&h, "ctrl+shift+l");
        pump(200);
        let pop = open_picker(&button).expect("Ctrl+Shift+L opens the picker");
        let sel = || picker_selected(&pop);
        assert_eq!(sel().as_deref(), Some("docker-compose-dev.yml"));
        press(&h, "Down");
        assert_eq!(sel().as_deref(), Some("docker-compose.yml"));
        press(&h, "Down");
        assert_eq!(
            sel().as_deref(),
            Some("services/pay/compose.yaml"),
            "skips the services/pay/ header"
        );
        press(&h, "Down");
        assert_eq!(sel().as_deref(), Some("docker-compose-dev.yml"), "wraps to the top");
        press(&h, "Up");
        assert_eq!(sel().as_deref(), Some("services/pay/compose.yaml"), "wraps to the bottom");
        press(&h, "Up");
        assert_eq!(sel().as_deref(), Some("docker-compose.yml"));
    });
}

#[test]
fn ui_launcher_filter_no_match_does_nothing_and_backspace_brings_the_list_back() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("ui-launcher-filter");
        let (_, record, _) = fake_docker(&h);
        let (_, button) = ui_launcher_setup(&h, UI_FILES);
        press(&h, "ctrl+shift+l");
        let pop = open_picker(&button).expect("picker");
        type_keys(&h, "zzz");
        pump(400);
        assert!(picker_files(&pop).is_empty(), "nothing matches");
        press(&h, "Return");
        assert!(open_picker(&button).is_some(), "Enter with no match keeps the picker open");
        assert!(
            !std::fs::read_to_string(&record).unwrap_or_default().contains(" up"),
            "and runs nothing"
        );
        for _ in 0..3 {
            press(&h, "BackSpace");
        }
        pump(400);
        assert_eq!(picker_files(&pop).len(), 3, "the full list is back");
        type_keys(&h, "dev");
        pump(400);
        assert_eq!(picker_files(&pop), ["docker-compose-dev.yml"]);
        press(&h, "Down");
        assert_eq!(picker_selected(&pop).as_deref(), Some("docker-compose-dev.yml"));
    });
}

#[test]
fn ui_launcher_row_buttons_clicked_with_the_mouse_run_their_action() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("ui-launcher-row-buttons");
        let (_, record, _) = fake_docker(&h);
        let (_, button) = ui_launcher_setup(&h, UI_FILES);
        click_widget(&h, &button);
        let pop = open_picker(&button).expect("picker");
        // Only the selected file shows its action chips: select the pay file first.
        press(&h, "Down");
        press(&h, "Down");
        assert_eq!(picker_selected(&pop).as_deref(), Some("services/pay/compose.yaml"));
        let row = widgets(&pop)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk4::ListBoxRow>().ok())
            .find(|r| r.widget_name() == "services/pay/compose.yaml")
            .unwrap();
        let down = widgets(&row)
            .into_iter()
            .filter(|w| w.has_css_class("compose-chip-btn"))
            .find(|w| w.tooltip_text().is_some_and(|t| t == "down"))
            .expect("down chip");
        click_widget(&h, &down);
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        wait_for(8_000, || ran().contains(" down"));
        assert!(ran().contains("compose -f services/pay/compose.yaml down"), "{}", ran());
        assert!(open_picker(&button).is_none(), "the picker closes");
    });
}

#[test]
fn ui_launcher_a_busy_pane_disables_the_button_and_the_shortcut() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("ui-launcher-busy");
        fake_docker(&h);
        let (pane, button) = ui_launcher_setup(&h, UI_FILES);
        type_keys(&h, "sleep 30");
        press(&h, "Return");
        wait_for(8_000, || !button.is_sensitive());
        assert_eq!(button.tooltip_text().as_deref(), Some("Busy: wait for the command to finish"));
        click_widget(&h, &button);
        assert!(open_picker(&button).is_none(), "a click on the disabled button does nothing");
        press(&h, "ctrl+shift+l");
        assert!(open_picker(&button).is_none(), "nor does the shortcut");
        press(&h, "ctrl+c");
        wait_for(8_000, || button.is_sensitive());
        assert!(!h.app.inner.borrow().panes[&pane].busy);
    });
}

#[test]
fn ui_launcher_stack_bar_buttons_and_chips_work_with_the_mouse() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("ui-launcher-bar-clicks");
        let (_, record, ps) = fake_docker(&h);
        let (pane, button) = ui_launcher_setup(&h, UI_FILES);
        std::fs::write(&ps, PS_API_RUNNING).unwrap();
        // Start detached from the keyboard: open, move to docker-compose.yml, `d`.
        press(&h, "ctrl+shift+l");
        assert!(open_picker(&button).is_some());
        press(&h, "Down");
        // Tab moves to the first action chip (up -d); Enter runs it.
        press(&h, "Tab");
        press(&h, "Return");
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        wait_for(8_000, || ran().contains("compose -f docker-compose.yml up -d"));
        let bar = h.app.inner.borrow().panes[&pane].compose.bars().stack_bar.clone();
        wait_for(8_000, || button_with(&bar, "compose-chip", "api").is_some());
        screenshot(&h.app.window, "ui-launcher-stack-bar");

        let restart = button_with(&bar, "compose-bar-btn", "[restart]").unwrap();
        // Each bar action has its own colour: rebuild accent, restart yellow, stop red.
        assert!(restart.has_css_class("compose-restart"));
        assert!(button_with(&bar, "compose-bar-btn", "[rebuild]")
            .unwrap()
            .has_css_class("compose-rebuild"));
        assert!(button_with(&bar, "compose-bar-btn", "[stop]")
            .unwrap()
            .has_css_class("compose-stop"));
        click_widget(&h, &restart);
        wait_for(8_000, || ran().lines().any(|l| l.ends_with(" restart")));
        wait_for(8_000, || restart.is_sensitive());
        let chip = button_with(&bar, "compose-chip", "api").unwrap();
        click_widget(&h, &chip);
        wait_for(8_000, || ran().contains(" restart api"));
        let stop = button_with(&bar, "compose-bar-btn", "[stop]").unwrap();
        wait_for(8_000, || stop.is_sensitive());
        click_widget(&h, &stop);
        wait_for(8_000, || ran().lines().any(|l| l.ends_with(" stop")));
        std::fs::write(&ps, "").unwrap();
        wait_for(10_000, || !bar.is_visible());
    });
}

#[test]
fn ui_launcher_logs_line_stop_logs_clicked_with_the_mouse() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("ui-launcher-logs");
        let (_, record, _) = fake_docker(&h);
        let (pane, button) = ui_launcher_setup(&h, UI_FILES);
        press(&h, "ctrl+shift+l");
        assert!(open_picker(&button).is_some());
        press(&h, "ctrl+l");
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        wait_for(8_000, || ran().contains(" logs -f"));
        let logs_bar = h.app.inner.borrow().panes[&pane].compose.bars().logs_bar.clone();
        wait_for(8_000, || logs_bar.is_visible());
        let stop_logs = button_with(&logs_bar, "compose-bar-btn", "[stop logs]").unwrap();
        click_widget(&h, &stop_logs);
        wait_for(8_000, || !h.app.inner.borrow().panes[&pane].busy);
        wait_for(8_000, || !logs_bar.is_visible());
        assert!(!ran().contains(" stop\n") && !ran().contains(" down"), "{}", ran());
        wait_for(8_000, || button.is_sensitive());
    });
}

#[test]
fn ui_launcher_dragging_the_divider_narrows_a_pane_that_shows_the_stack_bar() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("ui-launcher-drag");
        let (_, record, ps) = fake_docker(&h);
        let (pane, button) = ui_launcher_setup(&h, UI_FILES);
        let many: String = (0..10)
            .map(|i| format!(r#"{{"Service":"service-number-{i}","State":"running","Health":"","ExitCode":0,"Publishers":[{{"URL":"0.0.0.0","TargetPort":80,"PublishedPort":{},"Protocol":"tcp"}}]}}"#, 8000 + i))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&ps, many).unwrap();
        press(&h, "ctrl+shift+l");
        assert!(open_picker(&button).is_some());
        press(&h, "Tab");
        press(&h, "Return");
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        wait_for(8_000, || ran().contains(" up -d"));
        // The bar shows only the services the started file declares.
        let started = h.app.inner.borrow().panes[&pane].compose.stack.as_ref().unwrap().file.clone();
        std::fs::write(started, declare_services(10)).unwrap();
        let (bar, frame) = {
            let i = h.app.inner.borrow();
            let bar = i.panes[&pane].compose.bars().stack_bar.clone();
            (bar, i.panes[&pane].frame.clone())
        };
        wait_for(8_000, || {
            bar.is_visible() && button_with(&bar, "compose-chip", "service-number-9").is_some()
        });
        // Split with the real shortcut, then drag the divider far to the left.
        press(&h, "ctrl+shift+e");
        pump(800);
        let paned = frame
            .ancestor(gtk4::Paned::static_type())
            .unwrap()
            .downcast::<gtk4::Paned>()
            .unwrap();
        let before = frame.width();
        let handle = paned_handle(&paned);
        let (x, y) = window_point(&h, &handle);
        xdo(&["mousemove", "--window", &h.window, &x.to_string(), &y.to_string()]);
        pump(100);
        xdo(&["mousedown", "1"]);
        for step in 1..=10 {
            let nx = x - step * (before / 2 / 10).max(10);
            xdo(&["mousemove", "--window", &h.window, &nx.to_string(), &y.to_string()]);
            pump(40);
        }
        xdo(&["mouseup", "1"]);
        pump(600);
        screenshot(&h.app.window, "ui-launcher-drag");
        let after = frame.width();
        assert!(after < before * 2 / 3, "the divider moved: {before} -> {after}");
        let clip = h.app.inner.borrow().panes[&pane].compose.stack_clip.clone();
        let width = clip.width() as f32;
        let stop = button_with(&bar, "compose-bar-btn", "[stop]").unwrap();
        let b = stop.compute_bounds(&clip).unwrap();
        assert!(b.x() + b.width() <= width + 1.0, "[stop] stays in view after the drag");
        for c in widgets(&clip).into_iter().filter(|w| w.has_css_class("compose-chip")) {
            let b = c.compute_bounds(&clip).unwrap();
            assert!(b.x() + b.width() <= width + 1.0, "a chip is cut off after the drag");
        }
    });
}

#[test]
fn ui_launcher_stop_clicked_during_the_first_pull_interrupts_the_up() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("ui-launcher-stop-while-pulling");
        let (_, record, _) = fake_docker(&h);
        let (pane, button) = ui_launcher_setup(&h, UI_FILES);
        // `up` in the pane, still pulling: no containers exist yet (ps is empty).
        press(&h, "ctrl+shift+l");
        assert!(open_picker(&button).is_some());
        press(&h, "Down");
        press(&h, "Return");
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        wait_for(8_000, || ran().contains("compose -f docker-compose.yml up"));
        let bar = h.app.inner.borrow().panes[&pane].compose.bars().stack_bar.clone();
        wait_for(8_000, || bar.is_visible());
        pump(1_000);
        assert!(h.app.inner.borrow().panes[&pane].busy, "up is still running");

        let stop = button_with(&bar, "compose-bar-btn", "[stop]").expect("[stop]");
        click_widget(&h, &stop);
        // The up ends (as with Ctrl+C) and the bar goes away; the button is usable again.
        wait_for(8_000, || !h.app.inner.borrow().panes[&pane].busy);
        wait_for(10_000, || !bar.is_visible());
        wait_for(8_000, || button.is_sensitive());
    });
}

#[test]
fn ui_launcher_stop_on_running_containers_uses_docker_compose_stop_not_ctrl_c() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("ui-launcher-stop-running");
        let (_, record, ps) = fake_docker(&h);
        let (pane, button) = ui_launcher_setup(&h, UI_FILES);
        std::fs::write(&ps, PS_API_RUNNING).unwrap();
        // Attached `up` in the pane, containers already running (not a first pull).
        press(&h, "ctrl+shift+l");
        assert!(open_picker(&button).is_some());
        press(&h, "Down");
        press(&h, "Return");
        let ran = || std::fs::read_to_string(&record).unwrap_or_default();
        wait_for(8_000, || ran().contains("compose -f docker-compose.yml up"));
        let bar = h.app.inner.borrow().panes[&pane].compose.bars().stack_bar.clone();
        wait_for(8_000, || button_with(&bar, "compose-chip", "api").is_some());

        let stop = button_with(&bar, "compose-bar-btn", "[stop]").expect("[stop]");
        click_widget(&h, &stop);
        wait_for(8_000, || ran().lines().any(|l| l.ends_with(" stop")));
        pump(1_000);
        // The proper stop runs in the background; the pane got no Ctrl+C, so the
        // attached `up` keeps running until compose stops it.
        assert!(h.app.inner.borrow().panes[&pane].busy, "no Ctrl+C was sent to the pane");
    });
}

fn all_by_class(root: &gtk4::Widget, class: &str, out: &mut Vec<gtk4::Widget>) {
    if root.has_css_class(class) {
        out.push(root.clone());
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        all_by_class(&c, class, out);
        child = c.next_sibling();
    }
}

fn all_of_type<T: IsA<gtk4::Widget>>(root: &gtk4::Widget, out: &mut Vec<T>) {
    if let Ok(t) = root.clone().downcast::<T>() {
        out.push(t);
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        all_of_type(&c, out);
        child = c.next_sibling();
    }
}

fn widget_text(w: &gtk4::Widget) -> String {
    let mut labels: Vec<gtk4::Label> = Vec::new();
    all_of_type(w, &mut labels);
    labels.iter().map(|l| l.label().to_string()).collect::<Vec<_>>().join(" ")
}

/// The name a tab shows, without its state glyph and unread dot.
fn tab_name(h: &Harness, tab: oxishell::model::TabId) -> String {
    let mut v = Vec::new();
    all_by_class(&tab_of(h, tab), "tab-label", &mut v);
    widget_text(&v[0])
}

/// The state glyph a tab shows ("" for a tab without an agent).
fn tab_glyph(h: &Harness, tab: oxishell::model::TabId) -> String {
    let mut v = Vec::new();
    all_by_class(&tab_of(h, tab), "tab-glyph", &mut v);
    v.first().filter(|g| g.is_visible()).map(widget_text).unwrap_or_default()
}

/// Mapped widgets with `class` anywhere in the window.
fn by_class(h: &Harness, class: &str) -> Vec<gtk4::Widget> {
    let mut out = Vec::new();
    all_by_class(h.app.window.upcast_ref(), class, &mut out);
    out.into_iter().filter(|w| w.is_mapped()).collect()
}

/// Tab ids of a workspace and the shown one.
fn ws_tabs(h: &Harness, wid: u64) -> (Vec<u64>, u64) {
    let i = h.app.inner.borrow();
    let ws = i.state.workspace(wid).unwrap();
    (ws.tabs.iter().map(|t| t.id).collect(), ws.tab().id)
}

fn tab_of(h: &Harness, tab: u64) -> gtk4::Widget {
    by_class(h, "tab")
        .into_iter()
        .find(|t| t.widget_name() == format!("tab-{tab}"))
        .unwrap_or_else(|| panic!("no tab widget for tab {tab}"))
}

fn shown_tab_page(h: &Harness) -> Option<String> {
    let bar_parent = by_class(h, "tab-bar").first()?.parent()?;
    find_descendant::<gtk4::Stack>(&bar_parent)?
        .visible_child_name()
        .map(|n| n.to_string())
}

/// Press on `from`, move in small real steps to `to` (window coordinates), release.
fn drag_between(h: &Harness, from: (i32, i32), to: (i32, i32)) {
    xdo(&["mousemove", "--window", &h.window, &from.0.to_string(), &from.1.to_string()]);
    pump(60);
    xdo(&["mousedown", "1"]);
    pump(60);
    let steps = 12;
    let (dx, dy) = ((to.0 - from.0) / steps, (to.1 - from.1) / steps);
    for _ in 0..steps {
        xdo(&["mousemove_relative", "--", &dx.to_string(), &dy.to_string()]);
        pump(25);
    }
    pump(150);
    xdo(&["mouseup", "1"]);
    pump(400);
}

fn at(h: &Harness, w: &gtk4::Widget, fx: f32) -> (i32, i32) {
    let p = w
        .compute_point(
            &h.app.window,
            &gtk4::graphene::Point::new(w.width() as f32 * fx, w.height() as f32 / 2.0),
        )
        .unwrap();
    (p.x() as i32, p.y() as i32)
}

#[test]
fn tab_bar_new_switch_and_close() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("wstabs");
        let p = h.app.focused_pane().unwrap();
        let wid = h.app.inner.borrow().state.active.unwrap();
        pump(300);
        let (term_before, pid) = {
            let i = h.app.inner.borrow();
            (i.panes[&p].term.clone(), i.panes[&p].pid.get().unwrap())
        };
        assert_eq!(by_class(&h, "tab-bar").len(), 1, "one tab bar for the workspace");
        assert_eq!(by_class(&h, "tab").len(), 1);
        h.point(&by_class(&h, "tab-new")[0]);
        xdo(&["click", "1"]);
        pump(400);
        let (tabs, shown) = ws_tabs(&h, wid);
        assert_eq!(tabs.len(), 2);
        assert_eq!(shown, tabs[1], "the new tab is shown");
        let q = h.app.focused_pane().unwrap();
        assert_ne!(q, p);
        assert!(h.app.inner.borrow().panes[&q].term.has_focus(), "the new tab's terminal has focus");
        assert_eq!(by_class(&h, "tab").len(), 2);
        assert_eq!(by_class(&h, "tab-bar").len(), 1, "still one bar");
        assert!(!term_before.is_mapped(), "the first tab's terminal is hidden");
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0, "and its shell keeps running");

        h.point(&tab_of(&h, tabs[0]));
        xdo(&["click", "1"]);
        pump(300);
        assert_eq!(h.app.focused_pane(), Some(p));
        assert_eq!(shown_tab_page(&h), Some(tabs[0].to_string()));
        assert!(tab_of(&h, tabs[0]).has_css_class("tab-active"));
        assert_eq!(h.app.inner.borrow().panes[&p].term, term_before, "not recreated");
        assert!(term_before.has_focus());

        let close = find_by_class(&tab_of(&h, tabs[1]), "tab-close").expect("tab has a close button");
        h.point(&close);
        xdo(&["click", "1"]);
        pump(400);
        assert_eq!(ws_tabs(&h, wid).0, vec![tabs[0]]);
        assert!(!h.app.inner.borrow().panes.contains_key(&q));
    });
}

#[test]
fn split_inside_a_tab_keeps_one_tab_bar() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("wstabsplit");
        let wid = h.app.inner.borrow().state.active.unwrap();
        h.app.new_tab(Some(wid), None).unwrap();
        pump(300);
        let q = h.app.split(None, oxishell::model::Orient::Horizontal, None).unwrap();
        pump(300);
        assert_eq!(by_class(&h, "tab-bar").len(), 1, "splits get no tab bars");
        assert_eq!(by_class(&h, "tab").len(), 2, "a split adds no tab");
        let (tabs, shown) = ws_tabs(&h, wid);
        assert_eq!(shown, tabs[1]);
        let i = h.app.inner.borrow();
        let ws = i.state.workspace(wid).unwrap();
        assert_eq!(ws.tab().layout.panes().len(), 2, "the split is in the shown tab");
        assert_eq!(ws.tabs[0].layout.panes().len(), 1);
        assert!(i.panes[&q].term.is_mapped());
    });
}

#[test]
fn jump_to_unread_selects_background_tab() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("wstabjump");
        let p = h.app.focused_pane().unwrap();
        let wid = h.app.inner.borrow().state.active.unwrap();
        let (t, q) = h.app.new_tab(Some(wid), None).unwrap();
        pump(300);
        h.app.focus_pane(p);
        pump(200);
        h.app.notify(Some(q), "Agent", "needs you", "test");
        pump(200);
        assert!(tab_of(&h, t).has_css_class("tab-unread"), "a hidden tab shows its unread dot");
        assert_eq!(h.app.jump_to_unread(), Some(q));
        pump(300);
        assert_eq!(shown_tab_page(&h), Some(t.to_string()));
        assert!(h.app.inner.borrow().panes[&q].term.has_focus());
        assert!(!tab_of(&h, t).has_css_class("tab-unread"));
    });
}

#[test]
fn a_running_agent_that_went_quiet_shows_as_done() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        use oxishell::agents::{AgentState, HookInfo};
        let h = Harness::new("agentstall");
        let p = h.app.focused_pane().unwrap();
        h.app.agent_event(p, "claude", AgentState::Running, HookInfo::default()).unwrap();
        pump(200);
        assert_eq!(h.app.inner.borrow().panes[&p].agent_state(), Some(AgentState::Running));
        // An interrupted turn sends no hook: the terminal just stops changing.
        h.app.inner.borrow().panes[&p]
            .last_output
            .set(Instant::now() - Duration::from_secs(60));
        pump(1500);
        assert_eq!(h.app.inner.borrow().panes[&p].agent_state(), Some(AgentState::Idle));
        let sidebar: gtk4::Widget = h.app.sidebar_list.clone().upcast();
        let mut lines = Vec::new();
        all_by_class(&sidebar, "ws-session", &mut lines);
        assert!(widget_text(&lines[0]).contains('✓'), "{}", widget_text(&lines[0]));
    });
}

#[test]
fn sidebar_lists_each_tab_and_click_selects_it() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        use oxishell::agents::{AgentState, HookInfo};
        let h = Harness::new("wstabsidebar");
        let p = h.app.focused_pane().unwrap();
        let wid = h.app.inner.borrow().state.active.unwrap();
        let sidebar: gtk4::Widget = h.app.sidebar_list.clone().upcast();
        let lines = || {
            let mut v = Vec::new();
            all_by_class(&sidebar, "ws-session", &mut v);
            v
        };
        pump(300);
        assert!(lines().is_empty(), "a single plain tab gets no line");
        let (t, q) = h.app.new_tab(Some(wid), None).unwrap();
        pump(300);
        let prompt = |s: &str| HookInfo {
            prompt: Some(s.into()),
            ..Default::default()
        };
        h.app.agent_event(p, "claude", AgentState::Running, prompt("Fix A")).unwrap();
        h.app.agent_event(q, "codex", AgentState::Waiting, prompt("Fix B")).unwrap();
        h.app.focus_pane(p);
        pump(200);
        let l = lines();
        assert_eq!(l.len(), 2, "one line per tab");
        let texts: Vec<String> = l.iter().map(widget_text).collect();
        let tips: Vec<String> = l.iter().map(|b| b.tooltip_text().unwrap_or_default().to_string()).collect();
        assert!(tips[0].contains("Claude") && texts[0].contains("Fix A"), "{tips:?} {texts:?}");
        assert!(
            tips[1].contains("Codex") && texts[1].contains("Needs input: Fix B"),
            "{tips:?} {texts:?}"
        );
        assert!(
            texts[0].chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)),
            "running shows a spinner: {texts:?}"
        );
        assert!(l[0].has_css_class("ws-session-active") && !l[1].has_css_class("ws-session-active"));
        h.point(&l[1]);
        xdo(&["click", "1"]);
        pump(300);
        assert_eq!(ws_tabs(&h, wid).1, t, "clicking a line shows its tab");
        assert_eq!(h.app.focused_pane(), Some(q));
        assert_eq!(lines()[1], l[1], "line widgets are reused across refreshes");

        // Only tabs with an AI agent are listed: a plain shell tab gets no line.
        h.app.new_tab(Some(wid), None).unwrap();
        pump(300);
        let l = lines();
        assert_eq!(l.len(), 2, "plain shell tabs are not listed");
        assert!(l
            .iter()
            .all(|x| !widget_text(x).contains("cmux") && !widget_text(x).contains("home")));
    });
}

#[test]
fn sidebar_lists_every_agent_in_a_split_tab() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        use oxishell::agents::{AgentState, HookInfo};
        use oxishell::model::Orient;
        let h = Harness::new("wssplitagents");
        let p1 = h.app.focused_pane().unwrap();
        let p2 = h.app.split(Some(p1), Orient::Horizontal, None).unwrap();
        let p3 = h.app.split(Some(p2), Orient::Vertical, None).unwrap();
        pump(300);
        let prompt = |s: &str| HookInfo {
            prompt: Some(s.into()),
            ..Default::default()
        };
        h.app.agent_event(p1, "claude", AgentState::Running, prompt("Fix A")).unwrap();
        h.app.agent_event(p2, "codex", AgentState::Waiting, prompt("Fix B")).unwrap();
        h.app.agent_event(p3, "gemini", AgentState::Idle, prompt("Fix C")).unwrap();
        h.app.focus_pane(p1);
        pump(200);
        let sidebar: gtk4::Widget = h.app.sidebar_list.clone().upcast();
        let lines = || {
            let mut v = Vec::new();
            all_by_class(&sidebar, "ws-session", &mut v);
            v
        };
        let l = lines();
        let texts: Vec<String> = l.iter().map(widget_text).collect();
        assert_eq!(l.len(), 3, "one line per agent pane of the split tab: {texts:?}");
        assert!(
            texts[0].contains("Fix A") && texts[1].contains("Fix B") && texts[2].contains("Fix C"),
            "{texts:?}"
        );
        assert!(l[0].has_css_class("ws-session-active") && !l[2].has_css_class("ws-session-active"));
        h.point(&l[2]);
        xdo(&["click", "1"]);
        pump(300);
        assert_eq!(h.app.focused_pane(), Some(p3), "clicking a line focuses its pane");
        let l = lines();
        assert!(l[2].has_css_class("ws-session-active") && !l[0].has_css_class("ws-session-active"));

        // A named tab with several agents keeps each agent's own summary.
        let t = ws_tabs(&h, h.app.inner.borrow().state.active.unwrap()).1;
        h.app.rename_tab(t, Some("api")).unwrap();
        pump(200);
        let texts: Vec<String> = lines().iter().map(widget_text).collect();
        assert!(texts[1].contains("Fix B"), "{texts:?}");
        let headers = || {
            let mut v = Vec::new();
            all_by_class(&sidebar, "ws-tab-header", &mut v);
            v
        };
        assert!(headers().is_empty(), "one tab with agents gets no tab header");

        // Agents in a second tab: each tab's lines sit under a "Tab N" header.
        let wid = h.app.inner.borrow().state.active.unwrap();
        let (t2, q) = h.app.new_tab(Some(wid), None).unwrap();
        h.app.agent_event(q, "claude", AgentState::Running, prompt("Fix D")).unwrap();
        h.app.focus_pane(p1);
        pump(300);
        let hs = headers();
        let titles: Vec<String> = hs.iter().map(widget_text).collect();
        assert_eq!(titles, ["api", "Tab 2"], "a named tab shows its name, else its number");
        let mut ordered = Vec::new();
        all_by_class(&sidebar, "ws-session", &mut ordered);
        all_by_class(&sidebar, "ws-tab-header", &mut ordered);
        ordered.sort_by_key(|w| w.compute_point(&sidebar, &gtk4::graphene::Point::zero()).unwrap().y() as i32);
        let order: Vec<String> = ordered.iter().map(widget_text).collect();
        assert!(
            order[0] == "api" && order[1].contains("Fix A") && order[4].contains("Tab 2") && order[5].contains("Fix D"),
            "headers sit above their tab's lines: {order:?}"
        );
        assert!(lines().iter().all(|l| l.has_css_class("ws-session-grouped")));
        assert!(hs[0].has_css_class("ws-tab-header-active") && !hs[1].has_css_class("ws-tab-header-active"));
        h.point(&hs[1]);
        xdo(&["click", "1"]);
        pump(300);
        assert_eq!(ws_tabs(&h, wid).1, t2, "clicking a tab header shows that tab");
    });
}

#[test]
fn dragging_a_tab_reorders_it() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("wstabdrag");
        let p = h.app.focused_pane().unwrap();
        let wid = h.app.inner.borrow().state.active.unwrap();
        let first = ws_tabs(&h, wid).0[0];
        let (t1, _) = h.app.new_tab(Some(wid), None).unwrap();
        let (t2, _) = h.app.new_tab(Some(wid), None).unwrap();
        pump(400);
        drag_between(&h, at(&h, &tab_of(&h, t2), 0.5), at(&h, &tab_of(&h, first), 0.2));
        assert_eq!(ws_tabs(&h, wid), (vec![t2, first, t1], t2), "moved to the front, still shown");

        // Dropping a tab on a terminal does nothing: no move, no pasted text.
        h.app.focus_pane(p);
        pump(300);
        let term = h.app.inner.borrow().panes[&p].term.clone();
        let target: gtk4::Widget = term.clone().upcast();
        drag_between(&h, at(&h, &tab_of(&h, t1), 0.5), at(&h, &target, 0.5));
        assert_eq!(ws_tabs(&h, wid).0, vec![t2, first, t1]);
        let text = term.text_format(vte4::Format::Text).unwrap_or_default();
        assert!(!text.contains("oxishell-tab"), "the drag payload must not be pasted: {text}");
    });
}

#[test]
fn ctrl_tab_cycles_tabs_and_ctrl_shift_t_opens_one() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("wstabkeys");
        let p = h.app.focused_pane().unwrap();
        let wid = h.app.inner.borrow().state.active.unwrap();
        pump(300);
        h.app.inner.borrow().panes[&p].term.grab_focus();
        pump(100);
        xdo(&["key", "--window", &h.window, "ctrl+shift+t"]);
        pump(400);
        let (tabs, shown) = ws_tabs(&h, wid);
        assert_eq!(tabs.len(), 2, "Ctrl+Shift+T opens a tab");
        assert_eq!(shown, tabs[1]);
        let q = h.app.focused_pane().unwrap();
        xdo(&["key", "--window", &h.window, "ctrl+Tab"]);
        pump(300);
        assert_eq!(h.app.focused_pane(), Some(p), "Ctrl+Tab wraps to the first tab");
        xdo(&["key", "--window", &h.window, "ctrl+shift+Tab"]);
        pump(300);
        assert_eq!(h.app.focused_pane(), Some(q), "Ctrl+Shift+Tab goes back");
        assert!(h.app.inner.borrow().panes[&q].term.has_focus());
    });
}

#[test]
fn agent_tab_shows_its_task_and_an_animated_state_glyph() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        use oxishell::agents::{AgentState, HookInfo};
        let h = Harness::new("wstabtitle");
        let p = h.app.focused_pane().unwrap();
        let wid = h.app.inner.borrow().state.active.unwrap();
        let t = ws_tabs(&h, wid).1;
        pump(300);
        assert_eq!(tab_glyph(&h, t), "", "a plain shell tab has no glyph");
        let info = HookInfo {
            prompt: Some("Fix A".into()),
            ..Default::default()
        };
        h.app.agent_event(p, "claude", AgentState::Running, info).unwrap();
        pump(200);
        assert_eq!(tab_name(&h, t), "Claude: Fix A");
        let spinner = |g: &str| g.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c));
        let first = tab_glyph(&h, t);
        assert!(spinner(&first), "a working agent spins on the tab: {first:?}");
        wait_for(2_000, || tab_glyph(&h, t) != first);
        h.app.inner.borrow().panes[&p]
            .term
            .feed_child(b"sh -c \"printf '\\033]0;\\342\\234\\263 Title T\\007'; sleep 3\"\r");
        wait_for(5_000, || tab_name(&h, t) == "Claude: Title T");
        h.app.agent_event(p, "claude", AgentState::Waiting, HookInfo::default()).unwrap();
        pump(200);
        assert_eq!(tab_name(&h, t), "Claude: Title T", "needing input does not rename the tab");
        assert_eq!(tab_glyph(&h, t), "?", "needs input is a steady ? on the tab");
        pump(700);
        assert_eq!(tab_glyph(&h, t), "?", "and it does not blink");
        let tip = tab_of(&h, t).tooltip_text().unwrap_or_default();
        assert!(tip.contains("Needs input"), "the state is in the tooltip: {tip}");
        h.app.agent_event(p, "claude", AgentState::Idle, HookInfo::default()).unwrap();
        pump(200);
        assert_eq!(tab_name(&h, t), "Claude: Title T", "finishing does not rename the tab");
        assert_eq!(tab_glyph(&h, t), "✓");
    });
}

#[test]
fn rebuilding_frees_the_old_tab_bar() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("wstableak");
        let wid = h.app.inner.borrow().state.active.unwrap();
        let t = ws_tabs(&h, wid).1;
        pump(300);
        let (tab, area) = {
            let tab = tab_of(&h, t);
            let fill = by_class(&h, "tab-fill")[0].clone();
            (tab.downgrade(), fill.parent().unwrap().downgrade())
        };
        h.app.rebuild_workspace(wid);
        pump(300);
        assert!(tab.upgrade().is_none(), "the old tab widget is freed");
        assert!(area.upgrade().is_none(), "the old bar (drop target area) is freed");
    });
}

#[test]
fn double_clicking_a_tab_renames_it() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        use oxishell::agents::{AgentState, HookInfo};
        let h = Harness::new("wstabrename");
        let p = h.app.focused_pane().unwrap();
        let wid = h.app.inner.borrow().state.active.unwrap();
        h.app.new_tab(Some(wid), None).unwrap();
        let t = ws_tabs(&h, wid).0[0];
        h.app.focus_pane(p);
        let info = HookInfo {
            prompt: Some("Fix A".into()),
            ..Default::default()
        };
        h.app.agent_event(p, "claude", AgentState::Running, info).unwrap();
        pump(300);
        let settings = gtk4::Settings::default().unwrap();
        let saved = settings.gtk_double_click_time();
        settings.set_gtk_double_click_time(1500);
        h.point(&tab_of(&h, t));
        xdo(&["click", "--repeat", "2", "--delay", "60", "1"]);
        pump(300);
        settings.set_gtk_double_click_time(saved);
        let dialog = h.app.window.visible_dialog().expect("double click opens rename");
        let entry = find_descendant::<gtk4::Entry>(dialog.upcast_ref()).expect("name entry");
        entry.set_text("api server");
        dialog.emit_by_name::<()>("response", &[&"ok"]);
        dialog.force_close();
        let deadline = Instant::now() + Duration::from_secs(3);
        while h.app.window.visible_dialog().is_some() {
            assert!(Instant::now() < deadline, "rename dialog did not close");
            pump(20);
        }
        pump(200);
        assert_eq!(tab_name(&h, t), "api server");
        let tip = tab_of(&h, t).tooltip_text().unwrap_or_default();
        assert!(tip.contains("Fix A"), "the agent's summary moves to the tooltip: {tip}");
        let sidebar: gtk4::Widget = h.app.sidebar_list.clone().upcast();
        let mut lines = Vec::new();
        all_by_class(&sidebar, "ws-session", &mut lines);
        let first = widget_text(&lines[0]);
        assert!(
            lines[0].tooltip_text().unwrap_or_default().contains("Claude") && first.contains("api server"),
            "{first}"
        );
        assert_eq!(h.app.focused_pane(), Some(p), "renaming keeps the tab shown");

        // Clearing the name brings the automatic label back.
        h.app.rename_tab(t, None).unwrap();
        pump(200);
        assert_eq!(tab_name(&h, t), "Claude: Fix A");
    });
}

/// Pump for `ms`, but give up on draining a busy main loop (a focus or
/// layout loop would otherwise hang the test instead of failing it).
fn pump_bounded(ms: u64) {
    let context = glib::MainContext::default();
    let until = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < until {
        let slice = Instant::now() + Duration::from_millis(20);
        while context.pending() && Instant::now() < slice {
            context.iteration(false);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn switching_back_before_a_new_tab_is_shown_settles() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("wstabsettle");
        let p = h.app.focused_pane().unwrap();
        let wid = h.app.inner.borrow().state.active.unwrap();
        let first = ws_tabs(&h, wid).1;
        // New tab, then straight back to the first one before the new tab's
        // terminal was ever shown (as `oxi tab new; oxi tab select` can do).
        h.app.new_tab(Some(wid), None).unwrap();
        h.app.focus_pane(p);
        pump_bounded(1500);
        assert_eq!(ws_tabs(&h, wid).1, first, "the first tab stays shown");
        assert_eq!(h.app.focused_pane(), Some(p));
        let context = glib::MainContext::default();
        let mut busy = 0;
        for _ in 0..50 {
            if context.pending() {
                busy += 1;
                context.iteration(false);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(busy < 25, "the main loop must go idle, it was busy in {busy}/50 checks");
    });
}

#[test]
fn workspace_rows_show_no_ports_git_branch_or_folder() {
    if !have_input() {
        return;
    }
    gtk4::test_synced(|| {
        let h = Harness::new("wsrowclean");
        let p = h.app.focused_pane().unwrap();
        pump(300);
        {
            let mut i = h.app.inner.borrow_mut();
            let pane = i.panes.get_mut(&p).unwrap();
            pane.git = Some(oxishell::meta::GitInfo {
                branch: "feature/auth".into(),
                changes: 2,
                ..Default::default()
            });
            pane.ports = vec![8020];
        }
        h.app.refresh();
        pump(100);
        let row: gtk4::Widget = h.app.sidebar_list.row_at_index(0).unwrap().upcast();
        let text = widget_text(&row);
        assert!(!text.contains("feature/auth") && !text.contains('⎇'), "no git branch: {text}");
        assert!(!text.contains("8020") && !text.contains("ports"), "no ports: {text}");
        let cwd = h.app.inner.borrow().panes[&p].cwd.clone().unwrap();
        assert!(!text.contains(&oxishell::meta::display_path(&cwd)), "no folder path: {text}");
    });
}
