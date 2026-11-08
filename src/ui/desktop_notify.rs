//! Desktop notifications, sent straight to the notification server
//! (`org.freedesktop.Notifications`: GNOME Shell, KDE Plasma, dunst, mako…).
//!
//! `GApplication::send_notification` goes through `org.gtk.Notifications`,
//! which on GNOME cannot carry a sound or an urgency: an agent asking for
//! input arrived as a silent banner that slid away and piled up in the
//! message tray. Talking to the server directly lets every notification
//!
//! - play an event from the system sound theme (`sound-name`), or have us play
//!   it with libcanberra when the server cannot (no `sound` capability);
//! - be critical when an agent needs input, so it stays on screen until handled;
//! - replace the same pane's previous notification instead of stacking;
//! - disappear once the pane has been looked at (see [`Desktop::sync`]);
//! - focus its pane when clicked, raising the window with the server's
//!   activation token.
//!
//! When there is no session bus or no server, `gio::Notification` is used.

use crate::model::PaneId;
use crate::notify::Kind;
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

const BUS: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const IFACE: &str = "org.freedesktop.Notifications";
const APP_NAME: &str = "cmux";
const CALL_TIMEOUT_MS: i32 = 5000;

/// One notification to show.
pub struct Request<'a> {
    /// Id in the in-app notification centre.
    pub id: u64,
    pub pane: Option<PaneId>,
    pub kind: Kind,
    pub title: &'a str,
    pub body: &'a str,
    pub sound: bool,
}

type Activate = Box<dyn Fn(u64, Option<String>)>;

pub struct Desktop {
    gapp: gio::Application,
    conn: Option<gio::DBusConnection>,
    /// The server plays `sound-name` itself (`None` until it has told us).
    server_sound: Cell<Option<bool>>,
    /// The server renders markup in the body (then text must be escaped).
    body_markup: Cell<bool>,
    /// Our id → server id, for notifications the server still shows.
    shown: RefCell<HashMap<u64, u32>>,
    /// Our ids sent but not yet acknowledged with a server id.
    pending: RefCell<HashSet<u64>>,
    /// Pending ids that were read (or replaced) before the server answered.
    retired: RefCell<HashSet<u64>>,
    /// The latest notification of each pane; the next one replaces it.
    by_pane: RefCell<HashMap<PaneId, u64>>,
    /// Activation tokens (server id → token) announced just before a click.
    tokens: RefCell<HashMap<u32, String>>,
    on_activate: RefCell<Option<Activate>>,
    subscription: RefCell<Option<gio::SignalSubscriptionId>>,
}

impl Desktop {
    pub fn new(gapp: &impl IsA<gio::Application>) -> Rc<Desktop> {
        let conn = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
            .map_err(|e| eprintln!("cmux-linux: no session bus, using GTK notifications: {e}"))
            .ok();
        let d = Rc::new(Desktop {
            gapp: gapp.clone().upcast(),
            conn,
            server_sound: Cell::new(None),
            body_markup: Cell::new(true),
            shown: RefCell::default(),
            pending: RefCell::default(),
            retired: RefCell::default(),
            by_pane: RefCell::default(),
            tokens: RefCell::default(),
            on_activate: RefCell::new(None),
            subscription: RefCell::new(None),
        });
        d.subscribe();
        d.query_capabilities();
        d
    }

    /// Called with (notification id, activation token) when one is clicked.
    pub fn set_on_activate(&self, f: impl Fn(u64, Option<String>) + 'static) {
        *self.on_activate.borrow_mut() = Some(Box::new(f));
    }

    fn query_capabilities(self: &Rc<Self>) {
        let Some(conn) = &self.conn else { return };
        let w = Rc::downgrade(self);
        conn.call(
            Some(BUS),
            PATH,
            IFACE,
            "GetCapabilities",
            None,
            Some(glib::VariantTy::new("(as)").unwrap()),
            gio::DBusCallFlags::NONE,
            CALL_TIMEOUT_MS,
            gio::Cancellable::NONE,
            move |r| {
                let Some(d) = w.upgrade() else { return };
                match r.ok().and_then(|v| v.get::<(Vec<String>,)>()) {
                    Some((caps,)) => {
                        d.server_sound.set(Some(caps.iter().any(|c| c == "sound")));
                        d.body_markup.set(caps.iter().any(|c| c == "body-markup"));
                    }
                    // No server answering: we will play sounds ourselves.
                    None => d.server_sound.set(Some(false)),
                }
            },
        );
    }

    fn subscribe(self: &Rc<Self>) {
        let Some(conn) = &self.conn else { return };
        let w: Weak<Desktop> = Rc::downgrade(self);
        // Sender is matched by id below, not by the well-known name.
        let id = conn.signal_subscribe(
            None,
            Some(IFACE),
            None,
            Some(PATH),
            None,
            gio::DBusSignalFlags::NONE,
            move |_, _, _, _, member, params| {
                if let Some(d) = w.upgrade() {
                    d.on_signal(member, params);
                }
            },
        );
        *self.subscription.borrow_mut() = Some(id);
    }

    fn our_id(&self, server_id: u32) -> Option<u64> {
        self.shown.borrow().iter().find(|(_, s)| **s == server_id).map(|(o, _)| *o)
    }

    fn on_signal(&self, member: &str, params: &glib::Variant) {
        match member {
            "ActivationToken" => {
                if let Some((sid, token)) = params.get::<(u32, String)>() {
                    if self.our_id(sid).is_some() {
                        self.tokens.borrow_mut().insert(sid, token);
                    }
                }
            }
            "ActionInvoked" => {
                let Some((sid, _action)) = params.get::<(u32, String)>() else {
                    return;
                };
                let Some(id) = self.our_id(sid) else { return };
                let token = self.tokens.borrow_mut().remove(&sid);
                if let Some(f) = self.on_activate.borrow().as_ref() {
                    f(id, token);
                }
            }
            "NotificationClosed" => {
                let Some((sid, _reason)) = params.get::<(u32, u32)>() else { return };
                self.shown.borrow_mut().retain(|_, s| *s != sid);
                self.tokens.borrow_mut().remove(&sid);
            }
            _ => {}
        }
    }

    /// Show a notification for `r`, replacing the pane's previous one.
    pub fn show(self: &Rc<Self>, r: &Request) {
        let Some(conn) = self.conn.clone() else {
            self.show_fallback(r);
            return;
        };
        // One notification per pane: replace the last one rather than stack.
        let mut replaces = 0u32;
        if let Some(pane) = r.pane {
            if let Some(prev) = self.by_pane.borrow_mut().insert(pane, r.id) {
                if let Some(sid) = self.shown.borrow_mut().remove(&prev) {
                    replaces = sid;
                } else if self.pending.borrow().contains(&prev) {
                    self.retired.borrow_mut().insert(prev);
                }
            }
        }

        let server_sound = self.server_sound.get().unwrap_or(true);
        let hints = glib::VariantDict::new(None);
        hints.insert_value("desktop-entry", &crate::APP_ID.to_variant());
        hints.insert_value("urgency", &r.kind.urgency().to_variant());
        hints.insert_value("category", &r.kind.category().to_variant());
        if r.sound && server_sound {
            hints.insert_value("sound-name", &r.kind.sound().to_variant());
        } else {
            // Silence the server's default sound (we play it ourselves, or not at all).
            hints.insert_value("suppress-sound", &true.to_variant());
        }
        if r.sound && !server_sound {
            play_sound(r.kind.sound());
        }

        let body = if self.body_markup.get() {
            glib::markup_escape_text(r.body).to_string()
        } else {
            r.body.to_string()
        };
        let title = if r.title.is_empty() { APP_NAME } else { r.title };
        let actions: Vec<String> = vec!["default".into(), "Open".into()];
        let params = glib::Variant::tuple_from_iter([
            APP_NAME.to_variant(),
            replaces.to_variant(),
            crate::APP_ID.to_variant(),
            title.to_variant(),
            body.to_variant(),
            actions.to_variant(),
            hints.end(),
            (-1i32).to_variant(),
        ]);

        let id = r.id;
        self.pending.borrow_mut().insert(id);
        let w = Rc::downgrade(self);
        let fallback = FallbackCopy::from(r);
        conn.call(
            Some(BUS),
            PATH,
            IFACE,
            "Notify",
            Some(&params),
            Some(glib::VariantTy::new("(u)").unwrap()),
            gio::DBusCallFlags::NONE,
            CALL_TIMEOUT_MS,
            gio::Cancellable::NONE,
            move |res| {
                let Some(d) = w.upgrade() else { return };
                d.pending.borrow_mut().remove(&id);
                match res.ok().and_then(|v| v.get::<(u32,)>()) {
                    Some((sid,)) => {
                        if d.retired.borrow_mut().remove(&id) {
                            d.close(sid);
                        } else {
                            d.shown.borrow_mut().insert(id, sid);
                        }
                    }
                    None => {
                        d.retired.borrow_mut().remove(&id);
                        eprintln!("cmux-linux: notification server unavailable, using GTK notifications");
                        d.server_sound.set(Some(false));
                        let r = fallback.request();
                        if r.sound && server_sound {
                            play_sound(r.kind.sound()); // the server never got to play it
                        }
                        d.show_fallback(&r);
                    }
                }
            },
        );
    }

    /// GApplication notification: no sound or urgency on GNOME, but works
    /// through the portal inside sandboxes.
    fn show_fallback(&self, r: &Request) {
        let n = gio::Notification::new(if r.title.is_empty() { APP_NAME } else { r.title });
        n.set_body(Some(r.body));
        n.set_priority(match r.kind.urgency() {
            crate::notify::URGENCY_CRITICAL => gio::NotificationPriority::Urgent,
            crate::notify::URGENCY_LOW => gio::NotificationPriority::Low,
            _ => gio::NotificationPriority::Normal,
        });
        n.set_default_action_and_target_value("app.open-notification", Some(&r.id.to_variant()));
        let key = match r.pane {
            Some(p) => format!("cmux-pane-{p}"),
            None => format!("cmux-{}", r.id),
        };
        self.gapp.send_notification(Some(&key), &n);
    }

    /// Withdraw desktop notifications whose in-app entry has been read
    /// (pane focused, "mark all read", cleared, pane closed…).
    pub fn sync(&self, is_unread: impl Fn(u64) -> bool) {
        let stale: Vec<(u64, u32)> = self
            .shown
            .borrow()
            .iter()
            .filter(|(id, _)| !is_unread(**id))
            .map(|(id, sid)| (*id, *sid))
            .collect();
        for (id, sid) in stale {
            self.shown.borrow_mut().remove(&id);
            self.close(sid);
        }
        let late: Vec<u64> = self.pending.borrow().iter().copied().filter(|id| !is_unread(*id)).collect();
        self.retired.borrow_mut().extend(late);
        self.by_pane.borrow_mut().retain(|_, id| is_unread(*id));
    }

    fn close(&self, sid: u32) {
        self.tokens.borrow_mut().remove(&sid);
        let Some(conn) = &self.conn else { return };
        conn.call(
            Some(BUS),
            PATH,
            IFACE,
            "CloseNotification",
            Some(&(sid,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            CALL_TIMEOUT_MS,
            gio::Cancellable::NONE,
            |_| {},
        );
    }

    /// On quit: nothing would answer a click any more, so withdraw everything.
    pub fn close_all(&self) {
        let Some(conn) = &self.conn else { return };
        let all: Vec<u32> = self.shown.borrow_mut().drain().map(|(_, s)| s).collect();
        for sid in all {
            let _ = conn.call_sync(
                Some(BUS),
                PATH,
                IFACE,
                "CloseNotification",
                Some(&(sid,).to_variant()),
                None,
                gio::DBusCallFlags::NONE,
                300,
                gio::Cancellable::NONE,
            );
        }
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        if let (Some(conn), Some(id)) = (&self.conn, self.subscription.borrow_mut().take()) {
            #[allow(deprecated)]
            conn.signal_unsubscribe(id);
        }
    }
}

/// Owned copy of a request, for the fallback after an asynchronous failure.
struct FallbackCopy {
    id: u64,
    pane: Option<PaneId>,
    kind: Kind,
    title: String,
    body: String,
    sound: bool,
}

impl From<&Request<'_>> for FallbackCopy {
    fn from(r: &Request) -> Self {
        FallbackCopy {
            id: r.id,
            pane: r.pane,
            kind: r.kind,
            title: r.title.to_string(),
            body: r.body.to_string(),
            sound: r.sound,
        }
    }
}

impl FallbackCopy {
    fn request(&self) -> Request<'_> {
        Request {
            id: self.id,
            pane: self.pane,
            kind: self.kind,
            title: &self.title,
            body: &self.body,
            sound: self.sound,
        }
    }
}

/// Play a sound-theme event ourselves: libcanberra respects the desktop's
/// sound theme, volume and "event sounds" setting. Without it, beep.
fn play_sound(event: &str) {
    let argv = [
        std::ffi::OsStr::new("canberra-gtk-play"),
        std::ffi::OsStr::new("--id"),
        std::ffi::OsStr::new(event),
        std::ffi::OsStr::new("--description"),
        std::ffi::OsStr::new("cmux notification"),
    ];
    let flags = gio::SubprocessFlags::STDOUT_SILENCE | gio::SubprocessFlags::STDERR_SILENCE;
    match gio::Subprocess::newv(&argv, flags) {
        Ok(p) => p.wait_async(gio::Cancellable::NONE, |_| {}),
        Err(_) => {
            if let Some(d) = gdk::Display::default() {
                d.beep();
            }
        }
    }
}
