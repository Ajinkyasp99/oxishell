//! Workspace and pane-layout model (GTK-free, serializable).
//!
//! A workspace owns a binary split tree whose leaves are pane ids. The GUI
//! renders this tree; the CLI and session persistence read and mutate it.

use serde::{Deserialize, Serialize};

pub type PaneId = u64;
pub type WorkspaceId = u64;
/// Workspaces, tabs and panes share one id space.
pub type TabId = u64;

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

/// One tab of a workspace: its own split layout and focused pane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    /// 0 only in sessions from before tabs, until [`AppState::normalize_ids`].
    #[serde(default)]
    pub id: TabId,
    pub layout: Node,
    pub focused: PaneId,
    /// Name the user gave the tab; without one, the tab is labelled by what
    /// runs in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A workspace owns tabs (never empty); `active_tab` is the one shown.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "WorkspaceRepr")]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    /// Stripe colour in the sidebar, one of [`WORKSPACE_COLORS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Sidebar icon, a key of [`WORKSPACE_ICONS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// A saved workspace: with `tabs`, or (before tabs) one `layout` + `focused`.
#[derive(Deserialize)]
struct WorkspaceRepr {
    id: WorkspaceId,
    name: String,
    #[serde(default)]
    tabs: Option<Vec<Tab>>,
    #[serde(default)]
    active_tab: usize,
    #[serde(default)]
    layout: Option<Node>,
    #[serde(default)]
    focused: Option<PaneId>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    icon: Option<String>,
}

impl TryFrom<WorkspaceRepr> for Workspace {
    type Error = String;
    fn try_from(r: WorkspaceRepr) -> Result<Workspace, String> {
        let tabs = match (r.tabs, r.layout, r.focused) {
            (Some(tabs), _, _) => tabs,
            (None, Some(layout), Some(focused)) => vec![Tab {
                id: 0,
                layout,
                focused,
                name: None,
            }],
            _ => return Err(format!("workspace {} has no tabs", r.id)),
        };
        if tabs.is_empty() || r.active_tab >= tabs.len() {
            return Err(format!("workspace {} has no tabs or an invalid active tab", r.id));
        }
        Ok(Workspace {
            id: r.id,
            name: r.name,
            tabs,
            active_tab: r.active_tab,
            color: r.color,
            icon: r.icon,
        })
    }
}

impl Workspace {
    /// The shown tab.
    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active_tab]
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active_tab]
    }

    /// The focused pane of the shown tab.
    pub fn focused(&self) -> PaneId {
        self.tab().focused
    }

    /// Every pane of every tab, tab by tab in layout order.
    pub fn panes(&self) -> Vec<PaneId> {
        self.tabs.iter().flat_map(|t| t.layout.panes()).collect()
    }

    /// Index of the tab holding `pane`.
    pub fn tab_index_of(&self, pane: PaneId) -> Option<usize> {
        self.tabs.iter().position(|t| t.layout.contains(pane))
    }

    pub fn tab_index(&self, tab: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == tab)
    }
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
    NoSuchTab(TabId),
    DifferentTabs,
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelError::NoSuchWorkspace(id) => write!(f, "no such workspace: {id}"),
            ModelError::NoSuchPane(id) => write!(f, "no such pane: {id}"),
            ModelError::InvalidName => write!(f, "name must be 1-64 characters without control characters"),
            ModelError::DifferentWorkspaces => write!(f, "panes are in different workspaces"),
            ModelError::NoSuchTab(id) => write!(f, "no such tab: {id}"),
            ModelError::DifferentTabs => write!(f, "panes are in different tabs"),
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

/// Find the workspace and tab index holding `pane`.
fn locate(workspaces: &[Workspace], pane: PaneId) -> Option<(usize, usize)> {
    workspaces
        .iter()
        .enumerate()
        .find_map(|(w, ws)| ws.tab_index_of(pane).map(|t| (w, t)))
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
        self.workspaces.iter().find(|w| w.tab_index_of(pane).is_some()).map(|w| w.id)
    }

    pub fn all_panes(&self) -> Vec<(WorkspaceId, PaneId)> {
        self.workspaces
            .iter()
            .flat_map(|w| w.panes().into_iter().map(move |p| (w.id, p)))
            .collect()
    }

    /// Create a workspace with one pane; returns (workspace, pane). It becomes active.
    pub fn create_workspace(&mut self, name: Option<&str>) -> Result<(WorkspaceId, PaneId), ModelError> {
        let name = match name {
            Some(n) => validate_name(n)?,
            None => format!("Workspace {}", self.workspaces.len() + 1),
        };
        let id = self.alloc();
        let tab = self.alloc();
        let pane = self.alloc();
        self.workspaces.push(Workspace {
            id,
            name,
            tabs: vec![Tab {
                id: tab,
                layout: Node::leaf(pane),
                focused: pane,
                name: None,
            }],
            active_tab: 0,
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
        Ok(ws.panes())
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
        let (w, t) = locate(&self.workspaces, pane).ok_or(ModelError::NoSuchPane(pane))?;
        let new = self.alloc();
        let ws = &mut self.workspaces[w];
        let tab = &mut ws.tabs[t];
        tab.layout.split_balanced(pane, new, orient);
        tab.focused = new;
        ws.active_tab = t;
        Ok(new)
    }

    /// Close `pane`. Its tab's last pane closes the tab; the workspace's last
    /// tab closes the workspace.
    pub fn close_pane(&mut self, pane: PaneId) -> Result<PaneClosed, ModelError> {
        let (w, t) = locate(&self.workspaces, pane).ok_or(ModelError::NoSuchPane(pane))?;
        let wid = self.workspaces[w].id;
        let ws = &mut self.workspaces[w];
        let tab = &mut ws.tabs[t];
        let order = tab.layout.panes();
        let layout = std::mem::replace(&mut tab.layout, Node::leaf(0));
        match layout.remove_balanced(pane) {
            Some(rest) => {
                tab.layout = rest;
                if tab.focused == pane {
                    // Focus the previous pane in reading order (or the first).
                    let pos = order.iter().position(|p| *p == pane).unwrap_or(0);
                    let remaining = tab.layout.panes();
                    tab.focused = *remaining.get(pos.saturating_sub(1)).unwrap_or(&remaining[0]);
                }
                Ok(PaneClosed::Refocus(ws.focused()))
            }
            None if ws.tabs.len() > 1 => {
                ws.tabs.remove(t);
                if t < ws.active_tab || (t == ws.active_tab && t > 0) {
                    ws.active_tab -= 1;
                }
                Ok(PaneClosed::Refocus(ws.focused()))
            }
            None => {
                self.close_workspace(wid)?;
                Ok(PaneClosed::WorkspaceRemoved(wid))
            }
        }
    }

    /// Open a tab after the shown one with a single new pane; it is shown and
    /// focused. Returns (tab, pane).
    pub fn new_tab(&mut self, wid: WorkspaceId) -> Result<(TabId, PaneId), ModelError> {
        self.workspace(wid).ok_or(ModelError::NoSuchWorkspace(wid))?;
        let (tab, pane) = (self.alloc(), self.alloc());
        let ws = self.workspace_mut(wid)?;
        let at = ws.active_tab + 1;
        ws.tabs.insert(
            at,
            Tab {
                id: tab,
                layout: Node::leaf(pane),
                focused: pane,
                name: None,
            },
        );
        ws.active_tab = at;
        Ok((tab, pane))
    }

    /// Workspace and index of tab `tab`.
    pub fn locate_tab(&self, tab: TabId) -> Option<(WorkspaceId, usize)> {
        self.workspaces.iter().find_map(|w| w.tab_index(tab).map(|i| (w.id, i)))
    }

    /// Show tab `tab` (and select its workspace). Returns its focused pane.
    pub fn select_tab(&mut self, tab: TabId) -> Result<PaneId, ModelError> {
        let (wid, i) = self.locate_tab(tab).ok_or(ModelError::NoSuchTab(tab))?;
        let ws = self.workspace_mut(wid)?;
        ws.active_tab = i;
        let focused = ws.focused();
        self.active = Some(wid);
        Ok(focused)
    }

    /// Show the tab `delta` places from the shown one (wrapping). Returns its
    /// focused pane.
    pub fn cycle_tab(&mut self, wid: WorkspaceId, delta: isize) -> Result<PaneId, ModelError> {
        let ws = self.workspace_mut(wid)?;
        let n = ws.tabs.len() as isize;
        ws.active_tab = (ws.active_tab as isize + delta).rem_euclid(n) as usize;
        Ok(ws.focused())
    }

    /// Move tab `tab` to position `index` (clamped) of its workspace; the
    /// shown tab stays shown.
    pub fn move_tab(&mut self, tab: TabId, index: usize) -> Result<WorkspaceId, ModelError> {
        let (wid, i) = self.locate_tab(tab).ok_or(ModelError::NoSuchTab(tab))?;
        let ws = self.workspace_mut(wid)?;
        let shown = ws.tab().id;
        let t = ws.tabs.remove(i);
        let to = index.min(ws.tabs.len());
        ws.tabs.insert(to, t);
        ws.active_tab = ws.tab_index(shown).unwrap();
        Ok(wid)
    }

    /// Name tab `tab`; `None` or a blank name clears it (back to the automatic
    /// label).
    pub fn rename_tab(&mut self, tab: TabId, name: Option<&str>) -> Result<(), ModelError> {
        let (wid, i) = self.locate_tab(tab).ok_or(ModelError::NoSuchTab(tab))?;
        let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => Some(validate_name(n)?),
            None => None,
        };
        self.workspace_mut(wid)?.tabs[i].name = name;
        Ok(())
    }

    /// Panes of tab `tab`, which [`Self::close_pane`] on each would close.
    pub fn tab_panes(&self, tab: TabId) -> Result<Vec<PaneId>, ModelError> {
        let (wid, i) = self.locate_tab(tab).ok_or(ModelError::NoSuchTab(tab))?;
        Ok(self.workspace(wid).unwrap().tabs[i].layout.panes())
    }

    /// Swap the positions of two panes in the same workspace. Focus and the
    /// split sizes are unchanged. Swapping a pane with itself is a no-op.
    pub fn swap_panes(&mut self, x: PaneId, y: PaneId) -> Result<WorkspaceId, ModelError> {
        let (wx, tx) = locate(&self.workspaces, x).ok_or(ModelError::NoSuchPane(x))?;
        let (wy, ty) = locate(&self.workspaces, y).ok_or(ModelError::NoSuchPane(y))?;
        if wx != wy {
            return Err(ModelError::DifferentWorkspaces);
        }
        if tx != ty {
            return Err(ModelError::DifferentTabs);
        }
        if x != y {
            self.workspaces[wx].tabs[tx].layout.swap(x, y);
        }
        Ok(self.workspaces[wx].id)
    }

    /// Focus `pane`, showing its tab and workspace.
    pub fn focus(&mut self, pane: PaneId) -> Result<WorkspaceId, ModelError> {
        let (w, t) = locate(&self.workspaces, pane).ok_or(ModelError::NoSuchPane(pane))?;
        let ws = &mut self.workspaces[w];
        ws.active_tab = t;
        ws.tabs[t].focused = pane;
        self.active = Some(ws.id);
        Ok(ws.id)
    }

    pub fn focus_direction(&mut self, dir: Direction) -> Option<PaneId> {
        let ws = self.active.and_then(|a| self.workspaces.iter_mut().find(|w| w.id == a))?;
        let tab = ws.tab_mut();
        let next = tab.layout.neighbor(tab.focused, dir)?;
        tab.focused = next;
        Some(next)
    }

    /// Ensure `next_id` is beyond every id in use (after loading a session).
    pub fn normalize_ids(&mut self) {
        let max = self
            .workspaces
            .iter()
            .flat_map(|w| std::iter::once(w.id).chain(w.tabs.iter().map(|t| t.id)).chain(w.panes()))
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(max);
        // Tabs from sessions saved before tabs existed get their ids now.
        let mut zero = Vec::new();
        for (w, ws) in self.workspaces.iter().enumerate() {
            for (t, tab) in ws.tabs.iter().enumerate() {
                if tab.id == 0 {
                    zero.push((w, t));
                }
            }
        }
        for (w, t) in zero {
            let id = self.alloc();
            self.workspaces[w].tabs[t].id = id;
        }
    }

    /// Structural validity: unique ids, focused pane present, active present.
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for w in &self.workspaces {
            if !seen.insert(w.id) {
                return Err(format!("duplicate id {}", w.id));
            }
            validate_name(&w.name).map_err(|e| e.to_string())?;
            if w.tabs.is_empty() || w.active_tab >= w.tabs.len() {
                return Err(format!("workspace {} has no tabs or an invalid active tab", w.id));
            }
            for t in &w.tabs {
                if t.id == 0 || !seen.insert(t.id) {
                    return Err(format!("missing or duplicate tab id {}", t.id));
                }
                for p in t.layout.panes() {
                    if !seen.insert(p) {
                        return Err(format!("duplicate id {p}"));
                    }
                }
                if !t.layout.contains(t.focused) {
                    return Err(format!("tab {} focuses missing pane", t.id));
                }
                check_ratios(&t.layout)?;
            }
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
        assert_eq!(s.workspace(w).unwrap().tab().layout.panes(), vec![p1, p2, p3]);
        assert_eq!(s.workspace(w).unwrap().focused(), p3);
        assert_eq!(s.close_pane(p3).unwrap(), PaneClosed::Refocus(p2));
        assert_eq!(s.workspace(w).unwrap().tab().layout.panes(), vec![p1, p2]);
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
        let l = &s.active_workspace().unwrap().tab().layout;
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
        assert!(ws.tab_mut().layout.set_ratio_at(&[true], 0.3));
        assert!(ws.tab_mut().layout.set_ratio_at(&[], 0.7));
        assert!(!ws.tab_mut().layout.set_ratio_at(&[false], 0.3)); // leaf
        assert!(s.validate().is_ok());
        let json = serde_json::to_string(&s).unwrap();
        let back: AppState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.workspaces[0].tab().layout, s.workspaces[0].tab().layout);
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
        assert_eq!(widths(&s.workspace(w).unwrap().tab().layout), vec![0.25; 4]);
        // Splitting a middle pane keeps the row equal too.
        s.split(p1, Orient::Horizontal).unwrap();
        assert_eq!(widths(&s.workspace(w).unwrap().tab().layout), vec![0.2; 5]);
        assert!(s.validate().is_ok());
    }

    #[test]
    fn cross_direction_split_only_balances_its_own_line() {
        let (mut s, w, p1) = state_with_ws();
        let p2 = s.split(p1, Orient::Horizontal).unwrap();
        s.workspace_mut(w).unwrap().tab_mut().layout.set_ratio_at(&[], 0.7);
        let p3 = s.split(p2, Orient::Vertical).unwrap();
        s.split(p3, Orient::Vertical).unwrap();
        let rects = s.workspace(w).unwrap().tab().layout.rects();
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
        assert_eq!(widths(&s.workspace(w).unwrap().tab().layout), vec![0.333; 3]);
        s.close_pane(p1).unwrap();
        assert_eq!(widths(&s.workspace(w).unwrap().tab().layout), vec![0.5; 2]);
    }

    #[test]
    fn swap_panes_exchanges_positions_and_keeps_sizes() {
        let (mut s, w, p1) = state_with_ws();
        let p2 = s.split(p1, Orient::Horizontal).unwrap();
        let p3 = s.split(p2, Orient::Vertical).unwrap();
        s.workspace_mut(w).unwrap().tab_mut().layout.set_ratio_at(&[], 0.7);
        let before = s.workspace(w).unwrap().tab().layout.rects();
        let focused = s.workspace(w).unwrap().focused();

        assert_eq!(s.swap_panes(p1, p3).unwrap(), w);
        let ws = s.workspace(w).unwrap();
        assert_eq!(ws.tab().layout.panes(), vec![p3, p2, p1]);
        assert_eq!(ws.focused(), focused);
        let after = ws.tab().layout.rects();
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
        assert_eq!(s.workspace(w).unwrap().tab().layout.panes(), vec![p1, p2]);
    }

    #[test]
    fn validate_rejects_corrupt_state() {
        let (mut s, w, _) = state_with_ws();
        s.workspace_mut(w).unwrap().tab_mut().focused = 12345;
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

    fn tab_ids(s: &AppState, w: WorkspaceId) -> Vec<TabId> {
        s.workspace(w).unwrap().tabs.iter().map(|t| t.id).collect()
    }

    #[test]
    fn new_tab_inserts_after_active_and_selects_it() {
        let (mut s, w, p) = state_with_ws();
        let first = s.workspace(w).unwrap().tab().id;
        let (t1, q1) = s.new_tab(w).unwrap();
        assert_eq!(tab_ids(&s, w), vec![first, t1]);
        assert_eq!(s.workspace(w).unwrap().focused(), q1);
        s.focus(p).unwrap();
        let (t2, _) = s.new_tab(w).unwrap();
        assert_eq!(tab_ids(&s, w), vec![first, t2, t1], "right after the shown tab");
        assert_eq!(s.workspace(w).unwrap().active_tab, 1);
        assert_eq!(s.workspace_of(q1), Some(w));
        assert_eq!(s.all_panes().len(), 3);
        assert!(s.validate().is_ok());
        assert_eq!(s.new_tab(999), Err(ModelError::NoSuchWorkspace(999)));
    }

    #[test]
    fn focus_selects_the_pane_tab_and_cycle_wraps() {
        let (mut s, w, p) = state_with_ws();
        let (_, q) = s.new_tab(w).unwrap();
        let (t2, r) = s.new_tab(w).unwrap();
        s.focus(p).unwrap();
        assert_eq!(s.workspace(w).unwrap().active_tab, 0);
        assert_eq!(s.cycle_tab(w, -1).unwrap(), r);
        assert_eq!(s.workspace(w).unwrap().tab().id, t2);
        assert_eq!(s.cycle_tab(w, 1).unwrap(), p);
        assert_eq!(s.cycle_tab(w, 1).unwrap(), q);
        assert_eq!(s.select_tab(t2).unwrap(), r);
        assert_eq!(s.select_tab(4242), Err(ModelError::NoSuchTab(4242)));
    }

    #[test]
    fn split_and_neighbor_stay_in_one_tab() {
        let (mut s, w, p) = state_with_ws();
        let right = s.split(p, Orient::Horizontal).unwrap();
        let (_, q) = s.new_tab(w).unwrap();
        // Splitting a pane of a hidden tab shows that tab.
        let q2 = s.split(q, Orient::Horizontal).unwrap();
        assert_eq!(s.workspace(w).unwrap().focused(), q2);
        assert_eq!(s.focus_direction(Direction::Left), Some(q));
        assert_eq!(s.focus_direction(Direction::Left), None, "no neighbour in another tab");
        s.focus(right).unwrap();
        assert_eq!(s.focus_direction(Direction::Left), Some(p));
        assert_eq!(s.workspace(w).unwrap().tabs[0].layout.panes(), vec![p, right]);
    }

    #[test]
    fn closing_last_pane_of_a_tab_removes_only_that_tab() {
        let (mut s, w, p) = state_with_ws();
        let (t1, q) = s.new_tab(w).unwrap();
        let (_, r) = s.new_tab(w).unwrap();
        // r's tab is shown; close the background tab of q.
        assert_eq!(s.close_pane(q), Ok(PaneClosed::Refocus(r)));
        assert!(s.workspace(w).unwrap().tab_index(t1).is_none());
        assert_eq!(s.workspace(w).unwrap().focused(), r, "the shown tab stays shown");
        // Closing the shown tab shows the previous one.
        assert_eq!(s.close_pane(r), Ok(PaneClosed::Refocus(p)));
        assert_eq!(s.workspace(w).unwrap().tabs.len(), 1);
        assert!(s.validate().is_ok());
    }

    #[test]
    fn closing_the_last_tab_closes_the_workspace() {
        let (mut s, w, p) = state_with_ws();
        let q = s.split(p, Orient::Vertical).unwrap();
        assert_eq!(s.close_pane(q), Ok(PaneClosed::Refocus(p)));
        assert_eq!(s.close_pane(p), Ok(PaneClosed::WorkspaceRemoved(w)));
        assert!(s.workspace(w).is_none());
    }

    #[test]
    fn move_tab_reorders_and_keeps_the_shown_tab() {
        let (mut s, w, _) = state_with_ws();
        let first = s.workspace(w).unwrap().tab().id;
        let (t1, _) = s.new_tab(w).unwrap();
        let (t2, _) = s.new_tab(w).unwrap();
        assert_eq!(s.move_tab(t2, 0), Ok(w));
        assert_eq!(tab_ids(&s, w), vec![t2, first, t1]);
        assert_eq!(s.workspace(w).unwrap().tab().id, t2, "still shown");
        assert_eq!(s.move_tab(t2, 99), Ok(w));
        assert_eq!(tab_ids(&s, w), vec![first, t1, t2]);
        assert_eq!(s.move_tab(5555, 0), Err(ModelError::NoSuchTab(5555)));
    }

    #[test]
    fn swap_across_tabs_is_rejected() {
        let (mut s, w, p) = state_with_ws();
        let (_, q) = s.new_tab(w).unwrap();
        assert_eq!(s.swap_panes(p, q), Err(ModelError::DifferentTabs));
    }

    #[test]
    fn tab_panes_lists_a_tab() {
        let (mut s, w, _) = state_with_ws();
        let (t, q) = s.new_tab(w).unwrap();
        let q2 = s.split(q, Orient::Vertical).unwrap();
        assert_eq!(s.tab_panes(t).unwrap(), vec![q, q2]);
        assert_eq!(s.locate_tab(t), Some((w, 1)));
    }

    #[test]
    fn validate_rejects_bad_tabs() {
        let (mut s, w, _) = state_with_ws();
        s.workspace_mut(w).unwrap().active_tab = 3;
        assert!(s.validate().is_err());
        s.workspace_mut(w).unwrap().active_tab = 0;
        let dup = s.workspace(w).unwrap().tab().clone();
        s.workspace_mut(w).unwrap().tabs.push(dup);
        assert!(s.validate().is_err(), "duplicate tab");
        let ws = s.workspace_mut(w).unwrap();
        ws.tabs.truncate(1);
        ws.tabs[0].id = 0;
        assert!(s.validate().is_err(), "tab without id");
    }

    #[test]
    fn rename_tab_sets_and_clears_its_name() {
        let (mut s, w, _) = state_with_ws();
        let (t, _) = s.new_tab(w).unwrap();
        assert_eq!(s.workspace(w).unwrap().tabs[1].name, None);
        s.rename_tab(t, Some("  tax rules ")).unwrap();
        assert_eq!(s.workspace(w).unwrap().tabs[1].name.as_deref(), Some("tax rules"));
        assert_eq!(s.rename_tab(t, Some("bad\nname")), Err(ModelError::InvalidName));
        assert_eq!(s.rename_tab(t, Some(&"x".repeat(65))), Err(ModelError::InvalidName));
        s.rename_tab(t, Some("   ")).unwrap();
        assert_eq!(s.workspace(w).unwrap().tabs[1].name, None, "blank clears the name");
        s.rename_tab(t, Some("again")).unwrap();
        s.rename_tab(t, None).unwrap();
        assert_eq!(s.workspace(w).unwrap().tabs[1].name, None);
        assert_eq!(s.rename_tab(777, Some("x")), Err(ModelError::NoSuchTab(777)));
        // Named tabs round-trip; unnamed ones store no name.
        s.rename_tab(t, Some("api")).unwrap();
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["workspaces"][0]["tabs"][1]["name"], "api");
        assert!(json["workspaces"][0]["tabs"][0].get("name").is_none());
        let back: AppState = serde_json::from_value(json).unwrap();
        assert_eq!(back.workspace(w).unwrap().tabs[1].name.as_deref(), Some("api"));
    }
}
