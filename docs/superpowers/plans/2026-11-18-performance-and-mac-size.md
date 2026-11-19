# Idle work, startup, memory and macOS download size — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cut oxishell's idle helper processes and wakeups, make history import linear, build Compose
stack widgets on demand, and shrink the macOS DMG (no cursor theme, no ICU, GTK-free `oxi`), with no
user-visible behaviour change.

**Architecture:** Scheduling decisions (port and git back-off) move into a new GTK-free module
`src/meta_schedule.rs` with pure, time-injected unit tests; `ui/meta_worker.rs` consults it and does one
batched port lookup per batch. Window visibility is tracked once on `App` and drives the status strip and
the metadata tick. Packaging changes live in `build.rs`, `scripts/build-macos.sh` and the two workflows.

**Tech Stack:** Rust 2021 (MSRV 1.87), gtk4-rs 0.9 (`v4_14`), libadwaita 0.7, vte4 0.8 (`v0_76`),
bash packaging scripts, GitHub Actions (macos-15, macos-15-intel), meson/ninja for VTE.

**Spec:** `docs/superpowers/specs/2026-11-18-performance-and-mac-size-design.md`

## Global Constraints

- **Never run `git commit`, `git commit --amend` or `git push`.** Leave every change uncommitted; the owner
  commits. Read-only git (status, diff, log) is fine. No AI attribution anywhere.
- Both platforms: OS-specific code only in `crate::platform` / `crate::meta` or behind
  `#[cfg(target_os = "...")]`. No `/proc` reads from shared code.
- Nothing in the meta worker runs on the GTK thread; per-pane job coalescing and one ppid table per batch
  stay.
- Values pinned by the spec: ports 9 s → 18 s → 30 s cap, 2 follow-up checks after a tree change; git TTL
  5 s → 10 → 20 → 30 s cap, fixed 5 s without shell integration; Compose `commands` TTL 10 min, `roots`
  5 min, `files` 30 s doubling to a 5 min cap; shallow automatic scan `max_depth: 1` only when the scan
  root is exactly `$HOME` or `/`; history target < 50 ms for 20,000 and 50,000 unique entries (release);
  VTE 0.84.1; switch `VTE_SOURCE=1` default on.
- Linux checks before a task counts as done: `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test --lib --bins`. GUI suites (Tasks 4, 6, 7, 8 and
  the end): `dbus-run-session -- xvfb-run -a cargo test --test e2e --test interactions -- --test-threads=1`.
- macOS compile check on Linux after every Rust task:
  `PKG_CONFIG_ALLOW_CROSS=1 CARGO_TARGET_DIR=target/macos-check cargo clippy --target aarch64-apple-darwin --all-targets -- -D warnings`.
  If the target or SDK is missing on this machine, say so in the task report; do not skip silently.
- No new shortcuts.

## Review Focus

1. **Server started in the background from an idle prompt** (`python3 -m http.server 38420 &`): its port
   must appear within ~7 s (tree change + follow-ups). Pinned by the e2e check in Task 4.
2. **A pane closed while its port/git state is cached**: no stale state is reused by a new pane that gets
   the same `PaneId`, and the maps do not grow without bound. Pinned by `forget_drops_state` in Task 2 and
   the pruning step in Task 4.
3. **`git commit` in a pane whose git TTL has grown to 30 s**: the branch/changes chip updates within one
   tick. Pinned by `env_file_change_forces_git_refresh` (Task 2) and the e2e step in Task 4.
4. **History files with more than 20,000 unique entries, timed and untimed duplicates mixed**: identical
   result to the old algorithm. Pinned by the oracle tests in Task 3.
5. **Window minimized while a Compose stack runs**: crash detection keeps the 3 s tick. Pinned by the
   `request_meta` hidden-mode unit check in Task 6; on-screen minimize is checked by hand (Xvfb has no WM).

---

### Task 0: Baseline measurements and helper-launch counter in `scripts/bench.sh`

**Files:**
- Modify: `scripts/bench.sh`

**Interfaces:**
- Produces: a new bench output line `child processes started per minute, N idle panes in a git repo: <n>`
  for N = 1 and 4, used by later tasks for before/after numbers.

- [ ] **Step 1: Add the counter.** After the existing memory/CPU sections, start a fresh app whose first
  pane's cwd is a temporary git repo (`git init` in `$TMP/repo`, one commit, set `[terminal]` cwd or `cd`
  via `oxi pane send`). Count processes whose ancestry includes the app's PID over 60 s by sampling
  `/proc/*/stat` start times every 100 ms, or with `strace -f -qq -e trace=execve -p $PID -o …` when
  `strace` is available (builder's choice; print which method was used). Repeat with 4 panes
  (`oxi pane split` ×3).
- [ ] **Step 2: Record the baseline.** Run `cargo build --release && xvfb-run -a scripts/bench.sh` on the
  unchanged code and save the full output to
  `docs/superpowers/plans/2026-11-18-baseline-bench.txt`.
- [ ] **Step 3: Leave uncommitted.**

---

### Task 1: Batched listening-port lookup (both platforms)

**Files:**
- Modify: `src/platform/mod.rs` (parser + test), `src/platform/macos.rs:120-135`, `src/meta.rs:13,153-180`
  (and its tests near `:297`)

**Interfaces:**
- Produces:
  - `pub fn parse_lsof_listen_by_pid(out: &str) -> HashMap<i32, Vec<u16>>` in `src/platform/mod.rs`
    (each `n` line belongs to the most recent `p<pid>` line; ports sorted, deduplicated per pid; pids with
    no LISTEN lines are absent).
  - `pub fn listening_ports_by_pid(pids: &[i32]) -> HashMap<i32, Vec<u16>>` re-exported from `crate::meta`
    on both platforms. macOS: one `/usr/sbin/lsof -nP -w -a -iTCP -sTCP:LISTEN -p <pids,> -Fpn`. Linux:
    `/proc/net/tcp{,6}` read once, then each pid's `fd` dir.
  - `listening_ports(pids)` stays, implemented as the sorted, deduplicated union of
    `listening_ports_by_pid(pids)`.

- [ ] **Step 1: Write the failing tests**

```rust
// src/platform/mod.rs tests
#[test]
fn lsof_listen_ports_by_pid() {
    let out = "p100\nf5\nn*:3000\nf6\nn[::1]:3000\np200\nf7\nn127.0.0.1:5173\np300\n";
    let m = parse_lsof_listen_by_pid(out);
    assert_eq!(m.get(&100), Some(&vec![3000]));
    assert_eq!(m.get(&200), Some(&vec![5173]));
    assert!(!m.contains_key(&300));
    assert!(parse_lsof_listen_by_pid("").is_empty());
}

// src/meta.rs tests
#[test]
fn listening_ports_by_pid_attributes_to_owner() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let me = std::process::id() as i32;
    let m = listening_ports_by_pid(&[me, 1]);
    assert!(m.get(&me).is_some_and(|v| v.contains(&port)));
    assert!(m.get(&1).is_none_or(|v| !v.contains(&port)));
}
```

- [ ] **Step 2: Run them to make sure they fail.** `cargo test --lib by_pid` → FAIL (functions not defined).
- [ ] **Step 3: Implement** the three functions above; delete nothing else.
- [ ] **Step 4: Run** `cargo test --lib by_pid detects_listening_port` → PASS; then the Linux checks and the
  macOS compile check from Global Constraints.
- [ ] **Step 5: Leave uncommitted.**

---

### Task 2: Port and git schedules (`src/meta_schedule.rs`, GTK-free)

**Files:**
- Create: `src/meta_schedule.rs`
- Modify: `src/lib.rs` (add `pub mod meta_schedule;`)

**Interfaces:**
- Produces (all time is passed in as `Instant` so tests control it):

```rust
pub const PORT_BASE: Duration = Duration::from_secs(9);
pub const PORT_MAX: Duration = Duration::from_secs(30);
pub const PORT_FOLLOWUPS: u8 = 2;
pub const GIT_BASE: Duration = Duration::from_secs(5);
pub const GIT_MAX: Duration = Duration::from_secs(30);

#[derive(Default)] pub struct PortSchedule { /* PaneId -> state */ }
impl PortSchedule {
    /// `tree`: the pane's process-tree pids, sorted ascending.
    pub fn due(&mut self, pane: PaneId, tree: &[i32], force: bool, now: Instant) -> bool;
    pub fn record(&mut self, pane: PaneId, ports: &[u16], now: Instant);
    pub fn forget(&mut self, pane: PaneId);
    pub fn len(&self) -> usize;
}

pub struct GitBackoff<V> { /* PathBuf -> (checked: Instant, ttl: Duration, value: V) */ }
impl<V: Clone + PartialEq> GitBackoff<V> {
    pub fn new() -> Self;
    /// `fixed`: pane has no shell integration (ttl stays GIT_BASE).
    /// `fresh`: a prompt was drawn since the last job (bypass the cache).
    pub fn get(&mut self, cwd: &Path, fixed: bool, fresh: bool, now: Instant, make: impl FnOnce() -> V) -> V;
}

/// Per-pane env-file modification time tracker.
#[derive(Default)] pub struct PromptWatch { /* PaneId -> Option<SystemTime> */ }
impl PromptWatch {
    /// Returns (integrated, prompt_since_last): integrated = the file exists;
    /// prompt_since_last = its mtime differs from the one seen on the previous call
    /// (false on the first call for a pane).
    pub fn observe(&mut self, pane: PaneId, mtime: Option<SystemTime>) -> (bool, bool);
    pub fn forget(&mut self, pane: PaneId);
}
```

`PortSchedule::due` decides in this order (this is the part the tests pin):

```text
no state for pane            -> create {tree, interval: PORT_BASE, followups: 0}; true
force                        -> true
tree != state.tree           -> state.tree = tree; interval = PORT_BASE; followups = PORT_FOLLOWUPS; true
followups > 0                -> followups -= 1; true
now - state.last >= interval -> true
otherwise                    -> false
```

`record`: if `ports` differs from the last result → `interval = PORT_BASE`, else
`interval = min(interval * 2, PORT_MAX)`; store `ports` and `last = now`.
`GitBackoff::get`: reuse when `!fresh && now - checked < ttl`; otherwise call `make`; new ttl is `GIT_BASE`
if `fixed` or the value changed, else `min(ttl * 2, GIT_MAX)`. When more than 256 entries, drop those
older than `GIT_MAX` (as today's `git_cache.retain`).

- [ ] **Step 1: Write the failing tests** (in `src/meta_schedule.rs`):
  - `new_pane_is_due_then_waits_base_interval`: `due` at t0 → true; `record([])`; at t0+8 s → false;
    t0+9 s → true.
  - `unchanged_result_backs_off_to_30s`: record the same ports at 9 s, then 18 s; intervals observed are
    9, 18, 30, 30 (due exactly at those offsets, not 1 s before).
  - `changed_result_resets_interval`: after backing off to 30 s, record different ports → next due at +9 s.
  - `tree_change_is_due_now_and_twice_more`: after a back-off, pass a different tree → true, and the next
    two calls 1 s apart → true, the third → false.
  - `force_is_always_due`.
  - `forget_drops_state`: after `forget`, `due` for that pane is true and `len()` decreased.
  - `git_backs_off_while_unchanged` (5, 10, 20, 30, 30 s), `git_change_resets_to_5s`,
    `git_fixed_stays_5s`, `env_file_change_forces_git_refresh` (`fresh = true` calls `make` even inside ttl).
  - `prompt_watch`: first observe of `Some(t1)` → (true, false); `Some(t1)` again → (true, false);
    `Some(t2)` → (true, true); `None` → (false, false).
- [ ] **Step 2: Run** `cargo test --lib meta_schedule` → FAIL (module missing).
- [ ] **Step 3: Implement** the module as specified.
- [ ] **Step 4: Run** `cargo test --lib meta_schedule` → PASS; Linux checks; macOS compile check.
- [ ] **Step 5: Leave uncommitted.**

---

### Task 3: Linear history import

**Files:**
- Modify: `src/history.rs:142-160` (`from_oldest_timed`), tests in the same file

**Interfaces:**
- `History::from_oldest_timed(lists: Vec<Vec<Timed>>) -> History` keeps its signature and result.

New algorithm (walk newest first; the newest occurrence decides the time, as today's last `push_at` does):

```text
seen = HashSet
for list in lists.iter().rev():
  for (cmd, t) in list.iter().rev():
    c = clean(cmd) else continue
    if seen.insert(c.clone()):
      entries.push(c.clone())          // entries are stored newest first
      if let Some(t) = t: times.insert(c, t)
      if entries.len() == MAX_ENTRIES: return
```

- [ ] **Step 1: Write the failing tests.** Move today's loop into the test module as
  `fn oracle(lists: Vec<Vec<Timed>>) -> History` (built with `push_at`, exactly the old code). Tests compare
  `entries` and `time_of` for every entry between `oracle` and `from_oldest_timed`:
  - `matches_oracle_with_duplicates_across_and_within_files`
  - `matches_oracle_timed_then_untimed_and_untimed_then_timed` (same command `ls` as `(ls, Some(1))`
    then `(ls, None)`, and the reverse)
  - `matches_oracle_with_rejected_entries` (blank and whitespace-only lines that `clean` drops)
  - `matches_oracle_beyond_max_entries` (`MAX_ENTRIES + 500` unique, with some duplicates of the oldest)
  - `import_is_fast` (`#[ignore]`, run with `--release -- --ignored`): 20,000 and 50,000 unique entries each
    take < 50 ms; print both times.
  The oracle tests pass against the old code; `import_is_fast` fails against it.
- [ ] **Step 2: Run** `cargo test --release --lib history -- --include-ignored` → `import_is_fast` FAILS
  (≈0.75 s / ≈3 s).
- [ ] **Step 3: Implement** the algorithm above in `from_oldest_timed`. `push_at` stays for live commands.
- [ ] **Step 4: Run** the same command → all PASS; record the two printed times for the docs. If either is
  ≥ 50 ms, stop and report (the spec says do not move loading to a thread without asking).
- [ ] **Step 5: Leave uncommitted.**

---

### Task 4: Meta worker uses the schedules; batched ports; git back-off

**Files:**
- Modify: `src/ui/meta_worker.rs:16-30` (`Job`), `:170-250` (batch loop), `src/ui/mod.rs:523-537`
  (`select_workspace`), `:985-993` (focus), `:1567` (`command_finished`), `:1918-1960`
  (`request_meta`, `meta_job`), `src/ui/pane.rs:648`, `tests/e2e.rs:331-345`

**Interfaces:**
- Consumes: Task 1 `meta::listening_ports_by_pid`; Task 2 `PortSchedule`, `GitBackoff`, `PromptWatch`.
- Produces:
  - `Job` gains `pub force_ports: bool` and `pub compose_only: bool` (both `false` from `meta_job`).
    `compose_only` = skip ports and git; everything else as usual (used by Task 6).
  - `App::request_meta(&self, active_only: bool, force_ports: bool)` (was one bool). Callers:
    3 s/15 s tick `(…, false)`; `select_workspace` `(true, true)`; focus change and pane spawn
    `(true, false)`.
  - `App::request_pane_meta(&self, pane: PaneId)`: sends one ordinary job for that pane.

- [ ] **Step 1: Write the failing e2e check.** In `tests/e2e.rs` Phase 6, after the existing foreground
  `http.server 38419` check and its Ctrl+C, wait for the prompt, then send
  `python3 -m http.server 38420 --bind 127.0.0.1 &\r` and assert port 38420 appears within **8 s**; then
  `kill %1`. Also, in a pane `cd`'d into a clean temp git repo (one commit), wait 35 s idle so the git TTL
  reaches its 30 s cap, send `touch newfile\r`, and assert the pane's `git.changes` goes from 0 to 1
  within 4 s.
- [ ] **Step 2: Run** the GUI suites; the background-server assertion may already pass on today's 3 s
  cadence — that is fine, it guards the new schedule. Note the result.
- [ ] **Step 3: Implement the worker batch loop:**
  1. Coalesce jobs (unchanged). Build the ppid table iff any job has `ports && !compose_only`.
  2. For each live job with ports wanted: `tree = process_tree_in(table, shell_pid)` sorted;
     if `ports_sched.due(pane, &tree, job.force_ports, now)` collect `(pane, tree)`.
  3. One `meta::listening_ports_by_pid(all due pids)` call when anything is due; per due pane, union its
     tree's ports (sorted, dedup), `ports_sched.record(...)`, `Some(ports)`. Not due → `ports: None`.
  4. Git: `(integrated, prompted) = prompt_watch.observe(pane, mtime(env_file))`;
     `git_backoff.get(cwd, !integrated, prompted, now, || meta::git_info(cwd))`. `Update.git` becomes
     `Option<Option<GitInfo>>`: outer `None` = not looked up (`compose_only` jobs), and `attach` then
     leaves `p.git` untouched; `Some(g)` behaves as today's `g`.
  5. When a job's shell is dead (`kill(pid, 0) != 0`): `ports_sched.forget(pane)`,
     `prompt_watch.forget(pane)`. Also, if `ports_sched.len() > 512`, rebuild it empty.
- [ ] **Step 4: Wire the App side:** `request_meta(active_only, force_ports)` sets `job.force_ports`;
  `select_workspace` passes `force_ports = true`; `command_finished` calls `self.request_pane_meta(pane)`
  before its early returns (first line after the `contains_key` check).
- [ ] **Step 5: Run** unit tests, the GUI suites (both new checks PASS, existing port check at 15 s
  PASS), clippy, the macOS compile check.
- [ ] **Step 6: Measure.** `cargo build --release && xvfb-run -a scripts/bench.sh`; compare the
  "child processes started per minute" lines with the Task 0 baseline and note them.
- [ ] **Step 7: Leave uncommitted.**

---

### Task 5: Compose discovery TTLs, back-off and shallow `$HOME`/`/` scan

**Files:**
- Modify: `src/ui/meta_worker.rs:44-125` (`ComposeCache`) and its tests at `:345-383`

**Interfaces:**
- Produces:

```rust
pub struct ComposeTtls { pub commands: Duration, pub roots: Duration, pub files_base: Duration, pub files_max: Duration }
impl Default for ComposeTtls { /* 10 min, 5 min, 30 s, 5 min */ }
impl ComposeTtls { pub fn zero() -> Self; /* all Duration::ZERO, for tests */ }
struct ComposeCache { ttls: ComposeTtls, home: PathBuf /* crate::paths::home() */, files: HashMap<PathBuf, FilesEntry>, /* roots, commands, declared, sent as today */ }
struct FilesEntry { at: Instant, ttl: Duration, files: Vec<ComposeFile>, deep: bool }
```

`files` lookup for `root` (this is what the tests pin):

```text
shallow_root = root == home || root == "/"
if rescan or no entry or entry.at.elapsed() >= entry.ttl:
    deep   = rescan || entry.deep (false if no entry)
    limits = if shallow_root && !deep { Limits { max_depth: 1, ..Default } } else { Limits::default() }
    found  = compose::scan_limited(&root, &limits)
    ttl    = if rescan || no entry || found != entry.files { files_base } else { min(entry.ttl * 2, files_max) }
    store FilesEntry { at: now, ttl, files: found, deep }
```

`roots` and `commands` use the existing `cached()` helper with their own TTLs. `declared` keeps
`files_base`.

- [ ] **Step 1: Write the failing tests** (replace the two existing ones, which construct `ttl:` directly):
  - `compose_files_added_later_are_found_without_a_rescan` (with `ComposeTtls::zero()`).
  - `the_scan_cache_is_reused_within_its_lifetime` (default TTLs; rescan bypasses).
  - `files_ttl_doubles_while_unchanged_and_resets_on_change`: inspect `FilesEntry.ttl` after successive
    expired lookups with `files_base = 1 ms`, `files_max = 4 ms`: 1, 2, 4, 4 ms; after adding a compose file
    → back to 1 ms.
  - `home_is_scanned_shallow_until_a_rescan`: `home` = a temp dir with `a/b/docker-compose.yml`
    (depth 2) → automatic scan finds nothing; `rescan = true` finds it; a following automatic (expired)
    scan still finds it.
  - `other_folders_are_scanned_deep`: same layout under a non-home temp dir → found without a rescan.
  - `command_detection_is_cached_for_ten_minutes`: `ComposeTtls::default().commands == 600 s`.
- [ ] **Step 2: Run** `cargo test --lib meta_worker` → FAIL (types missing).
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --lib meta_worker` → PASS; Linux checks; macOS compile check.
- [ ] **Step 5: Leave uncommitted.**

---

### Task 6: Hidden-window mode (status strip + metadata tick)

**Files:**
- Modify: `src/ui/system_status.rs:1-35`, `src/ui/mod.rs` (App struct near `:64`, construction near
  `:283` and after the window is built, `request_meta`), `src/ui/meta_worker.rs:333-345` (tick)

**Interfaces:**
- Consumes: Task 4 `Job.compose_only`, `request_meta(active_only, force_ports)`.
- Produces:
  - `pub(super) struct SystemStatus { pub root: gtk4::Box, … }` from `system_status::new()`, with
    `pub fn set_hidden(&self, hidden: bool)`: while hidden the 2 s timer neither samples nor redraws; on
    `false` it samples and redraws at once.
  - `App.hidden: Cell<bool>`, `App::is_hidden(&self) -> bool`, `App::set_hidden(self: &Rc<Self>, hidden: bool)`
    (no-op when unchanged; calls `system_status.set_hidden`; on `false` calls `request_meta(true, true)`).
  - `fn window_hidden(w: &gtk4::Window) -> bool` = `w.is_suspended()` or the surface is a `gdk::Toplevel`
    whose `state()` contains `gdk::ToplevelState::MINIMIZED`.
  - `App::request_hidden_meta(&self)`: jobs only for panes of the visible workspace whose
    `compose.stack.is_some()`, with `compose_only = true`.
  - `fn hidden_jobs(state: &AppState, panes_with_stack: &HashSet<PaneId>) -> Vec<PaneId>` (pure helper in
    `ui/mod.rs` or `meta_worker.rs`) so the selection is unit-testable.

- [ ] **Step 1: Write the failing test** `hidden_tick_only_refreshes_running_stacks`: with an `AppState`
  holding one workspace of three panes and a stack set `{p2}`, `hidden_jobs` returns `[p2]`; with an
  empty set it returns `[]`.
- [ ] **Step 2: Run** it → FAIL.
- [ ] **Step 3: Implement.** In the tick: `if a.is_hidden() && active_only_tick { a.request_hidden_meta() }
  else { a.request_meta(active_only_tick, false) }`; the 15 s full pass runs regardless. Connect
  `window.connect_suspended_notify` and, in `connect_realize`, the surface's `notify::state`; both call
  `set_hidden(window_hidden(&window))`.
- [ ] **Step 4: Run** unit tests, GUI suites, clippy, macOS compile check.
- [ ] **Step 5: Check by hand on a Linux desktop** (Xvfb has no window manager): minimize, wait 10 s,
  restore; the CPU chart shows a gap and refreshes immediately on restore; `strace -f -e trace=execve` on
  the app shows no `git` runs from the visible workspace while minimized except at the 15 s pass. Note the
  result; macOS minimize is checked by hand on a Mac later.
- [ ] **Step 6: Leave uncommitted.**

---

### Task 7: Build Compose stack and logs bars on first use

**Files:**
- Modify: `src/ui/compose_ui.rs:26-175` (`PaneCompose`), `:270-320` (`sync`), `:383-440`
  (`draw_chips`, `ensure_clock`), `:1135-1200` (`Clip`), `src/ui/pane.rs:420-421`

**Interfaces:**
- Produces:
  - `PaneCompose` keeps `button`, `logs_clip`, `stack_clip` (empty, hidden clips placed under the
    terminal exactly where the bars are today) and state fields; the widgets
    `stack_bar, logs_bar, dot, status, chips, chips_key, bar_buttons, logs_label` move into
    `struct Bars` held as `bars: RefCell<Option<Bars>>`.
  - `fn bars(&self) -> std::cell::Ref<'_, Bars>`: creates the bars on first call (the body of today's
    constructor code for them), puts each bar into its clip with a new `Clip::set_child(&gtk4::Widget)`,
    and binds visibility as `clip()` does today.
  - `sync`: when neither `logs` nor `stack` is shown and `bars` is `None`, it touches nothing.
  - `pub rebuild_btn` is unused outside this file: drop the field (it stays in `bar_buttons`).
- [ ] **Step 1: Write the failing test** in `tests/interactions.rs`, next to the existing Compose tests:
  `compose_bars_are_built_only_when_a_stack_starts`: a fresh pane's `stack_clip` has no child; after the
  existing test helper starts a stack (reuse the setup of `a_crashing_service_is_reported_once`), the
  clip has a child and the bar is visible. Expose a `#[doc(hidden)] pub fn has_bars(&self) -> bool` if
  the test cannot reach the clip otherwise.
- [ ] **Step 2: Run** the GUI suites → the new test FAILS.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the GUI suites (all Compose tests PASS), unit tests, clippy, macOS compile check.
- [ ] **Step 5: Measure** with `xvfb-run -a scripts/bench.sh` (RSS and private anon at 1, 10, 20 panes)
  against the Task 0 baseline. If the saving is under ~0.1 MB per pane, report it and ask the owner
  whether to keep the change.
- [ ] **Step 6: Leave uncommitted.**

---

### Task 8: `oxi` without GTK on macOS

**Files:**
- Create: `build.rs` (Cargo picks up a root `build.rs` without a `Cargo.toml` change)

- [ ] **Step 1: Write `build.rs`**: when `CARGO_CFG_TARGET_OS == "macos"`, print
  `cargo:rustc-link-arg-bin=oxi=-Wl,-dead_strip_dylibs`; always print `cargo:rerun-if-changed=build.rs`.
- [ ] **Step 2: Verify Linux is unchanged:** `cargo build --release` and
  `ldd target/release/oxi | grep -Ec 'gtk|vte|adw'` → `0`; Linux checks; the macOS compile check (it does
  not link, so the flag itself is verified on the Mac in Task 9).
- [ ] **Step 3: Leave uncommitted.**

---

### Task 9: `build-macos.sh` — drop cursors, check `oxi` linkage, print sizes

**Files:**
- Modify: `scripts/build-macos.sh:70-80` (icons), after `dylibbundler` (checks), end (sizes)

- [ ] **Step 1:** After `cp -RL "$BREW/share/icons/Adwaita" …`, print the cursor folder size
  (`du -sk`) as `size: cursors removed <n> KiB`, then `rm -rf "$R/share/icons/Adwaita/cursors"`.
- [ ] **Step 2:** After bundling, fail with a message if `otool -L "$C/MacOS/oxi"` matches
  `libgtk|libadwaita|libvte`.
- [ ] **Step 3:** At the end print `size: app <n> KiB`, `size: Frameworks <n> KiB`, `size: dmg <n> KiB`.
- [ ] **Step 4:** `bash -n scripts/build-macos.sh` and `shellcheck scripts/build-macos.sh` if installed.
- [ ] **Step 5: Leave uncommitted.**

---

### Task 10: VTE without ICU (`VTE_SOURCE`, default 1) and CI

**Files:**
- Modify: `scripts/build-macos.sh` (new section before `cargo build`; dylibbundler `-s`), the header
  comment's requirements list, `.github/workflows/ci.yml:59-88`, `.github/workflows/release.yml:68-105`

- [ ] **Step 1: Source build section.** With `VTE_SOURCE=${VTE_SOURCE:-1}` = 1:
  `VTE_VERSION=0.84.1`, `VTE_SHA256=<sha256 of vte-0.84.1.tar.xz from download.gnome.org; compute it with
  curl + sha256sum on Linux and paste it>`; prefix
  `target/vte-noicu/$VTE_VERSION-$ARCH`. If `$PREFIX/lib/pkgconfig/vte-2.91-gtk4.pc` exists, reuse it;
  otherwise download `https://download.gnome.org/sources/vte/0.84/vte-0.84.1.tar.xz`, verify the hash,
  and `meson setup` with GTK 4 only and ICU, GTK 3, introspection, vala, docs and tests off — read the
  option names from that tarball's `meson_options.txt` (expected: `-Dgtk3=false -Dgtk4=true -Dicu=false
  -Dgir=false -Dvapi=false -Ddocs=false`), `--prefix "$PREFIX" --libdir lib --buildtype release`, then
  `ninja install`. Export `PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig:${PKG_CONFIG_PATH:-}"` before
  `cargo build`. Add `-s "$PREFIX/lib"` before `-s "$BREW/lib"` in the `dylibbundler` call.
  With `VTE_SOURCE=0`: today's path, unchanged.
- [ ] **Step 2: ICU check.** After bundling, when `VTE_SOURCE=1`, fail with the `otool -L` lines that
  mention `libicu` if any `libicu*` file is in `$C/Frameworks`.
- [ ] **Step 3: Header comment** lists `meson ninja` as Homebrew requirements and documents `VTE_SOURCE`.
- [ ] **Step 4: CI.** In both workflows' macOS jobs: add `meson ninja` to `brew install` (keep `vte3`
  so its dependencies and `cargo clippy` keep working), and an `actions/cache@v4` step for
  `target/vte-noicu` keyed
  `vte-noicu-0.84.1-${{ runner.arch }}-${{ hashFiles('scripts/build-macos.sh') }}-<brew gtk4 version>`
  (get the version in a prior step with `brew list --versions gtk4`).
- [ ] **Step 5:** `bash -n`, `shellcheck` if installed; YAML parses
  (`python3 -c 'import yaml,sys;[yaml.safe_load(open(f)) for f in sys.argv[1:]]' .github/workflows/*.yml`).
- [ ] **Step 6: Leave uncommitted.**

---

### Task 11: Docs, full verification, hand-off

**Files:**
- Modify: `docs/PERFORMANCE.md` (refresh policy + new measured numbers), `docs/PACKAGING.md` (VTE source
  build, `VTE_SOURCE`, cursor removal, `oxi` linker flag), `docs/RELEASE_NOTES.md` (user-facing summary)
- Keep: `docs/superpowers/plans/2026-11-18-baseline-bench.txt` (the owner decides whether to commit it)

- [ ] **Step 1:** Final `xvfb-run -a scripts/bench.sh` on a release build; put before/after numbers in
  `docs/PERFORMANCE.md` (startup, idle CPU, RSS/anon at 1/10/20 panes, child processes per minute,
  history import times). Update its "Metadata refresh policy" section to the new rules.
- [ ] **Step 2:** Update `docs/PACKAGING.md` and `docs/RELEASE_NOTES.md`. README: change only statements
  that became wrong.
- [ ] **Step 3:** Run every Linux check from Global Constraints plus the macOS compile check; all pass.
- [ ] **Step 4: Hand-off report** per platform:
  - Linux: what was run and the numbers.
  - macOS: compile check only from Linux; still to do on the CI `macos` job after the owner pushes a
    branch: `build-macos.sh` output (the `size:` lines, the `oxi` and ICU checks), DMG size vs the v0.3.0
    release (38.95 MiB arm64). By hand on a Mac: cursors (I-beam, link pointer, split and window-edge
    resize, corner grip, pane drag), UTF-8 text (accents, CJK, emoji, Arabic/Hebrew; paste; input
    method), minimize behaviour, `oxi ping` time. GUI suites are Linux-only.
  - Ask the owner to push and paste the CI `macos` log back for fixes.
