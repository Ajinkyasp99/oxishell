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

    /// Panes laid side by side along `o` at the top of this subtree: a split
    /// in the other direction (or a leaf) counts as one slot.
    fn slots(&self, o: Orient) -> usize {
        match self {
            Node::Split { orient, a, b, .. } if *orient == o => a.slots(o) + b.slots(o),
            _ => 1,
        }
    }

    /// Give every slot of the row/column along `o` rooted here the same size.
    fn equalize(&mut self, o: Orient) {
        if let Node::Split { orient, ratio, a, b } = self {
            if *orient == o {
                let (sa, sb) = (a.slots(o), b.slots(o));
                *ratio = sa as f64 / (sa + sb) as f64;
                a.equalize(o);
                b.equalize(o);
            }
        }
    }

    /// Exchange the positions of leaves `x` and `y`; split sizes stay put.
    /// Returns false (and changes nothing) unless both are in this tree.
    pub fn swap(&mut self, x: PaneId, y: PaneId) -> bool {
        if !self.contains(x) || !self.contains(y) {
            return false;
        }
        self.swap_ids(x, y);
        true
    }

    fn swap_ids(&mut self, x: PaneId, y: PaneId) {
        match self {
            Node::Leaf { pane } if *pane == x => *pane = y,
            Node::Leaf { pane } if *pane == y => *pane = x,
            Node::Leaf { .. } => {}
            Node::Split { a, b, .. } => {
                a.swap_ids(x, y);
                b.swap_ids(x, y);
            }
        }
    }

    /// Splits from the root down to leaf `pane`: (went to `b`, orientation).
    fn path_to(&self, pane: PaneId) -> Option<Vec<(bool, Orient)>> {
        match self {
            Node::Leaf { pane: p } => (*p == pane).then(Vec::new),
            Node::Split { orient, a, b, .. } => {
                for (side, child) in [(false, a), (true, b)] {
                    if let Some(mut rest) = child.path_to(pane) {
                        rest.insert(0, (side, *orient));
                        return Some(rest);
                    }
                }
                None
            }
        }
    }

    fn at_mut(&mut self, dirs: &[bool]) -> &mut Node {
        match (self, dirs.split_first()) {
            (Node::Split { a, b, .. }, Some((go_b, rest))) => if *go_b { b } else { a }.at_mut(rest),
            (n, _) => n,
        }
    }

    /// Equalize the row/column along `o` formed by the trailing splits of
    /// `path` that all run along `o`.
    fn equalize_line(&mut self, path: &[(bool, Orient)], o: Orient) {
        let top = path.iter().rposition(|(_, x)| *x != o).map_or(0, |i| i + 1);
        let dirs: Vec<bool> = path[..top].iter().map(|(d, _)| *d).collect();
        self.at_mut(&dirs).equalize(o);
    }

    /// Split `target` like [`Node::split`], then share the row/column the new
    /// pane joined equally, so repeated splits make equal panes instead of
    /// halving the newest one each time.
    pub fn split_balanced(&mut self, target: PaneId, new: PaneId, orient: Orient) -> bool {
        if !self.split(target, new, orient) {
            return false;
        }
        if let Some(path) = self.path_to(new) {
            self.equalize_line(&path, orient);
        }
        true
    }

    /// Remove `target` like [`Node::remove`], then re-share the row/column
    /// it left so the freed space goes to all its neighbours.
    pub fn remove_balanced(self, target: PaneId) -> Option<Node> {
        let path = self.path_to(target);
        let mut rest = self.remove(target)?;
        if let Some(path) = path {
            if let Some((_, o)) = path.last().copied() {
                rest.equalize_line(&path[..path.len() - 1], o);
            }
        }
        Some(rest)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub layout: Node,
    pub focused: PaneId,
    /// Stripe colour in the sidebar, one of [`WORKSPACE_COLORS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Sidebar icon, a key of [`WORKSPACE_ICONS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// Workspace colours. Themes draw them from their own palette.
pub const WORKSPACE_COLORS: [&str; 7] = ["red", "yellow", "green", "cyan", "blue", "magenta", "gray"];

/// Workspace icons: key (stored, used by `oxi`) and symbolic icon name.
pub const WORKSPACE_ICONS: [(&str, &str); 12] = [
    ("terminal", "utilities-terminal-symbolic"),
    ("server", "network-server-symbolic"),
    ("database", "drive-harddisk-symbolic"),
    ("web", "web-browser-symbolic"),
    ("tools", "applications-engineering-symbolic"),
    ("docs", "folder-documents-symbolic"),
    ("science", "applications-science-symbolic"),
    ("star", "starred-symbolic"),
    ("lock", "security-high-symbolic"),
    ("cloud", "weather-overcast-symbolic"),
    ("warning", "dialog-warning-symbolic"),
    ("home", "user-home-symbolic"),
];

/// Symbolic icon name for a workspace icon key.
pub fn icon_name(key: &str) -> Option<&'static str> {
    WORKSPACE_ICONS.iter().find(|(k, _)| *k == key).map(|(_, n)| *n)
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
    DifferentWorkspaces,
    InvalidColor,
    InvalidIcon,
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelError::NoSuchWorkspace(id) => write!(f, "no such workspace: {id}"),
            ModelError::NoSuchPane(id) => write!(f, "no such pane: {id}"),
            ModelError::InvalidName => write!(f, "name must be 1-64 characters without control characters"),
            ModelError::DifferentWorkspaces => write!(f, "panes are in different workspaces"),
            ModelError::InvalidColor => write!(f, "colour must be one of: {}, or none", WORKSPACE_COLORS.join(", ")),
            ModelError::InvalidIcon => {
                let keys: Vec<&str> = WORKSPACE_ICONS.iter().map(|(k, _)| *k).collect();
                write!(f, "icon must be one of: {}, or none", keys.join(", "))
            }
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
            color: None,
            icon: None,
        });
        self.active = Some(id);
        Ok((id, pane))
    }

    pub fn rename(&mut self, id: WorkspaceId, name: &str) -> Result<(), ModelError> {
        let name = validate_name(name)?;
        self.workspace_mut(id)?.name = name;
        Ok(())
    }

    /// Set or clear (`None`) a workspace's sidebar colour.
    pub fn set_color(&mut self, id: WorkspaceId, color: Option<&str>) -> Result<(), ModelError> {
        let color = match color.map(str::to_ascii_lowercase) {
            Some(c) if WORKSPACE_COLORS.contains(&c.as_str()) => Some(c),
            Some(_) => return Err(ModelError::InvalidColor),
            None => None,
        };
        self.workspace_mut(id)?.color = color;
        Ok(())
    }

    /// Set or clear (`None`) a workspace's sidebar icon.
    pub fn set_icon(&mut self, id: WorkspaceId, icon: Option<&str>) -> Result<(), ModelError> {
        let icon = match icon.map(str::to_ascii_lowercase) {
            Some(i) if icon_name(&i).is_some() => Some(i),
            Some(_) => return Err(ModelError::InvalidIcon),
            None => None,
        };
        self.workspace_mut(id)?.icon = icon;
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
        ws.layout.split_balanced(pane, new, orient);
        ws.focused = new;
        Ok(new)
    }

    pub fn close_pane(&mut self, pane: PaneId) -> Result<PaneClosed, ModelError> {
        let wid = self.workspace_of(pane).ok_or(ModelError::NoSuchPane(pane))?;
        let ws = self.workspace_mut(wid)?;
        let order = ws.layout.panes();
        let layout = std::mem::replace(&mut ws.layout, Node::leaf(0));
        match layout.remove_balanced(pane) {
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

    /// Swap the positions of two panes in the same workspace. Focus and the
    /// split sizes are unchanged. Swapping a pane with itself is a no-op.
    pub fn swap_panes(&mut self, x: PaneId, y: PaneId) -> Result<WorkspaceId, ModelError> {
        let wid = self.workspace_of(x).ok_or(ModelError::NoSuchPane(x))?;
        let wy = self.workspace_of(y).ok_or(ModelError::NoSuchPane(y))?;
        if wid != wy {
            return Err(ModelError::DifferentWorkspaces);
        }
        if x != y {
            self.workspace_mut(wid)?.layout.swap(x, y);
        }
        Ok(wid)
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

    #[test]
    fn workspace_colour_and_icon_can_be_set_and_cleared() {
        let mut s = AppState::default();
        let (w, _) = s.create_workspace(Some("prod")).unwrap();
        assert_eq!(s.workspace(w).unwrap().color, None);
        s.set_color(w, Some("Red")).unwrap();
        assert_eq!(s.workspace(w).unwrap().color.as_deref(), Some("red"));
        s.set_icon(w, Some("server")).unwrap();
        assert_eq!(s.workspace(w).unwrap().icon.as_deref(), Some("server"));
        s.set_color(w, None).unwrap();
        s.set_icon(w, None).unwrap();
        assert_eq!(s.workspace(w).unwrap().color, None);
        assert_eq!(s.workspace(w).unwrap().icon, None);
    }

    #[test]
    fn unknown_colours_and_icons_are_rejected() {
        let mut s = AppState::default();
        let (w, _) = s.create_workspace(None).unwrap();
        s.set_color(w, Some("blue")).unwrap();
        assert_eq!(s.set_color(w, Some("chartreuse")), Err(ModelError::InvalidColor));
        assert_eq!(s.set_icon(w, Some("unicorn")), Err(ModelError::InvalidIcon));
        assert_eq!(s.workspace(w).unwrap().color.as_deref(), Some("blue"), "unchanged");
        assert_eq!(s.set_color(999, Some("red")), Err(ModelError::NoSuchWorkspace(999)));
        assert!(ModelError::InvalidColor
            .to_string()
            .contains("red, yellow, green, cyan, blue, magenta, gray"));
        assert!(ModelError::InvalidIcon.to_string().contains("terminal"));
    }

    #[test]
    fn every_icon_has_a_symbolic_icon_name() {
        assert_eq!(WORKSPACE_ICONS.len(), 12);
        for (key, _) in WORKSPACE_ICONS {
            assert!(icon_name(key).is_some_and(|n| n.ends_with("-symbolic")), "{key}");
        }
        assert_eq!(icon_name("nope"), None);
    }

    #[test]
    fn sessions_without_colour_or_icon_still_load() {
        let old = r#"{"id":1,"name":"api","layout":{"type":"leaf","pane":2},"focused":2}"#;
        let w: Workspace = serde_json::from_str(old).unwrap();
        assert_eq!((w.color, w.icon), (None, None));
        let mut s = AppState::default();
        let (id, _) = s.create_workspace(None).unwrap();
        s.set_color(id, Some("green")).unwrap();
        s.set_icon(id, Some("web")).unwrap();
        let back: AppState = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.workspace(id).unwrap().color.as_deref(), Some("green"));
        assert_eq!(back.workspace(id).unwrap().icon.as_deref(), Some("web"));
    }

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

    fn widths(l: &Node) -> Vec<f64> {
        l.rects().iter().map(|(_, r)| (r.w * 1000.0).round() / 1000.0).collect()
    }

    #[test]
    fn repeated_splits_share_space_equally() {
        let (mut s, w, p1) = state_with_ws();
        let mut last = p1;
        for _ in 0..3 {
            last = s.split(last, Orient::Horizontal).unwrap();
        }
        assert_eq!(widths(&s.workspace(w).unwrap().layout), vec![0.25; 4]);
        // Splitting a middle pane keeps the row equal too.
        s.split(p1, Orient::Horizontal).unwrap();
        assert_eq!(widths(&s.workspace(w).unwrap().layout), vec![0.2; 5]);
        assert!(s.validate().is_ok());
    }

    #[test]
    fn cross_direction_split_only_balances_its_own_line() {
        let (mut s, w, p1) = state_with_ws();
        let p2 = s.split(p1, Orient::Horizontal).unwrap();
        s.workspace_mut(w).unwrap().layout.set_ratio_at(&[], 0.7);
        let p3 = s.split(p2, Orient::Vertical).unwrap();
        s.split(p3, Orient::Vertical).unwrap();
        let rects = s.workspace(w).unwrap().layout.rects();
        // The user's 70/30 column split survives; the right column is thirds.
        assert!((rects[0].1.w - 0.7).abs() < 1e-9);
        for (_, r) in &rects[1..] {
            assert!((r.h - 1.0 / 3.0).abs() < 1e-9, "{rects:?}");
        }
    }

    #[test]
    fn closing_a_pane_rebalances_its_line() {
        let (mut s, w, p1) = state_with_ws();
        let p2 = s.split(p1, Orient::Horizontal).unwrap();
        let p3 = s.split(p2, Orient::Horizontal).unwrap();
        s.split(p3, Orient::Horizontal).unwrap();
        s.close_pane(p3).unwrap();
        assert_eq!(widths(&s.workspace(w).unwrap().layout), vec![0.333; 3]);
        s.close_pane(p1).unwrap();
        assert_eq!(widths(&s.workspace(w).unwrap().layout), vec![0.5; 2]);
    }

    #[test]
    fn swap_panes_exchanges_positions_and_keeps_sizes() {
        let (mut s, w, p1) = state_with_ws();
        let p2 = s.split(p1, Orient::Horizontal).unwrap();
        let p3 = s.split(p2, Orient::Vertical).unwrap();
        s.workspace_mut(w).unwrap().layout.set_ratio_at(&[], 0.7);
        let before = s.workspace(w).unwrap().layout.rects();
        let focused = s.workspace(w).unwrap().focused;

        assert_eq!(s.swap_panes(p1, p3).unwrap(), w);
        let ws = s.workspace(w).unwrap();
        assert_eq!(ws.layout.panes(), vec![p3, p2, p1]);
        assert_eq!(ws.focused, focused);
        let after = ws.layout.rects();
        // Same slots, same sizes; only the occupants changed.
        let rect_of = |rs: &[(PaneId, Rect)], p| rs.iter().find(|(id, _)| *id == p).unwrap().1;
        assert_eq!(rect_of(&after, p3), rect_of(&before, p1));
        assert_eq!(rect_of(&after, p1), rect_of(&before, p3));
        assert_eq!(rect_of(&after, p2), rect_of(&before, p2));
        assert!(s.validate().is_ok());
    }

    #[test]
    fn swap_panes_rejects_other_workspaces_and_unknown_panes() {
        let (mut s, w, p1) = state_with_ws();
        let p2 = s.split(p1, Orient::Horizontal).unwrap();
        let (_, other) = s.create_workspace(None).unwrap();
        assert_eq!(s.swap_panes(p1, other), Err(ModelError::DifferentWorkspaces));
        assert_eq!(s.swap_panes(p1, 9999), Err(ModelError::NoSuchPane(9999)));
        // Swapping with itself changes nothing.
        s.swap_panes(p2, p2).unwrap();
        assert_eq!(s.workspace(w).unwrap().layout.panes(), vec![p1, p2]);
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
