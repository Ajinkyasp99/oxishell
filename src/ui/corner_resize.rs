//! Drag where dividers meet (┤ ├ ┬ ┴ ┼) to resize in both directions at once.
//!
//! Each split is a `gtk4::Paned`, which only moves along its own axis. A
//! capture-phase drag on the workspace page looks at the divider handles under
//! the pointer: when both a vertical and a horizontal one are in reach, it
//! claims the press and moves all of them together. Anywhere else the press is
//! left to the paned's own single-direction drag.

use super::AppRc;
use crate::model::WorkspaceId;
use gtk4::prelude::*;
use gtk4::{gdk, graphene, Orientation};
use std::cell::RefCell;
use std::rc::Rc;

/// Widget-name prefix carrying a split's path in the layout tree.
const PREFIX: &str = "split:";
/// Reach (px) past a handle's ends, so a junction is found where one handle
/// stops at the other.
const ALONG: f32 = 6.0;
/// Reach (px) to either side of a handle; stays inside the gap between panes.
const ACROSS: f32 = 4.0;

/// Handles showing the four-way cursor, with the cursor to restore.
type HotHandles = Rc<RefCell<Vec<(gtk4::Widget, Option<gdk::Cursor>)>>>;

/// Widget name for the split at `path` (false = first child, true = second).
pub fn split_name(path: &[bool]) -> String {
    let bits: String = path.iter().map(|b| if *b { '1' } else { '0' }).collect();
    format!("{PREFIX}{bits}")
}

fn split_path(name: &str) -> Option<Vec<bool>> {
    name.strip_prefix(PREFIX)?
        .chars()
        .map(|c| match c {
            '0' => Some(false),
            '1' => Some(true),
            _ => None,
        })
        .collect()
}

fn handle_of(p: &gtk4::Paned) -> Option<gtk4::Widget> {
    let mut child = p.first_child();
    while let Some(c) = child {
        if c.css_name() == "separator" {
            return Some(c);
        }
        child = c.next_sibling();
    }
    None
}

fn collect_splits(w: &gtk4::Widget, out: &mut Vec<gtk4::Paned>) {
    // Pane contents (terminal, overlays) never hold layout splits.
    if w.has_css_class("pane") {
        return;
    }
    if let Some(p) = w.downcast_ref::<gtk4::Paned>() {
        out.push(p.clone());
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        collect_splits(&c, out);
        child = c.next_sibling();
    }
}

/// Divider handles in reach of (`x`, `y`) on `page`, when they include both
/// orientations (a junction); empty otherwise.
fn junction_at(page: &gtk4::Widget, x: f64, y: f64) -> Vec<(gtk4::Paned, gtk4::Widget)> {
    let mut splits = Vec::new();
    collect_splits(page, &mut splits);
    let point = graphene::Point::new(x as f32, y as f32);
    let hits: Vec<_> = splits
        .into_iter()
        .filter_map(|p| {
            let h = handle_of(&p)?;
            if !h.is_drawable() {
                return None;
            }
            let b = h.compute_bounds(page)?;
            // A horizontal paned has a vertical handle: long along y.
            let (ax, ay) = if p.orientation() == Orientation::Horizontal {
                (ACROSS, ALONG)
            } else {
                (ALONG, ACROSS)
            };
            let reach = graphene::Rect::new(b.x() - ax, b.y() - ay, b.width() + 2.0 * ax, b.height() + 2.0 * ay);
            reach.contains_point(&point).then_some((p, h))
        })
        .collect();
    let has = |o| hits.iter().any(|(p, _)| p.orientation() == o);
    if has(Orientation::Horizontal) && has(Orientation::Vertical) {
        hits
    } else {
        Vec::new()
    }
}

/// Add junction dragging (and its four-way cursor) to workspace `wid`'s page.
pub fn attach(app: &AppRc, page: &gtk4::Box, wid: WorkspaceId) {
    // Splits being moved, with their position at the press.
    let active: Rc<RefCell<Vec<(gtk4::Paned, i32)>>> = Rc::default();
    let drag = gtk4::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    // Capture: decide before the paned's own handle drag claims the press.
    drag.set_propagation_phase(gtk4::PropagationPhase::Capture);
    {
        let active = active.clone();
        drag.connect_drag_begin(move |g, x, y| {
            let hits = g.widget().map(|page| junction_at(&page, x, y)).unwrap_or_default();
            if hits.is_empty() {
                g.set_state(gtk4::EventSequenceState::Denied);
                return;
            }
            g.set_state(gtk4::EventSequenceState::Claimed);
            *active.borrow_mut() = hits
                .into_iter()
                .map(|(p, _)| {
                    let start = p.position();
                    (p, start)
                })
                .collect();
        });
    }
    {
        let active = active.clone();
        drag.connect_drag_update(move |_, dx, dy| {
            for (p, start) in active.borrow().iter() {
                let d = if p.orientation() == Orientation::Horizontal { dx } else { dy };
                p.set_position(start + d.round() as i32);
            }
        });
    }
    {
        let w = app.weak();
        drag.connect_drag_end(move |_, _, _| {
            let moved = std::mem::take(&mut *active.borrow_mut());
            let Some(a) = w.upgrade() else { return };
            if moved.is_empty() {
                return;
            }
            // Save like a divider drag: position as a ratio of the split's size.
            for (p, _) in &moved {
                let size = if p.orientation() == Orientation::Horizontal {
                    p.width()
                } else {
                    p.height()
                };
                let Some(path) = split_path(&p.widget_name()) else { continue };
                if size > 0 {
                    if let Ok(ws) = a.inner.borrow_mut().state.workspace_mut(wid) {
                        ws.layout.set_ratio_at(&path, p.position() as f64 / size as f64);
                    }
                }
            }
            a.schedule_save();
        });
    }
    page.add_controller(drag);

    // Four-way cursor while the pointer is on a junction. The handles keep
    // their own col/row-resize cursor everywhere else.
    let hot: HotHandles = Rc::default();
    let restore = {
        let hot = hot.clone();
        move || {
            for (h, cursor) in hot.borrow_mut().drain(..) {
                h.set_cursor(cursor.as_ref());
            }
        }
    };
    let motion = gtk4::EventControllerMotion::new();
    {
        let restore = restore.clone();
        motion.connect_motion(move |m, x, y| {
            let handles: Vec<gtk4::Widget> = m
                .widget()
                .map(|page| junction_at(&page, x, y))
                .unwrap_or_default()
                .into_iter()
                .map(|(_, h)| h)
                .collect();
            let same = {
                let hot = hot.borrow();
                hot.len() == handles.len() && hot.iter().all(|(h, _)| handles.contains(h))
            };
            if same {
                return;
            }
            restore();
            let mut hot = hot.borrow_mut();
            for h in handles {
                let previous = h.cursor();
                h.set_cursor_from_name(Some("move"));
                hot.push((h, previous));
            }
        });
    }
    motion.connect_leave(move |_| restore());
    page.add_controller(motion);
}

#[cfg(test)]
mod tests {
    use super::{split_name, split_path};

    #[test]
    fn split_names_round_trip_their_path() {
        for path in [vec![], vec![false], vec![true, false, true]] {
            assert_eq!(split_path(&split_name(&path)), Some(path));
        }
        assert_eq!(split_name(&[true, false]), "split:10");
        assert_eq!(split_path("pane"), None);
        assert_eq!(split_path("split:12"), None);
    }
}
