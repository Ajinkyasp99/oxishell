//! Account usage pinned below the scrolling workspace list.

use super::AppRc;
use crate::agents::AgentState;
use crate::usage::{ClaudeReports, Usage, Window};
use gtk4::prelude::*;
use gtk4::{glib, Orientation};
use std::cell::RefCell;
use std::time::{Duration, Instant};

pub struct Footer {
    pub root: gtk4::Box,
    pub claude: RefCell<ClaudeReports>,
    codex: RefCell<Option<Usage>>,
    /// Wake the reader thread: (Codex active, Claude active).
    tx: std::sync::mpsc::SyncSender<(bool, bool)>,
    codex_row: UsageRow,
    claude_row: UsageRow,
}

/// What the background thread reads from disk.
pub struct Polled {
    codex: Option<Usage>,
    /// Claude Code's own cached usage: (fetched at, usage).
    claude_cache: Option<(i64, Usage)>,
}

#[derive(PartialEq)]
struct RowSnapshot {
    usage: Option<Usage>,
    expired: [bool; 2],
}

struct UsageRow {
    button: gtk4::MenuButton,
    five_hour: UsageChip,
    weekly: UsageChip,
    details: gtk4::Box,
    name: &'static str,
    claude: bool,
    snapshot: RefCell<Option<RowSnapshot>>,
}

struct UsageChip {
    root: gtk4::Box,
    meter: gtk4::Overlay,
    label: gtk4::Label,
    bar: gtk4::ProgressBar,
    reset: gtk4::Label,
}

impl UsageChip {
    fn new(text: &str) -> Self {
        let root = gtk4::Box::new(Orientation::Vertical, 1);
        root.set_valign(gtk4::Align::Start);
        let meter = gtk4::Overlay::new();
        meter.add_css_class("usage-chip");
        meter.set_overflow(gtk4::Overflow::Hidden);
        let bar = gtk4::ProgressBar::new();
        bar.set_can_target(false);
        let reset = label("—", "usage-reset");
        let label = label(text, "usage-chip-text");
        label.set_can_target(false);
        meter.set_child(Some(&bar));
        meter.add_overlay(&label);
        // Let the value determine the compact pill's width and height.
        meter.set_measure_overlay(&label, true);
        reset.set_xalign(0.5);
        reset.set_justify(gtk4::Justification::Center);
        reset.set_ellipsize(gtk4::pango::EllipsizeMode::None);
        root.append(&meter);
        root.append(&reset);
        Self {
            root,
            meter,
            label,
            bar,
            reset,
        }
    }
}

impl UsageRow {
    fn new(name: &'static str, short_name: &str, claude: bool) -> Self {
        let content = gtk4::Box::new(Orientation::Horizontal, 6);
        let name_label = label(short_name, "usage-agent");
        name_label.set_hexpand(true);
        name_label.set_valign(gtk4::Align::Start);
        let five_hour = UsageChip::new("5h —");
        let weekly = UsageChip::new("Wk —");
        content.append(&name_label);
        content.append(&five_hour.root);
        content.append(&weekly.root);
        let details = gtk4::Box::new(Orientation::Vertical, 0);
        details.add_css_class("usage-details");
        details.set_size_request(230, -1);
        let popover = gtk4::Popover::builder().child(&details).position(gtk4::PositionType::Top).build();
        let button = gtk4::MenuButton::builder()
            .child(&content)
            .popover(&popover)
            .has_frame(false)
            .build();
        button.add_css_class("usage-summary");
        button.set_visible(false);
        Self {
            button,
            five_hour,
            weekly,
            details,
            name,
            claude,
            snapshot: RefCell::new(None),
        }
    }

    fn update(&self, usage: Option<&Usage>, now: i64) {
        let windows = [usage.and_then(|u| u.five_hour.as_ref()), usage.and_then(|u| u.weekly.as_ref())];
        // Update dates across midnight even when the quota snapshot is unchanged.
        self.five_hour.reset.set_label(&compact_reset(windows[0], now, false));
        self.weekly.reset.set_label(&compact_reset(windows[1], now, true));
        let snapshot = RowSnapshot {
            usage: usage.cloned(),
            expired: windows.map(|w| w.is_some_and(|w| w.expired(now))),
        };
        if self.snapshot.borrow().as_ref() == Some(&snapshot) {
            return;
        }
        for (chip, short, title, window) in [
            (&self.five_hour, "5h", "5-hour limit", windows[0]),
            (&self.weekly, "Wk", "Weekly limit", windows[1]),
        ] {
            let current = window.filter(|w| !w.expired(now));
            let percent = current
                .map(|w| {
                    if w.used_percent.fract() == 0.0 {
                        format!("{:.0}%", w.used_percent)
                    } else {
                        format!("{:.1}%", w.used_percent)
                    }
                })
                .unwrap_or_else(|| "—".into());
            chip.label.set_label(&format!("{short} {percent}"));
            chip.bar.set_fraction(current.map(|w| w.used_percent / 100.0).unwrap_or(0.0));
            if current.is_some_and(|w| w.used_percent >= 90.0) {
                chip.meter.add_css_class("usage-low");
            } else {
                chip.meter.remove_css_class("usage-low");
            }
            chip.root
                .set_tooltip_text(Some(&format!("{title} · {percent} used\n{}", reset_text(window, now))));
        }
        self.button.set_tooltip_text(Some(&format!(
            "{} · quota used\n5-hour: {}\nWeekly: {}\nClick for details",
            self.name,
            reset_text(windows[0], now),
            reset_text(windows[1], now)
        )));
        // Keep the same button and popover alive across background refreshes.
        while let Some(child) = self.details.first_child() {
            self.details.remove(&child);
        }
        card(&self.details, self.name, usage, self.claude, now);
        *self.snapshot.borrow_mut() = Some(snapshot);
    }
}

impl Footer {
    pub fn new() -> (Self, async_channel::Receiver<Polled>) {
        let root = gtk4::Box::new(Orientation::Vertical, 2);
        root.add_css_class("usage-footer");
        root.set_visible(false);
        root.append(&label("USAGE · % USED", "usage-heading"));
        let codex_row = UsageRow::new("Codex", "Codex", false);
        let claude_row = UsageRow::new("Claude Code", "Claude", true);
        root.append(&codex_row.button);
        root.append(&claude_row.button);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let (utx, urx) = async_channel::bounded(1);
        std::thread::Builder::new()
            .name("cmux-usage".into())
            .spawn(move || {
                let mut last = None;
                let mut cached = None;
                let claude_file = crate::usage::claude_config_file();
                let mut claude_mtime = None;
                let mut claude_cache = None;
                while let Ok((codex, claude)) = rx.recv() {
                    if codex && last.is_none_or(|t: Instant| t.elapsed() >= Duration::from_secs(15)) {
                        cached = crate::usage::read_codex(&crate::usage::codex_home());
                        last = Some(Instant::now());
                    }
                    if claude {
                        // Re-read only when Claude Code rewrote its config.
                        let m = std::fs::metadata(&claude_file).and_then(|m| m.modified()).ok();
                        if m != claude_mtime {
                            claude_mtime = m;
                            claude_cache = crate::usage::read_claude_cache(&claude_file);
                        }
                    }
                    let polled = Polled {
                        codex: cached.clone(),
                        claude_cache: claude_cache.clone(),
                    };
                    if utx.send_blocking(polled).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn usage thread");
        (
            Self {
                root,
                claude: RefCell::new(ClaudeReports::default()),
                codex: RefCell::new(None),
                tx,
                codex_row,
                claude_row,
            },
            urx,
        )
    }
}

fn active_provider<'a>(foreground: Option<&'a str>, hook: Option<(&'a str, AgentState)>, has_child: bool) -> Option<&'a str> {
    let agent = foreground.or_else(|| {
        hook.filter(|(_, state)| *state != AgentState::Ended && has_child)
            .map(|(agent, _)| agent)
    })?;
    matches!(agent, "codex" | "claude").then_some(agent)
}

fn active(app: &AppRc) -> (bool, bool) {
    let i = app.inner.borrow();
    let mut codex = false;
    let mut claude = false;
    for pane in i.panes.values() {
        match active_provider(
            pane.fg_agent,
            pane.agent.as_ref().map(|(agent, state, _)| (agent.as_str(), *state)),
            pane.has_running_child(),
        ) {
            Some("codex") => codex = true,
            Some("claude") => claude = true,
            _ => {}
        }
    }
    (codex, claude)
}

pub fn attach(app: &AppRc, rx: async_channel::Receiver<Polled>) {
    let w = app.weak();
    glib::spawn_future_local(async move {
        while let Ok(polled) = rx.recv().await {
            let Some(a) = w.upgrade() else { return };
            *a.usage_footer.codex.borrow_mut() = polled.codex;
            if let Some((fetched_at, usage)) = polled.claude_cache {
                a.usage_footer.claude.borrow_mut().merge_cache(fetched_at, usage);
            }
            render(&a);
        }
    });
    let w = app.weak();
    // Also redraw countdowns when the agents are idle and no metadata changes.
    glib::timeout_add_seconds_local(15, move || {
        let Some(a) = w.upgrade() else { return glib::ControlFlow::Break };
        refresh(&a);
        glib::ControlFlow::Continue
    });
}

pub fn refresh(app: &AppRc) {
    app.usage_footer
        .claude
        .borrow_mut()
        .retain_panes(|pane| app.inner.borrow().panes.contains_key(&pane));
    let (codex, claude) = active(app);
    if codex || claude {
        let _ = app.usage_footer.tx.try_send((codex, claude));
    }
    render(app);
}

fn render(app: &AppRc) {
    let (codex, claude) = active(app);
    render_cards(&app.usage_footer, codex, claude, crate::usage::now());
}

fn render_cards(footer: &Footer, codex: bool, claude: bool, now: i64) {
    footer.root.set_visible(codex || claude);
    footer.codex_row.button.set_visible(codex);
    footer.claude_row.button.set_visible(claude);
    if codex {
        let usage = footer.codex.borrow();
        footer.codex_row.update(usage.as_ref(), now);
    }
    if claude {
        footer.claude_row.update(Some(&footer.claude.borrow().latest), now);
    }
}

fn label(text: &str, class: &str) -> gtk4::Label {
    let label = gtk4::Label::builder()
        .label(text)
        .xalign(0.0)
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .build();
    label.add_css_class(class);
    label
}

fn card(root: &gtk4::Box, name: &str, usage: Option<&Usage>, claude: bool, now: i64) {
    let card = gtk4::Box::new(Orientation::Vertical, 6);
    card.append(&label(name, "ws-name"));
    limit(&card, "5-hour limit", usage.and_then(|u| u.five_hour.as_ref()), now);
    limit(&card, "Weekly limit", usage.and_then(|u| u.weekly.as_ref()), now);
    let empty = usage.is_none_or(|u| u.five_hour.is_none() && u.weekly.is_none());
    if empty {
        let hint = if claude {
            "Enable usage: cmux hooks install claude\nThen restart Claude and send a prompt."
        } else {
            "Waiting for Codex usage data"
        };
        let l = label(hint, "ws-sub");
        l.set_wrap(true);
        l.set_selectable(true);
        l.set_ellipsize(gtk4::pango::EllipsizeMode::None);
        l.set_tooltip_text(Some(if claude {
            "Run cmux hooks install claude, then restart Claude. Limits come from Claude Code's cached usage, and live from its status line after each response. API-key sessions may not report quotas."
        } else {
            "Limits are read from local Codex session logs after a response. API-key sessions may not report subscription quotas."
        }));
        card.append(&l);
    } else {
        card.set_tooltip_text(Some("Last quota snapshot reported by the CLI, shared across sessions. Updated after agent responses; usage elsewhere may not appear until the next response."));
    }
    root.append(&card);
}

fn limit(card: &gtk4::Box, title: &str, window: Option<&Window>, now: i64) {
    let group = gtk4::Box::new(Orientation::Vertical, 2);
    let current = window.filter(|w| !w.expired(now));
    let text = current
        .map(|w| format!("{title} · {:.1}% used", w.used_percent))
        .unwrap_or_else(|| format!("{title} · —"));
    group.append(&label(&text, "usage-limit"));
    let bar = gtk4::ProgressBar::new();
    bar.set_fraction(current.map(|w| w.used_percent / 100.0).unwrap_or(0.0));
    bar.set_hexpand(true);
    if current.is_some_and(|w| w.used_percent >= 90.0) {
        bar.add_css_class("usage-low");
    }
    group.append(&bar);
    let reset = reset_text(window, now);
    let reset_label = label(&reset, "ws-sub");
    if let Some(timestamp) = window.and_then(|w| w.resets_at) {
        if let Ok(date) = glib::DateTime::from_unix_local(timestamp) {
            let time = reset_time(&date);
            if let Ok(formatted) = date.format("%a %d %b %Y") {
                reset_label.set_tooltip_text(Some(&format!("Resets {formatted}, {time} ({})", date.timezone_abbreviation())));
            }
        }
    }
    group.append(&reset_label);
    card.append(&group);
}

fn reset_text(window: Option<&Window>, now: i64) -> String {
    let Some(window) = window else { return "Not reported yet".into() };
    if window.expired(now) {
        return window.reset_text(now);
    }
    if let Some(date) = window.resets_at.and_then(|t| glib::DateTime::from_unix_local(t).ok()) {
        if let Ok(day) = date.format("%d %b") {
            return format!("Resets {day}, {}", reset_time(&date));
        }
    }
    "Reset time unavailable".into()
}

fn reset_time(date: &glib::DateTime) -> String {
    let hour = date.hour();
    let hour12 = if hour % 12 == 0 { 12 } else { hour % 12 };
    format!("{hour12}:{:02} {}", date.minute(), if hour < 12 { "AM" } else { "PM" })
}

fn compact_reset(window: Option<&Window>, now: i64, weekly: bool) -> String {
    let Some(window) = window else { return "—".into() };
    if window.expired(now) {
        return "Updating".into();
    }
    let Some(date) = window.resets_at.and_then(|t| glib::DateTime::from_unix_local(t).ok()) else {
        return "—".into();
    };
    let time = reset_time(&date);
    let same_day = glib::DateTime::from_unix_local(now)
        .is_ok_and(|today| (date.year(), date.month(), date.day_of_month()) == (today.year(), today.month(), today.day_of_month()));
    if weekly || !same_day {
        if let Ok(day) = date.format("%d %b") {
            return format!("{time}\n{day}");
        }
    }
    time
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_clock_uses_am_pm_at_midnight_noon_and_evening() {
        for (hour, minute, expected) in [(0, 5, "12:05 AM"), (8, 15, "8:15 AM"), (12, 0, "12:00 PM"), (20, 30, "8:30 PM")] {
            let date = glib::DateTime::from_utc(2026, 11, 7, hour, minute, 0.0).unwrap();
            assert_eq!(reset_time(&date), expected);
        }
    }

    #[test]
    fn cards_only_follow_open_sessions_not_saved_or_stale_hooks() {
        assert_eq!(active_provider(Some("codex"), None, true), Some("codex"));
        assert_eq!(active_provider(Some("claude"), None, true), Some("claude"));
        assert_eq!(active_provider(None, Some(("claude", AgentState::Idle)), true), Some("claude"));
        assert_eq!(active_provider(None, Some(("claude", AgentState::Idle)), false), None);
        assert_eq!(active_provider(None, Some(("codex", AgentState::Ended)), true), None);
        assert_eq!(active_provider(Some("gemini"), Some(("codex", AgentState::Idle)), true), None);
        assert_eq!(active_provider(None, None, false), None);
    }

    #[test]
    fn footer_widgets_show_both_windows_and_hide_closed_sessions() {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            eprintln!("skipped GTK footer test: no display; run under xvfb-run");
            return;
        }
        super::super::fonts::register().unwrap();
        gtk4::init().unwrap();
        let font = gtk4::Label::new(None)
            .pango_context()
            .load_font(&gtk4::pango::FontDescription::from_string("Hack 11"))
            .unwrap();
        assert_eq!(font.describe().family().as_deref(), Some("Hack"));
        let (footer, _rx) = Footer::new();
        let five_reset = glib::DateTime::from_local(2026, 11, 7, 20, 30, 0.0).unwrap().to_unix();
        let week_reset = glib::DateTime::from_local(2026, 11, 14, 8, 15, 0.0).unwrap().to_unix();
        let now = five_reset - 3600;
        let usage = Usage {
            five_hour: Some(Window {
                used_percent: 20.0,
                resets_at: Some(five_reset),
            }),
            weekly: Some(Window {
                used_percent: 65.0,
                resets_at: Some(week_reset),
            }),
        };
        *footer.codex.borrow_mut() = Some(usage.clone());
        footer.claude.borrow_mut().update(1, usage);
        render_cards(&footer, false, false, now);
        assert!(!footer.root.get_visible());
        assert!(!footer.codex_row.button.get_visible());
        assert!(!footer.claude_row.button.get_visible());
        render_cards(&footer, true, true, now);
        assert!(footer.root.get_visible());
        assert_eq!(footer.codex_row.five_hour.label.text(), "5h 20%");
        assert_eq!(footer.codex_row.weekly.label.text(), "Wk 65%");
        assert_eq!(footer.claude_row.five_hour.label.text(), "5h 20%");
        assert_eq!(footer.codex_row.five_hour.bar.fraction(), 0.2);
        assert_eq!(footer.codex_row.weekly.bar.fraction(), 0.65);
        assert_eq!(footer.claude_row.five_hour.bar.fraction(), 0.2);
        assert_eq!(footer.codex_row.five_hour.reset.text(), "8:30 PM");
        assert_eq!(footer.codex_row.weekly.reset.text(), "8:15 AM\n14 Nov");
        assert!(footer
            .codex_row
            .five_hour
            .root
            .tooltip_text()
            .unwrap()
            .contains("Resets 07 Nov, 8:30 PM"));
        assert!(footer
            .codex_row
            .weekly
            .root
            .tooltip_text()
            .unwrap()
            .contains("Resets 14 Nov, 8:15 AM"));
        let first = footer.codex_row.details.first_child().unwrap();
        let second = footer.claude_row.details.first_child().unwrap();
        for (card, title) in [(&first, "Codex"), (&second, "Claude Code")] {
            let name = card.first_child().unwrap().downcast::<gtk4::Label>().unwrap();
            assert_eq!(name.text(), title);
            let five_hour = name.next_sibling().unwrap();
            let weekly = five_hour.next_sibling().unwrap();
            for (group, text, fraction, reset) in [
                (five_hour, "5-hour limit · 20.0% used", 0.2, "Resets 07 Nov, 8:30 PM"),
                (weekly, "Weekly limit · 65.0% used", 0.65, "Resets 14 Nov, 8:15 AM"),
            ] {
                let l = group.first_child().unwrap().downcast::<gtk4::Label>().unwrap();
                assert_eq!(l.text(), text);
                let bar = l.next_sibling().unwrap().downcast::<gtk4::ProgressBar>().unwrap();
                assert_eq!(bar.fraction(), fraction);
                let r = bar.next_sibling().unwrap().downcast::<gtk4::Label>().unwrap();
                assert_eq!(r.text(), reset);
                assert!(r.tooltip_text().unwrap().starts_with("Resets "));
            }
        }
        render_cards(&footer, false, true, five_reset + 1);
        assert!(!footer.codex_row.button.get_visible());
        assert!(footer.claude_row.button.get_visible());
        assert_eq!(footer.claude_row.five_hour.label.text(), "5h —");
        assert_eq!(footer.claude_row.five_hour.bar.fraction(), 0.0);
        assert_eq!(footer.claude_row.five_hour.reset.text(), "Updating");
        let claude = footer.claude_row.details.first_child().unwrap();
        let group = claude.first_child().unwrap().next_sibling().unwrap();
        assert_eq!(
            group.first_child().unwrap().downcast::<gtk4::Label>().unwrap().text(),
            "5-hour limit · —"
        );
        render_cards(&footer, false, false, five_reset + 1);
        assert!(!footer.root.get_visible());

        // Background refreshes retain the row/popover and its existing content.
        render_cards(&footer, true, true, now);
        let detail = footer.codex_row.details.first_child().unwrap();
        render_cards(&footer, true, true, now + 15);
        assert_eq!(footer.codex_row.details.first_child().unwrap(), detail);
        footer.claude.borrow_mut().latest = Usage::default();
        render_cards(&footer, true, true, now);
        assert_eq!(footer.claude_row.five_hour.label.text(), "5h —");
        assert_eq!(footer.claude_row.weekly.label.text(), "Wk —");
        assert_eq!(footer.claude_row.weekly.bar.fraction(), 0.0);
        assert_eq!(footer.claude_row.weekly.reset.text(), "—");

        let css = gtk4::CssProvider::new();
        css.load_from_string(include_str!("style.css"));
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().unwrap(),
            &css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let (_, height, _, _) = footer.root.measure(Orientation::Vertical, 230);
        assert!(
            height <= 132,
            "footer should stay compact with reset times and both agents: {height}px"
        );

        for percent in [0.0, 32.0, 90.0, 100.0] {
            let card = gtk4::Box::new(Orientation::Vertical, 0);
            let window = Window {
                used_percent: percent,
                resets_at: Some(4600),
            };
            limit(&card, "5-hour limit", Some(&window), 1000);
            let group = card.first_child().unwrap();
            let text = group.first_child().unwrap().downcast::<gtk4::Label>().unwrap();
            assert_eq!(text.text(), format!("5-hour limit · {percent:.1}% used"));
            let bar = text.next_sibling().unwrap().downcast::<gtk4::ProgressBar>().unwrap();
            assert_eq!(bar.fraction(), percent / 100.0);
            assert_eq!(bar.has_css_class("usage-low"), percent >= 90.0);

            let usage = Usage {
                five_hour: Some(window),
                weekly: None,
            };
            footer.codex_row.update(Some(&usage), 1000);
            let chip = &footer.codex_row.five_hour;
            assert_eq!(chip.label.text(), format!("5h {percent}%"));
            assert_eq!(chip.bar.fraction(), percent / 100.0);
            assert_eq!(chip.meter.has_css_class("usage-low"), percent >= 90.0);
            assert_eq!(chip.meter.child().unwrap(), chip.bar);
            assert_eq!(chip.label.parent().unwrap(), chip.meter);
        }

        if let Some(path) = std::env::var_os("CMUX_USAGE_SCREENSHOT") {
            use gtk4::gsk::prelude::*;
            libadwaita::init().unwrap();
            libadwaita::StyleManager::default().set_color_scheme(libadwaita::ColorScheme::ForceDark);
            let mut sample = Usage {
                five_hour: Some(Window {
                    used_percent: 32.0,
                    resets_at: Some(five_reset),
                }),
                weekly: Some(Window {
                    used_percent: 6.0,
                    resets_at: Some(week_reset),
                }),
            };
            *footer.codex.borrow_mut() = Some(sample.clone());
            sample.five_hour.as_mut().unwrap().used_percent = 100.0;
            sample.weekly.as_mut().unwrap().used_percent = 49.0;
            footer.claude.borrow_mut().latest = sample;
            render_cards(&footer, true, true, now);
            let theme = gtk4::CssProvider::new();
            theme.load_from_string(&crate::theme::resolve("warp_dark", true).0.css());
            gtk4::style_context_add_provider_for_display(
                &gtk4::gdk::Display::default().unwrap(),
                &theme,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            footer.root.add_css_class("cmux-root");
            let window = gtk4::Window::builder()
                .child(&footer.root)
                .default_width(230)
                .resizable(false)
                .decorated(false)
                .build();
            window.present();
            let context = glib::MainContext::default();
            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(200) {
                while context.pending() {
                    context.iteration(false);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let snapshot = gtk4::Snapshot::new();
            gtk4::WidgetPaintable::new(Some(&footer.root)).snapshot(&snapshot, footer.root.width() as f64, footer.root.height() as f64);
            let node = snapshot.to_node().unwrap();
            window.renderer().unwrap().render_texture(node, None).save_to_png(path).unwrap();
            window.close();
        }
    }
}
