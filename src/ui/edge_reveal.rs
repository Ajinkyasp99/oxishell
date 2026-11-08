//! Edge reveal: header buttons that stay out of the way until the pointer
//! reaches the header. Window controls are not part of this; they stay put.
//!
//! With auto-hide off (Settings → Appearance) the buttons are always shown.

use gtk4::prelude::*;
use gtk4::{glib, Orientation};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// How long the buttons linger after the pointer leaves the header.
const LINGER: Duration = Duration::from_millis(700);
/// How long they show when auto-hide is (re)enabled, so people see they exist.
const HINT: Duration = Duration::from_millis(2500);

pub struct EdgeReveal {
    start: gtk4::Revealer,
    end: gtk4::Revealer,
    motion: gtk4::EventControllerMotion,
    menus: Vec<gtk4::MenuButton>,
    autohide: Cell<bool>,
    hide: RefCell<Option<glib::SourceId>>,
}

impl EdgeReveal {
    /// Pack `start` buttons at the header's start and `end` buttons at its end
    /// (inside the window controls), each in a sliding group.
    pub fn new(header: &libadwaita::HeaderBar, start: &[&gtk4::Widget], end: &[&gtk4::Widget], menus: Vec<gtk4::MenuButton>) -> Rc<Self> {
        let group = |ws: &[&gtk4::Widget], t: gtk4::RevealerTransitionType| {
            let b = gtk4::Box::new(Orientation::Horizontal, 2);
            b.add_css_class("edge-group");
            for w in ws {
                b.append(*w);
            }
            gtk4::Revealer::builder()
                .transition_type(t)
                .transition_duration(220)
                .reveal_child(true)
                .child(&b)
                .build()
        };
        let s = group(start, gtk4::RevealerTransitionType::SlideRight);
        let e = group(end, gtk4::RevealerTransitionType::SlideLeft);
        header.pack_start(&s);
        header.pack_end(&e);

        let motion = gtk4::EventControllerMotion::new();
        header.add_controller(motion.clone());
        let me = Rc::new(EdgeReveal {
            start: s,
            end: e,
            motion,
            menus,
            autohide: Cell::new(false),
            hide: RefCell::new(None),
        });

        let w = Rc::downgrade(&me);
        me.motion.connect_enter(move |_, _, _| {
            if let Some(m) = w.upgrade() {
                m.show();
            }
        });
        let w = Rc::downgrade(&me);
        me.motion.connect_leave(move |_| {
            if let Some(m) = w.upgrade() {
                m.hide_after(LINGER);
            }
        });
        for menu in &me.menus {
            // Closing a menu with the pointer elsewhere hides the buttons again.
            let w = Rc::downgrade(&me);
            menu.connect_active_notify(move |mb| {
                if let Some(m) = w.upgrade() {
                    if !mb.is_active() && !m.motion.contains_pointer() {
                        m.hide_after(LINGER);
                    }
                }
            });
        }
        me
    }

    pub fn set_autohide(self: &Rc<Self>, on: bool) {
        if self.autohide.replace(on) == on {
            return;
        }
        self.show();
        if on {
            self.hide_after(HINT);
        }
    }

    fn show(&self) {
        if let Some(id) = self.hide.borrow_mut().take() {
            id.remove();
        }
        self.start.set_reveal_child(true);
        self.end.set_reveal_child(true);
    }

    fn hide_after(self: &Rc<Self>, delay: Duration) {
        if !self.autohide.get() {
            return;
        }
        if let Some(id) = self.hide.borrow_mut().take() {
            id.remove();
        }
        let w = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(delay, move || {
            let Some(m) = w.upgrade() else { return };
            m.hide.borrow_mut().take();
            let busy = m.motion.contains_pointer() || m.menus.iter().any(|b| b.is_active());
            if m.autohide.get() && !busy {
                m.start.set_reveal_child(false);
                m.end.set_reveal_child(false);
            }
        });
        *self.hide.borrow_mut() = Some(id);
    }
}
