//! Theme gallery: a grid of live preview cards, Warp style. Clicking a card
//! applies the theme to the whole window immediately.

use super::AppRc;
use crate::theme::{Paint, Rgb, Theme};
use gtk4::prelude::*;
use gtk4::{cairo, gio, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;

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

/// Preferences group with the gallery, for the Settings dialog.
pub fn gallery(app: &AppRc) -> adw::PreferencesGroup {
    let dark = adw::StyleManager::default().is_dark();
    let current = app.inner.borrow().theme.id.clone();
    let (_, errors) = crate::theme::load_dir(&crate::theme::user_dir());
    let group = adw::PreferencesGroup::builder()
        .title("Theme")
        .description("Colours the whole window — background, accents, panels and terminal. Any Warp theme (.yaml) works: drop it into the themes folder.")
        .build();

    let open = gtk4::Button::builder()
        .label("Open Themes Folder")
        .valign(gtk4::Align::Center)
        .build();
    open.add_css_class("flat");
    {
        let w = app.weak();
        open.connect_clicked(move |_| {
            let dir = crate::theme::user_dir();
            let _ = crate::paths::ensure_private_dir(&dir);
            let uri = gio::File::for_path(&dir).uri();
            if let Err(e) = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE) {
                if let Some(a) = w.upgrade() {
                    a.toast(&format!("Themes folder: {} ({e})", dir.display()));
                }
            }
        });
    }
    group.set_header_suffix(Some(&open));

    let flow = gtk4::FlowBox::builder()
        .selection_mode(gtk4::SelectionMode::Single)
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(4)
        .column_spacing(8)
        .row_spacing(8)
        .activate_on_single_click(true)
        .build();
    for t in crate::theme::all(dark) {
        let c = card(&t);
        flow.append(&c);
        if t.id == current {
            if let Some(child) = c.parent().and_then(|p| p.downcast::<gtk4::FlowBoxChild>().ok()) {
                flow.select_child(&child);
            }
        }
    }
    let w = app.weak();
    flow.connect_child_activated(move |_, child| {
        let id = child.child().map(|c| c.widget_name().to_string()).unwrap_or_default();
        if let Some(a) = w.upgrade() {
            a.set_theme(&id);
        }
    });
    group.add(&flow);
    for e in errors {
        let l = gtk4::Label::builder().label(format!("⚠ {e}")).xalign(0.0).wrap(true).build();
        l.add_css_class("error");
        group.add(&l);
    }
    group
}
