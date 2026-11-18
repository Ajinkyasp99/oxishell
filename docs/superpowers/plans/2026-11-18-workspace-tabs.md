# Workspace Tabs Rework Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Replace per-split tab strips with tabs owned by workspaces (one tab bar per workspace, tabs listed under the workspace in the sidebar), each tab holding its own split layout.

**Architecture:** `model::Workspace` gets `tabs: Vec<Tab>` + `active_tab`; `Node::Leaf` returns to one pane. Each workspace page becomes tab bar + `gtk::Stack` of tab layouts (`ui/tabs.rs`). Sidebar session lines become tab lines. Per-pane chrome returns to 0.3.0.

**Tech Stack:** Rust, GTK4 0.9, libadwaita 0.7, vte4 0.8, serde.

**Spec:** `docs/superpowers/specs/2026-11-18-agent-sessions-and-pane-tabs-design.md` — section "Revision (2026-11-18 …)".

## Global Constraints
- No git commits (user commits). Terminals are never recreated by tab/layout operations.
- Version-1 and 0.3.0 session files load (one tab per workspace).
- Part 1 behaviour (agent summary/model/age, title rules, Review fixes) stays.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, lib, e2e and interactions suites green.

## Review Focus
1. Closing the last pane of a background tab removes only that tab; the shown tab and focus are untouched.
2. An agent in a background tab asks for input: sidebar line / notification / Ctrl+Shift+U show that tab.
3. A 0.3.0 session file loads as one tab per workspace with the same splits and focus.
4. Alt+arrows and the ✥ swap never reach panes of another tab.
5. Divider drags in one tab never change another tab's ratios.

### Task 1: Model — workspace tabs
Files: `src/model.rs` (restore `Node::Leaf { pane }` from HEAD; add `Tab`, `Workspace.tabs/active_tab`, helpers `tab()`, `tab_mut()`, `focused()`, `panes()`, `tab_index_of(pane)`; ops `new_tab(wid) -> (TabId, PaneId)`, `select_tab(tab) -> (WorkspaceId, PaneId)`, `cycle_tab(wid, delta) -> PaneId`, `move_tab(tab, index) -> WorkspaceId`, `close_tab(tab) -> (Vec<PaneId>, PaneClosed)`, `workspace_of_tab(tab)`; `split/close_pane/swap_panes/focus/focus_direction/validate/normalize_ids` per tab; `ModelError::DifferentTabs`, `NoSuchTab`), `src/session.rs` (VERSION 2, v1 migration).
Tests (lib): new_tab_inserts_after_active_and_selects_it; focus_selects_the_pane_tab_and_cycle_wraps; split_and_neighbor_stay_in_one_tab; closing_last_pane_of_a_tab_removes_only_that_tab; closing_the_last_tab_closes_the_workspace; move_tab_reorders; swap_across_tabs_is_rejected; close_tab_returns_its_panes; validate_rejects_bad_tabs; loads_v1_workspace_as_one_tab; tabs_round_trip.

### Task 2: UI — workspace tab bar, panes back to 0.3.0
Files: `src/ui/tabs.rs` (rewrite: `build_bar`, `sync`, tab DnD reorder), `src/ui/mod.rs` (page = bar + stack; `rebuild_workspace` builds every tab's tree; `build_node` saves ratios to its tab; `new_tab(wid, cwd)`, `select_tab`, `cycle_tab`, `move_tab`, `close_tab`; refresh rings back on frames), `src/ui/pane.rs` (✕/✥/compose overlay back, `actions` field removed), `src/ui/corner_resize.rs`, `src/ui/style.css`, `src/theme.rs` (rings back on `.pane`, tab-bar styles).
Tests (interactions): tab_bar_new_switch_and_close; split_inside_a_tab_keeps_one_tab_bar; jump_to_unread_selects_background_tab; dragging_a_tab_reorders_it; ctrl_tab_cycles_tabs_and_ctrl_shift_t_opens_one; tab_label_follows_the_agent_title_with_the_sidebar; rebuilding_frees_the_old_tab_bar; swap test back to the pane-corner handle; theme ring test back to `.pane`.

### Task 3: Sidebar tab lines
Files: `src/ui/sidebar.rs` (lines per tab; shown when >1 tab or any agent; active highlighted; click selects tab).
Tests: sidebar_lists_each_tab_and_click_selects_it (background tab with agent).

### Task 4: IPC / CLI
Files: `src/ipc.rs`, `src/ui/ipc_handler.rs`, `src/bin/oxi.rs`.
Tests (e2e): tabs_via_ipc_survive_restart (new/list/select/move/close, hidden tab keeps running, restart restores, closing last pane of a tab).

### Task 5: Docs + full verification
README, AGENTS, CLI, CONFIGURATION, ARCHITECTURE; all suites; bench spot check.
