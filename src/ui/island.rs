//! Agent Island: the capsule in the middle of the header bar.
//!
//! At rest it shows the active workspace name and a status dot for the agent
//! running there. When something finishes (an agent turn, a long command) it
//! stretches open for a few seconds to show what happened, then shrinks back.
//! Clicking it opens the notifications popover, so it also replaces the bell.

use crate::notify::Kind;
use gtk4::prelude::*;
use gtk4::{glib, Orientation};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

/// Agent activity in the active workspace, strongest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    /// An agent is blocked on the user.
    Waiting,
    /// An agent is working on a prompt.
    Working,
    /// An agent is open but not working (finished its turn, or detected
    /// by process name only).
    Present,
    /// No agent.
    None,
}

const DOT_CLASSES: [&str; 3] = ["working", "waiting", "present"];
const FLASH_CLASSES: [&str; 4] = ["flash-done", "flash-input", "flash-failed", "flash-info"];

pub struct Island {
    /// The header's title widget; owns the notifications popover.
    pub button: gtk4::MenuButton,
    /// Unread notification count.
    pub badge: gtk4::Label,
    dot: gtk4::Box,
    name: gtk4::Label,
    revealer: gtk4::Revealer,
    detail_box: gtk4::Box,
    detail_glyph: gtk4::Label,
    detail: gtk4::Label,
    collapse: Rc<RefCell<Option<glib::SourceId>>>,
}

impl Island {
    pub fn new() -> Self {
        let dot = gtk4::Box::new(Orientation::Horizontal, 0);
        dot.add_css_class("island-dot");
        dot.set_valign(gtk4::Align::Center);
        dot.set_visible(false);

        let name = gtk4::Label::builder()
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .max_width_chars(28)
            .build();
        name.add_css_class("island-name");

        let detail_glyph = gtk4::Label::new(None);
        detail_glyph.add_css_class("island-glyph");
        let detail = gtk4::Label::builder()
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .max_width_chars(46)
            .build();
        detail.add_css_class("island-detail");
        let sep = gtk4::Separator::new(Orientation::Vertical);
        sep.add_css_class("island-sep");
        let detail_box = gtk4::Box::new(Orientation::Horizontal, 6);
        detail_box.append(&sep);
        detail_box.append(&detail_glyph);
        detail_box.append(&detail);
        let revealer = gtk4::Revealer::builder()
            .transition_type(gtk4::RevealerTransitionType::SlideRight)
            .transition_duration(260)
            .child(&detail_box)
            .build();

        let badge = gtk4::Label::new(None);
        badge.add_css_class("island-badge");
        badge.set_valign(gtk4::Align::Center);
        badge.set_visible(false);

        let content = gtk4::Box::new(Orientation::Horizontal, 7);
        content.append(&dot);
        content.append(&name);
        content.append(&revealer);
        content.append(&badge);

        let button = gtk4::MenuButton::builder()
            .child(&content)
            .tooltip_text("Notifications (Ctrl+Shift+I)")
            .valign(gtk4::Align::Center)
            .build();
        button.add_css_class("island");

        Island {
            button,
            badge,
            dot,
            name,
            revealer,
            detail_box,
            detail_glyph,
            detail,
            collapse: Rc::new(RefCell::new(None)),
        }
    }

    /// Show `name` and the agent state of the active workspace.
    pub fn set_workspace(&self, name: &str, activity: Activity) {
        let name = if name.is_empty() { "oxishell" } else { name };
        if self.name.label() != name {
            self.name.set_label(name);
        }
        let want = match activity {
            Activity::Working => Some("working"),
            Activity::Waiting => Some("waiting"),
            Activity::Present => Some("present"),
            Activity::None => None,
        };
        for c in DOT_CLASSES {
            if Some(c) != want && self.dot.has_css_class(c) {
                self.dot.remove_css_class(c);
            }
        }
        if let Some(c) = want {
            if !self.dot.has_css_class(c) {
                self.dot.add_css_class(c);
            }
        }
        self.dot.set_visible(want.is_some());
    }

    pub fn set_unread(&self, n: usize) {
        self.badge.set_label(&n.to_string());
        self.badge.set_visible(n > 0);
    }

    /// Stretch open to show `text` for a few seconds. Bells are not shown:
    /// they are frequent and say nothing on their own.
    pub fn flash(&self, kind: Kind, text: &str) {
        let (glyph, class, secs) = match kind {
            Kind::AgentDone | Kind::CommandDone => ("✓", "flash-done", 6),
            Kind::AgentInput => ("●", "flash-input", 10),
            Kind::CommandFailed => ("✕", "flash-failed", 8),
            Kind::Message => ("•", "flash-info", 6),
            Kind::Bell => return,
        };
        for c in FLASH_CLASSES {
            self.detail_box.remove_css_class(c);
        }
        self.detail_box.add_css_class(class);
        self.detail_glyph.set_label(glyph);
        self.detail.set_label(text);
        self.revealer.set_reveal_child(true);

        if let Some(old) = self.collapse.borrow_mut().take() {
            old.remove();
        }
        let revealer = self.revealer.downgrade();
        let slot = self.collapse.clone();
        let id = glib::timeout_add_local_once(Duration::from_secs(secs), move || {
            slot.borrow_mut().take();
            if let Some(r) = revealer.upgrade() {
                r.set_reveal_child(false);
            }
        });
        *self.collapse.borrow_mut() = Some(id);
    }
}

/// One line for the island from a notification's title and body.
pub fn flash_text(kind: Kind, title: &str, body: &str, extra: Option<&str>) -> String {
    let first_line = |s: &str| s.lines().next().unwrap_or("").trim().to_string();
    let mut parts = vec![first_line(title)];
    match kind {
        // Command bodies are "<cmd> · <duration>"; worth showing.
        Kind::CommandDone | Kind::CommandFailed => {
            let b = first_line(body);
            if !b.is_empty() {
                parts.push(b);
            }
        }
        _ => {}
    }
    if let Some(e) = extra {
        if !e.is_empty() {
            parts.push(e.to_string());
        }
    }
    parts.retain(|p| !p.is_empty());
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_flash_includes_body() {
        assert_eq!(
            flash_text(Kind::CommandDone, "Command finished", "cargo test · 42.1s", None),
            "Command finished · cargo test · 42.1s"
        );
    }

    #[test]
    fn agent_flash_skips_message_but_keeps_extra() {
        assert_eq!(
            flash_text(Kind::AgentDone, "Claude Code finished", "long message\nmore", Some("2m 14s")),
            "Claude Code finished · 2m 14s"
        );
    }
}
