//! Clickable links in terminal output.
//!
//! URLs that programs print are detected with a regex (VTE shows a hand
//! cursor and underlines them on hover); explicit OSC 8 hyperlinks work too.
//!
//! - Click a link: a small popover offers **Open Link** and **Copy Link**.
//! - Ctrl+click a link: open it right away in the default browser.
//! - Right-click a link: the context menu starts with the same two actions.
//!
//! A click that selects text (drag, double-click) never triggers anything.

use super::{App, AppRc};
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
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

/// The link (OSC 8 hyperlink or detected URL) under widget point `x, y`.
fn link_at(term: &vte4::Terminal, x: f64, y: f64) -> Option<String> {
    if let Some(h) = term.check_hyperlink_at(x, y) {
        return Some(h.to_string());
    }
    term.check_match_at(x, y).0.map(|s| trim_link(&s).to_string())
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

pub fn copy(app: &Weak<App>, term: &vte4::Terminal, link: &str) {
    term.clipboard().set_text(link);
    if let Some(a) = app.upgrade() {
        a.toast("Link copied");
    }
}

/// Popover shown when a link is clicked.
fn build_popover(term: &vte4::Terminal) -> (gtk4::Popover, gtk4::Label) {
    let url = gtk4::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk4::pango::EllipsizeMode::Middle)
        .max_width_chars(56)
        .build();
    url.add_css_class("link-url");
    url.add_css_class("monospace");
    let button = |icon: &str, label: &str, action: &str| {
        gtk4::Button::builder()
            .child(&adw::ButtonContent::builder().icon_name(icon).label(label).build())
            .action_name(action)
            .hexpand(true)
            .build()
    };
    let open_btn = button("web-browser-symbolic", "Open Link", "link.open");
    open_btn.add_css_class("suggested-action");
    let copy_btn = button("edit-copy-symbolic", "Copy Link", "link.copy");
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
    (pop, url)
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

pub fn attach(app: &AppRc, term: &vte4::Terminal, menu: &gio::Menu) {
    keep_row_cache_in_sync_with_scrolling(term);
    match vte4::Regex::for_match(URL_PATTERN, PCRE2_UTF | PCRE2_MULTILINE | PCRE2_CASELESS) {
        Ok(re) => {
            let tag = term.match_add_regex(&re, 0);
            term.match_set_cursor_name(tag, "pointer");
        }
        Err(e) => eprintln!("oxishell: link regex: {e}"),
    }

    // The link the popover / context menu acts on.
    let current: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let group = gio::SimpleActionGroup::new();
    let make = |name: &str, f: fn(&Weak<App>, &vte4::Terminal, &str)| {
        let act = gio::SimpleAction::new(name, None);
        let w = app.weak();
        let cur = current.clone();
        let t = term.downgrade();
        act.connect_activate(move |_, _| {
            let (Some(t), Some(link)) = (t.upgrade(), cur.borrow().clone()) else {
                return;
            };
            f(&w, &t, &link);
        });
        group.add_action(&act);
    };
    make("open", open);
    make("copy", copy);
    term.insert_action_group("link", Some(&group));

    let link_section = gio::Menu::new();
    link_section.append(Some("Open Link"), Some("link.open"));
    link_section.append(Some("Copy Link"), Some("link.copy"));
    let menu_with_link = gio::Menu::new();
    menu_with_link.append_section(None, &link_section);
    for i in 0..menu.n_items() {
        if let Some(sec) = menu.item_link(i, gio::MENU_LINK_SECTION) {
            menu_with_link.append_section(None, &sec);
        }
    }

    let (pop, url_label) = build_popover(term);

    // Pointer position in terminal coordinates; also shows a hint over links.
    let pos = Rc::new(Cell::new((0.0f64, 0.0f64)));
    let over_link = Rc::new(Cell::new(false));
    {
        let motion = gtk4::EventControllerMotion::new();
        let pos = pos.clone();
        let over_link = over_link.clone();
        motion.connect_motion(move |m, x, y| {
            pos.set((x, y));
            let Some(t) = m.widget().and_downcast::<vte4::Terminal>() else {
                return;
            };
            let on = link_at(&t, x, y).is_some();
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
    let w = app.weak();
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
                let link = link_at(&t, x, y);
                t.set_context_menu_model(Some(if link.is_some() { &menu_with_link } else { &menu }));
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
                let Some(link) = link_at(&t, x, y) else {
                    return glib::Propagation::Proceed;
                };
                if ev.modifier_state().contains(gdk::ModifierType::CONTROL_MASK) {
                    open(&w, &t, &link);
                } else {
                    url_label.set_label(&normalize(&link));
                    url_label.set_tooltip_text(Some(&link));
                    *current.borrow_mut() = Some(link);
                    // The hover hint would sit on top of the popover.
                    t.set_tooltip_text(None);
                    over_link.set(false);
                    pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                    pop.popup();
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
