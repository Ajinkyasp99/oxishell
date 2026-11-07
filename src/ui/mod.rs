//! GTK4/libadwaita front end. The model in `crate::model` is the source of
//! truth; widgets are (re)built from it.

mod blocks;
mod chips;
mod dialogs;
mod fonts;
mod history_panel;
mod ipc_handler;
mod links;
mod meta_worker;
mod pane;
mod scroll;
mod sidebar;
mod suggest;
mod themes;
mod usage_footer;
pub mod vte_fix;
mod zoom;

use crate::agents::AgentState;
use crate::config::Config;
use crate::model::{AppState, Direction, Node, Orient, PaneClosed, PaneId, WorkspaceId};
use crate::notify::NotificationCenter;
use crate::session::{PaneSnapshot, Snapshot};
use gtk4::glib::translate::IntoGlib;
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib, Orientation};
use libadwaita as adw;
use pane::Pane;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use vte4::prelude::*;

pub struct Inner {
    pub state: AppState,
    pub config: Config,
    pub notes: NotificationCenter,
    pub panes: HashMap<PaneId, Pane>,
    pub pages: HashMap<WorkspaceId, gtk4::Box>,
    pub keymap: HashMap<(u32, gdk::ModifierType), String>,
    pub font_scale: f64,
    /// The resolved theme (see `crate::theme`).
    pub theme: crate::theme::Theme,
    /// Shell history for suggestions (newest first) plus commands typed this session.
    pub history: crate::history::History,
}

pub struct App {
    pub app: adw::Application,
    pub window: adw::ApplicationWindow,
    pub stack: gtk4::Stack,
    pub sidebar_list: gtk4::ListBox,
    sidebar_rows: RefCell<HashMap<WorkspaceId, sidebar::Row>>,
    pub usage_footer: usage_footer::Footer,
    pub split: gtk4::Paned,
    pub status: gtk4::Label,
    pub toasts: adw::ToastOverlay,
    pub notif_button: gtk4::MenuButton,
    pub notif_badge: gtk4::Label,
    /// Generated CSS for the current theme (whole-window colours, gradients).
    pub theme_css: gtk4::CssProvider,
    pub inner: RefCell<Inner>,
    pub meta_tx: std::sync::mpsc::Sender<meta_worker::Job>,
    suppress_select: Cell<bool>,
    save_pending: Cell<bool>,
    quitting: Cell<bool>,
}

pub type AppRc = Rc<App>;

/// Below this window width the sidebar is hidden automatically.
const NARROW_WIDTH: i32 = 640;

/// Entry point used by `main`: activate builds the window once.
pub fn run() -> glib::ExitCode {
    // Before any terminal draws: VTE 0.76 would otherwise crash ~30 s after a
    // zoom, font change or pane close (see vte_fix).
    vte_fix::install();
    // Register before GTK creates its first Pango font map. Build surfaces errors.
    let _ = fonts::register();
    let mut flags = gio::ApplicationFlags::empty();
    if std::env::var_os("CMUX_NON_UNIQUE").is_some() {
        flags |= gio::ApplicationFlags::NON_UNIQUE;
    }
    let app = adw::Application::builder().application_id(crate::APP_ID).flags(flags).build();
    let holder: Rc<RefCell<Option<AppRc>>> = Rc::new(RefCell::new(None));
    app.connect_activate(move |gapp| {
        if let Some(existing) = holder.borrow().as_ref() {
            existing.window.present();
            return;
        }
        let a = App::build(gapp);
        *holder.borrow_mut() = Some(a);
    });
    app.run_with_args::<&str>(&[])
}

/// Kept for the Phase 1 entry point signature.
pub fn build_window(app: &adw::Application) {
    App::build(app);
}

impl App {
    pub fn build(gapp: &adw::Application) -> AppRc {
        let font_err = fonts::register().err();
        let (config, config_err) = Config::load_from(&Config::path());

        let css = gtk4::CssProvider::new();
        css.load_from_string(include_str!("style.css"));
        gtk4::style_context_add_provider_for_display(
            &gdk::Display::default().expect("display"),
            &css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        // Generated per-theme CSS sits above the static stylesheet.
        let theme_css = gtk4::CssProvider::new();
        gtk4::style_context_add_provider_for_display(
            &gdk::Display::default().expect("display"),
            &theme_css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );

        // Header bar
        let header = adw::HeaderBar::new();
        let title = adw::WindowTitle::new("cmux", "");
        header.set_title_widget(Some(&title));
        let sidebar_btn = gtk4::Button::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("Toggle sidebar (Ctrl+Shift+B)")
            .build();
        sidebar_btn.set_action_name(Some("app.toggle-sidebar"));
        let new_ws = gtk4::Button::builder()
            .icon_name("tab-new-symbolic")
            .tooltip_text("New workspace (Ctrl+Shift+N)")
            .action_name("app.new-workspace")
            .build();
        header.pack_start(&sidebar_btn);
        header.pack_start(&new_ws);
        let split_r = gtk4::Button::builder()
            .icon_name("view-dual-symbolic")
            .tooltip_text("Split right (Ctrl+Shift+E)")
            .action_name("app.split-right")
            .build();
        let split_d = gtk4::Button::builder()
            .icon_name("view-paged-symbolic")
            .tooltip_text("Split down (Ctrl+Shift+O)")
            .action_name("app.split-down")
            .build();

        let menu = gio::Menu::new();
        menu.append(Some("Command Palette"), Some("app.command-palette"));
        menu.append(Some("Search Scrollback"), Some("app.search"));
        menu.append(Some("Settings"), Some("app.settings"));
        menu.append(Some("Keyboard Shortcuts"), Some("app.show-shortcuts"));
        menu.append(Some("About cmux-linux"), Some("app.about"));
        let menu_btn = gtk4::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .tooltip_text("Main menu")
            .build();

        let notif_badge = gtk4::Label::new(None);
        notif_badge.add_css_class("badge");
        notif_badge.set_visible(false);
        let bell_box = gtk4::Box::new(Orientation::Horizontal, 4);
        bell_box.append(&gtk4::Image::from_icon_name("preferences-system-notifications-symbolic"));
        bell_box.append(&notif_badge);
        let notif_button = gtk4::MenuButton::builder()
            .child(&bell_box)
            .tooltip_text("Notifications (Ctrl+Shift+I)")
            .build();

        header.pack_end(&menu_btn);
        header.pack_end(&notif_button);
        header.pack_end(&split_d);
        header.pack_end(&split_r);

        // Sidebar
        let sidebar_box = gtk4::Box::new(Orientation::Vertical, 0);
        sidebar_box.add_css_class("sidebar-pane");
        let heading = gtk4::Label::builder().label("WORKSPACES").xalign(0.0).build();
        heading.add_css_class("sidebar-heading");
        let sidebar_list = gtk4::ListBox::new();
        sidebar_list.add_css_class("ws-list");
        sidebar_list.set_selection_mode(gtk4::SelectionMode::Single);
        let sidebar_scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vexpand(true)
            .child(&sidebar_list)
            .build();
        sidebar_box.append(&heading);
        sidebar_box.append(&sidebar_scroll);
        let (usage_footer, usage_rx) = usage_footer::Footer::new();
        sidebar_box.append(&usage_footer.root);

        let stack = gtk4::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.set_transition_type(gtk4::StackTransitionType::None);
        // Size to the visible workspace only, not the largest hidden one.
        stack.set_hhomogeneous(false);
        stack.set_vhomogeneous(false);

        let split = gtk4::Paned::new(Orientation::Horizontal);
        split.set_start_child(Some(&sidebar_box));
        split.set_end_child(Some(&stack));
        split.set_shrink_start_child(false);
        split.set_resize_start_child(false);
        split.set_position(config.appearance.sidebar_width);
        split.set_vexpand(true);
        sidebar_box.set_visible(config.appearance.sidebar_visible);

        let status = gtk4::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .build();
        status.set_hexpand(true);
        status.add_css_class("dim-label");
        // System CPU / memory usage, right-aligned in the status bar.
        let sys_label = gtk4::Label::builder().xalign(1.0).build();
        sys_label.add_css_class("dim-label");
        sys_label.add_css_class("numeric");
        let status_bar = gtk4::Box::new(Orientation::Horizontal, 12);
        status_bar.add_css_class("statusbar");
        status_bar.append(&status);
        status_bar.append(&sys_label);
        {
            let mut sampler = crate::sysstat::Sampler::default();
            sys_label.set_label(&sampler.sample());
            let label = sys_label.downgrade();
            glib::timeout_add_seconds_local(2, move || match label.upgrade() {
                Some(l) => {
                    l.set_label(&sampler.sample());
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            });
        }

        let root = gtk4::Box::new(Orientation::Vertical, 0);
        root.add_css_class("cmux-root");
        root.append(&header);
        root.append(&split);
        root.append(&status_bar);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&root));

        let window = adw::ApplicationWindow::builder()
            .application(gapp)
            .title("cmux")
            .default_width(1200)
            .default_height(760)
            .width_request(480)
            .height_request(320)
            .content(&toasts)
            .build();

        // Optional initial size, e.g. CMUX_GEOMETRY=900x600.
        if let Some((w, h)) = std::env::var("CMUX_GEOMETRY").ok().and_then(|g| {
            let (w, h) = g.split_once('x')?;
            Some((w.parse::<i32>().ok()?, h.parse::<i32>().ok()?))
        }) {
            window.set_default_size(w.max(480), h.max(320));
        }

        let (meta_tx, meta_rx) = meta_worker::spawn();

        let app = Rc::new(App {
            app: gapp.clone(),
            window,
            stack,
            sidebar_list,
            sidebar_rows: RefCell::new(HashMap::new()),
            usage_footer,
            split,
            status,
            toasts,
            notif_button,
            notif_badge,
            theme_css,
            inner: RefCell::new(Inner {
                state: AppState::default(),
                config,
                notes: NotificationCenter::default(),
                panes: HashMap::new(),
                pages: HashMap::new(),
                keymap: HashMap::new(),
                font_scale: 1.0,
                theme: crate::theme::default_theme(true),
                history: crate::history::History::load(&crate::paths::home()),
            }),
            meta_tx,
            suppress_select: Cell::new(false),
            save_pending: Cell::new(false),
            quitting: Cell::new(false),
        });

        // Keep the title in sync.
        {
            let w = Rc::downgrade(&app);
            app.stack.connect_visible_child_name_notify(move |_| {
                if let Some(a) = w.upgrade() {
                    let name = a
                        .inner
                        .borrow()
                        .state
                        .active_workspace()
                        .map(|w| w.name.clone())
                        .unwrap_or_default();
                    title.set_subtitle(&name);
                }
            });
        }

        {
            // Hide the sidebar automatically in very narrow windows (not persisted).
            let w = Rc::downgrade(&app);
            app.window.connect_default_width_notify(move |_| {
                if let Some(a) = w.upgrade() {
                    a.fit_sidebar_to_width();
                }
            });
            app.fit_sidebar_to_width();
        }
        app.install_actions();
        app.install_key_handler();
        sidebar::connect(&app);
        dialogs::connect_notifications_popover(&app);
        meta_worker::attach(&app, meta_rx);
        usage_footer::attach(&app, usage_rx);
        app.apply_config();

        // Restore or start fresh.
        let restore = app.inner.borrow().config.session.restore;
        let mut restored = false;
        if restore {
            match Snapshot::load(&crate::session::session_path()) {
                Ok(Some(snap)) if !snap.state.workspaces.is_empty() => {
                    app.restore(snap);
                    restored = true;
                }
                Ok(_) => {}
                Err(e) => app.toast(&e),
            }
        }
        if !restored {
            let cwd = std::env::current_dir()
                .ok()
                .filter(|d| d != std::path::Path::new("/"))
                .unwrap_or_else(crate::paths::home);
            let _ = app.new_workspace(None, Some(cwd));
        }
        if let Some(e) = config_err {
            app.toast(&format!("Config error, using defaults: {e}"));
        }
        if let Some(e) = font_err {
            app.toast(&format!("Bundled Hack font unavailable: {e}"));
        }
        let conflicts = app.inner.borrow().config.shortcut_conflicts();
        for (k, actions) in conflicts {
            app.toast(&format!("Shortcut {k} is bound to several actions: {}", actions.join(", ")));
        }

        ipc_handler::start(&app);

        {
            let w = Rc::downgrade(&app);
            app.window.connect_close_request(move |_| {
                if let Some(a) = w.upgrade() {
                    a.quitting.set(true);
                    a.save_session(true);
                    a.shutdown_panes();
                }
                glib::Propagation::Proceed
            });
        }
        {
            let w = Rc::downgrade(&app);
            app.window.connect_is_active_notify(move |win| {
                if win.is_active() {
                    if let Some(a) = w.upgrade() {
                        // Coming back to the window counts as seeing the focused pane.
                        if let Some(p) = a.focused_pane() {
                            a.mark_read(p);
                        }
                    }
                }
            });
        }

        // SIGTERM/SIGINT/SIGHUP (logout, `kill`, Ctrl+C in a launching terminal):
        // close gracefully so the session and scrollback are saved.
        for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            let w = Rc::downgrade(&app);
            glib::unix_signal_add_local(sig, move || {
                if let Some(a) = w.upgrade() {
                    a.window.close();
                }
                glib::ControlFlow::Continue
            });
        }

        app.window.present();
        app.refresh();

        if let Some(ms) = std::env::var("CMUX_QUIT_AFTER_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
            let w = Rc::downgrade(&app);
            glib::timeout_add_local_once(std::time::Duration::from_millis(ms), move || {
                if let Some(a) = w.upgrade() {
                    a.window.close();
                }
            });
        }
        app
    }

    pub fn weak(self: &Rc<Self>) -> Weak<App> {
        Rc::downgrade(self)
    }

    pub fn toast(&self, msg: &str) {
        eprintln!("cmux-linux: {msg}");
        self.toasts.add_toast(
            adw::Toast::builder()
                .title(glib::markup_escape_text(msg).as_str())
                .timeout(6)
                .build(),
        );
    }

    // ---- workspaces ---------------------------------------------------------

    pub fn new_workspace(self: &Rc<Self>, name: Option<&str>, cwd: Option<PathBuf>) -> Result<(WorkspaceId, PaneId), String> {
        let (wid, pid) = self.inner.borrow_mut().state.create_workspace(name).map_err(|e| e.to_string())?;
        self.add_page(wid);
        self.create_pane(wid, pid, cwd, None);
        self.rebuild_workspace(wid);
        self.select_workspace(wid);
        self.schedule_save();
        Ok((wid, pid))
    }

    fn add_page(&self, wid: WorkspaceId) {
        let page = gtk4::Box::new(Orientation::Vertical, 0);
        page.set_hexpand(true);
        page.set_vexpand(true);
        self.stack.add_named(&page, Some(&wid.to_string()));
        self.inner.borrow_mut().pages.insert(wid, page);
    }

    pub fn select_workspace(self: &Rc<Self>, wid: WorkspaceId) {
        let focused = {
            let mut i = self.inner.borrow_mut();
            if i.state.select(wid).is_err() {
                return;
            }
            i.state.workspace(wid).map(|w| w.focused)
        };
        self.stack.set_visible_child_name(&wid.to_string());
        if let Some(p) = focused {
            self.focus_pane(p);
        }
        self.refresh();
        self.request_meta(true);
    }

    pub fn rename_workspace(self: &Rc<Self>, wid: WorkspaceId, name: &str) -> Result<(), String> {
        self.inner.borrow_mut().state.rename(wid, name).map_err(|e| e.to_string())?;
        self.refresh();
        self.schedule_save();
        Ok(())
    }

    pub fn move_workspace(self: &Rc<Self>, wid: WorkspaceId, to: usize) -> Result<(), String> {
        self.inner.borrow_mut().state.move_workspace(wid, to).map_err(|e| e.to_string())?;
        self.refresh();
        self.schedule_save();
        Ok(())
    }

    pub fn workspace_running(&self, wid: WorkspaceId) -> Vec<PaneId> {
        let i = self.inner.borrow();
        let Some(ws) = i.state.workspace(wid) else { return vec![] };
        ws.layout
            .panes()
            .into_iter()
            .filter(|p| i.panes.get(p).is_some_and(|p| p.has_running_child()))
            .collect()
    }

    /// Close a workspace. With `interactive`, asks first when programs are running.
    pub fn close_workspace(self: &Rc<Self>, wid: WorkspaceId, force: bool, interactive: bool) -> Result<(), String> {
        if self.inner.borrow().state.workspace(wid).is_none() {
            return Err(format!("no such workspace: {wid}"));
        }
        let running = self.workspace_running(wid).len();
        if !force && interactive {
            // Closing from the window (button, menu, shortcut) always asks first.
            let (name, panes) = {
                let i = self.inner.borrow();
                let ws = i.state.workspace(wid).unwrap();
                (ws.name.clone(), ws.layout.panes().len())
            };
            let mut body = if panes == 1 {
                "This closes its terminal and ends the shell.".to_string()
            } else {
                format!("This closes its {panes} terminals and ends their shells.")
            };
            if running > 0 {
                body.push_str(&format!(
                    "\n\n{running} pane{} still running a program, which will be terminated.",
                    if running == 1 { " is" } else { "s are" }
                ));
            }
            let w = self.weak();
            dialogs::confirm(
                self,
                &format!("Close workspace \u{201c}{name}\u{201d}?"),
                &body,
                "Close Workspace",
                move || {
                    if let Some(a) = w.upgrade() {
                        let _ = a.close_workspace(wid, true, false);
                    }
                },
            );
            return Ok(());
        }
        let confirm = self.inner.borrow().config.terminal.confirm_close_running;
        if !force && confirm && running > 0 {
            return Err("workspace has running programs; pass force to close it".into());
        }
        let panes = self.inner.borrow_mut().state.close_workspace(wid).map_err(|e| e.to_string())?;
        for p in panes {
            self.destroy_pane(p, true);
        }
        let page = self.inner.borrow_mut().pages.remove(&wid);
        if let Some(page) = page {
            self.stack.remove(&page);
        }
        self.inner.borrow_mut().notes.forget_workspace(wid);
        self.after_structure_change();
        Ok(())
    }

    fn after_structure_change(self: &Rc<Self>) {
        let next = self.inner.borrow().state.active;
        match next {
            Some(w) => self.select_workspace(w),
            None => {
                // Last workspace closed: close the window like any terminal app.
                self.refresh();
                if !self.quitting.get() {
                    self.window.close();
                }
            }
        }
        self.schedule_save();
    }

    // ---- panes --------------------------------------------------------------

    pub fn focused_pane(&self) -> Option<PaneId> {
        self.inner.borrow().state.active_workspace().map(|w| w.focused)
    }

    pub fn split(self: &Rc<Self>, pane: Option<PaneId>, orient: Orient, cwd: Option<PathBuf>) -> Result<PaneId, String> {
        let pane = pane.or_else(|| self.focused_pane()).ok_or("no pane to split")?;
        let cwd = cwd.or_else(|| self.inner.borrow().panes.get(&pane).and_then(|p| p.cwd.clone()));
        let new = self.inner.borrow_mut().state.split(pane, orient).map_err(|e| e.to_string())?;
        let wid = self.inner.borrow().state.workspace_of(new).unwrap();
        self.create_pane(wid, new, cwd, None);
        self.rebuild_workspace(wid);
        self.select_workspace(wid);
        self.focus_pane(new);
        self.schedule_save();
        Ok(new)
    }

    pub fn close_pane(self: &Rc<Self>, pane: PaneId, force: bool, interactive: bool) -> Result<(), String> {
        let (running, confirm) = {
            let i = self.inner.borrow();
            let p = i.panes.get(&pane).ok_or(format!("no such pane: {pane}"))?;
            (p.has_running_child(), i.config.terminal.confirm_close_running)
        };
        if running && !force && confirm {
            if interactive {
                let w = self.weak();
                dialogs::confirm(
                    self,
                    "Close pane?",
                    "A program is still running in this pane. Closing it will terminate it.",
                    "Close",
                    move || {
                        if let Some(a) = w.upgrade() {
                            let _ = a.close_pane(pane, true, false);
                        }
                    },
                );
                return Ok(());
            }
            return Err("pane has a running program; pass force to close it".into());
        }
        self.remove_pane(pane, true);
        Ok(())
    }

    /// Remove a pane from the model and UI. `kill` terminates its processes.
    fn remove_pane(self: &Rc<Self>, pane: PaneId, kill: bool) {
        let res = self.inner.borrow_mut().state.close_pane(pane);
        self.destroy_pane(pane, kill);
        match res {
            Ok(PaneClosed::Refocus(p)) => {
                let wid = self.inner.borrow().state.workspace_of(p);
                if let Some(wid) = wid {
                    self.rebuild_workspace(wid);
                    if self.inner.borrow().state.active == Some(wid) {
                        self.focus_pane(p);
                    }
                }
                self.refresh();
                self.schedule_save();
            }
            Ok(PaneClosed::WorkspaceRemoved(wid)) => {
                let page = self.inner.borrow_mut().pages.remove(&wid);
                if let Some(page) = page {
                    self.stack.remove(&page);
                }
                self.inner.borrow_mut().notes.forget_workspace(wid);
                self.after_structure_change();
            }
            Err(_) => {}
        }
    }

    fn destroy_pane(&self, pane: PaneId, kill: bool) {
        let p = self.inner.borrow_mut().panes.remove(&pane);
        self.inner.borrow_mut().notes.forget_pane(pane);
        let _ = std::fs::remove_file(crate::shellint::env_file(pane));
        if let Some(p) = p {
            pane::detach(&p.frame);
            if kill {
                p.terminate();
            }
        }
        crate::session::prune_scrollback(
            &self
                .inner
                .borrow()
                .state
                .all_panes()
                .into_iter()
                .map(|(_, p)| p)
                .collect::<Vec<_>>(),
        );
    }

    /// Called when a pane's shell exits on its own.
    pub fn on_child_exited(self: &Rc<Self>, pane: PaneId) {
        if self.quitting.get() || !self.inner.borrow().panes.contains_key(&pane) {
            return;
        }
        self.remove_pane(pane, false);
    }

    pub fn focus_pane(self: &Rc<Self>, pane: PaneId) {
        let (wid, term) = {
            let mut i = self.inner.borrow_mut();
            let Ok(wid) = i.state.focus(pane) else { return };
            (wid, i.panes.get(&pane).map(|p| p.term.clone()))
        };
        if self.stack.visible_child_name().as_deref() != Some(&wid.to_string()) {
            self.stack.set_visible_child_name(&wid.to_string());
        }
        if let Some(t) = term {
            if t.is_mapped() {
                if !t.has_focus() {
                    t.grab_focus();
                }
            } else {
                // A new pane is not mapped yet and cannot take focus; do it on map.
                let handler: Rc<Cell<Option<glib::SignalHandlerId>>> = Rc::new(Cell::new(None));
                let h = handler.clone();
                let id = t.connect_map(move |t| {
                    if let Some(id) = h.take() {
                        t.disconnect(id);
                    }
                    t.grab_focus();
                });
                handler.set(Some(id));
            }
        }
        self.mark_read(pane);
        self.refresh();
    }

    /// Called from a terminal's focus-enter handler (deferred to idle).
    pub fn on_pane_focused(self: &Rc<Self>, pane: PaneId) {
        // GTK can deliver focus to a terminal on a page that was just hidden (e.g. while
        // a new workspace is being shown). Only panes of the visible workspace count.
        let wid = self.inner.borrow().state.workspace_of(pane);
        if wid.is_none() || self.stack.visible_child_name().as_deref() != wid.map(|w| w.to_string()).as_deref() {
            return;
        }
        let changed = {
            let mut i = self.inner.borrow_mut();
            let before = i.state.active_workspace().map(|w| w.focused);
            let _ = i.state.focus(pane);
            before != Some(pane)
        };
        self.mark_read(pane);
        if changed {
            self.refresh();
            self.request_meta(true);
        }
    }

    pub fn focus_direction(self: &Rc<Self>, dir: Direction) {
        let next = self.inner.borrow_mut().state.focus_direction(dir);
        if let Some(p) = next {
            self.focus_pane(p);
        }
    }

    /// Rebuild the widget tree of a workspace from its layout.
    pub fn rebuild_workspace(self: &Rc<Self>, wid: WorkspaceId) {
        let (layout, page, frames) = {
            let i = self.inner.borrow();
            let Some(ws) = i.state.workspace(wid) else { return };
            let frames: HashMap<PaneId, gtk4::Box> = ws
                .layout
                .panes()
                .iter()
                .filter_map(|p| i.panes.get(p).map(|x| (*p, x.frame.clone())))
                .collect();
            (ws.layout.clone(), i.pages.get(&wid).cloned(), frames)
        };
        let Some(page) = page else { return };
        for f in frames.values() {
            pane::detach(f);
        }
        while let Some(c) = page.first_child() {
            page.remove(&c);
        }
        let root = self.build_node(wid, &layout, &frames, &mut Vec::new());
        page.append(&root);
    }

    fn build_node(
        self: &Rc<Self>,
        wid: WorkspaceId,
        node: &Node,
        frames: &HashMap<PaneId, gtk4::Box>,
        path: &mut Vec<bool>,
    ) -> gtk4::Widget {
        match node {
            Node::Leaf { pane } => frames
                .get(pane)
                .map(|f| f.clone().upcast())
                .unwrap_or_else(|| gtk4::Label::new(Some("missing pane")).upcast()),
            Node::Split { orient, ratio, a, b } => {
                let o = match orient {
                    Orient::Horizontal => Orientation::Horizontal,
                    Orient::Vertical => Orientation::Vertical,
                };
                let paned = gtk4::Paned::new(o);
                paned.set_wide_handle(true);
                paned.set_hexpand(true);
                paned.set_vexpand(true);
                // Let panes shrink below their natural size so the window can get small.
                paned.set_shrink_start_child(true);
                paned.set_shrink_end_child(true);
                path.push(false);
                let wa = self.build_node(wid, a, frames, path);
                path.pop();
                path.push(true);
                let wb = self.build_node(wid, b, frames, path);
                path.pop();
                paned.set_start_child(Some(&wa));
                paned.set_end_child(Some(&wb));

                let ready = Rc::new(Cell::new(false));
                let ratio = *ratio;
                {
                    let ready = ready.clone();
                    paned.add_tick_callback(move |p, _| {
                        let size = if p.orientation() == Orientation::Horizontal {
                            p.width()
                        } else {
                            p.height()
                        };
                        if size <= 0 {
                            return glib::ControlFlow::Continue;
                        }
                        p.set_position((ratio * size as f64).round() as i32);
                        ready.set(true);
                        glib::ControlFlow::Break
                    });
                }
                let my_path = path.clone();
                let w = self.weak();
                paned.connect_position_notify(move |p| {
                    if !ready.get() {
                        return;
                    }
                    let size = if p.orientation() == Orientation::Horizontal {
                        p.width()
                    } else {
                        p.height()
                    };
                    if size <= 0 {
                        return;
                    }
                    if let Some(a) = w.upgrade() {
                        let r = p.position() as f64 / size as f64;
                        if let Ok(ws) = a.inner.borrow_mut().state.workspace_mut(wid) {
                            ws.layout.set_ratio_at(&my_path, r);
                        }
                        a.schedule_save();
                    }
                });
                paned.upcast()
            }
        }
    }

    // ---- notifications ------------------------------------------------------

    /// Record a notification. Visible, focused panes in an active window are
    /// marked read immediately; otherwise the pane gets an attention ring and
    /// a desktop notification is sent.
    pub fn notify(self: &Rc<Self>, pane: Option<PaneId>, title: &str, body: &str, source: &str) -> u64 {
        let wid = pane.and_then(|p| self.inner.borrow().state.workspace_of(p));
        let seen = pane.is_some() && pane == self.focused_pane() && self.window.is_active();
        let (id, title, body, desktop) = {
            let mut i = self.inner.borrow_mut();
            let desktop = i.config.notifications.desktop;
            let n = i.notes.push(pane, wid, title, body, source);
            let r = (n.id, n.title.clone(), n.body.clone());
            if seen {
                if let Some(p) = pane {
                    i.notes.mark_pane_read(p);
                }
            }
            (r.0, r.1, r.2, desktop)
        };
        if !seen && desktop {
            let n = gio::Notification::new(if title.is_empty() { "cmux" } else { &title });
            let ws_name = wid.and_then(|w| self.inner.borrow().state.workspace(w).map(|w| w.name.clone()));
            let body = match ws_name {
                Some(ws) if body.is_empty() => ws,
                Some(ws) => format!("{ws}: {body}"),
                None => body,
            };
            n.set_body(Some(&body));
            n.set_default_action_and_target_value("app.open-notification", Some(&id.to_variant()));
            // Failures (no notification daemon) are reported by D-Bus asynchronously and ignored.
            self.app.send_notification(Some(&format!("cmux-{id}")), &n);
        }
        self.refresh();
        id
    }

    pub fn mark_read(self: &Rc<Self>, pane: PaneId) {
        let changed = self.inner.borrow_mut().notes.mark_pane_read(pane);
        if changed {
            self.refresh();
        }
    }

    pub fn jump_to_unread(self: &Rc<Self>) -> Option<PaneId> {
        let target = self.inner.borrow().notes.latest_unread().and_then(|n| n.pane);
        if let Some(p) = target {
            self.window.present();
            self.focus_pane(p);
        }
        target
    }

    pub fn open_notification(self: &Rc<Self>, id: u64) {
        let pane = self.inner.borrow().notes.all().iter().find(|n| n.id == id).and_then(|n| n.pane);
        self.window.present();
        if let Some(p) = pane {
            self.focus_pane(p);
        }
    }

    pub fn agent_event(
        self: &Rc<Self>,
        pane: PaneId,
        agent: &str,
        state: AgentState,
        message: Option<String>,
        session: Option<String>,
    ) -> Result<(), String> {
        {
            let mut i = self.inner.borrow_mut();
            let p = i.panes.get_mut(&pane).ok_or(format!("no such pane: {pane}"))?;
            p.agent = Some((agent.to_string(), state, message.clone()));
            if let Some(s) = session {
                p.agent_session = Some((agent.to_string(), s));
            }
        }
        if state.notifies() {
            let name = crate::agents::display_name(agent);
            let title = match state {
                AgentState::Waiting => format!("{name} needs input"),
                _ => format!("{name} finished"),
            };
            self.notify(Some(pane), &title, message.as_deref().unwrap_or(""), &format!("agent:{agent}"));
        } else {
            self.refresh();
        }
        self.schedule_save();
        Ok(())
    }

    // ---- chrome -------------------------------------------------------------

    /// Refresh sidebar, status bar, badges and pane classes from the model.
    pub fn refresh(self: &Rc<Self>) {
        sidebar::rebuild(self);
        usage_footer::refresh(self);
        let i = self.inner.borrow();
        let focused = i.state.active_workspace().map(|w| w.focused);
        for (id, p) in &i.panes {
            set_class(&p.frame, "focused", Some(*id) == focused);
            set_class(&p.frame, "attention", i.notes.unread_for_pane(*id) > 0);
            p.chips
                .update(p.python_env.as_deref(), p.cwd.as_deref(), p.git.as_ref(), i.config.appearance.chips);
        }
        let unread = i.notes.unread_total();
        self.notif_badge.set_label(&unread.to_string());
        self.notif_badge.set_visible(unread > 0);
        self.status.set_label(&status_text(&i));
        let title = i.state.active_workspace().map(|w| w.name.clone()).unwrap_or_default();
        drop(i);
        self.window.set_title(Some(&if title.is_empty() {
            "cmux".to_string()
        } else {
            format!("{title} — cmux")
        }));
    }

    fn fit_sidebar_to_width(&self) {
        let want = self.inner.borrow().config.appearance.sidebar_visible && self.window.default_width() >= NARROW_WIDTH;
        if let Some(s) = self.split.start_child() {
            // get_visible() is the property; is_visible() also needs visible parents.
            if s.get_visible() != want {
                s.set_visible(want);
            }
        }
    }

    pub fn set_sidebar_visible(self: &Rc<Self>, v: bool) {
        if let Some(s) = self.split.start_child() {
            s.set_visible(v);
        }
        let changed = {
            let mut i = self.inner.borrow_mut();
            let c = i.config.appearance.sidebar_visible != v;
            i.config.appearance.sidebar_visible = v;
            c
        };
        if changed {
            self.save_config();
        }
    }

    pub fn save_config(&self) {
        let cfg = self.inner.borrow().config.clone();
        if let Err(e) = cfg.save_to(&Config::path()) {
            self.toast(&format!("Could not save settings: {e}"));
        }
    }

    /// Apply appearance, fonts, colors and shortcuts from the current config.
    pub fn apply_config(self: &Rc<Self>) {
        let (mode, name) = {
            let c = &self.inner.borrow().config.appearance;
            (c.mode.clone(), c.theme.clone())
        };
        let sm = adw::StyleManager::default();
        let (theme, found) = crate::theme::resolve(&name, sm.is_dark());
        if !found {
            self.toast(&format!("Theme {name:?} not found; using the default theme"));
        }
        let theme = if theme.native {
            // The default theme follows the Style setting / the system.
            sm.set_color_scheme(match mode.as_str() {
                "light" => adw::ColorScheme::ForceLight,
                "dark" => adw::ColorScheme::ForceDark,
                _ => adw::ColorScheme::Default,
            });
            crate::theme::default_theme(sm.is_dark())
        } else {
            // Other themes decide light/dark themselves, like Warp.
            sm.set_color_scheme(if theme.is_dark() {
                adw::ColorScheme::ForceDark
            } else {
                adw::ColorScheme::ForceLight
            });
            theme
        };
        self.theme_css.load_from_string(&theme.css());
        self.inner.borrow_mut().theme = theme;
        {
            let w = self.weak();
            // The default theme re-colours terminals when the system switches light/dark.
            thread_local!(static HOOKED: Cell<bool> = const { Cell::new(false) });
            if !HOOKED.with(|h| h.replace(true)) {
                sm.connect_dark_notify(move |sm| {
                    if let Some(a) = w.upgrade() {
                        let native = a.inner.borrow().theme.native;
                        if native {
                            a.inner.borrow_mut().theme = crate::theme::default_theme(sm.is_dark());
                            a.apply_terminal_style();
                        }
                    }
                });
            }
        }
        self.apply_terminal_style();
        self.rebuild_keymap();
    }

    /// Switch theme by id, persist it and apply it to the whole window.
    pub fn set_theme(self: &Rc<Self>, id: &str) {
        self.inner.borrow_mut().config.appearance.theme = id.to_string();
        self.save_config();
        self.apply_config();
    }

    pub fn apply_terminal_style(&self) {
        let i = self.inner.borrow();
        for p in i.panes.values() {
            pane::style_terminal(&p.term, &i.config, i.font_scale, &i.theme);
            p.blocks.apply_style(&i.config, &i.theme);
        }
    }

    /// A command line was submitted in `pane`: start a command block for it.
    pub fn command_submitted(&self, pane: PaneId) {
        let Ok(i) = self.inner.try_borrow() else { return };
        let Some(p) = i.panes.get(&pane) else { return };
        if p.has_running_child() {
            return; // Enter inside a program (an agent, a REPL…), not a shell command
        }
        p.blocks.on_submit(
            p.cwd.as_deref(),
            p.git.as_ref().map(|g| g.branch.as_str()),
            crate::shellint::env_file(pane),
        );
    }

    fn rebuild_keymap(&self) {
        let shortcuts = self.inner.borrow().config.effective_shortcuts();
        let mut map = HashMap::new();
        for (action, accel) in shortcuts {
            if let Some((key, mods)) = gtk4::accelerator_parse(&accel) {
                map.insert((key.to_lower().into_glib(), mods), action);
            } else if !accel.is_empty() {
                self.toast(&format!("Invalid shortcut {accel:?} for {action}"));
            }
        }
        zoom::add_key_aliases(&mut map, &self.inner.borrow().config);
        self.inner.borrow_mut().keymap = map;
    }

    fn install_key_handler(self: &Rc<Self>) {
        let ctl = gtk4::EventControllerKey::new();
        ctl.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let w = self.weak();
        ctl.connect_key_pressed(move |_, key, _, state| {
            let Some(a) = w.upgrade() else { return glib::Propagation::Proceed };
            let mods = state & gtk4::accelerator_get_default_mod_mask();
            let action = a.inner.borrow().keymap.get(&(key.to_lower().into_glib(), mods)).cloned();
            if let Some(action) = action {
                a.run_action(&action);
                return glib::Propagation::Stop;
            }
            // Alt+1..9 selects workspace N.
            if mods == gdk::ModifierType::ALT_MASK {
                if let Some(d) = key.to_unicode().and_then(|c| c.to_digit(10)).filter(|d| (1..=9).contains(d)) {
                    let id = a.inner.borrow().state.workspaces.get(d as usize - 1).map(|w| w.id);
                    if let Some(id) = id {
                        a.select_workspace(id);
                        return glib::Propagation::Stop;
                    }
                }
            }
            glib::Propagation::Proceed
        });
        self.window.add_controller(ctl);
    }

    fn install_actions(self: &Rc<Self>) {
        for (name, _, _) in crate::config::DEFAULT_SHORTCUTS {
            let act = gio::SimpleAction::new(name, None);
            let w = self.weak();
            let n = name.to_string();
            act.connect_activate(move |_, _| {
                if let Some(a) = w.upgrade() {
                    a.run_action(&n);
                }
            });
            self.app.add_action(&act);
        }
        for name in ["show-shortcuts", "about"] {
            let act = gio::SimpleAction::new(name, None);
            let w = self.weak();
            let n = name.to_string();
            act.connect_activate(move |_, _| {
                if let Some(a) = w.upgrade() {
                    a.run_action(&n);
                }
            });
            self.app.add_action(&act);
        }
        // Parameterised actions (pane / workspace / notification ids).
        let ty = Some(glib::VariantTy::UINT64);
        for name in [
            "open-notification",
            "ws-rename",
            "ws-close",
            "ws-move-up",
            "ws-move-down",
            "pane-split-right",
            "pane-split-down",
            "pane-close",
            "pane-copy",
            "pane-paste",
            "pane-search",
            "pane-resume-agent",
        ] {
            let act = gio::SimpleAction::new(name, ty);
            let w = self.weak();
            let n = name.to_string();
            act.connect_activate(move |_, v| {
                let (Some(a), Some(id)) = (w.upgrade(), v.and_then(|v| v.get::<u64>())) else {
                    return;
                };
                a.run_id_action(&n, id);
            });
            self.app.add_action(&act);
        }
    }

    pub fn run_id_action(self: &Rc<Self>, name: &str, id: u64) {
        match name {
            "open-notification" => self.open_notification(id),
            "ws-rename" => dialogs::rename_workspace(self, id),
            "ws-close" => {
                let _ = self.close_workspace(id, false, true);
            }
            "ws-move-up" | "ws-move-down" => {
                let idx = self.inner.borrow().state.workspaces.iter().position(|w| w.id == id);
                if let Some(idx) = idx {
                    let to = if name == "ws-move-up" { idx.saturating_sub(1) } else { idx + 1 };
                    let _ = self.move_workspace(id, to);
                }
            }
            "pane-split-right" => self.report(self.split(Some(id), Orient::Horizontal, None).map(|_| ())),
            "pane-split-down" => self.report(self.split(Some(id), Orient::Vertical, None).map(|_| ())),
            "pane-close" => self.report(self.close_pane(id, false, true)),
            "pane-copy" | "pane-paste" | "pane-search" => {
                let t = self.inner.borrow().panes.get(&id).map(|p| (p.term.clone(), p.search.clone()));
                if let Some((t, s)) = t {
                    match name {
                        "pane-copy" => t.copy_clipboard_format(vte4::Format::Text),
                        "pane-paste" => t.paste_clipboard(),
                        _ => s.set_search_mode(true),
                    }
                }
            }
            "pane-resume-agent" => self.resume_agent(id),
            _ => {}
        }
    }

    fn report(&self, r: Result<(), String>) {
        if let Err(e) = r {
            self.toast(&e);
        }
    }

    /// Type the agent's resume command into the pane. Only reachable from an
    /// explicit user choice (context menu / palette); the user still presses Enter.
    pub fn resume_agent(self: &Rc<Self>, pane: PaneId) {
        let cmd = self
            .inner
            .borrow()
            .panes
            .get(&pane)
            .and_then(|p| p.agent_session.clone())
            .and_then(|(agent, sess)| crate::agents::resume_command(&agent, &sess));
        match cmd {
            Some(cmd) => {
                if let Some(p) = self.inner.borrow().panes.get(&pane) {
                    p.send_input(cmd.as_bytes());
                }
                self.focus_pane(pane);
            }
            None => self.toast("No resumable agent session recorded for this pane"),
        }
    }

    pub fn run_action(self: &Rc<Self>, action: &str) {
        let focused = self.focused_pane();
        let term = focused.and_then(|p| self.inner.borrow().panes.get(&p).map(|p| (p.term.clone(), p.search.clone())));
        match action {
            "new-workspace" => {
                let cwd = focused.and_then(|p| self.inner.borrow().panes.get(&p).and_then(|p| p.cwd.clone()));
                self.report(self.new_workspace(None, cwd).map(|_| ()));
            }
            "close-workspace" => {
                if let Some(w) = self.inner.borrow().state.active {
                    let _ = self.close_workspace(w, false, true);
                }
            }
            "rename-workspace" => {
                let w = self.inner.borrow().state.active;
                if let Some(w) = w {
                    dialogs::rename_workspace(self, w);
                }
            }
            "next-workspace" | "prev-workspace" => {
                let id = self
                    .inner
                    .borrow_mut()
                    .state
                    .select_relative(if action == "next-workspace" { 1 } else { -1 });
                if let Some(id) = id {
                    self.select_workspace(id);
                }
            }
            "split-right" => self.report(self.split(None, Orient::Horizontal, None).map(|_| ())),
            "split-down" => self.report(self.split(None, Orient::Vertical, None).map(|_| ())),
            "close-pane" => {
                if let Some(p) = focused {
                    self.report(self.close_pane(p, false, true));
                }
            }
            "focus-left" => self.focus_direction(Direction::Left),
            "focus-right" => self.focus_direction(Direction::Right),
            "focus-up" => self.focus_direction(Direction::Up),
            "focus-down" => self.focus_direction(Direction::Down),
            "copy" => {
                if let Some((t, _)) = term {
                    t.copy_clipboard_format(vte4::Format::Text);
                }
            }
            "paste" => {
                if let Some((t, _)) = term {
                    t.paste_clipboard();
                }
            }
            "search" => {
                if let Some((_, s)) = term {
                    s.set_search_mode(!s.is_search_mode());
                }
            }
            "command-palette" => dialogs::command_palette(self),
            "toggle-sidebar" => {
                // Toggle what is on screen (it may be auto-hidden in a narrow window).
                let shown = self.split.start_child().is_some_and(|s| s.get_visible());
                self.set_sidebar_visible(!shown);
            }
            "jump-unread" => {
                if self.jump_to_unread().is_none() {
                    self.toast("No unread notifications");
                }
            }
            "notifications" => self.notif_button.popup(),
            "zoom-in" | "zoom-out" | "zoom-reset" => {
                {
                    let mut i = self.inner.borrow_mut();
                    i.font_scale = match action {
                        "zoom-in" => (i.font_scale + 0.1).min(3.0),
                        "zoom-out" => (i.font_scale - 0.1).max(0.5),
                        _ => 1.0,
                    };
                }
                self.apply_terminal_style();
            }
            "settings" => dialogs::settings(self),
            "command-history" => dialogs::command_history(self),
            "show-shortcuts" => dialogs::shortcuts_help(self),
            "about" => dialogs::about(self),
            other => self.toast(&format!("Unknown action {other}")),
        }
    }

    // ---- metadata -----------------------------------------------------------

    /// Ask the worker for fresh metadata. `active_only` limits it to the visible workspace.
    pub fn request_meta(&self, active_only: bool) {
        let i = self.inner.borrow();
        let active = i.state.active;
        for (wid, pid) in i.state.all_panes() {
            if active_only && Some(wid) != active {
                continue;
            }
            if let Some(p) = i.panes.get(&pid) {
                if let Some(child) = p.pid.get() {
                    // Ports: every pass for the visible workspace; background ones on the slower full pass.
                    let _ = self.meta_tx.send(meta_worker::Job {
                        pane: pid,
                        shell_pid: child,
                        pty_fd: p.pty_fd(),
                        ports: Some(wid) == active || !active_only,
                        env_file: crate::shellint::env_file(pid),
                    });
                }
            }
        }
    }

    // ---- sessions -----------------------------------------------------------

    pub fn schedule_save(self: &Rc<Self>) {
        if self.save_pending.replace(true) {
            return;
        }
        let w = self.weak();
        glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || {
            if let Some(a) = w.upgrade() {
                a.save_pending.set(false);
                if !a.quitting.get() {
                    a.save_session(false);
                }
            }
        });
    }

    pub fn snapshot(&self) -> Snapshot {
        let i = self.inner.borrow();
        let panes = i
            .panes
            .iter()
            .map(|(id, p)| {
                let (agent, sess) = p.agent_session.clone().map(|(a, s)| (Some(a), Some(s))).unwrap_or((None, None));
                (
                    *id,
                    PaneSnapshot {
                        cwd: p.cwd.as_ref().map(|c| c.display().to_string()),
                        agent,
                        agent_session: sess,
                    },
                )
            })
            .collect();
        Snapshot {
            version: crate::session::VERSION,
            state: i.state.clone(),
            panes,
        }
    }

    pub fn save_session(&self, with_scrollback: bool) {
        let (restore, lines) = {
            let c = &self.inner.borrow().config;
            (c.session.restore, c.session.replay_lines)
        };
        if !restore {
            return;
        }
        if let Err(e) = self.snapshot().save(&crate::session::session_path()) {
            eprintln!("cmux-linux: saving session failed: {e}");
        }
        if lines == 0 {
            // Replay off: keep no terminal output on disk.
            crate::session::prune_scrollback(&[]);
        } else if with_scrollback {
            let i = self.inner.borrow();
            for (id, p) in &i.panes {
                if let Err(e) = crate::session::save_scrollback(*id, &p.scrollback_text(lines), lines) {
                    eprintln!("cmux-linux: saving scrollback failed: {e}");
                }
            }
        }
    }

    fn restore(self: &Rc<Self>, snap: Snapshot) {
        let lines = self.inner.borrow().config.session.replay_lines;
        let active = snap.state.active;
        let workspaces: Vec<_> = snap.state.workspaces.iter().map(|w| (w.id, w.layout.panes())).collect();
        self.inner.borrow_mut().state = snap.state;
        for (wid, panes) in workspaces {
            self.add_page(wid);
            for p in panes {
                let ps = snap.panes.get(&p).cloned().unwrap_or_default();
                let cwd = ps.cwd.map(PathBuf::from).filter(|d| d.is_dir());
                let text = if lines == 0 {
                    None
                } else {
                    crate::session::load_scrollback(p, lines)
                };
                self.create_pane(wid, p, cwd, text);
                if let (Some(a), Some(s)) = (ps.agent, ps.agent_session) {
                    if let Some(pane) = self.inner.borrow_mut().panes.get_mut(&p) {
                        pane.agent_session = Some((a, s));
                    }
                }
            }
            self.rebuild_workspace(wid);
        }
        if let Some(a) = active.or_else(|| self.inner.borrow().state.workspaces.first().map(|w| w.id)) {
            self.select_workspace(a);
        }
        let resumable = self.inner.borrow().panes.values().filter(|p| p.agent_session.is_some()).count();
        if resumable > 0 {
            self.toast(&format!(
                "{resumable} agent session(s) can be resumed from the pane menu or command palette"
            ));
        }
    }

    fn shutdown_panes(&self) {
        let panes: Vec<_> = self.inner.borrow_mut().panes.drain().collect();
        for (_, p) in panes {
            p.hangup();
        }
    }
}

fn set_class(w: &impl IsA<gtk4::Widget>, class: &str, on: bool) {
    if on {
        w.add_css_class(class);
    } else {
        w.remove_css_class(class);
    }
}

fn status_text(i: &Inner) -> String {
    let Some(ws) = i.state.active_workspace() else {
        return String::new();
    };
    let mut parts = vec![format!("Workspace: {}", ws.name)];
    if let Some(p) = i.panes.get(&ws.focused) {
        if let Some(c) = &p.cwd {
            parts.push(crate::meta::display_path(c));
        }
        if let Some(g) = &p.git {
            let mut b = format!("Branch: {}", g.branch);
            if g.ahead > 0 {
                b.push_str(&format!(" ↑{}", g.ahead));
            }
            if g.behind > 0 {
                b.push_str(&format!(" ↓{}", g.behind));
            }
            if g.changes > 0 {
                b.push_str(&format!(" ●{}", g.changes));
            }
            parts.push(b);
        }
        if !p.ports.is_empty() {
            parts.push(format!(
                "Ports: {}",
                p.ports.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ")
            ));
        }
        parts.push(format!("Agent: {}", p.agent_label().unwrap_or_else(|| "—".into())));
    }
    let n = ws.layout.panes().len();
    parts.push(format!("{n} pane{}", if n == 1 { "" } else { "s" }));
    parts.join("  |  ")
}
