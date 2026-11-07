//! Warp-style context chips above each terminal: Python/conda environment,
//! working directory and git branch. Chips hide when they have nothing to show.

use crate::meta::GitInfo;
use gtk4::prelude::*;
use gtk4::Orientation;
use std::path::Path;

pub struct Chips {
    pub bar: gtk4::Box,
    env: (gtk4::Box, gtk4::Label),
    dir: (gtk4::Box, gtk4::Label),
    git: (gtk4::Box, gtk4::Label),
}

fn chip(kind: &str, icon: Option<&str>, glyph: Option<&str>) -> (gtk4::Box, gtk4::Label) {
    let b = gtk4::Box::new(Orientation::Horizontal, 5);
    b.add_css_class("chip");
    b.add_css_class(kind);
    if let Some(i) = icon {
        let img = gtk4::Image::from_icon_name(i);
        img.set_pixel_size(12);
        b.append(&img);
    }
    if let Some(g) = glyph {
        let l = gtk4::Label::new(Some(g));
        l.add_css_class("chip-glyph");
        b.append(&l);
    }
    let label = gtk4::Label::builder().ellipsize(gtk4::pango::EllipsizeMode::Start).build();
    b.append(&label);
    b.set_visible(false);
    (b, label)
}

impl Chips {
    pub fn new() -> Chips {
        let bar = gtk4::Box::new(Orientation::Horizontal, 6);
        bar.add_css_class("chips");
        let env = chip("chip-env", None, Some(">_"));
        let dir = chip("chip-dir", Some("folder-symbolic"), None);
        let git = chip("chip-git", None, Some("⎇"));
        dir.1.set_max_width_chars(48);
        env.1.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        env.1.set_max_width_chars(24);
        git.1.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        git.1.set_max_width_chars(32);
        bar.append(&env.0);
        bar.append(&dir.0);
        bar.append(&git.0);
        Chips { bar, env, dir, git }
    }

    pub fn update(&self, env: Option<&str>, cwd: Option<&Path>, git: Option<&GitInfo>, enabled: bool) {
        let set = |c: &(gtk4::Box, gtk4::Label), text: Option<String>, tip: Option<String>| match text {
            Some(t) => {
                if c.1.label() != t {
                    c.1.set_label(&t);
                }
                c.0.set_tooltip_text(tip.as_deref());
                c.0.set_visible(true);
            }
            None => c.0.set_visible(false),
        };
        set(&self.env, env.map(str::to_string), env.map(|e| format!("Python environment: {e}")));
        set(&self.dir, cwd.map(crate::meta::display_path), cwd.map(|c| c.display().to_string()));
        set(
            &self.git,
            git.map(|g| {
                let mut s = g.branch.clone();
                if g.ahead > 0 {
                    s.push_str(&format!(" ↑{}", g.ahead));
                }
                if g.behind > 0 {
                    s.push_str(&format!(" ↓{}", g.behind));
                }
                if g.changes > 0 {
                    s.push_str(&format!(" ●{}", g.changes));
                }
                s
            }),
            git.map(|g| format!("Branch {} — {} changed file(s)", g.branch, g.changes)),
        );
        let any = self.env.0.get_visible() || self.dir.0.get_visible() || self.git.0.get_visible();
        self.bar.set_visible(enabled && any);
    }
}
