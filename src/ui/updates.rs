//! The "new version available" notice (see [`crate::update_check`]).
//! Nothing happens unless Settings → Notifications → Check for updates is on.

use super::AppRc;
use crate::update_check::{self, Release, State};
use gtk4::{gio, glib};
use libadwaita as adw;
use std::path::PathBuf;

fn state_path() -> PathBuf {
    crate::paths::state_dir().join("update-check.json")
}

/// Check now if it is turned on and due, and again every hour (each check
/// still asks GitHub at most once a day).
pub fn attach(app: &AppRc) {
    check(app);
    let w = app.weak();
    glib::timeout_add_seconds_local(3600, move || {
        let Some(a) = w.upgrade() else {
            return glib::ControlFlow::Break;
        };
        check(&a);
        glib::ControlFlow::Continue
    });
}

pub fn check(app: &AppRc) {
    if !app.inner.borrow().config.updates.check {
        return;
    }
    let path = state_path();
    let mut state = State::load(&path);
    let now = crate::usage::now();
    if !state.due(now) {
        return;
    }
    // Record the attempt first, so a failing or slow request is not repeated.
    state.last_check = now;
    if let Err(e) = state.save(&path) {
        eprintln!("oxishell: update check skipped, cannot save state: {e}");
        return;
    }
    let (tx, rx) = async_channel::bounded(1);
    let url = update_check::url();
    let spawned = std::thread::Builder::new().name("oxishell-update".into()).spawn(move || {
        let release = update_check::fetch(&url).ok().and_then(|body| update_check::parse_release(&body));
        let _ = tx.send_blocking(release);
    });
    if spawned.is_err() {
        return;
    }
    let w = app.weak();
    glib::spawn_future_local(async move {
        let Ok(Some(release)) = rx.recv().await else { return };
        let Some(a) = w.upgrade() else { return };
        let mut state = State::load(&path);
        if state.announce(env!("CARGO_PKG_VERSION"), &release) {
            let _ = state.save(&path);
            show(&a, &release);
        }
    });
}

/// A toast that stays until dismissed, with a button to the release notes.
fn show(app: &AppRc, release: &Release) {
    let toast = adw::Toast::builder()
        .title(glib::markup_escape_text(&format!("oxishell {} is out", release.version)).as_str())
        .button_label("Release notes")
        .timeout(0)
        .build();
    let url = release.url.clone();
    let w = app.weak();
    toast.connect_button_clicked(move |_| {
        if let Some(a) = w.upgrade() {
            gtk4::UriLauncher::new(&url).launch(Some(&a.window), gio::Cancellable::NONE, |_| {});
        }
    });
    app.toasts.add_toast(toast.clone());
    *app.update_notice.borrow_mut() = Some(toast);
}
