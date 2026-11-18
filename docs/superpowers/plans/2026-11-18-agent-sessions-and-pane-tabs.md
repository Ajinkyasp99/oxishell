# Agent Sessions Sidebar + Per-Pane Tabs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show every live agent session (agent, task summary, model, time since activity) in the sidebar, and give every split slot an Orca-style tab strip holding several terminals.

**Architecture:** Part 1 adds a GTK-free `agent_info` module for summary/model/time text, carries `prompt`/`model` from hook payloads through `oxi agent-event` → IPC → `Pane`, captures qualifying VTE titles, and renders one clickable line per session in `sidebar.rs`. Part 2 changes `model::Node::Leaf` to a tab group (`tabs`, `active`) with migration of old sessions, then renders each group as a new `ui/tabs.rs` widget (strip + `gtk::Stack` of existing pane frames), adds shortcuts and IPC/CLI.

**Tech Stack:** Rust 1.87, GTK4 0.9 (v4_14), libadwaita 0.7 (v1_5), vte4 0.8, serde/serde_json, clap.

**Spec:** `docs/superpowers/specs/2026-11-18-agent-sessions-and-pane-tabs-design.md`

## Global Constraints

- **No git commits.** The user commits everything. Never run `git commit`, `git commit --amend` or `git push`. Each task ends with a checkpoint (`cargo fmt`, `cargo clippy`, `git diff --stat`), not a commit.
- Pane frames / VTE terminals are never recreated by any tab or layout operation; they are only detached and re-parented.
- Never guess: unknown model → show nothing; the shell's own title is never a summary.
- A session file written by 0.3.0 (`"version":1`, `{"type":"leaf","pane":N}`) must load with every pane intact.
- All user-supplied/agent-supplied text shown in UI goes through `crate::notify::sanitize`.
- No new per-pane polling; startup and idle CPU stay within `docs/PERFORMANCE.md` (`scripts/bench.sh`).
- Shortcuts exist in both `DEFAULT_SHORTCUTS` tables (`cfg(not(target_os="macos"))` and `cfg(target_os="macos")`) and pass `Config::shortcut_conflicts()`.
- Tests: `cargo test --lib`; GUI tests run `xvfb-run -a cargo test --test interactions -- --test-threads=1` and `xvfb-run -a cargo test --test e2e -- --nocapture`.

## Review Focus

1. **A 0.3.0 session file** loads into one-tab groups with the same panes, focus and ratios → Task 4 `loads_v1_session_leaf_as_single_tab_group`.
2. **An agent in a background tab asks for input**; clicking its sidebar line, the desktop notification, or Ctrl+Shift+U shows that tab and focuses its terminal → Task 3 interaction test + Task 5 `jump_to_unread_selects_background_tab`.
3. **Closing the last tab** of a slot removes the slot; of the only slot, closes the workspace exactly as closing the last pane does today → Task 4 `closing_last_tab_removes_slot_and_last_slot_removes_workspace`.
4. **Shell title leakage**: a pane with no agent (bash sets `user@host: ~/dir`), or an agent that has exited, never shows a title as summary → Task 1 `title_qualification` + Task 2 e2e `title_cleared_when_agent_ends`.
5. **Hostile prompt text** (multi-line, escape sequences, 10 kB) shows as one sanitized line ≤ 120 chars → Task 1 `prompt_first_line_is_sanitized_and_capped`.

---

## Part 1 — Agent sessions in the sidebar

### Task 1: GTK-free agent text helpers

**Files:**
- Create: `src/agent_info.rs`
- Modify: `src/lib.rs` (add `pub mod agent_info;`)
- Test: unit tests in `src/agent_info.rs`

**Interfaces:**
- Produces:
  - `pub fn title_summary(title: &str, agent: &str) -> Option<String>` — strip leading spinner/status glyphs (`✳ ✶ ✻ ✽ ✢ · * ⏺` and braille `U+2800..=U+28FF`) and whitespace; `None` if empty, or equal (case-insensitive) to the agent's display name (`agents::display_name(agent)`) or the agent id. Result sanitized, ≤ 120 chars.
  - `pub fn prompt_line(prompt: &str) -> Option<String>` — first non-empty line, sanitized, ≤ 120 chars (ellipsis `…` when cut).
  - `pub fn short_model(model: &str) -> Option<String>` — table: contains `opus`→`opus`, `sonnet`→`sonnet`, `haiku`→`haiku`, `fable`→`fable`; `gpt-N…`→`gpt-N` (keep the version, drop suffixes like `-codex`); `gemini-X-pro`→`gemini-X`; other non-empty → first 12 chars; empty → `None`.
  - `pub fn time_ago(d: std::time::Duration) -> String` — `<5 s`→`now`, `<60 s`→`{s}s`, `<60 m`→`{m}m`, `<24 h`→`{h}h`, else `{d}d`.
  - `pub fn summary(title: Option<&str>, prompt: Option<&str>, state: Option<AgentState>) -> String` — title → prompt → state label (`AgentState::label()`, `"Running"` when `None`); when `state == Some(Waiting)` and the result isn't the state label, prefix `"Needs input: "`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn title_qualification() {
    assert_eq!(title_summary("✳ Fix token refresh race", "claude").as_deref(), Some("Fix token refresh race"));
    assert_eq!(title_summary("⠙ Add tests", "codex").as_deref(), Some("Add tests"));
    assert_eq!(title_summary("✳ Claude Code", "claude"), None);
    assert_eq!(title_summary("  ✳  ", "claude"), None);
}
#[test] fn prompt_first_line_is_sanitized_and_capped() {
    assert_eq!(prompt_line("\n  fix the bug\nsecond line").as_deref(), Some("fix the bug"));
    let l = prompt_line(&format!("\x1b]0;evil\x07{}", "x".repeat(10_000))).unwrap();
    assert!(!l.contains('\x1b') && !l.contains('\x07'));
    assert!(l.chars().count() <= 120 && l.ends_with('…'));
    assert_eq!(prompt_line("   \n\n"), None);
}
#[test] fn model_names_are_shortened() {
    assert_eq!(short_model("claude-opus-5-5").as_deref(), Some("opus"));
    assert_eq!(short_model("Opus 5.5").as_deref(), Some("opus"));
    assert_eq!(short_model("gpt-5-codex").as_deref(), Some("gpt-5"));
    assert_eq!(short_model("gemini-3-pro").as_deref(), Some("gemini-3"));
    assert_eq!(short_model(""), None);
}
#[test] fn time_ago_buckets() {
    use std::time::Duration as D;
    assert_eq!(time_ago(D::from_secs(2)), "now");
    assert_eq!(time_ago(D::from_secs(42)), "42s");
    assert_eq!(time_ago(D::from_secs(4 * 60 + 5)), "4m");
    assert_eq!(time_ago(D::from_secs(2 * 3600)), "2h");
    assert_eq!(time_ago(D::from_secs(3 * 86400)), "3d");
}
#[test] fn summary_prefers_title_then_prompt_then_state() {
    use crate::agents::AgentState::*;
    assert_eq!(summary(Some("T"), Some("P"), Some(Running)), "T");
    assert_eq!(summary(None, Some("P"), Some(Running)), "P");
    assert_eq!(summary(None, None, Some(Idle)), "Done");
    assert_eq!(summary(None, Some("P"), Some(Waiting)), "Needs input: P");
    assert_eq!(summary(None, None, Some(Waiting)), "Needs input");
    assert_eq!(summary(None, None, None), "Running");
}
```

- [ ] **Step 2: Run** `cargo test --lib agent_info` — Expected: FAIL (module/functions missing).
- [ ] **Step 3: Implement** the five functions in `src/agent_info.rs`. Use `notify::sanitize(s, 120)`-style capping; check `notify::sanitize`'s exact signature and whether it already appends `…`.
- [ ] **Step 4: Run** `cargo test --lib agent_info` — Expected: 5 passed.
- [ ] **Step 5: Checkpoint** `cargo fmt && cargo clippy --all-targets -- -D warnings && git diff --stat` (no commit).

### Task 2: Capture prompt, model and title per pane

**Files:**
- Modify: `src/agents.rs:387-404` (`parse_hook_payload`) and its tests
- Modify: `src/bin/oxi.rs` (`AgentEvent` handling ~305-345; `AgentUsage` ~269-285)
- Modify: `src/ui/ipc_handler.rs` (`agent.event`, `agent.usage`, `pane_json`)
- Modify: `src/ui/pane.rs` (`Pane` fields; `window-title-changed` handler ~455; `agent_label`)
- Modify: `src/ui/mod.rs:1195` (`App::agent_event`), and wherever `fg_agent` is updated from the meta worker
- Test: `src/agents.rs` unit tests; `tests/e2e.rs` new test

**Interfaces:**
- Consumes: Task 1 `title_summary`, `prompt_line`, `short_model`.
- Produces:
  - `pub struct HookInfo { pub session: Option<String>, pub message: Option<String>, pub prompt: Option<String>, pub model: Option<String> }` and `pub fn parse_hook_payload(agent: &str, v: &Value) -> HookInfo` (replaces the tuple). `prompt` from `"prompt"`; `model` from `"model"` when it is a string, or `"/model/display_name"` / `"/model/id"`.
  - `Pane` fields: `pub agent_title: Option<String>`, `pub agent_prompt: Option<String>`, `pub agent_model: Option<String>`, `pub agent_activity: Option<Instant>`, `pub agent_seen_title: Option<String>` (the VTE title at the moment the agent was first seen; titles equal to it never qualify).
  - `Pane::agent_id(&self) -> Option<&str>` — hook agent if state ≠ `Ended`, else `fg_agent`.
  - `Pane::clear_agent_info(&mut self)` — clears the five fields above.
  - `App::agent_event(self, pane, agent, state, message, session, prompt: Option<String>, model: Option<String>)`.
  - IPC `agent.event` optional params `prompt`, `model` (strings, sanitized; `prompt` via `prompt_line`, `model` capped at 64 chars). IPC `agent.usage` optional `model` string. `pane_json` adds `"agent_summary"`, `"agent_model"` (short), `"agent_title"`.

- [ ] **Step 1: Write failing unit tests** in `src/agents.rs`: update existing `parse_hook_payload` tests to the struct, plus

```rust
#[test] fn hook_payload_carries_prompt_and_model() {
    let h = parse_hook_payload("claude", &json!({"session_id":"s1","prompt":"fix login\nmore"}));
    assert_eq!(h.prompt.as_deref(), Some("fix login\nmore"));
    let h = parse_hook_payload("codex", &json!({"session_id":"s2","model":"gpt-5-codex","prompt":"x"}));
    assert_eq!(h.model.as_deref(), Some("gpt-5-codex"));
    let h = parse_hook_payload("claude", &json!({"model":{"id":"claude-opus-5-5","display_name":"Opus 5.5"}}));
    assert_eq!(h.model.as_deref(), Some("Opus 5.5"));
}
```

- [ ] **Step 2: Run** `cargo test --lib agents` — Expected: FAIL (compile error on `HookInfo`).
- [ ] **Step 3: Implement** `HookInfo`; in `oxi agent-event` send `prompt` (first 2000 chars) and `model` in the `agent.event` params; in `oxi agent-usage` send `"model": payload.pointer("/model/display_name")` alongside `rate_limits` (update the "only quota fields" comment to "quota fields and the model name").
- [ ] **Step 4: Implement app side.** In `App::agent_event`: set `agent_activity = now`; on `Running` with `prompt` → `agent_prompt = prompt_line(..)` and clear `agent_title` only if the prompt changed; `model` → `agent_model`; first time an agent is seen in the pane, set `agent_seen_title` to the current VTE title; on `Ended` → `clear_agent_info()`. `agent.usage` with `model` sets `agent_model` on that pane. In the `window-title-changed` handler: if `agent_id()` is `Some` and the title ≠ `agent_seen_title`, set `agent_title = title_summary(title, agent)` and `agent_activity = now`; then rebuild the sidebar (already done). When the meta worker sets `fg_agent` from `Some` to `None` and the hook state is not live, call `clear_agent_info()`; when it goes `None`→`Some`, record `agent_seen_title`. Change `agent_label()` to return `"{display_name} — {summary}"` using Task 1 `summary` (keeps existing callers meaningful).
- [ ] **Step 5: Write e2e test** `agent_prompt_title_and_model_reach_pane_list` in `tests/e2e.rs`: start app, `call("agent.event", {pane, agent:"claude", state:"running", prompt:"Fix the login bug\nmore", model:"claude-opus-5-5"})`, then `pane.send` `printf '\033]0;✳ Fix token refresh\007'`; poll `pane.list` until `agent_summary == "Fix token refresh"` and `agent_model == "opus"`. Then `agent.event state:"ended"` and assert `agent_summary` is null — this is `title_cleared_when_agent_ends`. Also assert a fresh pane with no agent where `printf '\033]0;user@host: ~\007'` was sent has `agent_summary` null.
- [ ] **Step 6: Run** `cargo test --lib && xvfb-run -a cargo test --test e2e agent_prompt -- --nocapture` — Expected: PASS.
- [ ] **Step 7: Checkpoint** (fmt, clippy, `git diff --stat`; no commit).

### Task 3: Session lines in the sidebar

**Files:**
- Modify: `src/ui/sidebar.rs` (`Row`: replace `agent: gtk4::Label` with `sessions: gtk4::Box`; `rebuild`)
- Modify: `src/notify.rs` (new query)
- Modify: `src/ui/style.css` (session line + state dot classes)
- Modify: `src/ui/mod.rs` (a 30 s `glib::timeout_add_seconds_local` calling `sidebar::rebuild` only if no existing periodic refresh already rebuilds the sidebar at ≤ 30 s — check `meta_worker::attach` first)
- Test: `src/notify.rs` unit test; `tests/interactions.rs` new test

**Interfaces:**
- Consumes: Task 2 `Pane::agent_id`, `agent_title`, `agent_prompt`, `agent_model`, `agent_activity`; Task 1 `summary`, `short_model`, `time_ago`.
- Produces:
  - `NotificationCenter::latest_non_agent_for_workspace(&self, ws: WorkspaceId) -> Option<&Notification>` — like `latest_for_workspace` but skips `source` starting with `"agent:"`.
  - `pub(super) fn session_lines(app: &AppRc, ws: &Workspace) -> Vec<SessionLine>` where `struct SessionLine { pane: PaneId, agent: String, state: Option<AgentState>, summary: String, model: Option<String>, ago: Option<String> }` — every pane in `ws.layout.panes()` (all tabs, layout order) with `agent_id().is_some()`.
  - CSS classes: `ws-session`, `ws-session-dot` + `dot-running|dot-waiting|dot-done|dot-detected`, `ws-session-agent`, `ws-session-summary`, `ws-session-meta`, `ws-session-more`.

- [ ] **Step 1: Failing unit test** in `src/notify.rs`: push an `agent:claude` note then a `bell` note then another `agent:codex` note for workspace 1; assert `latest_non_agent_for_workspace(1)` returns the bell one; mark it read → `None`.
- [ ] **Step 2: Run** `cargo test --lib notify` — FAIL; **implement**; re-run — PASS.
- [ ] **Step 3: Implement the lines.** Each line is a flat `gtk4::Button` (class `ws-session`) whose child is a horizontal box: dot · agent short name (`Claude`, `Codex`, `Gemini`, `Qoder`, `OpenCode`, `Cursor`, `Amp`, `Aider`; unknown → display name) · summary (hexpand, ellipsize End) · `model  ago` (class `ws-session-meta`). Dot class: Running→`dot-running`, Waiting→`dot-waiting`, Idle→`dot-done`, no hook state (fg only)→`dot-detected`; colours from theme tokens already used for ANSI blue/yellow/green/grey in `style.css`. Show at most 5; if more, a `ws-session-more` label `+N more`. Tooltip: `"{display name} · {full summary}\n{cwd}"`. Click → `app.focus_pane(pane)` deferred with `glib::idle_add_local_once` (the click happens inside the list being rebuilt). Reuse line widgets per pane id across rebuilds (keep a `HashMap<PaneId, SessionLineWidgets>` in `Row`) so a click in flight is not dropped — same reason `Row` exists. The `attention` label uses `latest_non_agent_for_workspace`.
- [ ] **Step 4: Interaction test** `sidebar_lists_each_agent_session_and_click_focuses_it` in `tests/interactions.rs`: create workspace, split once, call `app.agent_event` on both panes (claude running with prompt "Fix A"; codex waiting with prompt "Fix B"), focus pane 1, `pump`; find widgets with class `ws-session` in the sidebar → exactly 2, texts contain `Fix A` and `Needs input: Fix B`; click the second via `xdo` → focused pane becomes pane 2.
- [ ] **Step 5: Run** `xvfb-run -a cargo test --test interactions sidebar_lists -- --test-threads=1` — PASS; run whole `interactions` suite — PASS.
- [ ] **Step 6: Checkpoint** (fmt, clippy, diff stat; no commit). Manually launch `cargo run`, run `claude` in a pane, confirm the line updates as the title changes.

---

## Part 2 — Tabs in each pane slot

### Task 4: Tab groups in the model and session migration

**Files:**
- Modify: `src/model.rs` (`Node`, its impl, `AppState` ops, `validate`, tests)
- Modify: `src/session.rs` (`VERSION = 2`, migration test)
- Modify: `src/ui/mod.rs` `build_node` and any `Node::Leaf { pane }` match so it compiles (render the active tab's frame only — real tab UI comes in Task 5)
- Test: unit tests in `src/model.rs`, `src/session.rs`

**Interfaces:**
- Produces:
  - `Node::Leaf { tabs: Vec<PaneId>, active: usize }`; `Node::leaf(pane)` → one-tab group.
  - Serialization: `#[serde(try_from = "NodeRepr")]` on `Node` with a private `NodeRepr` that accepts `{"type":"leaf","pane":N}` **or** `{"type":"leaf","tabs":[..],"active":i}`; empty `tabs` or `active >= len` is an error. `Serialize` writes the new form.
  - `Node::panes()` — all tabs in layout order, tab order within a leaf (unchanged signature).
  - `Node::visible_panes(&self) -> Vec<PaneId>` — active tab per leaf.
  - `Node::group_of(&self, pane: PaneId) -> Option<(&[PaneId], usize)>` — tabs and active index of the group holding `pane`.
  - `contains`, `split`, `remove`, `path_to`, `split_balanced`, `remove_balanced` match a leaf when `tabs.contains(&target)` (they act on the whole slot). `rects()`/`layout()` emit the **active** tab per leaf, so `neighbor()` only ever returns visible panes. `swap(x, y)` swaps the two whole groups (`tabs` and `active`).
  - `AppState::new_tab(&mut self, after: PaneId) -> Result<PaneId, ModelError>` — insert right after `after`, make it active and focused.
  - `AppState::focus(pane)` (existing) additionally makes `pane` its group's active tab.
  - `AppState::cycle_tab(&mut self, pane: PaneId, delta: isize) -> Result<PaneId, ModelError>` — wraps; focuses the result.
  - `AppState::move_tab(&mut self, pane: PaneId, onto: PaneId, index: usize) -> Result<WorkspaceId, ModelError>` — `onto` is any pane of the target group in the same workspace (`DifferentWorkspaces` otherwise); `index` clamped; source group emptied → slot removed via `remove_balanced`; moved tab becomes active and focused.
  - `AppState::close_pane(pane)`: if its group has > 1 tab, remove it from `tabs` (new active = previous tab, or 0), refocus that tab if `pane` was focused, return `PaneClosed::Refocus`; else existing slot-removal behaviour.
  - `validate()`: every leaf has non-empty `tabs`, `active < tabs.len()`, and `workspace.focused` is the active tab of its group.
  - `session::VERSION = 2`; loading `version` 1 or 2 succeeds.

- [ ] **Step 1: Write failing tests** in `src/model.rs`:

```rust
#[test] fn new_tab_goes_after_source_and_is_focused() { /* create ws (pane p); t = new_tab(p); group_of(p) == ([p,t],1); focused == t; panes()==[p,t]; visible_panes()==[t] */ }
#[test] fn focus_selects_tab_and_cycle_wraps() { /* p,t,u in one group; focus(p) → active 0; cycle_tab(p, -1) → u; cycle_tab(u, 1) → p */ }
#[test] fn split_splits_the_whole_group() { /* p,t in group; split(t, Horizontal) → new slot; rects() has 2 entries: active of first group and new pane */ }
#[test] fn neighbor_never_returns_hidden_tab() { /* left group [p,t] active t, right group [q]; neighbor(q, Left) == Some(t) */ }
#[test] fn closing_last_tab_removes_slot_and_last_slot_removes_workspace() {
    /* [p,t] | [q]: close t → group [p], Refocus(p) if t focused;
       close p → slot removed, Refocus(q); close q → WorkspaceRemoved */ }
#[test] fn move_tab_reorders_and_moves_between_groups() {
    /* [p,t,u] | [q]: move_tab(u, p, 0) → [u,p,t]; move_tab(u, q, 9) → [p,t] | [q,u] active u, focused u;
       move_tab(q, q, ..) no-op ok; moving only tab of a group removes its slot */ }
#[test] fn swap_moves_whole_groups() { /* [p,t] | [q] swap(t,q) → [q] | [p,t] with active preserved */ }
#[test] fn validate_rejects_bad_groups() { /* empty tabs, active out of range, focused hidden tab → Err */ }
```

and in `src/session.rs`:

```rust
#[test] fn loads_v1_session_leaf_as_single_tab_group() {
    // existing v1 fixture (session.rs ~195) plus a split fixture:
    // {"type":"split","orient":"horizontal","ratio":0.3,"a":{"type":"leaf","pane":2},"b":{"type":"leaf","pane":4}}
    // → leaves tabs [2] and [4], active 0, ratio 0.3 kept, focused unchanged, validate() Ok
}
#[test] fn tab_groups_round_trip() { /* save layout with [2,5] active 1 → load → equal; JSON contains "tabs":[2,5] */ }
```

Fill the bodies; each comment lists the exact assertions.
- [ ] **Step 2: Run** `cargo test --lib model session` — Expected: FAIL (compile).
- [ ] **Step 3: Implement** the interfaces above; make `ui/mod.rs::build_node` take `Node::Leaf { tabs, active }` and place `frames[&tabs[*active]]`. Fix every other compile error from the `Leaf` change.
- [ ] **Step 4: Audit callers of `panes()`/`rects()`** outside `model.rs` (`grep -rn "\.panes()\|\.rects()" src`): `check_split_room` (`ui/mod.rs:730`) must use visible geometry (it uses `rects()`, which is now visible-only — confirm); sidebar ports/agents, ipc `workspace.list`, session save, pane counts keep `panes()` (all tabs). Note the decision per call site in the task report.
- [ ] **Step 5: Run** `cargo test --lib` and `xvfb-run -a cargo test --test e2e terminal_workspaces_splits -- --nocapture` — Expected: PASS (one-tab behaviour identical to today).
- [ ] **Step 6: Checkpoint** (no commit).

### Task 5: Tab group widget and strip

**Files:**
- Create: `src/ui/tabs.rs`
- Modify: `src/ui/mod.rs` (`mod tabs;`, `build_node`, `rebuild_workspace`, `focus_pane`, `refresh`, `close_pane` rebuild path, new `App::new_tab`)
- Modify: `src/ui/pane.rs:327-369` (move ✥ handle + compose button + ✕ out of the per-pane overlay into the strip; keep chips/search/body in the frame)
- Modify: `src/ui/compose_ui.rs` (`PaneCompose::new` currently appends its button into the pane's `actions` box — let it return the button so the strip can host the focused tab's button)
- Modify: `src/ui/style.css`
- Test: `tests/interactions.rs`

**Interfaces:**
- Consumes: Task 4 model ops; Task 1 `summary`; Task 2 `Pane` agent fields.
- Produces:
  - `pub(super) struct TabGroup { pub root: gtk4::Box, strip: gtk4::Box, tabs_box: gtk4::Box, stack: gtk4::Stack, … }`
  - `pub(super) fn build_group(app: &AppRc, wid: WorkspaceId, tabs: &[PaneId], active: usize, frames: &HashMap<PaneId, gtk4::Box>) -> TabGroup` — stack pages named by pane id holding the existing frames; strip = scrolled `tabs_box` + `+` button, then the active tab's compose button, ✥ handle (drag source keeps swapping the whole group) on the right.
  - `pub(super) fn sync(app: &AppRc)` — for every group: visible stack child = active tab; tab button labels, active/unread classes. Called from `App::refresh` and after `focus_pane`.
  - `pub(super) fn tab_label(p: &Pane) -> String` — agent: `"{short agent}: {summary}"`; else busy foreground program name; else cwd basename; else `"Terminal"`; ellipsized to 28 chars in the widget, full text as tooltip.
  - `App::new_tab(self: &Rc<Self>, pane: Option<PaneId>, cwd: Option<PathBuf>) -> Result<PaneId, String>` — mirrors `App::split` (cwd defaults to source pane's cwd, `create_pane`, `rebuild_workspace`, `focus_pane`, `schedule_save`).
  - Group widgets are rebuilt on each `rebuild_workspace`; keep a `HashMap<PaneId /*first tab*/, TabGroup>` on `App` only if `sync` needs it — otherwise find strips by widget name `tabgroup-{first pane}`.
  - CSS classes: `tab-strip`, `tab`, `tab-active`, `tab-unread`, `tab-close`, `tab-new`, `tab-group` (+ existing `focused`/`attention` move from pane frame to `tab-group` for the ring).
- Behaviour: tab click → `focus_pane(pane)`; middle-click or tab ✕ → `app.pane-close` action for that pane; `+` and double-click on empty strip → `new_tab(Some(active pane))`; unread in a hidden tab → `tab-unread` dot; group gets `attention` when any tab has unread; strip always shown, even with one tab.

- [ ] **Step 1: Interaction tests** in `tests/interactions.rs`:
  - `tab_strip_new_switch_and_close`: one pane; click `.tab-new` → 2 tabs, second active, its terminal has focus, first pane's VTE widget is the *same object* as before (`glib::ObjectExt::as_ptr` equality) and its shell pid is alive; click first `.tab` → first active and stack visible child named its id; click second tab's `.tab-close` → 1 tab left.
  - `jump_to_unread_selects_background_tab`: two tabs, focus tab 1, `app.notify(Some(tab2), "x", "y", "test")`; `.tab-unread` on tab 2; `app.jump_to_unread()` → stack visible child is tab 2, tab 2's terminal has focus.
  - `split_of_a_tabbed_slot_keeps_its_tabs`: two tabs then `app.split(None, Horizontal, None)` → two `.tab-group`, first still has 2 `.tab`.
- [ ] **Step 2: Run** `xvfb-run -a cargo test --test interactions tab_ -- --test-threads=1` — FAIL.
- [ ] **Step 3: Implement** `ui/tabs.rs`, wire `build_node` to `build_group`, move the pane header controls, call `tabs::sync` from `refresh` and at the end of `focus_pane`. The existing `dragging_a_pane_handle_onto_another_pane_swaps_them` test must keep passing (find the handle by class `pane-drag` in the strip).
- [ ] **Step 4: Run** the three new tests and the full `interactions` and `e2e` suites — PASS.
- [ ] **Step 5: Checkpoint** (no commit). Manual: `cargo run`, open tabs, run `claude` in a background tab, confirm label and dot.

### Task 6: Drag tabs to reorder and between groups

**Files:**
- Modify: `src/ui/tabs.rs`
- Modify: `src/ui/mod.rs` (new `App::move_tab`)
- Test: `tests/interactions.rs`

**Interfaces:**
- Consumes: Task 4 `AppState::move_tab`; Task 5 strip widgets.
- Produces: `App::move_tab(self: &Rc<Self>, pane: PaneId, onto: PaneId, index: usize) -> Result<(), String>` (model op → `rebuild_workspace` → `focus_pane(pane)` → `schedule_save`). Drag payload string `"oxishell-tab:{pane}"` (distinct from `PANE_DRAG_PREFIX` so dropping a tab on a terminal body does nothing and a pane-swap drag on a strip does nothing). Drop index = position of the tab under the pointer (before it if pointer x < tab midpoint), or end when dropped on empty strip space.

- [ ] **Step 1: Interaction test** `dragging_a_tab_reorders_and_moves_it_to_another_group`: groups [p,t] | [q]; xdotool drag `t`'s tab onto `p`'s tab left half → [t,p]; drag `t` onto the right strip's empty area → [p] | [q,t], `t` focused, `t`'s VTE object unchanged.
- [ ] **Step 2: Run** — FAIL. **Step 3: Implement** (`DragSource` on each tab, `DropTarget` on each strip). **Step 4: Run** test + full `interactions` — PASS.
- [ ] **Step 5: Checkpoint** (no commit).

### Task 7: Shortcuts and palette

**Files:**
- Modify: `src/config.rs` (both `DEFAULT_SHORTCUTS` tables + tests)
- Modify: `src/ui/mod.rs` (action dispatch where `"split-right"` etc. are handled; palette entries)
- Test: `src/config.rs` unit test

**Interfaces:**
- Consumes: Task 5 `App::new_tab`; Task 4 `AppState::cycle_tab`.
- Produces: actions `new-tab` (Linux `<Ctrl><Shift>t`, macOS `<Meta><Shift>t`, label "New tab"), `next-tab` (`<Ctrl>Tab` both, "Next tab"), `prev-tab` (`<Ctrl><Shift>Tab` both, "Previous tab"). `close-pane` label becomes "Close tab". `App::cycle_tab(self: &Rc<Self>, delta: isize)` — model `cycle_tab` on the focused pane, then `focus_pane(result)`.

- [ ] **Step 1: Failing test** `tab_shortcuts_are_bound_without_conflicts`: `default_shortcut("new-tab")`, `"next-tab"`, `"prev-tab"` are non-empty and `Config::default().shortcut_conflicts().is_empty()`. Note `<Ctrl><Shift>Tab` may arrive as `ISO_Left_Tab` — the key handler must map it (add an interaction test `ctrl_tab_cycles_tabs` with `xdo key ctrl+Tab` / `ctrl+shift+Tab`).
- [ ] **Step 2: Run** `cargo test --lib config` — FAIL. **Step 3: Implement.** **Step 4: Run** lib tests and `ctrl_tab_cycles_tabs` — PASS.
- [ ] **Step 5: Checkpoint** (no commit).

### Task 8: IPC and `oxi tab`

**Files:**
- Modify: `src/ui/ipc_handler.rs` (`pane_json`, new methods)
- Modify: `src/ipc.rs` (method allow-list, if one exists there)
- Modify: `src/bin/oxi.rs` (new `tab` subcommand)
- Test: `tests/e2e.rs`

**Interfaces:**
- Consumes: Task 5 `App::new_tab`, Task 6 `App::move_tab`, `App::focus_pane`.
- Produces:
  - `pane_json` adds `"group": <first pane id of its group>`, `"tab_index"`, `"active_tab": bool`, `"visible": bool` (active tab of its group **and** its workspace is active).
  - `pane.new-tab {pane?: u64, cwd?: string}` → `{"pane": id}` (cwd validated exactly like `pane.split` — reuse its helper).
  - `pane.focus` (existing) now also selects the tab — no new select method.
  - `pane.move-tab {pane: u64, onto: u64, index?: u64}` → `{"pane": pane}`.
  - `oxi tab new [--cwd DIR]`, `oxi tab focus <PANE>`, `oxi tab move <PANE> --onto <PANE> [--index N]`; default pane is `$OXISHELL_PANE_ID`.

- [ ] **Step 1: e2e test** `tabs_via_ipc_survive_restart`: `pane.new-tab` twice from pane p → `pane.list` shows 3 panes with the same `group`, tab indexes 0,1,2, only the last `active_tab`; `pane.send` `echo hidden-ok` to tab 1 (hidden) then `pane.read` it contains `hidden-ok`; `pane.move-tab` tab 2 onto p index 0; `oxi tab new` via `cli()`; stop and restart app → same groups, order and active tab; `pane.close` all but one tab → one pane, workspace kept.
- [ ] **Step 2: Run** `xvfb-run -a cargo test --test e2e tabs_via_ipc -- --nocapture` — FAIL. **Step 3: Implement.** **Step 4: Run** it and the full e2e suite — PASS.
- [ ] **Step 5: Checkpoint** (no commit).

### Task 9: Docs and performance check

**Files:**
- Modify: `README.md` (features table: new **Tabs** row; Workspaces row mentions per-agent session lines), `docs/AGENTS.md` (summary/model sources, title rule), `docs/CLI.md` (`oxi tab`, new IPC fields), `docs/CONFIGURATION.md` (new shortcuts), `docs/ARCHITECTURE.md` (`Node::Leaf { tabs, active }`, `ui/tabs.rs`, `agent_info.rs`)

- [ ] **Step 1: Update docs** to match the shipped behaviour (shortcut values from Task 7, CLI from Task 8).
- [ ] **Step 2: Run** `scripts/bench.sh` — Expected: startup and idle CPU within the numbers in `docs/PERFORMANCE.md`; report the numbers.
- [ ] **Step 3: Full verification** `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --lib && xvfb-run -a cargo test --test interactions -- --test-threads=1 && xvfb-run -a cargo test --test e2e` — all PASS.
- [ ] **Step 4: Checkpoint** `git status` / `git diff --stat` for the user to commit (no commit).
