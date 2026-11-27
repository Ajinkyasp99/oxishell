//! Clickable links in terminal output.
//!
//! URLs that programs print are detected with a regex (VTE shows a hand
//! cursor and underlines them on hover); explicit OSC 8 hyperlinks work too.
//!
//! - Click a link: a small popover offers **Open Link** and **Copy Link**.
//! - Ctrl+click a link: open it right away in the default browser.
//! - Right-click a link: the context menu starts with the same two actions.
//!
//! Existing local paths offer a workspace preview, an editor, and Copy Path.
//!
//! A click that selects text (drag, double-click) never triggers anything.

use super::project_files::{self, FileMode};
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

// Keep a complete token, even when it has a location suffix. A partial
// filename can resolve to a different existing file in the same directory.
macro_rules! location_pattern {
    () => {
        r#"(?::[0-9]+(?::[0-9]+)?(?:[-–][0-9]+(?::[0-9]+)?)?|\#L[0-9]+(?:C[0-9]+)?(?:[-–]L[0-9]+(?:C[0-9]+)?)?)?"#
    };
}

const FILE_PATTERN: &str = concat!(
    r#"(?<![\p{L}\p{M}\p{N}_./~@+:%#-])(?:"#,
    r#"(?:~/|\.{1,2}/|/)[\p{L}\p{M}\p{N}_.@+%/-]+"#,
    r#"|[\p{L}\p{M}\p{N}_.@+%-]+/[\p{L}\p{M}\p{N}_.@+%/-]+"#,
    r#"|[\p{L}\p{N}_@+%-][\p{L}\p{M}\p{N}_.@+%-]*\.[\p{L}][\p{L}\p{M}\p{N}_.@+%-]*"#,
    r#"|\.[\p{L}_][\p{L}\p{M}\p{N}_.@+%-]*"#,
    r#"|[\p{L}_][\p{L}\p{M}\p{N}_@+%-]*(?=:[0-9]|\#L[0-9])"#,
    r#"|README|Dockerfile|Containerfile|Makefile|GNUmakefile|LICENSE|COPYING|NOTICE|CHANGELOG|Gemfile|Rakefile|Procfile|Justfile|justfile)"#,
    location_pattern!(),
    r#"(?![\p{L}\p{M}\p{N}_/~@+:%#-]|\.[\p{L}\p{M}\p{N}_.@+%/-])"#,
);

const QUOTED_FILE_PATTERN: &str = concat!(r#"(?:`[^`\r\n]+`|"[^"\r\n]+"|'[^'\r\n]+')"#, location_pattern!(),);

/// Python tracebacks: `File "app/main.py", line 12`.
pub const PY_FILE_PATTERN: &str = r#"File "[^"\r\n]+", line [0-9]+"#;

/// What a click on the terminal points at.
#[derive(Debug, Clone, PartialEq)]
enum Link {
    Url(String),
    File(FileTarget),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileTarget {
    pub path: PathBuf,
    pub location: Option<(u32, Option<u32>)>,
}

impl Link {
    /// Text shown in the popover and copied by "Copy".
    fn text(&self) -> String {
        match self {
            Link::Url(u) => u.clone(),
            Link::File(FileTarget { path, location }) => match location {
                Some((line, Some(col))) => format!("{}:{line}:{col}", path.display()),
                Some((line, None)) => format!("{}:{line}", path.display()),
                None => path.display().to_string(),
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
    if link.get(..4).is_some_and(|prefix| prefix.eq_ignore_ascii_case("www.")) {
        format!("https://{link}")
    } else {
        link.to_string()
    }
}

fn number(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|n| *n > 0)
}

fn location(text: &str) -> Option<(u32, Option<u32>)> {
    let (text, hash) = if let Some(text) = text.strip_prefix('#') {
        (text, true)
    } else {
        (text.strip_prefix(':')?, false)
    };
    let parse = |part: &str| {
        let (part, delimiter) = if hash { (part.strip_prefix('L')?, 'C') } else { (part, ':') };
        match part.split_once(delimiter) {
            Some((line, col)) if col.bytes().all(|c| c.is_ascii_digit()) && !col.is_empty() => Some((number(line)?, number(col))),
            Some(_) => None,
            None => Some((number(part)?, None)),
        }
    };
    if let Some((start, end)) = text.split_once(['-', '–']) {
        parse(end)?;
        parse(start)
    } else {
        parse(text)
    }
}

fn valid_path_text(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(char::is_control)
}

fn resolve_match(text: &str, cwd: Option<&Path>, home: &Path) -> Option<FileTarget> {
    let resolve = |text: &str, location| {
        if !valid_path_text(text) {
            return None;
        }
        editor::resolve(text, cwd, home).map(|path| FileTarget { path, location })
    };
    if text.starts_with("File \"") {
        let reference = editor::parse_file_ref(text)?;
        return resolve(&reference.path, Some((reference.line, reference.col)));
    }
    let resolve_path = |text: &str| {
        resolve(text, None).or_else(|| {
            text.match_indices([':', '#'])
                .find_map(|(i, _)| resolve(&text[..i], Some(location(&text[i..])?)))
        })
    };
    if let Some(quote @ ('"' | '\'' | '`')) = text.chars().next() {
        let (path, suffix) = text[1..].split_once(quote)?;
        return if suffix.is_empty() {
            resolve_path(path)
        } else {
            resolve(path, Some(location(suffix)?))
        };
    }
    resolve_path(text).or_else(|| {
        let trimmed = text.trim_end_matches('.');
        (trimmed != text).then(|| resolve_path(trimmed)).flatten()
    })
}

fn file_uri(text: &str) -> bool {
    text.get(..7).is_some_and(|scheme| scheme.eq_ignore_ascii_case("file://"))
}

fn resolve_file_uri(uri: &str) -> Option<FileTarget> {
    let (uri, location) = match uri.split_once('#') {
        Some((base, fragment)) => (base, location(&format!("#{fragment}"))),
        None => (uri, None),
    };
    if uri.contains('?') {
        return None;
    }
    let (path, host) = glib::filename_from_uri(uri).ok()?;
    if host.is_some_and(|host| !host.eq_ignore_ascii_case("localhost"))
        || path.to_str().is_some_and(|text| !valid_path_text(text))
        || !path.is_absolute()
    {
        return None;
    }
    if path.is_file() {
        return Some(FileTarget { path, location });
    }
    // Some terminal clients append a colon location before emitting the URI.
    if location.is_none() {
        return resolve_match(path.to_str()?, None, Path::new("/"));
    }
    None
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
            return uri_link(&h);
        }
        let (text, tag) = term.check_match_at(x, y);
        let text = text?;
        if !self.file_tags.contains(&tag) {
            return uri_link(trim_link(&text));
        }
        resolve_match(&text, self.cwd().as_deref(), &crate::paths::home()).map(Link::File)
    }
}

fn uri_link(text: &str) -> Option<Link> {
    if file_uri(text) {
        return resolve_file_uri(text).map(Link::File);
    }
    Some(Link::Url(text.to_string()))
}

fn activate(ctx: &Ctx, term: &vte4::Terminal, link: &Link) {
    match link {
        Link::Url(u) => open(&ctx.app, term, u),
        Link::File(target) => {
            if let Some(app) = ctx.app.upgrade() {
                project_files::open_file(&app, ctx.pane, target.clone(), FileMode::Preview);
            }
        }
    }
}

fn activate_editor(ctx: &Ctx, _term: &vte4::Terminal, link: &Link) {
    if let Link::File(target) = link {
        let app = ctx.app.clone();
        let pane = ctx.pane;
        let target = target.clone();
        // Let the menu return focus to the terminal before focusing the editor.
        glib::idle_add_local_once(move || {
            if let Some(app) = app.upgrade() {
                project_files::open_file(&app, pane, target, FileMode::Edit);
            }
        });
    }
}

fn activate_external_editor(ctx: &Ctx, _term: &vte4::Terminal, link: &Link) {
    if let (Some(app), Link::File(target)) = (ctx.app.upgrade(), link) {
        open_file(&app, ctx.pane, target);
    }
}

fn copy_link(ctx: &Ctx, term: &vte4::Terminal, link: &Link) {
    term.clipboard().set_text(&link.text());
    if let Some(a) = ctx.app.upgrade() {
        a.toast(match link {
            Link::Url(_) => "Link copied",
            Link::File(_) => "Path copied",
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
pub(super) fn open_file(app: &AppRc, pane: PaneId, target: &FileTarget) {
    let Some(term) = app.inner.borrow().panes.get(&pane).map(|p| p.term.clone()) else {
        return;
    };
    let path = &target.path;
    if !path.is_file() {
        app.toast(&format!("File is no longer available: {}", path.display()));
        return;
    }
    let configured = app.inner.borrow().config.editor.command.clone();
    let env = |k| std::env::var(k).ok();
    let chosen = editor::choose_editor(&configured, env("VISUAL").as_deref(), env("EDITOR").as_deref(), |p| {
        glib::find_program_in_path(p).is_some()
    });
    let Some(ed) = chosen.filter(|_| path.to_str().is_some()) else {
        open(&app.weak(), &term, &gio::File::for_path(path).uri());
        return;
    };
    let argv = editor::editor_argv_at(&ed, path, target.location);
    let dir = path.parent().map(Path::to_path_buf);
    if editor::is_terminal_editor(&ed) {
        let cmd: Vec<String> = argv.iter().map(|a| shell_quote(a)).collect();
        match app.split(Some(pane), Orient::Horizontal, dir) {
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
    open_button: gtk4::Button,
    editor: gtk4::Button,
    external_editor: gtk4::Button,
    copy: adw::ButtonContent,
    hint: gtk4::Label,
}

// Ctrl+Click opens directly; Cmd+Click on macOS, where Ctrl+Click is a right click.
#[cfg(not(target_os = "macos"))]
const LINK_HINT: &str = "Tip: Ctrl+Click opens links directly";
#[cfg(not(target_os = "macos"))]
const FILE_HINT: &str = "Tip: Ctrl+Click previews the file";
#[cfg(not(target_os = "macos"))]
const HOVER_HINT: &str = "Click for link options · Ctrl+Click to open";
#[cfg(target_os = "macos")]
const LINK_HINT: &str = "Tip: ⌘-Click opens links directly";
#[cfg(target_os = "macos")]
const FILE_HINT: &str = "Tip: ⌘-Click previews the file";
#[cfg(target_os = "macos")]
const HOVER_HINT: &str = "Click for link options · ⌘-Click to open";

impl Popover {
    fn show(&self, link: &Link) {
        let (text, open, copy, icon, hint) = match link {
            Link::Url(u) => (normalize(u), "Open Link", "Copy Link", "web-browser-symbolic", LINK_HINT),
            Link::File(_) => (link.text(), "Preview", "Copy Path", "document-open-symbolic", FILE_HINT),
        };
        self.url.set_label(&text);
        self.url.set_tooltip_text(Some(&text));
        self.open.set_label(open);
        self.open.set_icon_name(icon);
        let file = matches!(link, Link::File(_));
        self.editor.set_visible(file);
        self.external_editor.set_visible(file);
        if file {
            self.open_button.remove_css_class("suggested-action");
        } else {
            self.open_button.add_css_class("suggested-action");
        }
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
    let (editor_btn, _) = button("document-edit-symbolic", "Edit File", "link.edit");
    editor_btn.add_css_class("suggested-action");
    let (external_btn, _) = button("window-new-symbolic", "Open in External Editor", "link.editor");
    let (copy_btn, copy_content) = button("edit-copy-symbolic", "Copy Link", "link.copy");
    let hint = gtk4::Label::new(Some(LINK_HINT));
    hint.add_css_class("link-hint");
    hint.set_xalign(0.0);

    let buttons = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    buttons.set_homogeneous(true);
    buttons.append(&editor_btn);
    buttons.append(&open_btn);
    buttons.append(&copy_btn);
    let col = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    col.set_margin_top(4);
    col.set_margin_bottom(4);
    col.set_margin_start(4);
    col.set_margin_end(4);
    col.append(&url);
    col.append(&buttons);
    col.append(&external_btn);
    col.append(&hint);

    let pop = gtk4::Popover::builder().child(&col).position(gtk4::PositionType::Bottom).build();
    pop.add_css_class("link-popover");
    pop.set_parent(term);
    for b in [&open_btn, &editor_btn, &external_btn, &copy_btn] {
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
        open_button: open_btn,
        editor: editor_btn,
        external_editor: external_btn,
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
fn add_match(term: &vte4::Terminal, pattern: &str, flags: u32, pointer: bool) -> Option<i32> {
    match vte4::Regex::for_match(pattern, flags) {
        Ok(re) => {
            let tag = term.match_add_regex(&re, 0);
            if pointer {
                term.match_set_cursor_name(tag, "pointer");
            }
            Some(tag)
        }
        Err(e) => {
            eprintln!("oxishell: link regex: {e}");
            None
        }
    }
}

/// The pane's context menu with `open` / `copy` entries on top.
fn menu_with(menu: &gio::Menu, file: bool) -> gio::Menu {
    let section = gio::Menu::new();
    if file {
        section.append(Some("Edit File"), Some("link.edit"));
        section.append(Some("Preview"), Some("link.open"));
        section.append(Some("Open in External Editor"), Some("link.editor"));
        section.append(Some("Copy Path"), Some("link.copy"));
    } else {
        section.append(Some("Open Link"), Some("link.open"));
        section.append(Some("Copy Link"), Some("link.copy"));
    }
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
    add_match(term, URL_PATTERN, PCRE2_UTF | PCRE2_MULTILINE | PCRE2_CASELESS, true);
    let file_tags = [(PY_FILE_PATTERN, true), (QUOTED_FILE_PATTERN, false), (FILE_PATTERN, true)]
        .into_iter()
        .filter_map(|(p, pointer)| add_match(term, p, PCRE2_UTF | PCRE2_MULTILINE, pointer))
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
    make("edit", activate_editor);
    make("editor", activate_external_editor);
    make("copy", copy_link);
    term.insert_action_group("link", Some(&group));

    let menu_with_link = menu_with(menu, false);
    let menu_with_file = menu_with(menu, true);

    let popover = build_popover(term);

    // Pointer position in terminal coordinates; also shows a hint over links.
    let pos = Rc::new(Cell::new(None));
    let over_link = Rc::new(Cell::new(false));
    {
        let motion = gtk4::EventControllerMotion::new();
        let position = pos.clone();
        let over_link = over_link.clone();
        let ctx = ctx.clone();
        motion.connect_motion(move |m, x, y| {
            position.set(Some((x, y)));
            let Some(t) = m.widget().and_downcast::<vte4::Terminal>() else {
                return;
            };
            let on = ctx.link_at(&t, x, y).is_some();
            if on != over_link.get() {
                over_link.set(on);
                t.set_tooltip_text(on.then_some(HOVER_HINT));
            }
        });
        let pos = pos.clone();
        motion.connect_leave(move |_| pos.set(None));
        term.add_controller(motion);
    }

    // Capture before VTE handles selection and the context menu. Leave the
    // events available to VTE.
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
        let Some((x, y)) = pos.get() else {
            pressed_at.set(None);
            return glib::Propagation::Proceed;
        };
        match (ev.event_type(), button) {
            (gdk::EventType::ButtonPress, 3) => {
                let link = ctx.link_at(&t, x, y);
                t.set_context_menu_model(Some(match link {
                    Some(Link::Url(_)) => &menu_with_link,
                    Some(Link::File(_)) => &menu_with_file,
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
                if ev.modifier_state().contains(super::primary_mask()) {
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

    #[test]
    fn resolves_agent_paths_and_local_uris_with_optional_locations() {
        let root = std::env::temp_dir().join(format!("oxishell-link-targets-{}", std::process::id()));
        let dir = root.join("src");
        std::fs::create_dir_all(&dir).unwrap();
        for file in ["src/main.rs", "src/notes here", "README", ".project", "src/colon:name"] {
            std::fs::write(root.join(file), "").unwrap();
        }
        let resolve = |input| resolve_match(input, Some(&root), &root);
        for (input, path, location) in [
            ("src/main.rs", "src/main.rs", None),
            ("README", "README", None),
            ("./README.", "README", None),
            ("~/.project", ".project", None),
            ("src/main.rs:13:2–15:3", "src/main.rs", Some((13, Some(2)))),
            ("README#L13C2-L15C3", "README", Some((13, Some(2)))),
            ("`src/notes here`:2:3", "src/notes here", Some((2, Some(3)))),
            ("\"src/colon:name:2\"", "src/colon:name", Some((2, None))),
            ("File \"src/notes here\", line 8", "src/notes here", Some((8, None))),
        ] {
            assert_eq!(
                resolve(input),
                Some(FileTarget {
                    path: root.join(path),
                    location
                }),
                "{input}"
            );
        }
        for missing in ["src", "missing.rs", "src/main.rs:0", "src/main.rs:1:2:3", "src/main.rs:1-"] {
            assert_eq!(resolve(missing), None, "{missing}");
        }
        let file = root.join("src/notes here");
        let uri = glib::filename_to_uri(&file, None).unwrap();
        assert_eq!(
            resolve_file_uri(&format!("{uri}#L8C2-L10")),
            Some(FileTarget {
                path: file.clone(),
                location: Some((8, Some(2)))
            })
        );
        let uri = glib::filename_to_uri(&file, Some("localhost")).unwrap();
        assert_eq!(
            resolve_file_uri(&uri),
            Some(FileTarget {
                path: file.clone(),
                location: None
            })
        );
        let uri = glib::filename_to_uri(&file, Some("remote.example")).unwrap();
        assert_eq!(resolve_file_uri(&uri), None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
