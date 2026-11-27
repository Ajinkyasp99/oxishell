//! The project tree and file editor beside a workspace's terminals.

use super::links::{self, FileTarget};
use super::AppRc;
use crate::model::{PaneId, WorkspaceId};
use gtk4::prelude::*;
use gtk4::{gio, glib, Orientation};
use libadwaita::prelude::AdwDialogExt;
use std::cell::{Cell, RefCell};
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;
use std::time::Duration;

const MAX_PREVIEW_BYTES: u64 = 1024 * 1024;
const DISK_CHANGED: &str = "The file changed on disk. Your edits are still here. Reload from disk to use the latest version.";
const WATCH_DELAY: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Directory,
    LinkedDirectory,
    File,
}

enum Item {
    Entry { path: PathBuf, kind: Kind },
    Message(String),
}

fn list_dir(dir: &Path) -> io::Result<Vec<Item>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(dir)? {
        let entry = item?;
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        let path = entry.path();
        let kind = if file_type.is_dir() {
            Kind::Directory
        } else if file_type.is_symlink() && path.is_dir() {
            Kind::LinkedDirectory
        } else {
            Kind::File
        };
        entries.push((path, kind));
    }
    entries.sort_unstable_by(|(a, a_kind), (b, b_kind)| {
        (*a_kind == Kind::File)
            .cmp(&(*b_kind == Kind::File))
            .then_with(|| a.file_name().cmp(&b.file_name()))
    });
    Ok(entries.into_iter().map(|(path, kind)| Item::Entry { path, kind }).collect())
}

fn show_entries(store: &gio::ListStore, entries: Vec<Item>) {
    let entries: Vec<_> = entries.into_iter().map(glib::BoxedAnyObject::new).collect();
    store.splice(0, store.n_items(), &entries);
}

fn show_message(store: &gio::ListStore, message: impl Into<String>) {
    show_entries(store, vec![Item::Message(message.into())]);
}

fn show_directory(store: &gio::ListStore, entries: io::Result<Vec<Item>>) {
    match entries {
        Ok(entries) if entries.is_empty() => show_message(store, "Empty folder"),
        Ok(entries) => show_entries(store, entries),
        Err(err) => show_message(store, format!("Couldn't read folder: {err}")),
    }
}

fn child_model(item: &glib::Object) -> Option<gio::ListModel> {
    let item = item.downcast_ref::<glib::BoxedAnyObject>()?.borrow::<Item>();
    let Item::Entry {
        path,
        kind: Kind::Directory,
    } = &*item
    else {
        return None;
    };
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    show_message(&store, "Loading folder…");
    let dest = store.downgrade();
    let path = path.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            if !std::fs::symlink_metadata(&path)?.is_dir() {
                return Err(io::Error::other("This entry is no longer a folder"));
            }
            list_dir(&path)
        })
        .await;
        if let Some(store) = dest.upgrade() {
            show_directory(&store, result.unwrap_or_else(|_| Err(io::Error::other("Folder read interrupted"))));
        }
    });
    Some(store.upcast())
}

fn tree_factory() -> gtk4::SignalListItemFactory {
    let factory = gtk4::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk4::ListItem>().unwrap();
        let row = gtk4::Box::new(Orientation::Horizontal, 6);
        row.append(&gtk4::Image::new());
        let label = header_label();
        label.set_hexpand(true);
        row.append(&label);
        let expander = gtk4::TreeExpander::new();
        expander.set_child(Some(&row));
        item.set_child(Some(&expander));
    });
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk4::ListItem>().unwrap();
        let row = item.item().and_downcast::<gtk4::TreeListRow>().unwrap();
        let entry = row.item().and_downcast::<glib::BoxedAnyObject>().unwrap();
        let expander = item.child().and_downcast::<gtk4::TreeExpander>().unwrap();
        let children = expander.child().unwrap();
        let icon = children.first_child().and_downcast::<gtk4::Image>().unwrap();
        let label = children.last_child().and_downcast::<gtk4::Label>().unwrap();
        expander.set_list_row(Some(&row));
        let entry = entry.borrow::<Item>();
        match &*entry {
            Item::Entry { path, kind } => {
                label.set_label(&path.file_name().unwrap_or_default().to_string_lossy());
                label.remove_css_class("dim-label");
                let tooltip = if *kind == Kind::LinkedDirectory {
                    format!("{} (linked folder)", path.display())
                } else {
                    path.display().to_string()
                };
                label.set_tooltip_text(Some(&tooltip));
                icon.set_icon_name(Some(if *kind == Kind::File {
                    "text-x-generic-symbolic"
                } else {
                    "folder-symbolic"
                }));
                icon.set_visible(true);
                item.set_selectable(true);
                item.set_activatable(*kind != Kind::LinkedDirectory);
            }
            Item::Message(message) => {
                label.set_label(message);
                label.set_tooltip_text(Some(message));
                label.add_css_class("dim-label");
                icon.set_visible(false);
                item.set_selectable(false);
                item.set_activatable(false);
            }
        }
    });
    factory.connect_unbind(|_, item| {
        let item = item.downcast_ref::<gtk4::ListItem>().unwrap();
        if let Some(expander) = item.child().and_downcast::<gtk4::TreeExpander>() {
            expander.set_list_row(None);
        }
    });
    factory
}

#[derive(Debug)]
enum PreviewError {
    Io(io::Error),
    SpecialFile,
    TooLarge,
    Binary,
}

impl From<io::Error> for PreviewError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl PreviewError {
    fn message(&self) -> String {
        match self {
            Self::Io(err) if err.kind() == io::ErrorKind::NotFound => "This file no longer exists.".into(),
            Self::Io(err) => format!("Couldn't read file: {err}"),
            Self::SpecialFile => "Only regular files can be previewed.".into(),
            Self::TooLarge => "This file exceeds the 1 MiB preview limit.".into(),
            Self::Binary => "This file is binary or uses an encoding other than UTF-8.".into(),
        }
    }

    fn can_open(&self) -> bool {
        !matches!(self, Self::Io(err) if err.kind() == io::ErrorKind::NotFound) && !matches!(self, Self::SpecialFile)
    }
}

fn read_text(path: &Path) -> Result<String, PreviewError> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(PreviewError::SpecialFile);
    }
    // Recheck the opened file. A path replaced with a FIFO between the stat
    // and open must not block the worker.
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(PreviewError::SpecialFile);
    }
    if meta.len() > MAX_PREVIEW_BYTES {
        return Err(PreviewError::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_PREVIEW_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PREVIEW_BYTES {
        return Err(PreviewError::TooLarge);
    }
    if bytes.contains(&0) {
        return Err(PreviewError::Binary);
    }
    String::from_utf8(bytes).map_err(|_| PreviewError::Binary)
}

#[derive(Clone, PartialEq, Eq)]
struct LoadedText {
    path: PathBuf,
    text: String,
}

fn load_text(path: &Path) -> Result<LoadedText, PreviewError> {
    let path = path.canonicalize()?;
    let text = read_text(&path)?;
    Ok(LoadedText { path, text })
}

fn save_text(path: &Path, original: &LoadedText, text: &str) -> Result<(), String> {
    static SAVE_LOCK: Mutex<()> = Mutex::new(());
    if text.len() as u64 > MAX_PREVIEW_BYTES {
        return Err("This file exceeds the 1 MiB editor limit.".into());
    }
    // Both workspaces can be editing the same file. Serialize the comparison
    // and replacement so two saves in this app cannot pass the same check.
    let _guard = SAVE_LOCK.lock().map_err(|e| e.to_string())?;
    if path.canonicalize().as_ref().ok() != Some(&original.path) {
        return Err(DISK_CHANGED.into());
    }
    let file = gio::File::for_path(&original.path);
    let info = file
        .query_info(
            "standard::type,etag::value,access::can-write",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
        )
        .map_err(|e| e.to_string())?;
    if info.file_type() != gio::FileType::Regular {
        return Err(DISK_CHANGED.into());
    }
    if !info.boolean("access::can-write") {
        return Err("This file is read-only.".into());
    }
    let etag = info.etag().ok_or("Couldn't verify the current version of this file.")?;
    if !matches!(read_text(&original.path), Ok(current) if current == original.text) {
        return Err(DISK_CHANGED.into());
    }
    file.replace_contents(
        text.as_bytes(),
        Some(etag.as_str()),
        false,
        gio::FileCreateFlags::NONE,
        gio::Cancellable::NONE,
    )
    .map(|_| ())
    .map_err(|e| {
        if e.matches(gio::IOErrorEnum::WrongEtag) {
            DISK_CHANGED.into()
        } else {
            e.to_string()
        }
    })
}

fn header_label() -> gtk4::Label {
    gtk4::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk4::pango::EllipsizeMode::Middle)
        .build()
}

fn header_button(icon_name: &str, tooltip: &str, name: &str) -> gtk4::Button {
    let button = gtk4::Button::builder().icon_name(icon_name).tooltip_text(tooltip).build();
    button.set_widget_name(name);
    button.add_css_class("flat");
    button.update_property(&[gtk4::accessible::Property::Label(tooltip)]);
    button
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FileMode {
    Preview,
    Edit,
}

type AfterSave = Box<dyn FnOnce(&Rc<Panel>)>;

pub(super) struct Panel {
    pub split: gtk4::Paned,
    pub terminal_area: gtk4::Overlay,
    views: gtk4::Stack,
    panel: gtk4::Box,
    title: gtk4::Label,
    path_label: gtk4::Label,
    back: gtk4::Button,
    refresh: gtk4::Button,
    editor: gtk4::Button,
    save: gtk4::Button,
    reload: gtk4::Button,
    default_app: gtk4::Button,
    directory: gio::ListStore,
    tree: gtk4::TreeListModel,
    list: gtk4::ListView,
    content: gtk4::Stack,
    text: gtk4::TextView,
    status: gtk4::Label,
    edit_status: gtk4::Label,
    cwd: RefCell<Option<PathBuf>>,
    root: RefCell<Option<PathBuf>>,
    target: RefCell<Option<(PaneId, FileTarget)>>,
    loaded: RefCell<Option<LoadedText>>,
    saving: Cell<bool>,
    save_error: RefCell<Option<String>>,
    autosave_delay: Cell<Option<Duration>>,
    autosave_timer: RefCell<Option<glib::SourceId>>,
    autosave_due: Cell<bool>,
    after_save: RefCell<Option<AfterSave>>,
    reloading: Cell<bool>,
    file_monitor: RefCell<Option<gio::FileMonitor>>,
    disk_timer: RefCell<Option<glib::SourceId>>,
    disk_generation: Cell<u64>,
    root_request: Cell<u64>,
    file_request: Cell<u64>,
    positioned: Cell<bool>,
}

impl Panel {
    pub fn new(app: &AppRc, workspace: WorkspaceId, page: &gtk4::Box) -> Rc<Self> {
        let title = header_label();
        title.add_css_class("heading");
        title.set_hexpand(true);
        let path_label = header_label();
        path_label.add_css_class("dim-label");
        path_label.add_css_class("monospace");
        path_label.add_css_class("project-file-path");
        let back = header_button("go-previous-symbolic", "Back to Project", "project-back");
        let refresh = header_button("view-refresh-symbolic", "Refresh Project", "project-refresh");
        let editor = header_button("document-edit-symbolic", "Edit File", "project-editor");
        editor.set_label("Edit");
        let save = header_button(
            "document-save-symbolic",
            if cfg!(target_os = "macos") {
                "Save File (⌘S)"
            } else {
                "Save File (Ctrl+S)"
            },
            "project-save",
        );
        save.set_label("Save");
        let reload = header_button("document-revert-symbolic", "Reload from Disk", "project-reload");
        let default_app = header_button("document-open-symbolic", "Open with Default App", "project-default-app");
        let close = header_button("window-close-symbolic", "Close File Panel", "project-close");
        let header = gtk4::Box::new(Orientation::Horizontal, 2);
        for child in [&back, &refresh] {
            header.append(child);
        }
        header.append(&title);
        for child in [&editor, &save, &reload, &default_app, &close] {
            header.append(child);
        }

        let directory = gio::ListStore::new::<glib::BoxedAnyObject>();
        let tree = gtk4::TreeListModel::new(directory.clone(), false, false, child_model);
        let selection = gtk4::SingleSelection::new(Some(tree.clone()));
        selection.set_autoselect(false);
        let list = gtk4::ListView::new(Some(selection), Some(tree_factory()));
        list.set_widget_name("project-tree");
        list.set_single_click_activate(true);
        list.update_property(&[gtk4::accessible::Property::Label("Project files and folders")]);
        let tree_scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .child(&list)
            .build();

        let text = gtk4::TextView::builder()
            .editable(false)
            .monospace(true)
            .left_margin(10)
            .right_margin(10)
            .top_margin(8)
            .bottom_margin(8)
            .build();
        text.set_widget_name("file-preview-text");
        text.update_property(&[gtk4::accessible::Property::Label("File contents")]);
        text.buffer().set_enable_undo(true);
        let text_scroll = gtk4::ScrolledWindow::builder().child(&text).build();
        let status = gtk4::Label::builder().wrap(true).margin_start(10).margin_end(10).build();
        status.add_css_class("dim-label");
        status.set_widget_name("file-preview-status");
        let content = gtk4::Stack::new();
        content.set_vhomogeneous(false);
        content.set_hhomogeneous(false);
        content.add_named(&status, Some("status"));
        content.add_named(&text_scroll, Some("text"));
        let edit_status = gtk4::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .margin_start(10)
            .margin_end(10)
            .margin_bottom(6)
            .build();
        edit_status.add_css_class("dim-label");
        edit_status.set_widget_name("project-edit-status");
        edit_status.set_visible(false);

        let views = gtk4::Stack::new();
        views.set_vexpand(true);
        views.set_hhomogeneous(false);
        views.set_vhomogeneous(false);
        views.add_named(&tree_scroll, Some("explorer"));
        views.add_named(&content, Some("preview"));
        let panel = gtk4::Box::new(Orientation::Vertical, 0);
        panel.add_css_class("project-files");
        panel.set_size_request(156, -1);
        panel.set_visible(false);
        panel.append(&header);
        panel.append(&path_label);
        panel.append(&edit_status);
        panel.append(&views);

        let terminal_area = gtk4::Overlay::new();
        terminal_area.set_child(Some(page));
        let split = gtk4::Paned::new(Orientation::Horizontal);
        split.set_start_child(Some(&terminal_area));
        split.set_end_child(Some(&panel));
        split.set_resize_start_child(true);
        split.set_resize_end_child(true);

        let this = Rc::new(Self {
            split,
            terminal_area,
            views,
            panel,
            title,
            path_label,
            back,
            refresh,
            editor,
            save,
            reload,
            default_app,
            directory,
            tree,
            list,
            content,
            text,
            status,
            edit_status,
            cwd: RefCell::new(None),
            root: RefCell::new(None),
            target: RefCell::new(None),
            loaded: RefCell::new(None),
            saving: Cell::new(false),
            save_error: RefCell::new(None),
            autosave_delay: Cell::new(app.inner.borrow().config.editor.autosave_delay()),
            autosave_timer: RefCell::new(None),
            autosave_due: Cell::new(false),
            after_save: RefCell::new(None),
            reloading: Cell::new(false),
            file_monitor: RefCell::new(None),
            disk_timer: RefCell::new(None),
            disk_generation: Cell::new(0),
            root_request: Cell::new(0),
            file_request: Cell::new(0),
            positioned: Cell::new(false),
        });
        this.connect(app, workspace, &close);
        this
    }

    fn connect(self: &Rc<Self>, app: &AppRc, workspace: WorkspaceId, close: &gtk4::Button) {
        let weak = Rc::downgrade(self);
        let app_weak = app.weak();
        self.list.connect_activate(move |_, position| {
            let (Some(panel), Some(app)) = (weak.upgrade(), app_weak.upgrade()) else {
                return;
            };
            let Some(row) = panel.tree.row(position) else { return };
            let Some(item) = row.item().and_downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let item = item.borrow::<Item>();
            match &*item {
                Item::Entry { kind: Kind::Directory, .. } => row.set_expanded(!row.is_expanded()),
                Item::Entry { path, kind: Kind::File } => {
                    if let Some(pane) = panel_pane(&app, workspace) {
                        let target = FileTarget {
                            path: path.clone(),
                            location: None,
                        };
                        panel.leave(&app, move |panel| panel.open_file(pane, target, FileMode::Edit));
                    }
                }
                _ => {}
            }
        });
        for (button, reload) in [(&self.back, false), (&self.refresh, true)] {
            let weak = Rc::downgrade(self);
            let app_weak = app.weak();
            button.connect_clicked(move |_| {
                let (Some(panel), Some(app)) = (weak.upgrade(), app_weak.upgrade()) else {
                    return;
                };
                let cwd = if reload { None } else { panel.cwd.borrow().clone() };
                let cwd = cwd.or_else(|| pane_cwd(&app, workspace));
                panel.leave(&app, move |panel| panel.open_explorer(cwd, reload));
            });
        }
        {
            let weak = Rc::downgrade(self);
            let app_weak = app.weak();
            self.default_app.connect_clicked(move |_| {
                let (Some(panel), Some(app)) = (weak.upgrade(), app_weak.upgrade()) else {
                    return;
                };
                let Some((source, target)) = panel.target.borrow().clone() else {
                    return;
                };
                let i = app.inner.borrow();
                let pane = if i.state.workspace_of(source) == Some(workspace) {
                    Some(source)
                } else {
                    i.state.workspace(workspace).map(|w| w.focused())
                };
                let term = pane.and_then(|pane| i.panes.get(&pane).map(|p| p.term.clone()));
                drop(i);
                if let Some(term) = term {
                    links::open(&app.weak(), &term, &gio::File::for_path(&target.path).uri());
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            self.editor.connect_clicked(move |_| {
                if let Some(panel) = weak.upgrade().filter(|p| p.loaded.borrow().is_some()) {
                    panel.text.set_editable(true);
                    panel.sync_edit_actions();
                    panel.text.grab_focus();
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            self.save.connect_clicked(move |_| {
                if let Some(panel) = weak.upgrade() {
                    panel.save_file();
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            let app_weak = app.weak();
            self.reload.connect_clicked(move |_| {
                if let (Some(panel), Some(app)) = (weak.upgrade(), app_weak.upgrade()) {
                    panel.reload_file(&app);
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            self.text.buffer().connect_modified_changed(move |_| {
                if let Some(panel) = weak.upgrade() {
                    panel.sync_edit_actions();
                    panel.schedule_autosave();
                }
            });
            let weak = Rc::downgrade(self);
            self.text.buffer().connect_changed(move |_| {
                if let Some(panel) = weak.upgrade() {
                    panel.schedule_autosave();
                }
            });
            let focus = gtk4::EventControllerFocus::new();
            let weak = Rc::downgrade(self);
            focus.connect_leave(move |_| {
                if let Some(panel) = weak.upgrade() {
                    panel.flush_autosave();
                }
            });
            self.panel.add_controller(focus);
        }
        let hide = {
            let weak = Rc::downgrade(self);
            let app_weak = app.weak();
            move || {
                let (Some(panel), Some(app)) = (weak.upgrade(), app_weak.upgrade()) else {
                    return;
                };
                let app_weak = app.weak();
                panel.leave(&app, move |panel| {
                    panel.cancel_preview();
                    stop_scrolling(&panel.list);
                    panel.panel.set_visible(false);
                    if let Some(app) = app_weak.upgrade() {
                        if let Some(pane) = panel_pane(&app, workspace) {
                            app.focus_pane(pane);
                        }
                    }
                });
            }
        };
        let on_close = hide.clone();
        close.connect_clicked(move |_| on_close());
        let keys = gtk4::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk4::gdk::Key::Escape {
                hide();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.panel.add_controller(keys);
    }

    fn show(&self, view: &str) {
        let preview = view == "preview";
        if preview {
            stop_scrolling(&self.list);
        }
        self.views.set_visible_child_name(view);
        self.back.set_visible(preview);
        self.refresh.set_visible(!preview);
        self.sync_edit_actions();
        self.panel.set_visible(true);
        let width = self.split.width();
        if width > 0 && !self.positioned.replace(true) {
            self.split.set_position(width - (width * 2 / 5).clamp(156, 520));
        }
    }

    fn set_path(&self, path: &Path) {
        self.path_label.set_label(&crate::meta::display_path(path));
        self.path_label.set_tooltip_text(Some(&path.to_string_lossy()));
    }

    fn cancel_preview(&self) {
        self.cancel_autosave();
        self.autosave_due.set(false);
        self.stop_file_watch();
        self.file_request.set(self.file_request.get().wrapping_add(1));
        self.target.borrow_mut().take();
        self.loaded.borrow_mut().take();
        self.save_error.borrow_mut().take();
        self.text.set_editable(false);
        stop_scrolling(&self.text);
        self.set_text("");
    }

    fn set_text(&self, text: &str) {
        let buffer = self.text.buffer();
        buffer.begin_irreversible_action();
        buffer.set_text(text);
        buffer.end_irreversible_action();
        buffer.set_modified(false);
    }

    pub(super) fn pending_edits(&self) -> Option<&'static str> {
        if self.saving.get() {
            Some("Wait for the file to finish saving.")
        } else if self.text.buffer().is_modified() {
            Some("Save your edits or reload from disk before leaving this file.")
        } else {
            None
        }
    }

    fn leave(self: &Rc<Self>, app: &AppRc, navigate: impl FnOnce(&Rc<Self>) + 'static) {
        if let Some(message) = self.pending_edits() {
            if !self.after_autosave(navigate) {
                app.toast(message);
            }
        } else {
            navigate(self);
        }
    }

    pub(super) fn after_autosave(self: &Rc<Self>, navigate: impl FnOnce(&Rc<Self>) + 'static) -> bool {
        if !self.can_autosave() || self.after_save.borrow().is_some() {
            return false;
        }
        self.after_save.replace(Some(Box::new(navigate)));
        self.flush_autosave();
        true
    }

    pub(super) fn set_autosave(self: &Rc<Self>, delay: Option<Duration>) {
        if self.autosave_delay.replace(delay) != delay {
            if delay.is_none() {
                self.autosave_due.set(false);
                self.after_save.borrow_mut().take();
            }
            self.schedule_autosave();
            self.sync_edit_actions();
        }
    }

    fn can_autosave(&self) -> bool {
        self.autosave_delay.get().is_some()
            && self.text.is_editable()
            && self.text.buffer().is_modified()
            && !self.reloading.get()
            && self.save_error.borrow().is_none()
    }

    fn cancel_autosave(&self) {
        if let Some(timer) = self.autosave_timer.borrow_mut().take() {
            timer.remove();
        }
    }

    fn schedule_autosave(self: &Rc<Self>) {
        self.cancel_autosave();
        let Some(delay) = self.autosave_delay.get().filter(|_| self.can_autosave()) else {
            return;
        };
        let weak = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(delay, move || {
            if let Some(panel) = weak.upgrade() {
                panel.autosave_timer.borrow_mut().take();
                panel.flush_autosave();
            }
        });
        self.autosave_timer.replace(Some(timer));
    }

    pub(super) fn flush_autosave(self: &Rc<Self>) {
        if self.can_autosave() {
            self.cancel_autosave();
            if self.saving.get() {
                self.autosave_due.set(true);
            } else {
                self.save_file();
            }
        }
    }

    fn sync_edit_actions(&self) {
        let preview = self.views.visible_child_name().as_deref() == Some("preview");
        let editing = preview && self.text.is_editable();
        let modified = self.text.buffer().is_modified();
        self.editor.set_visible(preview && !editing);
        self.editor.set_sensitive(self.loaded.borrow().is_some());
        self.default_app.set_visible(preview && !editing);
        self.save.set_visible(editing);
        self.save.set_sensitive(modified && !self.saving.get());
        self.reload.set_visible(editing);
        self.reload.set_sensitive(!self.saving.get());
        let error = self.save_error.borrow();
        let autosave_error = error
            .as_ref()
            .filter(|_| self.autosave_delay.get().is_some())
            .map(|message| format!("Autosave paused. {message}"));
        let status = if self.saving.get() {
            "Saving…"
        } else if let Some(message) = autosave_error.as_deref() {
            message
        } else if let Some(message) = error.as_deref() {
            message
        } else if modified {
            "Unsaved changes"
        } else {
            ""
        };
        self.edit_status.set_label(status);
        self.edit_status.set_visible(editing && !status.is_empty());
    }

    fn save_file(self: &Rc<Self>) {
        let buffer = self.text.buffer();
        if !self.text.is_editable() || !buffer.is_modified() || self.saving.get() {
            return;
        }
        let (Some((_, target)), Some(original)) = (self.target.borrow().clone(), self.loaded.borrow().clone()) else {
            return;
        };
        let draft = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true).to_string();
        self.cancel_autosave();
        self.autosave_due.set(false);
        self.invalidate_disk_check();
        self.saving.set(true);
        self.save_error.borrow_mut().take();
        self.sync_edit_actions();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                save_text(&target.path, &original, &draft)?;
                Ok::<_, String>(LoadedText {
                    path: original.path,
                    text: draft,
                })
            })
            .await
            .unwrap_or_else(|_| Err("File save was interrupted.".into()));
            let Some(panel) = weak.upgrade() else { return };
            panel.saving.set(false);
            match result {
                Ok(document) => {
                    let current = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
                    let modified = current.as_str() != document.text;
                    panel.loaded.replace(Some(document));
                    buffer.set_modified(modified);
                    panel.queue_disk_check();
                    if !modified {
                        let navigate = panel.after_save.borrow_mut().take();
                        if let Some(navigate) = navigate {
                            navigate(&panel);
                        }
                    } else if panel.autosave_due.replace(false) || panel.after_save.borrow().is_some() {
                        panel.flush_autosave();
                    } else if panel.autosave_timer.borrow().is_none() {
                        panel.schedule_autosave();
                    }
                }
                Err(message) => {
                    panel.cancel_autosave();
                    panel.after_save.borrow_mut().take();
                    panel.save_error.replace(Some(message));
                }
            }
            panel.sync_edit_actions();
        });
    }

    fn reload_file(self: &Rc<Self>, app: &AppRc) {
        let Some((pane, target)) = self.target.borrow().clone().filter(|_| !self.saving.get()) else {
            return;
        };
        if !self.text.buffer().is_modified() {
            self.open_file(pane, target, FileMode::Edit);
            return;
        }
        let weak = Rc::downgrade(self);
        let request = self.file_request.get();
        self.reloading.set(true);
        self.cancel_autosave();
        let dialog = super::dialogs::confirm(
            app,
            "Discard unsaved edits?",
            "Reloading will discard the edits in this panel.",
            "Discard and Reload",
            move || {
                if let Some(panel) = weak.upgrade().filter(|p| p.file_request.get() == request && !p.saving.get()) {
                    panel.open_file(pane, target.clone(), FileMode::Edit);
                }
            },
        );
        let weak = Rc::downgrade(self);
        dialog.connect_closed(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.reloading.set(false);
                panel.schedule_autosave();
            }
        });
    }

    fn watch_file(self: &Rc<Self>, path: &Path) {
        let Ok(monitor) = gio::File::for_path(path).monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) else {
            return;
        };
        let weak = Rc::downgrade(self);
        monitor.connect_changed(move |_, _, _, _| {
            if let Some(panel) = weak.upgrade() {
                panel.queue_disk_check();
            }
        });
        self.file_monitor.replace(Some(monitor));
        // Catch an external write between the initial read and watch registration.
        self.queue_disk_check();
    }

    fn invalidate_disk_check(&self) {
        self.disk_generation.set(self.disk_generation.get().wrapping_add(1));
        if let Some(timer) = self.disk_timer.borrow_mut().take() {
            timer.remove();
        }
    }

    fn stop_file_watch(&self) {
        if let Some(monitor) = self.file_monitor.borrow_mut().take() {
            monitor.cancel();
        }
        self.invalidate_disk_check();
    }

    fn queue_disk_check(self: &Rc<Self>) {
        self.invalidate_disk_check();
        if self.saving.get() {
            return;
        }
        let Some((_, target)) = self.target.borrow().clone() else { return };
        let generation = self.disk_generation.get();
        let weak = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(WATCH_DELAY, move || {
            let Some(panel) = weak.upgrade() else { return };
            panel.disk_timer.borrow_mut().take();
            glib::spawn_future_local(async move {
                let result = gio::spawn_blocking(move || load_text(&target.path)).await;
                if let (Some(panel), Ok(result)) = (weak.upgrade(), result) {
                    if panel.disk_generation.get() == generation {
                        panel.apply_disk_change(result);
                    }
                }
            });
        });
        self.disk_timer.replace(Some(timer));
    }

    fn apply_disk_change(&self, result: Result<LoadedText, PreviewError>) {
        if result
            .as_ref()
            .is_ok_and(|document| self.loaded.borrow().as_ref() == Some(document))
        {
            return;
        }
        let buffer = self.text.buffer();
        if buffer.is_modified() {
            self.cancel_autosave();
            self.save_error.replace(Some(DISK_CHANGED.into()));
        } else {
            self.save_error.borrow_mut().take();
            self.default_app
                .set_sensitive(result.as_ref().err().is_none_or(PreviewError::can_open));
            match result {
                Ok(document) => {
                    let offset = buffer.iter_at_mark(&buffer.get_insert()).offset();
                    self.set_text(&document.text);
                    buffer.place_cursor(&buffer.iter_at_offset(offset.min(buffer.char_count())));
                    self.text.scroll_mark_onscreen(&buffer.get_insert());
                    self.loaded.replace(Some(document));
                    self.content.set_visible_child_name("text");
                }
                Err(error) => {
                    self.loaded.borrow_mut().take();
                    self.text.set_editable(false);
                    self.set_text("");
                    self.status.set_label(&error.message());
                    self.content.set_visible_child_name("status");
                }
            }
        }
        self.sync_edit_actions();
    }

    fn open_explorer(self: &Rc<Self>, cwd: Option<PathBuf>, refresh: bool) {
        self.cancel_preview();
        self.title.set_label("Project Explorer");
        self.title.set_tooltip_text(None);
        self.show("explorer");
        if !refresh && self.list.is_sensitive() && self.cwd.borrow().as_ref() == cwd.as_ref() {
            if let Some(root) = self.root.borrow().as_ref() {
                self.set_path(root);
                self.list.grab_focus();
                return;
            }
        }
        let request = self.root_request.get().wrapping_add(1);
        self.root_request.set(request);
        self.cwd.replace(cwd.clone());
        let Some(cwd) = cwd else {
            self.root.borrow_mut().take();
            self.path_label.set_label("");
            self.path_label.set_tooltip_text(None);
            show_message(&self.directory, "No working directory for this terminal");
            self.list.set_sensitive(true);
            return;
        };
        self.set_path(&cwd);
        self.list.set_sensitive(false);
        self.refresh.grab_focus();
        if self.root.borrow().is_none() {
            show_message(&self.directory, "Loading project…");
        }
        let existing = self.root.borrow().clone().filter(|_| !refresh);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                let root = crate::compose::repo_root(&cwd);
                let entries = (existing.as_ref() != Some(&root)).then(|| list_dir(&root));
                (root, entries)
            })
            .await;
            let Some(panel) = weak.upgrade().filter(|p| p.root_request.get() == request) else {
                return;
            };
            panel.list.set_sensitive(true);
            if panel.refresh.has_focus() {
                panel.list.grab_focus();
            }
            match result {
                Ok((root, entries)) => {
                    panel.root.replace(Some(root.clone()));
                    if panel.views.visible_child_name().as_deref() == Some("explorer") {
                        panel.set_path(&root);
                    }
                    if let Some(entries) = entries {
                        show_directory(&panel.directory, entries);
                    }
                }
                Err(_) => {
                    panel.root.borrow_mut().take();
                    show_message(&panel.directory, "Couldn't load project");
                }
            }
        });
    }

    fn open_file(self: &Rc<Self>, pane: PaneId, target: FileTarget, mode: FileMode) {
        self.cancel_preview();
        let path = target.path.clone();
        let location = target.location;
        self.target.replace(Some((pane, target)));
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let title = match location {
            Some((line, Some(col))) => format!("{name}:{line}:{col}"),
            Some((line, None)) => format!("{name}:{line}"),
            None => name.into_owned(),
        };
        self.title.set_label(&title);
        self.title.set_tooltip_text(Some(&title));
        self.set_path(&path);
        self.default_app.set_sensitive(false);
        self.status.set_label("Loading file…");
        self.content.set_visible_child_name("status");
        self.show("preview");
        if mode == FileMode::Edit {
            self.back.grab_focus();
        }
        let request = self.file_request.get();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let read_path = path.clone();
            let result = gio::spawn_blocking(move || load_text(&read_path))
                .await
                .unwrap_or_else(|_| Err(io::Error::other("File read interrupted").into()));
            let Some(panel) = weak.upgrade().filter(|p| p.file_request.get() == request) else {
                return;
            };
            let can_open = result.as_ref().err().is_none_or(PreviewError::can_open);
            panel.default_app.set_sensitive(can_open);
            match result {
                Ok(document) => {
                    panel.content.set_visible_child_name("text");
                    panel.show_text(&document.text, location);
                    panel.loaded.replace(Some(document));
                    panel.watch_file(&path);
                    if mode == FileMode::Edit {
                        panel.text.set_editable(true);
                        if panel.back.has_focus() {
                            panel.text.grab_focus();
                        }
                    }
                }
                Err(err) => panel.status.set_label(&err.message()),
            }
            panel.sync_edit_actions();
        });
    }

    fn show_text(&self, text: &str, location: Option<(u32, Option<u32>)>) {
        let buffer = self.text.buffer();
        self.set_text(text);
        let (line, column) = location.unwrap_or((1, None));
        let line = (line - 1).min((buffer.line_count() - 1) as u32) as i32;
        let start = buffer.iter_at_line(line).unwrap_or_else(|| buffer.start_iter());
        let mut end = start;
        if !end.ends_line() {
            end.forward_to_line_end();
        }
        let column = (column.unwrap_or(1) - 1).min(end.line_offset() as u32) as i32;
        let position = buffer.iter_at_line_offset(line, column).unwrap_or(start);
        if location.is_some() {
            let mut line_end = start;
            line_end.forward_line();
            let table = buffer.tag_table();
            let highlight = table.lookup("source-line").unwrap_or_else(|| {
                let tag = gtk4::TextTag::new(Some("source-line"));
                table.add(&tag);
                tag
            });
            let mut color = self.text.color();
            color.set_alpha(0.12);
            highlight.set_paragraph_background_rgba(Some(&color));
            buffer.apply_tag(&highlight, &start, &line_end);
        }
        buffer.place_cursor(&position);
        self.text.scroll_to_mark(&buffer.get_insert(), 0.05, true, 0.0, 0.36);
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        self.cancel_autosave();
        self.stop_file_watch();
    }
}

/// GTK 4.14 can disconnect the scroll animation's frame-clock handler twice
/// during unmap. Stop at the current position before hiding the view.
fn stop_scrolling(view: &impl IsA<gtk4::Scrollable>) {
    for adjustment in [view.hadjustment(), view.vadjustment()].into_iter().flatten() {
        adjustment.set_value(adjustment.value());
    }
}

fn panel_pane(app: &AppRc, workspace: WorkspaceId) -> Option<PaneId> {
    app.inner.borrow().state.workspace(workspace).map(|w| w.focused())
}

fn pane_cwd(app: &AppRc, workspace: WorkspaceId) -> Option<PathBuf> {
    let i = app.inner.borrow();
    let pane = i.state.workspace(workspace)?.focused();
    i.panes.get(&pane)?.cwd.clone()
}

pub fn explore(app: &AppRc) {
    let workspace = app.inner.borrow().state.active;
    if let Some(workspace) = workspace {
        if let Some(panel) = app.file_panels.borrow().get(&workspace).cloned() {
            let cwd = pane_cwd(app, workspace);
            panel.leave(app, move |panel| panel.open_explorer(cwd, false));
        }
    }
}

pub(super) fn open_file(app: &AppRc, pane: PaneId, target: FileTarget, mode: FileMode) {
    let workspace = app.inner.borrow().state.workspace_of(pane);
    if let Some(workspace) = workspace {
        if let Some(panel) = app.file_panels.borrow().get(&workspace).cloned() {
            panel.leave(app, move |panel| panel.open_file(pane, target, mode));
        }
    }
}

pub fn has_focus(app: &AppRc) -> bool {
    let Some(focus) = gtk4::prelude::GtkWindowExt::focus(&app.window) else {
        return false;
    };
    let workspace = app.inner.borrow().state.active;
    let panels = app.file_panels.borrow();
    workspace
        .and_then(|id| panels.get(&id))
        .is_some_and(|p| focus.is_ancestor(&p.panel))
}

fn focused_text(app: &AppRc) -> Option<gtk4::TextView> {
    gtk4::prelude::GtkWindowExt::focus(&app.window)
        .and_downcast::<gtk4::TextView>()
        .filter(|text| text.widget_name() == "file-preview-text")
}

pub fn copy_text(app: &AppRc) -> bool {
    let Some(text) = focused_text(app) else { return false };
    text.buffer().copy_clipboard(&text.clipboard());
    true
}

pub fn paste_text(app: &AppRc) {
    if let Some(text) = focused_text(app).filter(|text| text.is_editable()) {
        text.buffer().paste_clipboard(&text.clipboard(), None, true);
    }
}

pub fn save_focused(app: &AppRc) -> bool {
    let workspace = app.inner.borrow().state.active;
    let panel = workspace.and_then(|id| app.file_panels.borrow().get(&id).cloned());
    if let Some(panel) = panel.filter(|p| p.text.is_editable() && has_focus(app)) {
        panel.save_file();
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn save_preserves_permissions_and_serializes_competing_edits() {
        let dir = std::env::temp_dir().join(format!("oxishell-save-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("script");
        std::fs::write(&path, "initial\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
        let original = load_text(&path).unwrap();
        let writes = std::thread::scope(|scope| {
            let a = scope.spawn(|| save_text(&path, &original, "first edit\n"));
            let b = scope.spawn(|| save_text(&path, &original, "second edit\n"));
            [a.join().unwrap(), b.join().unwrap()]
        });
        assert_eq!(writes.iter().filter(|result| result.is_ok()).count(), 1, "{writes:?}");
        let expected = if writes[0].is_ok() { "first edit\n" } else { "second edit\n" };
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o750);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn save_refuses_a_removed_file_or_retargeted_symlink() {
        let dir = std::env::temp_dir().join(format!("oxishell-save-links-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("file");
        let other = dir.join("other");
        let link = dir.join("link");
        std::fs::write(&path, "initial").unwrap();
        std::fs::write(&other, "initial").unwrap();
        symlink(&path, &link).unwrap();
        let original = load_text(&link).unwrap();
        std::fs::remove_file(&link).unwrap();
        symlink(&other, &link).unwrap();
        assert!(save_text(&link, &original, "draft").is_err());
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "initial");
        std::fs::remove_file(&path).unwrap();
        assert!(save_text(&path, &original, "draft").is_err());
        assert!(!path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
