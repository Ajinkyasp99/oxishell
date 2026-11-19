//! Pure, time-injected scheduling for the metadata worker (GTK-free).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::model::PaneId;

pub const PORT_BASE: Duration = Duration::from_secs(9);
pub const PORT_MAX: Duration = Duration::from_secs(30);
pub const PORT_FOLLOWUPS: u8 = 2;
pub const GIT_BASE: Duration = Duration::from_secs(5);
pub const GIT_MAX: Duration = Duration::from_secs(30);

struct PortState {
    tree: Vec<i32>,
    interval: Duration,
    followups: u8,
    /// Records left that keep `PORT_BASE` after a tree change (the immediate
    /// check and its follow-ups do not back off).
    hold: u8,
    last: Instant,
    result: Option<Vec<u16>>,
}

/// Decides when a pane's listening ports need re-scanning.
#[derive(Default)]
pub struct PortSchedule {
    panes: HashMap<PaneId, PortState>,
}

impl PortSchedule {
    /// `tree`: the pane's process-tree pids, sorted ascending.
    pub fn due(&mut self, pane: PaneId, tree: &[i32], force: bool, now: Instant) -> bool {
        let Some(st) = self.panes.get_mut(&pane) else {
            self.panes.insert(
                pane,
                PortState {
                    tree: tree.to_vec(),
                    interval: PORT_BASE,
                    followups: 0,
                    hold: 0,
                    last: now,
                    result: None,
                },
            );
            return true;
        };
        if force {
            return true;
        }
        if st.tree != tree {
            st.tree = tree.to_vec();
            st.interval = PORT_BASE;
            st.followups = PORT_FOLLOWUPS;
            st.hold = PORT_FOLLOWUPS + 1;
            return true;
        }
        if st.followups > 0 {
            st.followups -= 1;
            return true;
        }
        now.saturating_duration_since(st.last) >= st.interval
    }

    pub fn record(&mut self, pane: PaneId, ports: &[u16], now: Instant) {
        let Some(st) = self.panes.get_mut(&pane) else {
            return;
        };
        st.interval = match &st.result {
            Some(prev) if st.hold == 0 && prev.as_slice() == ports => (st.interval * 2).min(PORT_MAX),
            _ => PORT_BASE,
        };
        st.hold = st.hold.saturating_sub(1);
        st.result = Some(ports.to_vec());
        st.last = now;
    }

    pub fn forget(&mut self, pane: PaneId) {
        self.panes.remove(&pane);
    }

    pub fn len(&self) -> usize {
        self.panes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }
}

/// Per-cwd git result cache whose TTL doubles while the value is unchanged.
pub struct GitBackoff<V> {
    entries: HashMap<PathBuf, (Instant, Duration, V)>,
}

impl<V: Clone + PartialEq> Default for GitBackoff<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V: Clone + PartialEq> GitBackoff<V> {
    pub fn new() -> Self {
        Self { entries: HashMap::new() }
    }

    /// A prompt was drawn in some pane: a command there may have changed any
    /// repo, so no cached entry may stay backed off longer than `GIT_BASE`.
    /// Values are kept.
    pub fn reset_ttls(&mut self) {
        for (_, ttl, _) in self.entries.values_mut() {
            *ttl = (*ttl).min(GIT_BASE);
        }
    }

    /// `fixed`: pane has no shell integration (ttl stays GIT_BASE).
    /// `fresh`: a prompt was drawn since the last job (bypass the cache and
    /// start the back-off again from GIT_BASE).
    pub fn get(&mut self, cwd: &Path, fixed: bool, fresh: bool, now: Instant, make: impl FnOnce() -> V) -> V {
        if let Some((checked, ttl, value)) = self.entries.get(cwd) {
            let ttl = if fixed { (*ttl).min(GIT_BASE) } else { *ttl };
            if !fresh && now.saturating_duration_since(*checked) < ttl {
                return value.clone();
            }
        }
        let value = make();
        let ttl = match self.entries.get(cwd) {
            Some((_, ttl, old)) if !fixed && !fresh && *old == value => (*ttl * 2).min(GIT_MAX),
            _ => GIT_BASE,
        };
        self.entries.insert(cwd.to_path_buf(), (now, ttl, value.clone()));
        if self.entries.len() > 256 {
            self.entries
                .retain(|_, (checked, _, _)| now.saturating_duration_since(*checked) <= GIT_MAX);
        }
        value
    }
}

/// Per-pane env-file modification time tracker.
#[derive(Default)]
pub struct PromptWatch {
    panes: HashMap<PaneId, Option<SystemTime>>,
}

impl PromptWatch {
    /// Returns (integrated, prompt_since_last).
    pub fn observe(&mut self, pane: PaneId, mtime: Option<SystemTime>) -> (bool, bool) {
        let changed = match self.panes.insert(pane, mtime) {
            None => false,
            Some(prev) => prev != mtime && mtime.is_some(),
        };
        (mtime.is_some(), changed)
    }

    pub fn forget(&mut self, pane: PaneId) {
        self.panes.remove(&pane);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn pane() -> PaneId {
        PaneId::default()
    }

    #[test]
    fn new_pane_is_due_then_waits_base_interval() {
        let (mut p, t0, id) = (PortSchedule::default(), Instant::now(), pane());
        assert!(p.due(id, &[1], false, t0));
        p.record(id, &[], t0);
        assert!(!p.due(id, &[1], false, t0 + s(8)));
        assert!(p.due(id, &[1], false, t0 + s(9)));
    }

    #[test]
    fn unchanged_result_backs_off_to_30s() {
        let (mut p, t0, id) = (PortSchedule::default(), Instant::now(), pane());
        let mut now = t0;
        assert!(p.due(id, &[1], false, now));
        p.record(id, &[80], now);
        for step in [9, 18, 30, 30] {
            assert!(!p.due(id, &[1], false, now + s(step - 1)));
            now += s(step);
            assert!(p.due(id, &[1], false, now));
            p.record(id, &[80], now);
        }
    }

    #[test]
    fn changed_result_resets_interval() {
        let (mut p, t0, id) = (PortSchedule::default(), Instant::now(), pane());
        let mut now = t0;
        p.due(id, &[1], false, now);
        for _ in 0..4 {
            p.record(id, &[80], now);
            now += s(60);
        }
        p.record(id, &[81], now);
        assert!(!p.due(id, &[1], false, now + s(8)));
        assert!(p.due(id, &[1], false, now + s(9)));
    }

    #[test]
    fn tree_change_is_due_now_and_twice_more() {
        let (mut p, t0, id) = (PortSchedule::default(), Instant::now(), pane());
        p.due(id, &[1], false, t0);
        p.record(id, &[80], t0);
        p.record(id, &[80], t0);
        assert!(p.due(id, &[1, 2], false, t0 + s(1)));
        assert!(p.due(id, &[1, 2], false, t0 + s(2)));
        assert!(p.due(id, &[1, 2], false, t0 + s(3)));
        assert!(!p.due(id, &[1, 2], false, t0 + s(4)));
    }

    #[test]
    fn tree_change_keeps_base_interval_through_followups() {
        let (mut p, t0, id) = (PortSchedule::default(), Instant::now(), pane());
        let mut now = t0;
        assert!(p.due(id, &[1], false, now));
        p.record(id, &[80], now);
        for step in [9, 18, 30] {
            now += s(step);
            assert!(p.due(id, &[1], false, now));
            p.record(id, &[80], now);
        }
        // Backed off to 30 s; a process starts: the immediate check and two
        // follow-ups all find the same ports.
        for _ in 0..=PORT_FOLLOWUPS {
            now += s(3);
            assert!(p.due(id, &[1, 2], false, now));
            p.record(id, &[80], now);
        }
        assert!(!p.due(id, &[1, 2], false, now + s(8)));
        assert!(p.due(id, &[1, 2], false, now + s(9)));
        // From there the normal back-off applies again.
        now += s(9);
        p.record(id, &[80], now);
        assert!(!p.due(id, &[1, 2], false, now + s(17)));
        assert!(p.due(id, &[1, 2], false, now + s(18)));
    }

    #[test]
    fn force_is_always_due() {
        let (mut p, t0, id) = (PortSchedule::default(), Instant::now(), pane());
        p.due(id, &[1], false, t0);
        p.record(id, &[], t0);
        assert!(p.due(id, &[1], true, t0 + s(1)));
    }

    #[test]
    fn forget_drops_state() {
        let (mut p, t0, id) = (PortSchedule::default(), Instant::now(), pane());
        p.due(id, &[1], false, t0);
        p.record(id, &[], t0);
        assert_eq!(p.len(), 1);
        p.forget(id);
        assert_eq!(p.len(), 0);
        assert!(p.due(id, &[1], false, t0 + s(1)));
    }

    fn git_run(g: &mut GitBackoff<u32>, now: Instant, v: u32, fixed: bool, fresh: bool) -> bool {
        let mut called = false;
        g.get(Path::new("/r"), fixed, fresh, now, || {
            called = true;
            v
        });
        called
    }

    #[test]
    fn git_backs_off_while_unchanged() {
        let (mut g, t0) = (GitBackoff::new(), Instant::now());
        let mut now = t0;
        assert!(git_run(&mut g, now, 1, false, false));
        for step in [5, 10, 20, 30, 30] {
            assert!(!git_run(&mut g, now + s(step - 1), 1, false, false));
            now += s(step);
            assert!(git_run(&mut g, now, 1, false, false));
        }
    }

    #[test]
    fn git_change_resets_to_5s() {
        let (mut g, t0) = (GitBackoff::new(), Instant::now());
        let mut now = t0;
        git_run(&mut g, now, 1, false, false);
        for step in [5, 10] {
            now += s(step);
            git_run(&mut g, now, 1, false, false);
        }
        now += s(20);
        assert!(git_run(&mut g, now, 2, false, false));
        assert!(!git_run(&mut g, now + s(4), 2, false, false));
        assert!(git_run(&mut g, now + s(5), 2, false, false));
    }

    #[test]
    fn git_fixed_stays_5s() {
        let (mut g, t0) = (GitBackoff::new(), Instant::now());
        let mut now = t0;
        for _ in 0..4 {
            assert!(git_run(&mut g, now, 1, true, false));
            assert!(!git_run(&mut g, now + s(4), 1, true, false));
            now += s(5);
        }
    }

    #[test]
    fn env_file_change_forces_git_refresh() {
        let (mut g, t0) = (GitBackoff::new(), Instant::now());
        git_run(&mut g, t0, 1, false, false);
        assert!(!git_run(&mut g, t0 + s(1), 1, false, false));
        assert!(git_run(&mut g, t0 + s(1), 1, false, true));
    }

    #[test]
    fn fresh_resets_ttl_to_base_even_when_unchanged() {
        let (mut g, t0) = (GitBackoff::new(), Instant::now());
        let mut now = t0;
        git_run(&mut g, now, 1, false, false);
        for step in [5, 10, 20, 30] {
            now += s(step);
            assert!(git_run(&mut g, now, 1, false, false));
        }
        // At the 30 s cap; a prompt brings the same value back.
        now += s(1);
        assert!(git_run(&mut g, now, 1, false, true));
        assert!(!git_run(&mut g, now + s(4), 1, false, false));
        assert!(git_run(&mut g, now + s(5), 1, false, false));
    }

    fn back_off_to_cap(g: &mut GitBackoff<u32>, cwd: &str, t0: Instant) -> Instant {
        let mut now = t0;
        g.get(Path::new(cwd), false, false, now, || 1);
        for step in [5, 10, 20, 30] {
            now += s(step);
            g.get(Path::new(cwd), false, false, now, || 1);
        }
        now
    }

    #[test]
    fn reset_ttls_caps_every_entry_to_base() {
        let (mut g, t0) = (GitBackoff::new(), Instant::now());
        let a = back_off_to_cap(&mut g, "/a", t0);
        let b = back_off_to_cap(&mut g, "/b", t0);
        assert_eq!(a, b);
        g.reset_ttls();
        for cwd in ["/a", "/b"] {
            let mut called = false;
            g.get(Path::new(cwd), false, false, a + s(4), || {
                called = true;
                1
            });
            assert!(!called, "{cwd} must still be cached at +4 s");
            g.get(Path::new(cwd), false, false, a + s(5), || {
                called = true;
                1
            });
            assert!(called, "{cwd} must be re-checked at +5 s");
        }
    }

    #[test]
    fn fixed_ignores_a_backed_off_entry() {
        let (mut g, t0) = (GitBackoff::new(), Instant::now());
        let now = back_off_to_cap(&mut g, "/r", t0);
        let mut called = false;
        g.get(Path::new("/r"), true, false, now + s(5), || {
            called = true;
            1
        });
        assert!(called, "fixed lookup must not reuse a 30 s entry after 5 s");
    }

    #[test]
    fn prompt_watch() {
        let (mut w, id) = (PromptWatch::default(), pane());
        let t1 = SystemTime::UNIX_EPOCH + s(1);
        let t2 = SystemTime::UNIX_EPOCH + s(2);
        assert_eq!(w.observe(id, Some(t1)), (true, false));
        assert_eq!(w.observe(id, Some(t1)), (true, false));
        assert_eq!(w.observe(id, Some(t2)), (true, true));
        assert_eq!(w.observe(id, None), (false, false));
    }
}
