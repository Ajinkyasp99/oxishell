//! Theme picker: live preview cards, Warp style. Clicking a card applies the
//! theme to the whole window immediately.
//!
//! Settings shows a few quick picks (the current theme first); "Browse all
//! themes" opens a subpage with every theme, a search field and a
//! light/dark filter, so the main settings page stays short.

use super::AppRc;
use crate::theme::{Paint, Rgb, Theme};
use gtk4::prelude::*;
use gtk4::{cairo, gio, glib, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const CARD_W: i32 = 168;
const CARD_H: i32 = 100;

fn set(cr: &cairo::Context, c: Rgb, a: f64) {
    cr.set_source_rgba(c.0 as f64 / 255.0, c.1 as f64 / 255.0, c.2 as f64 / 255.0, a);
}

fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::PI;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

fn paint_fill(cr: &cairo::Context, p: Paint, x0: f64, y0: f64, x1: f64, y1: f64) {
    match p {
        Paint::Solid(c) => set(cr, c, 1.0),
        Paint::Gradient(a, b) => {
            let g = cairo::LinearGradient::new(x0, y0, x1, y1);
            g.add_color_stop_rgb(0.0, a.0 as f64 / 255.0, a.1 as f64 / 255.0, a.2 as f64 / 255.0);
            g.add_color_stop_rgb(1.0, b.0 as f64 / 255.0, b.1 as f64 / 255.0, b.2 as f64 / 255.0);
            let _ = cr.set_source(&g);
        }
    }
}

/// Miniature window: background, a fake prompt and output, ANSI dots, accent bar.
fn draw_preview(cr: &cairo::Context, t: &Theme, w: f64, h: f64) {
    rounded(cr, 0.5, 0.5, w - 1.0, h - 1.0, 10.0);
    cr.save().ok();
    cr.clip();
    paint_fill(cr, t.background, 0.0, 0.0, 0.0, h);
    cr.paint().ok();
    // sidebar strip
    set(cr, t.surface(), if t.layered_background() { 0.55 } else { 1.0 });
    cr.rectangle(0.0, 0.0, 34.0, h);
    cr.fill().ok();
    paint_fill(cr, t.accent, 6.0, 0.0, 28.0, 0.0);
    rounded(cr, 6.0, 10.0, 22.0, 6.0, 3.0);
    cr.fill().ok();
    set(cr, t.foreground, 0.35);
    for i in 0..2 {
        rounded(cr, 6.0, 22.0 + i as f64 * 10.0, 18.0, 4.0, 2.0);
        cr.fill().ok();
    }
    // prompt + output lines
    set(cr, t.normal[2], 1.0);
    cr.rectangle(42.0, 12.0, 16.0, 5.0);
    cr.fill().ok();
    set(cr, t.foreground, 0.9);
    cr.rectangle(62.0, 12.0, 48.0, 5.0);
    cr.fill().ok();
    for (i, len) in [70.0, 54.0, 86.0].iter().enumerate() {
        set(cr, t.foreground, 0.55);
        cr.rectangle(42.0, 24.0 + i as f64 * 10.0, *len, 4.0);
        cr.fill().ok();
    }
    // ANSI colours
    for (i, c) in t.normal.iter().enumerate().skip(1).take(6) {
        set(cr, *c, 1.0);
        cr.arc(48.0 + (i - 1) as f64 * 14.0, h - 24.0, 4.5, 0.0, std::f64::consts::TAU);
        cr.fill().ok();
    }
    // accent bar
    paint_fill(cr, t.accent, 0.0, 0.0, w, 0.0);
    cr.rectangle(0.0, h - 6.0, w, 6.0);
    cr.fill().ok();
    cr.restore().ok();
    set(cr, t.foreground, 0.18);
    rounded(cr, 0.5, 0.5, w - 1.0, h - 1.0, 10.0);
    cr.set_line_width(1.0);
    cr.stroke().ok();
}

fn card(t: &Theme) -> gtk4::Box {
    let area = gtk4::DrawingArea::builder().content_width(CARD_W).content_height(CARD_H).build();
    let theme = t.clone();
    area.set_draw_func(move |_, cr, w, h| draw_preview(cr, &theme, w as f64, h as f64));
    let label = gtk4::Label::builder()
        .label(&t.name)
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .max_width_chars(20)
        .build();
    label.add_css_class("caption");
    if t.user {
        label.set_tooltip_text(Some("Your theme (~/.config/cmux-linux/themes)"));
    }
    let b = gtk4::Box::new(Orientation::Vertical, 6);
    b.set_margin_top(4);
    b.set_margin_bottom(4);
    b.append(&area);
    b.append(&label);
    b.set_widget_name(&t.id);
    b
}

/// Popular themes offered as quick picks after the current one.
const FEATURED: &[&str] = &[
    "default",
    "tokyo_night",
    "dracula",
    "catppuccin",
    "nord",
    "one_dark",
    "github_light",
    "solarized_light",
];
const QUICK_PICKS: usize = 6;

/// Ids of the quick picks: the current theme, then featured ones that exist.
pub fn quick_ids(current: &str, available: &[&str]) -> Vec<String> {
    let mut ids = vec![current.to_string()];
    for f in FEATURED {
        if ids.len() == QUICK_PICKS {
            break;
        }
        if *f != current && available.contains(f) {
            ids.push(f.to_string());
        }
    }
    ids
}

/// Light/dark filter of the theme browser.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shade {
    All,
    Dark,
    Light,
}

/// Search and shade filter used by the theme browser.
pub fn matches(name: &str, dark: bool, query: &str, shade_dark: Option<bool>) -> bool {
    shade_dark.is_none_or(|d| d == dark) && super::dialogs::fuzzy_match(query.trim(), name)
}

fn flow_box(max_per_line: u32) -> gtk4::FlowBox {
    gtk4::FlowBox::builder()
        .selection_mode(gtk4::SelectionMode::Single)
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(max_per_line)
        .column_spacing(8)
        .row_spacing(8)
        .activate_on_single_click(true)
        .valign(gtk4::Align::Start)
        .build()
}

fn card_id(child: &gtk4::FlowBoxChild) -> String {
    child.child().map(|c| c.widget_name().to_string()).unwrap_or_default()
}

fn select_id(flow: &gtk4::FlowBox, id: &str) {
    let mut w = flow.first_child();
    while let Some(c) = w {
        if let Ok(child) = c.clone().downcast::<gtk4::FlowBoxChild>() {
            if card_id(&child) == id {
                flow.select_child(&child);
                return;
            }
        }
        w = c.next_sibling();
    }
}

fn open_folder_button(app: &AppRc) -> gtk4::Button {
    let b = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text("Open themes folder — drop any Warp theme (.yaml) here")
        .valign(gtk4::Align::Center)
        .build();
    b.add_css_class("flat");
    let w = app.weak();
    b.connect_clicked(move |_| {
        let dir = crate::theme::user_dir();
        let _ = crate::paths::ensure_private_dir(&dir);
        let uri = gio::File::for_path(&dir).uri();
        if let Err(e) = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE) {
            if let Some(a) = w.upgrade() {
                a.toast(&format!("Themes folder: {} ({e})", dir.display()));
            }
        }
    });
    b
}

/// The theme section of Settings: quick-pick cards, then a group with
/// "Browse all themes" (opens the full browser as a subpage) and `extra`
/// rows (the light/dark style). Returns both groups in display order.
pub fn settings_groups(
    app: &AppRc,
    dialog: &adw::PreferencesDialog,
    extra: &[&gtk4::Widget],
) -> (adw::PreferencesGroup, adw::PreferencesGroup) {
    let picks = adw::PreferencesGroup::builder()
        .title("Theme")
        .description("Colours the whole window: background, accents, panels and terminal.")
        .build();
    picks.set_header_suffix(Some(&open_folder_button(app)));
    let quick = flow_box(3);
    picks.add(&quick);

    // Rebuilt after a pick in the browser so the current theme leads.
    let fill: Rc<dyn Fn()> = {
        let w = app.weak();
        let quick = quick.clone();
        Rc::new(move || {
            let Some(a) = w.upgrade() else { return };
            let dark = adw::StyleManager::default().is_dark();
            let current = a.inner.borrow().theme.id.clone();
            let all = crate::theme::all(dark);
            let ids: Vec<&str> = all.iter().map(|t| t.id.as_str()).collect();
            quick.remove_all();
            for id in quick_ids(&current, &ids) {
                if let Some(t) = all.iter().find(|t| t.id == id) {
                    quick.append(&card(t));
                }
            }
            select_id(&quick, &current);
        })
    };
    fill();
    {
        let w = app.weak();
        quick.connect_child_activated(move |_, child| {
            if let Some(a) = w.upgrade() {
                a.set_theme(&card_id(child));
            }
        });
    }

    let more = adw::PreferencesGroup::new();
    let dark = adw::StyleManager::default().is_dark();
    let count = crate::theme::all(dark).len();
    let browse = adw::ActionRow::builder()
        .title("Browse all themes")
        .subtitle(format!("{count} themes, with search and a light/dark filter"))
        .activatable(true)
        .build();
    browse.add_prefix(&gtk4::Image::from_icon_name("view-grid-symbolic"));
    browse.add_suffix(&gtk4::Image::from_icon_name("go-next-symbolic"));
    {
        let w = app.weak();
        let d = dialog.downgrade();
        let fill = fill.clone();
        browse.connect_activated(move |_| {
            let (Some(a), Some(d)) = (w.upgrade(), d.upgrade()) else { return };
            d.push_subpage(&browser_page(&a, fill.clone()));
        });
    }
    more.add(&browse);
    for x in extra {
        more.add(*x);
    }
    let (_, errors) = crate::theme::load_dir(&crate::theme::user_dir());
    for e in errors {
        let row = adw::ActionRow::builder()
            .title("Theme file could not be loaded")
            .subtitle(glib::markup_escape_text(&e).as_str())
            .build();
        row.add_prefix(&gtk4::Image::from_icon_name("dialog-warning-symbolic"));
        row.add_css_class("error");
        more.add(&row);
    }
    (picks, more)
}

/// Every theme, searchable and filterable by light/dark.
fn browser_page(app: &AppRc, on_pick: Rc<dyn Fn()>) -> adw::NavigationPage {
    let dark = adw::StyleManager::default().is_dark();
    let themes = crate::theme::all(dark);
    let current = app.inner.borrow().theme.id.clone();

    let search = gtk4::SearchEntry::builder().placeholder_text("Search themes").hexpand(true).build();
    let shade = Rc::new(Cell::new(Shade::All));
    let toggles = gtk4::Box::new(Orientation::Horizontal, 0);
    toggles.add_css_class("linked");
    let mut buttons = Vec::new();
    for label in ["All", "Dark", "Light"] {
        let b = gtk4::ToggleButton::with_label(label);
        if let Some(first) = buttons.first() {
            b.set_group(Some(first));
        } else {
            b.set_active(true);
        }
        toggles.append(&b);
        buttons.push(b);
    }
    let bar = gtk4::Box::new(Orientation::Horizontal, 8);
    bar.set_margin_start(12);
    bar.set_margin_end(12);
    bar.set_margin_top(6);
    bar.set_margin_bottom(6);
    bar.append(&search);
    bar.append(&toggles);

    let flow = flow_box(4);
    flow.set_margin_start(12);
    flow.set_margin_end(12);
    flow.set_margin_top(12);
    flow.set_margin_bottom(12);
    // id → (name, dark) for filtering
    let info: Rc<std::collections::HashMap<String, (String, bool)>> =
        Rc::new(themes.iter().map(|t| (t.id.clone(), (t.name.clone(), t.is_dark()))).collect());
    for t in &themes {
        flow.append(&card(t));
    }
    select_id(&flow, &current);
    let query = Rc::new(RefCell::new(String::new()));
    {
        let (info, query, shade) = (info.clone(), query.clone(), shade.clone());
        flow.set_filter_func(move |child| {
            let Some((name, dark)) = info.get(&card_id(child)) else {
                return true;
            };
            let want = match shade.get() {
                Shade::All => None,
                Shade::Dark => Some(true),
                Shade::Light => Some(false),
            };
            matches(name, *dark, &query.borrow(), want)
        });
    }

    let empty = adw::StatusPage::builder()
        .icon_name("edit-find-symbolic")
        .title("No Themes Found")
        .description("Try another name, or switch the light/dark filter.")
        .build();
    let clamp = adw::Clamp::builder().maximum_size(900).child(&flow).build();
    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build();
    let stack = gtk4::Stack::new();
    stack.add_named(&scroll, Some("list"));
    stack.add_named(&empty, Some("empty"));

    let refilter: Rc<dyn Fn()> = {
        let (flow, stack, info, query, shade) = (flow.clone(), stack.clone(), info.clone(), query.clone(), shade.clone());
        Rc::new(move || {
            flow.invalidate_filter();
            let want = match shade.get() {
                Shade::All => None,
                Shade::Dark => Some(true),
                Shade::Light => Some(false),
            };
            let any = info.values().any(|(n, d)| matches(n, *d, &query.borrow(), want));
            stack.set_visible_child_name(if any { "list" } else { "empty" });
        })
    };
    {
        let (query, refilter) = (query.clone(), refilter.clone());
        search.connect_search_changed(move |e| {
            *query.borrow_mut() = e.text().to_string();
            refilter();
        });
    }
    for (b, s) in buttons.iter().zip([Shade::All, Shade::Dark, Shade::Light]) {
        let (shade, refilter) = (shade.clone(), refilter.clone());
        b.connect_toggled(move |b| {
            if b.is_active() {
                shade.set(s);
                refilter();
            }
        });
    }
    {
        let w = app.weak();
        flow.connect_child_activated(move |_, child| {
            if let Some(a) = w.upgrade() {
                a.set_theme(&card_id(child));
                on_pick();
            }
        });
    }

    let header = adw::HeaderBar::new();
    header.pack_end(&open_folder_button(app));
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.add_top_bar(&bar);
    view.set_content(Some(&stack));
    let page = adw::NavigationPage::builder().title("Themes").child(&view).build();
    {
        let s = search.clone();
        page.connect_shown(move |_| {
            s.grab_focus();
        });
    }
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_picks_lead_with_current_and_skip_missing() {
        let avail = [
            "default",
            "dracula",
            "nord",
            "tokyo_night",
            "koi",
            "catppuccin",
            "one_dark",
            "github_light",
        ];
        let ids = quick_ids("koi", &avail);
        assert_eq!(ids[0], "koi");
        assert_eq!(ids.len(), QUICK_PICKS);
        assert!(!ids[1..].contains(&"koi".to_string()));
        let ids = quick_ids("dracula", &["default", "dracula"]);
        assert_eq!(ids, vec!["dracula", "default"]);
    }

    #[test]
    fn browser_filter_combines_search_and_shade() {
        assert!(matches("Tokyo Night", true, "tokyo", None));
        assert!(matches("Tokyo Night", true, "  ", Some(true)));
        assert!(!matches("Tokyo Night", true, "", Some(false)));
        assert!(!matches("Nord", true, "dracula", None));
        assert!(matches("GitHub Light", false, "ghl", Some(false)));
    }
}
