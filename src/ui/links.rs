//! Clickable links in terminal output.
//!
//! URLs that programs print are detected with a regex (VTE shows a hand
//! cursor and underlines them on hover); explicit OSC 8 hyperlinks work too.
//!
//! - Click a link: a small popover offers **Open Link** and **Copy Link**.
//! - Ctrl+click a link: open it right away in the default browser.
//! - Right-click a link: the context menu starts with the same two actions.
//!
//! File positions (`src/model.rs:230:9`, Python's `File "x.py", line 12`) are
//! links too when the file exists, relative to the pane's folder: they offer
//! **Open in Editor** and **Copy Path** instead (see [`crate::editor`]).
//!
//! A click that selects text (drag, double-click) never triggers anything.

use super::{App, AppRc};
use crate::editor;
use crate::model::{Orient, PaneId};
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use vte4::prelude::*;

use super::pane::{PCRE2_CASELESS, PCRE2_MULTILINE, PCRE2_UTF};

/// URLs in output. Trailing `.`, `,`, `:`, `;`, `!`, `?` are not part of the
/// link; box-drawing characters end it, so URLs inside TUI frames stay clean.
///
/// Only flat character-class runs: VTE matches with a PCRE2 depth limit of
/// 64, and a repeated group (e.g. for balanced parentheses) costs one level
/// per character, so long URLs would silently stop matching. An unbalanced
/// closing `)` is removed afterwards by [`trim_link`].
pub const URL_PATTERN: &str = concat!(
    r#"(?:\b(?:https?|ftp|file)://|\bmailto:|\bwww\.)"#,
    r#"[^\s<>"'`{}\[\]\x{2500}-\x{259f}]*"#,
    r#"[^\s<>"'`{}\[\].,;:!?\x{2500}-\x{259f}]"#,
);

/// `path:line[:col]` as compilers, test runners and agents print it. The file
/// name needs an extension starting with a letter, so clock times
/// (`12:30:45`) and addresses (`10.0.0.1:8080`) never match, and it may not
/// start in the middle of a word or path (URLs keep their own match).
/// Flat character classes only, for the same depth limit as [`URL_PATTERN`].
pub const FILE_PATTERN: &str = concat!(
    r#"(?<![\w./~@+-])(?:~/|\.{1,2}/|/)?"#,
    r#"[\w@+-][\w.@+/-]*\.[A-Za-z]\w*"#,
    r#":[0-9]+(?::[0-9]+)?"#,
);

/// Python tracebacks: `File "app/main.py", line 12`.
pub const PY_FILE_PATTERN: &str = r#"File "[^"\r\n]+", line [0-9]+"#;

/// What a click on the terminal points at.
#[derive(Debug, Clone, PartialEq)]
pub enum Link {
    Url(String),
    File { path: PathBuf, line: u32, col: Option<u32> },
}

impl Link {
    /// Text shown in the popover and copied by "Copy".
    fn text(&self) -> String {
        match self {
            Link::Url(u) => u.clone(),
            Link::File { path, line, col } => match col {
                Some(c) => format!("{}:{line}:{c}", path.display()),
                None => format!("{}:{line}", path.display()),
            },
        }
    }
}

/// Drop a closing `)` that belongs to the surrounding text, as in
/// `(see https://example.com/a)`, keeping balanced ones (`…/Foo_(bar)`).
pub fn trim_link(link: &str) -> &str {
    let mut s = link;
    loop {
        let open = s.matches('(').count();
        let close = s.matches(')').count();
        if close > open && s.ends_with(')') {
            s = s[..s.len() - 1].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        } else {
            return s;
        }
    }
}

/// What to hand to the browser: `www.example.com` gets `https://`.
pub fn normalize(link: &str) -> String {
    if link.len() >= 4 && link[..4].eq_ignore_ascii_case("www.") {
        format!("https://{link}")
    } else {
        link.to_string()
    }
}

/// Pointer movement smaller than this between press and release is a click.
const CLICK_SLOP: f64 = 4.0;

/// Where the terminal's links resolve: its regex tags for file positions and
/// the pane whose folder relative paths are relative to.
struct Ctx {
    app: Weak<App>,
    pane: PaneId,
    file_tags: Vec<i32>,
}

impl Ctx {
    fn cwd(&self) -> Option<PathBuf> {
        let app = self.app.upgrade()?;
        let inner = app.inner.try_borrow().ok()?;
        inner.panes.get(&self.pane)?.cwd.clone()
    }

    /// The link (OSC 8 hyperlink, detected URL, or an existing file) under
    /// widget point `x, y`.
    fn link_at(&self, term: &vte4::Terminal, x: f64, y: f64) -> Option<Link> {
        if let Some(h) = term.check_hyperlink_at(x, y) {
            return Some(Link::Url(h.to_string()));
        }
        let (text, tag) = term.check_match_at(x, y);
        let text = text?;
        if !self.file_tags.contains(&tag) {
            return Some(Link::Url(trim_link(&text).to_string()));
        }
        let r = editor::parse_file_ref(&text)?;
        let path = editor::resolve(&r.path, self.cwd().as_deref(), &crate::paths::home())?;
        Some(Link::File {
            path,
            line: r.line,
            col: r.col,
        })
    }
}

/// Open a link: URLs in the browser, files in the editor.
fn activate(ctx: &Ctx, term: &vte4::Terminal, link: &Link) {
    match link {
        Link::Url(u) => open(&ctx.app, term, u),
        Link::File { path, line, col } => open_file(ctx, term, path, *line, *col),
    }
}

fn copy_link(ctx: &Ctx, term: &vte4::Terminal, link: &Link) {
    term.clipboard().set_text(&link.text());
    if let Some(a) = ctx.app.upgrade() {
        a.toast(match link {
            Link::Url(_) => "Link copied",
            Link::File { .. } => "Path copied",
        });
    }
}

/// Quote `s` for a POSIX shell.
fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-+:,=@%".contains(&b)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Open `path` at `line` in the user's editor. Terminal editors get a new
/// pane to the right; with no editor found the desktop's default app opens
/// the file (without the line).
fn open_file(ctx: &Ctx, term: &vte4::Terminal, path: &Path, line: u32, col: Option<u32>) {
    let Some(app) = ctx.app.upgrade() else { return };
    let configured = app.inner.borrow().config.editor.command.clone();
    let env = |k| std::env::var(k).ok();
    let chosen = editor::choose_editor(&configured, env("VISUAL").as_deref(), env("EDITOR").as_deref(), |p| {
        glib::find_program_in_path(p).is_some()
    });
    let Some(ed) = chosen else {
        open(&ctx.app, term, &gio::File::for_path(path).uri());
        return;
    };
    let argv = editor::editor_argv(&ed, path, line, col);
    let dir = path.parent().map(Path::to_path_buf);
    if editor::is_terminal_editor(&ed) {
        let cmd: Vec<String> = argv.iter().map(|a| shell_quote(a)).collect();
        match app.split(Some(ctx.pane), Orient::Horizontal, dir) {
            Ok(new) => {
                if let Some(p) = app.inner.borrow().panes.get(&new) {
                    p.send_input(format!("{}\n", cmd.join(" ")).as_bytes());
                }
            }
            Err(e) => app.toast(&format!("Couldn't open {}: {e}", path.display())),
        }
        return;
    }
    let mut command = std::process::Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(d) = &dir {
        command.current_dir(d);
    }
    match command.spawn() {
        // Reap it so it doesn't linger as a zombie (GUI editors fork and exit).
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => app.toast(&format!("Couldn't start {}: {e}", argv[0])),
    }
}

pub fn open(app: &Weak<App>, term: &vte4::Terminal, link: &str) {
    let uri = normalize(link);
    let win = term.root().and_downcast::<gtk4::Window>();
    let w = app.clone();
    let shown = uri.clone();
    gtk4::UriLauncher::new(&uri).launch(win.as_ref(), gio::Cancellable::NONE, move |res| {
        if let Err(e) = res {
            if e.matches(gtk4::DialogError::Dismissed) || e.matches(gtk4::DialogError::Cancelled) {
                return;
            }
            if let Some(a) = w.upgrade() {
                a.toast(&format!("Couldn't open {shown}: {}", e.message()));
            }
        }
    });
}

/// The link popover's parts that change between URLs and files.
struct Popover {
    pop: gtk4::Popover,
    url: gtk4::Label,
    open: adw::ButtonContent,
    copy: adw::ButtonContent,
    hint: gtk4::Label,
}

impl Popover {
    fn show(&self, link: &Link) {
        let (text, open, copy, icon, hint) = match link {
            Link::Url(u) => (
                normalize(u),
                "Open Link",
                "Copy Link",
                "web-browser-symbolic",
                "Tip: Ctrl+Click opens links directly",
            ),
            Link::File { .. } => (
                link.text(),
                "Open in Editor",
                "Copy Path",
                "document-edit-symbolic",
                "Tip: Ctrl+Click opens the file directly",
            ),
        };
        self.url.set_label(&text);
        self.url.set_tooltip_text(Some(&text));
        self.open.set_label(open);
        self.open.set_icon_name(icon);
        self.copy.set_label(copy);
        self.hint.set_label(hint);
    }
}

/// Popover shown when a link is clicked.
fn build_popover(term: &vte4::Terminal) -> Popover {
    let url = gtk4::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk4::pango::EllipsizeMode::Middle)
        .max_width_chars(56)
        .build();
    url.add_css_class("link-url");
    url.add_css_class("monospace");
    let button = |icon: &str, label: &str, action: &str| {
        let content = adw::ButtonContent::builder().icon_name(icon).label(label).build();
        let b = gtk4::Button::builder().child(&content).action_name(action).hexpand(true).build();
        (b, content)
    };
    let (open_btn, open_content) = button("web-browser-symbolic", "Open Link", "link.open");
    open_btn.add_css_class("suggested-action");
    let (copy_btn, copy_content) = button("edit-copy-symbolic", "Copy Link", "link.copy");
    let hint = gtk4::Label::new(Some("Tip: Ctrl+Click opens links directly"));
    hint.add_css_class("link-hint");
    hint.set_xalign(0.0);

    let buttons = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    buttons.set_homogeneous(true);
    buttons.append(&open_btn);
    buttons.append(&copy_btn);
    let col = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    col.set_margin_top(4);
    col.set_margin_bottom(4);
    col.set_margin_start(4);
    col.set_margin_end(4);
    col.append(&url);
    col.append(&buttons);
    col.append(&hint);

    let pop = gtk4::Popover::builder().child(&col).position(gtk4::PositionType::Bottom).build();
    pop.add_css_class("link-popover");
    pop.set_parent(term);
    for b in [&open_btn, &copy_btn] {
        let p = pop.clone();
        b.connect_clicked(move |_| p.popdown());
    }
    {
        // Popovers are not owned by VTE; release it with the terminal.
        let p = pop.clone();
        term.connect_destroy(move |_| p.unparent());
    }
    Popover {
        pop,
        url,
        open: open_content,
        copy: copy_content,
        hint,
    }
}

/// Make links in `term` clickable. `menu` is the pane's normal context menu;
/// over a link it is shown with "Open Link" / "Copy Link" on top.
/// Mark VTE's visible-row cache ("ringview") stale. Toggling BiDi and back
/// is the only public call that does this; the redraw it queues coincides
/// with the one the scroll already queued.
fn invalidate_row_cache(term: &vte4::Terminal) {
    let bidi = term.enables_bidi();
    term.set_enable_bidi(!bidi);
    term.set_enable_bidi(bidi);
}

/// VTE 0.76 (Ubuntu 24.04) moves the view on GtkAdjustment changes
/// (scrollbar, wheel, kinetic scrolling) without invalidating its row cache,
/// and `check_hyperlink_at` / `check_match_at` only rebuild a cache marked
/// stale. Our hover handler runs before VTE's own motion handler (which
/// would rebuild it), so a link check between a scroll and the next redraw
/// read outside the cache and crashed. Newer VTE always rebuilds; until
/// then, mark the cache stale ourselves whenever the view scrolls.
fn keep_row_cache_in_sync_with_scrolling(term: &vte4::Terminal) {
    let hook = |t: &vte4::Terminal| {
        if let Some(adj) = t.vadjustment() {
            let t = t.downgrade();
            adj.connect_value_changed(move |_| {
                if let Some(t) = t.upgrade() {
                    invalidate_row_cache(&t);
                }
            });
        }
    };
    hook(term);
    term.connect_notify_local(Some("vadjustment"), move |t, _| hook(t));
}

/// Register `pattern` as a clickable match; returns its tag.
fn add_match(term: &vte4::Terminal, pattern: &str, flags: u32) -> Option<i32> {
    match vte4::Regex::for_match(pattern, flags) {
        Ok(re) => {
            let tag = term.match_add_regex(&re, 0);
            term.match_set_cursor_name(tag, "pointer");
            Some(tag)
        }
        Err(e) => {
            eprintln!("oxishell: link regex: {e}");
            None
        }
    }
}

/// The pane's context menu with `open` / `copy` entries on top.
fn menu_with(menu: &gio::Menu, open: &str, copy: &str) -> gio::Menu {
    let section = gio::Menu::new();
    section.append(Some(open), Some("link.open"));
    section.append(Some(copy), Some("link.copy"));
    let m = gio::Menu::new();
    m.append_section(None, &section);
    for i in 0..menu.n_items() {
        if let Some(sec) = menu.item_link(i, gio::MENU_LINK_SECTION) {
            m.append_section(None, &sec);
        }
    }
    m
}

pub fn attach(app: &AppRc, pane: PaneId, term: &vte4::Terminal, menu: &gio::Menu) {
    keep_row_cache_in_sync_with_scrolling(term);
    // URLs first: where a file pattern could also match, the URL wins.
    add_match(term, URL_PATTERN, PCRE2_UTF | PCRE2_MULTILINE | PCRE2_CASELESS);
    let file_tags = [FILE_PATTERN, PY_FILE_PATTERN]
        .iter()
        .filter_map(|p| add_match(term, p, PCRE2_UTF | PCRE2_MULTILINE))
        .collect();
    let ctx = Rc::new(Ctx {
        app: app.weak(),
        pane,
        file_tags,
    });

    // The link the popover / context menu acts on.
    let current: Rc<RefCell<Option<Link>>> = Rc::new(RefCell::new(None));
    let group = gio::SimpleActionGroup::new();
    let make = |name: &str, f: fn(&Ctx, &vte4::Terminal, &Link)| {
        let act = gio::SimpleAction::new(name, None);
        let ctx = ctx.clone();
        let cur = current.clone();
        let t = term.downgrade();
        act.connect_activate(move |_, _| {
            let (Some(t), Some(link)) = (t.upgrade(), cur.borrow().clone()) else {
                return;
            };
            f(&ctx, &t, &link);
        });
        group.add_action(&act);
    };
    make("open", activate);
    make("copy", copy_link);
    term.insert_action_group("link", Some(&group));

    let menu_with_link = menu_with(menu, "Open Link", "Copy Link");
    let menu_with_file = menu_with(menu, "Open in Editor", "Copy Path");

    let popover = build_popover(term);

    // Pointer position in terminal coordinates; also shows a hint over links.
    let pos = Rc::new(Cell::new((0.0f64, 0.0f64)));
    let over_link = Rc::new(Cell::new(false));
    {
        let motion = gtk4::EventControllerMotion::new();
        let pos = pos.clone();
        let over_link = over_link.clone();
        let ctx = ctx.clone();
        motion.connect_motion(move |m, x, y| {
            pos.set((x, y));
            let Some(t) = m.widget().and_downcast::<vte4::Terminal>() else {
                return;
            };
            let on = ctx.link_at(&t, x, y).is_some();
            if on != over_link.get() {
                over_link.set(on);
                t.set_tooltip_text(on.then_some("Click for link options · Ctrl+Click to open"));
            }
        });
        term.add_controller(motion);
    }

    // Raw button events in the capture phase: seen before VTE's own handling
    // (selection, context menu) and never claimed, so VTE works as before.
    let legacy = gtk4::EventControllerLegacy::new();
    legacy.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let pressed_at: Cell<Option<(f64, f64)>> = Cell::new(None);
    let menu = menu.clone();
    legacy.connect_event(move |c, ev| {
        let Some(t) = c.widget().and_downcast::<vte4::Terminal>() else {
            return glib::Propagation::Proceed;
        };
        let Some(button) = ev.downcast_ref::<gdk::ButtonEvent>().map(|b| b.button()) else {
            return glib::Propagation::Proceed;
        };
        let (x, y) = pos.get();
        match (ev.event_type(), button) {
            (gdk::EventType::ButtonPress, 3) => {
                let link = ctx.link_at(&t, x, y);
                t.set_context_menu_model(Some(match link {
                    Some(Link::Url(_)) => &menu_with_link,
                    Some(Link::File { .. }) => &menu_with_file,
                    None => &menu,
                }));
                *current.borrow_mut() = link;
            }
            (gdk::EventType::ButtonPress, 1) => pressed_at.set(Some((x, y))),
            (gdk::EventType::ButtonRelease, 1) => {
                let Some((px, py)) = pressed_at.take() else {
                    return glib::Propagation::Proceed;
                };
                let still = (x - px).abs() < CLICK_SLOP && (y - py).abs() < CLICK_SLOP;
                if !still || t.has_selection() {
                    return glib::Propagation::Proceed;
                }
                let Some(link) = ctx.link_at(&t, x, y) else {
                    return glib::Propagation::Proceed;
                };
                if ev.modifier_state().contains(gdk::ModifierType::CONTROL_MASK) {
                    activate(&ctx, &t, &link);
                } else {
                    popover.show(&link);
                    *current.borrow_mut() = Some(link);
                    // The hover hint would sit on top of the popover.
                    t.set_tooltip_text(None);
                    over_link.set(false);
                    popover.pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                    popover.pop.popup();
                }
            }
            _ => {}
        }
        glib::Propagation::Proceed
    });
    term.add_controller(legacy);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_unbalanced_closing_parens() {
        assert_eq!(trim_link("https://x.com/a)"), "https://x.com/a");
        assert_eq!(trim_link("https://x.com/a.)"), "https://x.com/a");
        assert_eq!(
            trim_link("https://en.wikipedia.org/wiki/Foo_(bar)"),
            "https://en.wikipedia.org/wiki/Foo_(bar)"
        );
        assert_eq!(
            trim_link("https://en.wikipedia.org/wiki/Foo_(bar))"),
            "https://en.wikipedia.org/wiki/Foo_(bar)"
        );
        assert_eq!(trim_link("https://x.com/(a)/b"), "https://x.com/(a)/b");
    }

    #[test]
    fn normalizes_www_links() {
        assert_eq!(normalize("www.rust-lang.org"), "https://www.rust-lang.org");
        assert_eq!(normalize("WWW.example.com/x"), "https://WWW.example.com/x");
        assert_eq!(normalize("https://github.com"), "https://github.com");
        assert_eq!(normalize("mailto:a@b.c"), "mailto:a@b.c");
    }
}
