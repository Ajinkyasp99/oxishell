//! Warp-style context chips above each terminal: Python/conda environment,
//! working directory and git branch. Chips hide when they have nothing to show.

use crate::meta::GitInfo;
use gtk4::prelude::*;
use gtk4::{glib, Orientation};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

pub struct Chips {
    pub bar: gtk4::Box,
    env: Chip,
    dir: Chip,
    git: Chip,
    /// Badges inside the git chip: commits ahead/behind and changed files.
    ahead: Badge,
    behind: Badge,
    dirty: Badge,
}

/// Small pill inside a chip: an optional icon and a count.
struct Badge {
    root: gtk4::Box,
    label: gtk4::Label,
}

fn badge(kind: &str, icon: Option<&str>, glyph: Option<&str>) -> Badge {
    let b = gtk4::Box::new(Orientation::Horizontal, 3);
    b.add_css_class("chip-badge");
    b.add_css_class(kind);
    b.set_valign(gtk4::Align::Center);
    if let Some(i) = icon {
        let img = gtk4::Image::from_icon_name(i);
        img.set_pixel_size(10);
        b.append(&img);
    }
    if let Some(g) = glyph {
        b.append(&gtk4::Label::new(Some(g)));
    }
    let label = gtk4::Label::new(None);
    b.append(&label);
    b.set_visible(false);
    Badge { root: b, label }
}

impl Badge {
    fn set(&self, n: u32, tip: String) {
        if n == 0 {
            self.root.set_visible(false);
            return;
        }
        let t = n.to_string();
        if self.label.label() != t {
            self.label.set_label(&t);
        }
        self.root.set_tooltip_text(Some(&tip));
        self.root.set_visible(true);
    }
}

struct Chip {
    root: gtk4::Box,
    label: gtk4::Label,
    /// Text the copy button puts on the clipboard (full path / bare branch name).
    copy_value: Rc<RefCell<String>>,
}

fn chip(kind: &str, icon: Option<&str>, glyph: Option<&str>, copy_tip: Option<&str>) -> Chip {
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
    let copy_value = Rc::new(RefCell::new(String::new()));
    if let Some(tip) = copy_tip {
        b.append(&copy_button(tip, copy_value.clone()));
    }
    b.set_visible(false);
    Chip {
        root: b,
        label,
        copy_value,
    }
}

/// Small flat button that copies `value` and briefly shows a check mark.
fn copy_button(tip: &str, value: Rc<RefCell<String>>) -> gtk4::Button {
    let btn = gtk4::Button::from_icon_name("edit-copy-symbolic");
    btn.add_css_class("flat");
    btn.add_css_class("chip-copy");
    btn.set_focus_on_click(false);
    btn.set_valign(gtk4::Align::Center);
    btn.set_tooltip_text(Some(tip));
    let tip = tip.to_string();
    btn.connect_clicked(move |b| {
        let text = value.borrow().clone();
        if text.is_empty() {
            return;
        }
        b.clipboard().set_text(&text);
        b.set_icon_name("object-select-symbolic");
        b.set_tooltip_text(Some("Copied"));
        b.add_css_class("copied");
        let weak = b.downgrade();
        let tip = tip.clone();
        glib::timeout_add_local_once(Duration::from_millis(1200), move || {
            if let Some(b) = weak.upgrade() {
                b.set_icon_name("edit-copy-symbolic");
                b.set_tooltip_text(Some(&tip));
                b.remove_css_class("copied");
            }
        });
    });
    btn
}

impl Chips {
    pub fn new() -> Chips {
        let bar = gtk4::Box::new(Orientation::Horizontal, 6);
        bar.add_css_class("chips");
        let env = chip("chip-env", None, Some(">_"), None);
        let dir = chip("chip-dir", Some("folder-symbolic"), None, Some("Copy full path"));
        let git = chip("chip-git", None, Some("⎇"), Some("Copy branch name"));
        dir.label.set_max_width_chars(48);
        env.label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        env.label.set_max_width_chars(24);
        git.label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        git.label.set_max_width_chars(32);
        bar.append(&env.root);
        bar.append(&dir.root);
        let ahead = badge("badge-ahead", None, Some("↑"));
        let behind = badge("badge-behind", None, Some("↓"));
        let dirty = badge("badge-dirty", Some("document-edit-symbolic"), None);
        let mut after = git.label.clone().upcast::<gtk4::Widget>();
        for b in [&ahead, &behind, &dirty] {
            git.root.insert_child_after(&b.root, Some(&after));
            after = b.root.clone().upcast();
        }
        bar.append(&git.root);
        Chips {
            bar,
            env,
            dir,
            git,
            ahead,
            behind,
            dirty,
        }
    }

    pub fn update(&self, env: Option<&str>, cwd: Option<&Path>, git: Option<&GitInfo>, enabled: bool) {
        let set = |c: &Chip, text: Option<String>, tip: Option<String>| match text {
            Some(t) => {
                if c.label.label() != t {
                    c.label.set_label(&t);
                }
                c.root.set_tooltip_text(tip.as_deref());
                c.root.set_visible(true);
            }
            None => c.root.set_visible(false),
        };
        *self.dir.copy_value.borrow_mut() = cwd.map(|c| c.display().to_string()).unwrap_or_default();
        *self.git.copy_value.borrow_mut() = git.map(|g| g.branch.clone()).unwrap_or_default();
        set(&self.env, env.map(str::to_string), env.map(|e| format!("Python environment: {e}")));
        set(&self.dir, cwd.map(crate::meta::display_path), cwd.map(|c| c.display().to_string()));
        set(
            &self.git,
            git.map(|g| g.branch.clone()),
            git.map(|g| format!("Branch {} — {} changed file(s)", g.branch, g.changes)),
        );
        let g = git.cloned().unwrap_or_default();
        let plural = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        self.ahead
            .set(g.ahead, format!("{} ahead of upstream", plural(g.ahead, "commit", "commits")));
        self.behind
            .set(g.behind, format!("{} behind upstream", plural(g.behind, "commit", "commits")));
        self.dirty.set(g.changes, format!("{} changed", plural(g.changes, "file", "files")));
        let any = self.env.root.get_visible() || self.dir.root.get_visible() || self.git.root.get_visible();
        self.bar.set_visible(enabled && any);
    }
}
