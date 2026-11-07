//! Workspace and pane-layout model (GTK-free, serializable).
//!
//! A workspace owns a binary split tree whose leaves are pane ids. The GUI
//! renders this tree; the CLI and session persistence read and mutate it.

use serde::{Deserialize, Serialize};

pub type PaneId = u64;
pub type WorkspaceId = u64;

/// Orientation of a split. `Horizontal` places children side by side
/// ("split right"); `Vertical` stacks them ("split down").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Orient {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Node {
    Leaf {
        pane: PaneId,
    },
    Split {
        orient: Orient,
        ratio: f64,
        a: Box<Node>,
        b: Box<Node>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Node {
    pub fn leaf(pane: PaneId) -> Node {
        Node::Leaf { pane }
    }

    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Leaf { pane } => out.push(*pane),
            Node::Split { a, b, .. } => {
                a.collect(out);
                b.collect(out);
            }
        }
    }

    pub fn contains(&self, id: PaneId) -> bool {
        match self {
            Node::Leaf { pane } => *pane == id,
            Node::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    /// Replace leaf `target` with a split of `target` and `new` (new goes second).
    pub fn split(&mut self, target: PaneId, new: PaneId, orient: Orient) -> bool {
        match self {
            Node::Leaf { pane } if *pane == target => {
                *self = Node::Split {
                    orient,
                    ratio: 0.5,
                    a: Box::new(Node::leaf(target)),
                    b: Box::new(Node::leaf(new)),
                };
                true
            }
            Node::Leaf { .. } => false,
            Node::Split { a, b, .. } => a.split(target, new, orient) || b.split(target, new, orient),
        }
    }

    /// Remove a leaf; its sibling takes the parent's place. Returns
    /// `None` when the whole tree was that single leaf.
    pub fn remove(self, target: PaneId) -> Option<Node> {
        match self {
            Node::Leaf { pane } if pane == target => None,
            leaf @ Node::Leaf { .. } => Some(leaf),
            Node::Split { orient, ratio, a, b } => {
                if matches!(*a, Node::Leaf { pane } if pane == target) {
                    return Some(*b);
                }
                if matches!(*b, Node::Leaf { pane } if pane == target) {
                    return Some(*a);
                }
                let a = a.remove(target).expect("non-leaf child cannot vanish");
                let b = b.remove(target).expect("non-leaf child cannot vanish");
                Some(Node::Split {
                    orient,
                    ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                })
            }
        }
    }

    /// Unit-square geometry for every leaf, used for directional focus.
    pub fn rects(&self) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        self.layout(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
            &mut out,
        );
        out
    }

    fn layout(&self, r: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Node::Leaf { pane } => out.push((*pane, r)),
            Node::Split { orient, ratio, a, b } => {
                let t = ratio.clamp(0.05, 0.95);
                let (ra, rb) = match orient {
                    Orient::Horizontal => (
                        Rect { w: r.w * t, ..r },
                        Rect {
                            x: r.x + r.w * t,
                            w: r.w * (1.0 - t),
                            ..r
                        },
                    ),
                    Orient::Vertical => (
                        Rect { h: r.h * t, ..r },
                        Rect {
                            y: r.y + r.h * t,
                            h: r.h * (1.0 - t),
                            ..r
                        },
                    ),
                };
                a.layout(ra, out);
                b.layout(rb, out);
            }
        }
    }

    /// The pane adjacent to `from` in `dir`, preferring the one with the
    /// largest overlap on the perpendicular axis, then the nearest.
    pub fn neighbor(&self, from: PaneId, dir: Direction) -> Option<PaneId> {
        let rects = self.rects();
        let (_, cur) = *rects.iter().find(|(p, _)| *p == from)?;
        const EPS: f64 = 1e-9;
        let overlap = |a0: f64, a1: f64, b0: f64, b1: f64| (a1.min(b1) - a0.max(b0)).max(0.0);
        rects
            .iter()
            .filter(|(p, _)| *p != from)
            .filter_map(|(p, r)| {
                let (dist, ov) = match dir {
                    Direction::Left => (cur.x - (r.x + r.w), overlap(cur.y, cur.y + cur.h, r.y, r.y + r.h)),
                    Direction::Right => (r.x - (cur.x + cur.w), overlap(cur.y, cur.y + cur.h, r.y, r.y + r.h)),
                    Direction::Up => (cur.y - (r.y + r.h), overlap(cur.x, cur.x + cur.w, r.x, r.x + r.w)),
                    Direction::Down => (r.y - (cur.y + cur.h), overlap(cur.x, cur.x + cur.w, r.x, r.x + r.w)),
                };
                (dist > -EPS && ov > EPS).then_some((*p, dist, ov))
            })
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap().then(b.2.partial_cmp(&a.2).unwrap()))
            .map(|(p, _, _)| p)
    }

    /// Set the ratio of the split reached by `path` (false = first child,
    /// true = second child). Used by the UI after a divider drag.
    pub fn set_ratio_at(&mut self, path: &[bool], ratio: f64) -> bool {
        match (self, path.split_first()) {
            (Node::Split { ratio: r, .. }, None) => {
                *r = ratio.clamp(0.05, 0.95);
                true
            }
            (Node::Split { a, b, .. }, Some((go_b, rest))) => {
                if *go_b {
                    b.set_ratio_at(rest, ratio)
                } else {
                    a.set_ratio_at(rest, ratio)
                }
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub layout: Node,
    pub focused: PaneId,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppState {
    pub workspaces: Vec<Workspace>,
    pub active: Option<WorkspaceId>,
    pub next_id: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ModelError {
    NoSuchWorkspace(WorkspaceId),
    NoSuchPane(PaneId),
    InvalidName,
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelError::NoSuchWorkspace(id) => write!(f, "no such workspace: {id}"),
            ModelError::NoSuchPane(id) => write!(f, "no such pane: {id}"),
            ModelError::InvalidName => write!(f, "name must be 1-64 characters without control characters"),
        }
    }
}
impl std::error::Error for ModelError {}

pub fn validate_name(name: &str) -> Result<String, ModelError> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 64 || n.chars().any(char::is_control) {
        return Err(ModelError::InvalidName);
    }
    Ok(n.to_string())
}

/// What happened when a pane was closed.
#[derive(Debug, PartialEq, Eq)]
pub enum PaneClosed {
    /// Pane removed; the workspace still has panes and this one is now focused.
    Refocus(PaneId),
    /// The pane was the last one; the workspace was removed.
    WorkspaceRemoved(WorkspaceId),
}

impl AppState {
    fn alloc(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    pub fn workspace_mut(&mut self, id: WorkspaceId) -> Result<&mut Workspace, ModelError> {
        self.workspaces
            .iter_mut()
            .find(|w| w.id == id)
            .ok_or(ModelError::NoSuchWorkspace(id))
    }

    pub fn active_workspace(&self) -> Option<&Workspace> {
        self.active.and_then(|id| self.workspace(id))
    }

    pub fn workspace_of(&self, pane: PaneId) -> Option<WorkspaceId> {
        self.workspaces.iter().find(|w| w.layout.contains(pane)).map(|w| w.id)
    }

    pub fn all_panes(&self) -> Vec<(WorkspaceId, PaneId)> {
        self.workspaces
            .iter()
            .flat_map(|w| w.layout.panes().into_iter().map(move |p| (w.id, p)))
            .collect()
    }

    /// Create a workspace with one pane; returns (workspace, pane). It becomes active.
    pub fn create_workspace(&mut self, name: Option<&str>) -> Result<(WorkspaceId, PaneId), ModelError> {
        let name = match name {
            Some(n) => validate_name(n)?,
            None => format!("Workspace {}", self.workspaces.len() + 1),
        };
        let id = self.alloc();
        let pane = self.alloc();
        self.workspaces.push(Workspace {
            id,
            name,
            layout: Node::leaf(pane),
            focused: pane,
        });
        self.active = Some(id);
        Ok((id, pane))
    }

    pub fn rename(&mut self, id: WorkspaceId, name: &str) -> Result<(), ModelError> {
        let name = validate_name(name)?;
        self.workspace_mut(id)?.name = name;
        Ok(())
    }

    pub fn select(&mut self, id: WorkspaceId) -> Result<(), ModelError> {
        self.workspace_mut(id)?;
        self.active = Some(id);
        Ok(())
    }

    /// Select the workspace `delta` positions from the active one (wrapping).
    pub fn select_relative(&mut self, delta: isize) -> Option<WorkspaceId> {
        let n = self.workspaces.len() as isize;
        if n == 0 {
            return None;
        }
        let cur = self
            .active
            .and_then(|a| self.workspaces.iter().position(|w| w.id == a))
            .unwrap_or(0) as isize;
        let id = self.workspaces[((cur + delta).rem_euclid(n)) as usize].id;
        self.active = Some(id);
        Some(id)
    }

    /// Remove a workspace; returns the panes that must be terminated.
    pub fn close_workspace(&mut self, id: WorkspaceId) -> Result<Vec<PaneId>, ModelError> {
        let idx = self
            .workspaces
            .iter()
            .position(|w| w.id == id)
            .ok_or(ModelError::NoSuchWorkspace(id))?;
        let ws = self.workspaces.remove(idx);
        if self.active == Some(id) {
            self.active = self.workspaces.get(idx.min(self.workspaces.len().saturating_sub(1))).map(|w| w.id);
        }
        Ok(ws.layout.panes())
    }

    /// Move a workspace to `to` (clamped).
    pub fn move_workspace(&mut self, id: WorkspaceId, to: usize) -> Result<(), ModelError> {
        let idx = self
            .workspaces
            .iter()
            .position(|w| w.id == id)
            .ok_or(ModelError::NoSuchWorkspace(id))?;
        let ws = self.workspaces.remove(idx);
        let to = to.min(self.workspaces.len());
        self.workspaces.insert(to, ws);
        Ok(())
    }

    /// Split `pane`; the new pane is focused. Returns the new pane id.
    pub fn split(&mut self, pane: PaneId, orient: Orient) -> Result<PaneId, ModelError> {
        let wid = self.workspace_of(pane).ok_or(ModelError::NoSuchPane(pane))?;
        let new = self.alloc();
        let ws = self.workspace_mut(wid)?;
        ws.layout.split(pane, new, orient);
        ws.focused = new;
        Ok(new)
    }

    pub fn close_pane(&mut self, pane: PaneId) -> Result<PaneClosed, ModelError> {
        let wid = self.workspace_of(pane).ok_or(ModelError::NoSuchPane(pane))?;
        let ws = self.workspace_mut(wid)?;
        let order = ws.layout.panes();
        let layout = std::mem::replace(&mut ws.layout, Node::leaf(0));
        match layout.remove(pane) {
            Some(rest) => {
                ws.layout = rest;
                if ws.focused == pane {
                    // Focus the previous pane in reading order (or the first).
                    let pos = order.iter().position(|p| *p == pane).unwrap_or(0);
                    let remaining = ws.layout.panes();
                    ws.focused = *remaining.get(pos.saturating_sub(1)).unwrap_or(&remaining[0]);
                }
                Ok(PaneClosed::Refocus(ws.focused))
            }
            None => {
                self.close_workspace(wid)?;
                Ok(PaneClosed::WorkspaceRemoved(wid))
            }
        }
    }

    pub fn focus(&mut self, pane: PaneId) -> Result<WorkspaceId, ModelError> {
        let wid = self.workspace_of(pane).ok_or(ModelError::NoSuchPane(pane))?;
        self.workspace_mut(wid)?.focused = pane;
        self.active = Some(wid);
        Ok(wid)
    }

    pub fn focus_direction(&mut self, dir: Direction) -> Option<PaneId> {
        let ws = self.active.and_then(|a| self.workspaces.iter_mut().find(|w| w.id == a))?;
        let next = ws.layout.neighbor(ws.focused, dir)?;
        ws.focused = next;
        Some(next)
    }

    /// Ensure `next_id` is beyond every id in use (after loading a session).
    pub fn normalize_ids(&mut self) {
        let max = self
            .workspaces
            .iter()
            .flat_map(|w| std::iter::once(w.id).chain(w.layout.panes()))
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(max);
    }

    /// Structural validity: unique ids, focused pane present, active present.
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for w in &self.workspaces {
            if !seen.insert(w.id) {
                return Err(format!("duplicate id {}", w.id));
            }
            validate_name(&w.name).map_err(|e| e.to_string())?;
            for p in w.layout.panes() {
                if !seen.insert(p) {
                    return Err(format!("duplicate id {p}"));
                }
            }
            if !w.layout.contains(w.focused) {
                return Err(format!("workspace {} focuses missing pane", w.id));
            }
            check_ratios(&w.layout)?;
        }
        if let Some(a) = self.active {
            if self.workspace(a).is_none() {
                return Err("active workspace missing".into());
            }
        }
        Ok(())
    }
}

fn check_ratios(n: &Node) -> Result<(), String> {
    match n {
        Node::Leaf { .. } => Ok(()),
        Node::Split { ratio, a, b, .. } => {
            if !ratio.is_finite() || *ratio <= 0.0 || *ratio >= 1.0 {
                return Err(format!("invalid split ratio {ratio}"));
            }
            check_ratios(a)?;
            check_ratios(b)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_ws() -> (AppState, WorkspaceId, PaneId) {
        let mut s = AppState::default();
        let (w, p) = s.create_workspace(Some("Backend")).unwrap();
        (s, w, p)
    }

    #[test]
    fn create_select_rename_close_workspaces() {
        let (mut s, w1, _) = state_with_ws();
        let (w2, _) = s.create_workspace(None).unwrap();
        assert_eq!(s.active, Some(w2));
        assert_eq!(s.workspace(w2).unwrap().name, "Workspace 2");
        s.select(w1).unwrap();
        assert_eq!(s.active, Some(w1));
        s.rename(w2, "  Frontend ").unwrap();
        assert_eq!(s.workspace(w2).unwrap().name, "Frontend");
        assert_eq!(s.rename(w2, "  "), Err(ModelError::InvalidName));
        assert_eq!(s.rename(w2, "a\u{1b}b"), Err(ModelError::InvalidName));
        assert_eq!(s.select(999), Err(ModelError::NoSuchWorkspace(999)));
        let panes = s.close_workspace(w1).unwrap();
        assert_eq!(panes.len(), 1);
        assert_eq!(s.active, Some(w2));
    }

    #[test]
    fn select_relative_wraps() {
        let (mut s, w1, _) = state_with_ws();
        let (w2, _) = s.create_workspace(None).unwrap();
        assert_eq!(s.select_relative(1), Some(w1));
        assert_eq!(s.select_relative(-1), Some(w2));
    }

    #[test]
    fn reorder_workspaces() {
        let (mut s, w1, _) = state_with_ws();
        let (w2, _) = s.create_workspace(None).unwrap();
        let (w3, _) = s.create_workspace(None).unwrap();
        s.move_workspace(w3, 0).unwrap();
        let ids: Vec<_> = s.workspaces.iter().map(|w| w.id).collect();
        assert_eq!(ids, vec![w3, w1, w2]);
        s.move_workspace(w3, 99).unwrap();
        assert_eq!(s.workspaces.last().unwrap().id, w3);
    }

    #[test]
    fn nested_splits_and_close() {
        let (mut s, w, p1) = state_with_ws();
        let p2 = s.split(p1, Orient::Horizontal).unwrap();
        let p3 = s.split(p2, Orient::Vertical).unwrap();
        assert_eq!(s.workspace(w).unwrap().layout.panes(), vec![p1, p2, p3]);
        assert_eq!(s.workspace(w).unwrap().focused, p3);
        assert_eq!(s.close_pane(p3).unwrap(), PaneClosed::Refocus(p2));
        assert_eq!(s.workspace(w).unwrap().layout.panes(), vec![p1, p2]);
        assert_eq!(s.close_pane(p1).unwrap(), PaneClosed::Refocus(p2));
        assert_eq!(s.close_pane(p2).unwrap(), PaneClosed::WorkspaceRemoved(w));
        assert!(s.workspaces.is_empty());
        assert_eq!(s.active, None);
        assert_eq!(s.close_pane(p2), Err(ModelError::NoSuchPane(p2)));
    }

    #[test]
    fn directional_focus() {
        // Layout:  p1 | p2
        //               --
        //               p3
        let (mut s, _, p1) = state_with_ws();
        let p2 = s.split(p1, Orient::Horizontal).unwrap();
        let p3 = s.split(p2, Orient::Vertical).unwrap();
        let l = &s.active_workspace().unwrap().layout;
        assert_eq!(l.neighbor(p1, Direction::Right), Some(p2));
        assert_eq!(l.neighbor(p3, Direction::Left), Some(p1));
        assert_eq!(l.neighbor(p3, Direction::Up), Some(p2));
        assert_eq!(l.neighbor(p2, Direction::Down), Some(p3));
        assert_eq!(l.neighbor(p1, Direction::Left), None);
        s.focus(p3).unwrap();
        assert_eq!(s.focus_direction(Direction::Up), Some(p2));
    }

    #[test]
    fn ratio_update_and_validation() {
        let (mut s, w, p1) = state_with_ws();
        let p2 = s.split(p1, Orient::Horizontal).unwrap();
        s.split(p2, Orient::Vertical).unwrap();
        let ws = s.workspace_mut(w).unwrap();
        assert!(ws.layout.set_ratio_at(&[true], 0.3));
        assert!(ws.layout.set_ratio_at(&[], 0.7));
        assert!(!ws.layout.set_ratio_at(&[false], 0.3)); // leaf
        assert!(s.validate().is_ok());
        let json = serde_json::to_string(&s).unwrap();
        let back: AppState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.workspaces[0].layout, s.workspaces[0].layout);
    }

    #[test]
    fn validate_rejects_corrupt_state() {
        let (mut s, w, _) = state_with_ws();
        s.workspace_mut(w).unwrap().focused = 12345;
        assert!(s.validate().is_err());
    }

    #[test]
    fn normalize_ids_after_load() {
        let (mut s, _, _) = state_with_ws();
        s.next_id = 0;
        s.normalize_ids();
        let (_, p) = s.create_workspace(None).unwrap();
        assert!(s.validate().is_ok(), "ids must stay unique, got pane {p}");
    }
}
