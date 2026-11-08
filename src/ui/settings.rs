//! Settings dialog. Every option lives on one of five short pages, grouped
//! by what it affects:
//!
//! | Page          | Groups                                                        |
//! |---------------|---------------------------------------------------------------|
//! | Appearance    | Theme (quick picks + "Browse all themes"), Window, Text and Cursor |
//! | Terminal      | Shell, Prompt, Typing Assistance                              |
//! | Notifications | Desktop Notifications, When to Notify, Agent Hooks            |
//! | Shortcuts     | Workspaces, Panes, Typing and Search, View, App               |
//! | Workflow      | Sessions, Custom Commands, Control Socket                     |
//!
//! Changes apply (and are saved) immediately. The header's search finds any
//! setting by name.

use super::AppRc;
use crate::config::{Config, CustomCommand, DEFAULT_SHORTCUTS};
use gtk4::prelude::*;
use gtk4::{glib, pango};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// Applies an edit to the live config, validates, saves and re-applies it.
type Ap = Rc<dyn Fn(&dyn Fn(&mut Config))>;

pub const PAGE_APPEARANCE: &str = "appearance";
pub const PAGE_TERMINAL: &str = "terminal";
pub const PAGE_NOTIFICATIONS: &str = "notifications";
pub const PAGE_SHORTCUTS: &str = "shortcuts";
pub const PAGE_WORKFLOW: &str = "workflow";

/// Shortcut sections, in display order. Actions not listed land in "Other".
pub const SHORTCUT_SECTIONS: &[(&str, &[&str])] = &[
    (
        "Workspaces",
        &[
            "new-workspace",
            "close-workspace",
            "rename-workspace",
            "next-workspace",
            "prev-workspace",
        ],
    ),
    (
        "Panes",
        &[
            "split-right",
            "split-down",
            "close-pane",
            "focus-left",
            "focus-right",
            "focus-up",
            "focus-down",
        ],
    ),
    (
        "Typing and Search",
        &["newline", "newline-alt", "copy", "paste", "search", "command-history"],
    ),
    ("View", &["toggle-sidebar", "zoom-in", "zoom-out", "zoom-reset"]),
    ("App", &["command-palette", "jump-unread", "notifications", "settings"]),
];

/// Open Settings, optionally on a given page (one of the `PAGE_*` names).
pub fn show(app: &AppRc, page: Option<&str>) {
    let cfg = app.inner.borrow().config.clone();
    let d = adw::PreferencesDialog::new();
    d.set_title("Settings");
    d.set_search_enabled(true);
    d.set_content_width(720);
    d.set_content_height(680);
    let ap = applier(app);

    d.add(&appearance_page(app, &d, &cfg, &ap));
    d.add(&terminal_page(app, &cfg, &ap));
    d.add(&notifications_page(app, &cfg, &ap));
    d.add(&shortcuts_page(app, &cfg, &ap));
    d.add(&workflow_page(&cfg, &ap));
    if let Some(p) = page {
        d.set_visible_page_name(p);
    }
    d.present(Some(&app.window));
}

fn applier(app: &AppRc) -> Ap {
    let w = app.weak();
    Rc::new(move |f: &dyn Fn(&mut Config)| {
        let Some(a) = w.upgrade() else { return };
        let mut c = a.inner.borrow().config.clone();
        f(&mut c);
        if c == a.inner.borrow().config {
            return;
        }
        if let Err(e) = c.validate() {
            a.toast(&e);
            return;
        }
        let sidebar_changed = c.appearance.sidebar_visible != a.inner.borrow().config.appearance.sidebar_visible;
        a.inner.borrow_mut().config = c.clone();
        a.save_config();
        a.apply_config();
        if sidebar_changed {
            a.set_sidebar_visible(c.appearance.sidebar_visible);
        }
        a.refresh();
        for (k, acts) in c.shortcut_conflicts() {
            a.toast(&format!("Shortcut {k} is used by: {}", acts.join(", ")));
        }
    })
}

// ---- row helpers -------------------------------------------------------------

fn page(name: &str, title: &str, icon: &str) -> adw::PreferencesPage {
    adw::PreferencesPage::builder().name(name).title(title).icon_name(icon).build()
}

fn group(page: &adw::PreferencesPage, title: &str, description: &str) -> adw::PreferencesGroup {
    let g = adw::PreferencesGroup::builder().title(title).build();
    if !description.is_empty() {
        g.set_description(Some(description));
    }
    page.add(&g);
    g
}

fn switch(g: &adw::PreferencesGroup, ap: &Ap, title: &str, subtitle: &str, active: bool, set: fn(&mut Config, bool)) -> adw::SwitchRow {
    let r = adw::SwitchRow::builder().title(title).active(active).build();
    if !subtitle.is_empty() {
        r.set_subtitle(subtitle);
    }
    g.add(&r);
    let ap = ap.clone();
    r.connect_active_notify(move |r| {
        let v = r.is_active();
        ap(&move |c| set(c, v));
    });
    r
}

#[allow(clippy::too_many_arguments)]
fn spin(
    g: &adw::PreferencesGroup,
    ap: &Ap,
    title: &str,
    subtitle: &str,
    (min, max, step): (f64, f64, f64),
    value: f64,
    set: fn(&mut Config, f64),
) -> adw::SpinRow {
    let r = adw::SpinRow::with_range(min, max, step);
    r.set_title(title);
    if !subtitle.is_empty() {
        r.set_subtitle(subtitle);
    }
    r.set_value(value);
    g.add(&r);
    let ap = ap.clone();
    r.connect_value_notify(move |r| {
        let v = r.value();
        ap(&move |c| set(c, v));
    });
    r
}

#[allow(clippy::too_many_arguments)]
fn combo(
    g: &adw::PreferencesGroup,
    ap: &Ap,
    title: &str,
    subtitle: &str,
    labels: &[&str],
    values: &'static [&'static str],
    current: &str,
    set: fn(&mut Config, &str),
) -> adw::ComboRow {
    let r = adw::ComboRow::builder().title(title).model(&gtk4::StringList::new(labels)).build();
    if !subtitle.is_empty() {
        r.set_subtitle(subtitle);
    }
    r.set_selected(values.iter().position(|v| *v == current).unwrap_or(0) as u32);
    g.add(&r);
    let ap = ap.clone();
    r.connect_selected_notify(move |r| {
        let v = values[(r.selected() as usize).min(values.len() - 1)];
        ap(&move |c| set(c, v));
    });
    r
}

// ---- pages -------------------------------------------------------------------

fn appearance_page(app: &AppRc, d: &adw::PreferencesDialog, cfg: &Config, ap: &Ap) -> adw::PreferencesPage {
    let p = page(PAGE_APPEARANCE, "Appearance", "applications-graphics-symbolic");

    // Built detached, then placed under "Browse all themes".
    let style_group = adw::PreferencesGroup::new();
    let style = combo(
        &style_group,
        ap,
        "Light or dark",
        "For the Default theme; other themes set their own",
        &["Follow system", "Light", "Dark"],
        &["system", "light", "dark"],
        &cfg.appearance.mode,
        |c, v| c.appearance.mode = v.to_string(),
    );
    style_group.remove(&style);
    let (picks, more) = super::themes::settings_groups(app, d, &[style.upcast_ref()]);
    p.add(&picks);
    p.add(&more);

    let win = group(&p, "Window", "");
    switch(
        &win,
        ap,
        "Show sidebar",
        "Workspaces, git branch, ports and agent status",
        cfg.appearance.sidebar_visible,
        |c, v| c.appearance.sidebar_visible = v,
    );
    switch(
        &win,
        ap,
        "Context chips",
        "Python environment, folder and git branch above each terminal",
        cfg.appearance.chips,
        |c, v| c.appearance.chips = v,
    );

    let text = group(&p, "Text and Cursor", "");
    let font_btn = gtk4::FontDialogButton::new(Some(gtk4::FontDialog::builder().title("Terminal font").build()));
    font_btn.set_font_desc(&pango::FontDescription::from_string(&cfg.appearance.font));
    font_btn.set_valign(gtk4::Align::Center);
    font_btn.set_use_font(true);
    let font = adw::ActionRow::builder()
        .title("Font")
        .subtitle("Ctrl+= and Ctrl+- zoom without changing it")
        .build();
    font.add_suffix(&font_btn);
    font.set_activatable_widget(Some(&font_btn));
    text.add(&font);
    {
        let ap = ap.clone();
        font_btn.connect_font_desc_notify(move |b| {
            if let Some(fd) = b.font_desc() {
                let s = fd.to_string();
                ap(&move |c| c.appearance.font = s.clone());
            }
        });
    }
    switch(
        &text,
        ap,
        "Vivid output colours",
        "Turn pastel, near-white theme colours into vivid ones so git, ls, grep and compiler output stands out",
        cfg.terminal.vivid_colors,
        |c, v| c.terminal.vivid_colors = v,
    );
    combo(
        &text,
        ap,
        "Cursor shape",
        "",
        &["█  Block", "▏  Vertical line", "▁  Horizontal line"],
        &["block", "ibeam", "underline"],
        &cfg.terminal.cursor_shape,
        |c, v| c.terminal.cursor_shape = v.to_string(),
    );
    switch(&text, ap, "Blinking cursor", "", cfg.terminal.cursor_blink, |c, v| {
        c.terminal.cursor_blink = v
    });
    p
}

fn terminal_page(app: &AppRc, cfg: &Config, ap: &Ap) -> adw::PreferencesPage {
    let p = page(PAGE_TERMINAL, "Terminal", "utilities-terminal-symbolic");

    let shell = group(
        &p,
        "Shell",
        "Leave the program empty to use your login shell ($SHELL). Applies to new terminals.",
    );
    let prog = adw::EntryRow::builder()
        .title("Shell program")
        .text(&cfg.terminal.shell)
        .show_apply_button(true)
        .build();
    shell.add(&prog);
    {
        let ap = ap.clone();
        let w = app.weak();
        prog.connect_apply(move |r| {
            let text = r.text().trim().to_string();
            if !text.is_empty() && glib::find_program_in_path(&text).is_none() {
                if let Some(a) = w.upgrade() {
                    a.toast(&format!("Shell {text:?} was not found"));
                }
                return;
            }
            ap(&move |c| c.terminal.shell = text.clone());
        });
    }
    spin(
        &shell,
        ap,
        "Scrollback lines",
        "Output kept per terminal; 0 keeps none",
        (0.0, 1_000_000.0, 1000.0),
        cfg.terminal.scrollback_lines as f64,
        |c, v| c.terminal.scrollback_lines = v as i64,
    );
    switch(
        &shell,
        ap,
        "Confirm before closing running programs",
        "Ask before closing a pane or workspace where something is still running",
        cfg.terminal.confirm_close_running,
        |c, v| c.terminal.confirm_close_running = v,
    );

    let prompt = group(&p, "Prompt", "Changes apply to new terminals.");
    let compact = switch(
        &prompt,
        ap,
        "Compact prompt",
        "Show just ❯ (red after a failed command); folder and branch are in the chips",
        cfg.terminal.compact_prompt,
        |c, v| c.terminal.compact_prompt = v,
    );
    let highlight = switch(
        &prompt,
        ap,
        "Highlight typed commands",
        "Commands you type are bold cyan so they stand apart from output (needs the compact prompt)",
        cfg.terminal.highlight_commands,
        |c, v| c.terminal.highlight_commands = v,
    );
    compact.bind_property("active", &highlight, "sensitive").sync_create().build();
    switch(
        &prompt,
        ap,
        "Command blocks",
        "A line and a header above every command: folder, branch, exit status and duration",
        cfg.terminal.command_blocks,
        |c, v| c.terminal.command_blocks = v,
    );

    let typing = group(&p, "Typing Assistance", "");
    switch(
        &typing,
        ap,
        "Command suggestions",
        "Show the newest matching command from your history as you type; press → to accept",
        cfg.terminal.suggestions,
        |c, v| c.terminal.suggestions = v,
    );
    switch(
        &typing,
        ap,
        "History panel on ↑",
        "Pick from recent commands; Enter runs, Tab inserts, Esc closes",
        cfg.terminal.history_panel,
        |c, v| c.terminal.history_panel = v,
    );
    p
}

fn notifications_page(app: &AppRc, cfg: &Config, ap: &Ap) -> adw::PreferencesPage {
    let p = page(PAGE_NOTIFICATIONS, "Notifications", "preferences-system-notifications-symbolic");
    let n = &cfg.notifications;

    let desk = group(
        &p,
        "Desktop Notifications",
        "Sent when the window or the pane is not in focus. Clicking one jumps to its pane.",
    );
    let desktop = switch(
        &desk,
        ap,
        "Desktop notifications",
        "An agent waiting for input stays on screen until you handle it",
        n.desktop,
        |c, v| c.notifications.desktop = v,
    );
    let sound = switch(&desk, ap, "Sound", "Play a sound from the system sound theme", n.sound, |c, v| {
        c.notifications.sound = v
    });
    desktop.bind_property("active", &sound, "sensitive").sync_create().build();

    let when = group(&p, "When to Notify", "");
    let cmd_done = switch(
        &when,
        ap,
        "Long command finishes",
        "Shell commands in panes you are not looking at",
        n.command_finished,
        |c, v| c.notifications.command_finished = v,
    );
    let cmd_min = spin(
        &when,
        ap,
        "Long means at least (seconds)",
        "",
        (1.0, 3600.0, 1.0),
        n.command_min_seconds as f64,
        |c, v| c.notifications.command_min_seconds = v as u64,
    );
    cmd_done.bind_property("active", &cmd_min, "sensitive").sync_create().build();
    switch(
        &when,
        ap,
        "Terminal bell",
        "A program ringing the bell marks its pane for attention",
        n.bell_attention,
        |c, v| c.notifications.bell_attention = v,
    );

    let hooks = group(
        &p,
        "Agent Hooks",
        "Let coding agents report when they need input or finish. Writes to the agent's own settings file and keeps a backup of it.",
    );
    for agent in crate::agents::SUPPORTED {
        hooks.add(&hook_row(app, agent));
    }
    p
}

fn shortcut_label(action: &str) -> &'static str {
    DEFAULT_SHORTCUTS
        .iter()
        .find(|(a, _, _)| *a == action)
        .map(|(_, _, l)| *l)
        .unwrap_or("")
}

fn shortcut_default(action: &str) -> &'static str {
    DEFAULT_SHORTCUTS
        .iter()
        .find(|(a, _, _)| *a == action)
        .map(|(_, d, _)| *d)
        .unwrap_or("")
}

/// Absolute path of our `cmux` CLI for hook commands.
fn cmux_cli() -> String {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("cmux")))
        .filter(|p| p.is_file());
    beside
        .or_else(|| glib::find_program_in_path("cmux"))
        .and_then(|p| std::fs::canonicalize(p).ok())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "cmux".into())
}

fn hook_status(agent: &str) -> (bool, String) {
    let home = crate::paths::home();
    if crate::agents::hooks_installed(&home, agent) {
        let file = crate::agents::hook_file(&home, agent)
            .map(|f| crate::meta::display_path(&f))
            .unwrap_or_default();
        return (true, format!("On, in {file}"));
    }
    let found = glib::find_program_in_path(crate::agents::binary(agent)).is_some();
    (
        false,
        if found {
            "Off".into()
        } else {
            format!("Off · {} not found on PATH", crate::agents::binary(agent))
        },
    )
}

fn hook_row(app: &AppRc, agent: &'static str) -> adw::SwitchRow {
    let (on, status) = hook_status(agent);
    let row = adw::SwitchRow::builder()
        .title(crate::agents::display_name(agent))
        .subtitle(glib::markup_escape_text(&status).as_str())
        .active(on)
        .build();
    let busy = Rc::new(Cell::new(false));
    let w = app.weak();
    row.connect_active_notify(move |r| {
        if busy.get() {
            return;
        }
        let Some(a) = w.upgrade() else { return };
        let home = crate::paths::home();
        let want = r.is_active();
        let res = if want {
            crate::agents::install(agent, &home, &cmux_cli())
        } else {
            crate::agents::uninstall(agent, &home)
        };
        let name = crate::agents::display_name(agent);
        match res {
            Ok(_) if want && agent == "codex" => a.toast(&format!(
                "{name} hooks installed. Restart Codex, then open /hooks to review and trust them"
            )),
            Ok(_) if want => a.toast(&format!("{name} hooks installed; restart running {name} sessions")),
            Ok(_) => a.toast(&format!("{name} hooks removed")),
            Err(e) => {
                a.toast(&format!("{name}: {e}"));
                busy.set(true);
                r.set_active(!want);
                busy.set(false);
            }
        }
        let (_, status) = hook_status(agent);
        r.set_subtitle(glib::markup_escape_text(&status).as_str());
    });
    row
}

fn shortcuts_page(app: &AppRc, cfg: &Config, ap: &Ap) -> adw::PreferencesPage {
    let p = page(PAGE_SHORTCUTS, "Shortcuts", "preferences-desktop-keyboard-shortcuts-symbolic");
    p.set_description("Type a GTK accelerator such as &lt;Ctrl&gt;&lt;Shift&gt;e and press Enter. Leave empty to disable.");
    let effective: std::collections::HashMap<String, String> = cfg.effective_shortcuts().into_iter().collect();
    let mut sections: Vec<(&str, Vec<&str>)> = SHORTCUT_SECTIONS.iter().map(|(t, a)| (*t, a.to_vec())).collect();
    let other: Vec<&str> = DEFAULT_SHORTCUTS
        .iter()
        .map(|(a, _, _)| *a)
        .filter(|a| !SHORTCUT_SECTIONS.iter().any(|(_, l)| l.contains(a)))
        .collect();
    if !other.is_empty() {
        sections.push(("Other", other));
    }

    let mut rows: Vec<(String, adw::EntryRow)> = Vec::new();
    let mut first: Option<adw::PreferencesGroup> = None;
    for (title, actions) in sections {
        let g = group(&p, title, "");
        for action in actions {
            let row = adw::EntryRow::builder()
                .title(shortcut_label(action))
                .text(effective.get(action).map(String::as_str).unwrap_or(""))
                .show_apply_button(true)
                .build();
            row.set_tooltip_text(Some(&format!("Default: {}", shortcut_default(action))));
            g.add(&row);
            let ap = ap.clone();
            let w = app.weak();
            let action_s = action.to_string();
            let default = shortcut_default(action).to_string();
            row.connect_apply(move |r| {
                let text = r.text().trim().to_string();
                if !text.is_empty() && gtk4::accelerator_parse(&text).is_none() {
                    if let Some(a) = w.upgrade() {
                        a.toast(&format!("{text:?} is not a valid shortcut"));
                    }
                    return;
                }
                let (action, default) = (action_s.clone(), default.clone());
                ap(&move |c| {
                    if text == default {
                        c.shortcuts.remove(&action);
                    } else {
                        c.shortcuts.insert(action.clone(), text.clone());
                    }
                });
            });
            rows.push((action.to_string(), row));
        }
        first.get_or_insert(g);
    }

    let reset = gtk4::Button::builder()
        .label("Reset All")
        .valign(gtk4::Align::Center)
        .tooltip_text("Restore every default shortcut")
        .build();
    reset.add_css_class("flat");
    {
        let ap = ap.clone();
        reset.connect_clicked(move |_| {
            ap(&|c| c.shortcuts.clear());
            for (action, row) in &rows {
                row.set_text(shortcut_default(action));
            }
        });
    }
    if let Some(g) = first {
        g.set_header_suffix(Some(&reset));
    }
    p
}

fn workflow_page(cfg: &Config, ap: &Ap) -> adw::PreferencesPage {
    let p = page(PAGE_WORKFLOW, "Workflow", "system-run-symbolic");

    let sess = group(&p, "Sessions", "");
    switch(
        &sess,
        ap,
        "Restore workspaces on start",
        "Workspaces, layouts, folders and agent sessions come back; commands are never re-run",
        cfg.session.restore,
        |c, v| c.session.restore = v,
    );
    spin(
        &sess,
        ap,
        "Replay previous output (lines)",
        "0 = off: terminals start clean. Replayed output is saved to disk",
        (0.0, 5000.0, 100.0),
        cfg.session.replay_lines as f64,
        |c, v| c.session.replay_lines = v as usize,
    );

    let cg = group(
        &p,
        "Custom Commands",
        "Shown in the command palette (Ctrl+Shift+P) as \"Run: name\". One per line, for example: build = cargo build --release",
    );
    let buf = gtk4::TextBuffer::new(None);
    buf.set_text(
        &cfg.commands
            .iter()
            .map(|c| format!("{} = {}", c.name, c.command))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let tv = gtk4::TextView::builder()
        .buffer(&buf)
        .monospace(true)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .height_request(140)
        .build();
    tv.set_wrap_mode(gtk4::WrapMode::WordChar);
    let frame = gtk4::Frame::builder().child(&tv).build();
    frame.add_css_class("view");
    cg.add(&frame);
    {
        let ap = ap.clone();
        buf.connect_changed(move |b| {
            let text = b.text(&b.start_iter(), &b.end_iter(), false).to_string();
            let cmds = parse_custom_commands(&text);
            ap(&move |c| c.commands = cmds.clone());
        });
    }

    let ipc = group(
        &p,
        "Control Socket",
        "How the cmux CLI, scripts and agent hooks talk to this window.",
    );
    let enabled = switch(
        &ipc,
        ap,
        "Enable control socket",
        "Needed for agent notifications. Takes effect after restarting cmux",
        cfg.ipc.enabled,
        |c, v| c.ipc.enabled = v,
    );
    let send = switch(
        &ipc,
        ap,
        "Allow cmux send to type into panes",
        "Off by default; only your own local programs can connect",
        cfg.ipc.allow_send,
        |c, v| c.ipc.allow_send = v,
    );
    enabled.bind_property("active", &send, "sensitive").sync_create().build();
    p
}

/// `name = command` lines; blank or incomplete lines are skipped.
pub fn parse_custom_commands(text: &str) -> Vec<CustomCommand> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(n, c)| CustomCommand {
            name: n.trim().to_string(),
            command: c.trim().to_string(),
        })
        .filter(|c| !c.name.is_empty() && !c.command.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shortcut_is_in_exactly_one_section() {
        for (action, _, _) in DEFAULT_SHORTCUTS {
            let n = SHORTCUT_SECTIONS.iter().filter(|(_, l)| l.contains(action)).count();
            assert_eq!(n, 1, "{action} is in {n} sections");
        }
        for (_, list) in SHORTCUT_SECTIONS {
            for a in *list {
                assert!(DEFAULT_SHORTCUTS.iter().any(|(d, _, _)| d == a), "unknown action {a}");
            }
        }
    }

    #[test]
    fn custom_commands_parse_skips_incomplete_lines() {
        let c = parse_custom_commands("build = cargo build\n\nbad line\n = x\ntest=cargo test -- --nocapture=1");
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].name, "build");
        assert_eq!(c[1].command, "cargo test -- --nocapture=1");
    }
}
