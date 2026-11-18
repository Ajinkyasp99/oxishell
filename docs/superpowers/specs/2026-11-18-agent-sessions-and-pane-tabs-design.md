# Agent sessions in the sidebar + per-pane tabs (Orca-style)

Date: 2026-11-18 · Status: design, awaiting approval

## Intent

Bring two pieces of [Orca](https://github.com/stablyai/orca)'s workflow into oxishell:

1. **The sidebar shows every agent session and what it is doing.** Orca lists each agent under its
   workspace with the agent, a one-line task summary, the model and how long ago it last did something.
   Today oxishell shows one `Claude Code — Needs input` line per workspace (the first pane with an agent),
   so a second agent in the same workspace is invisible and you cannot tell what any agent is working on.
2. **Tabs at the top of each pane.** Orca stacks several terminals in one split slot, each with a tab.
   Today a split slot holds exactly one terminal and the app has no tabs at all.

Why: the user runs several agents per project and wants the same at-a-glance overview and tab
ergonomics Orca has, in the native GTK app.

## Goals

- Sidebar: one line per live agent session in a workspace, including sessions in background tabs.
- Each line: state, agent name, task summary, model (when known), time since last activity.
- Clicking a session line jumps to that pane (switches workspace and tab, focuses the terminal).
- Pane tabs: every split slot is a tab group with a tab strip on top; new tab, close tab, switch tab,
  reorder tabs, move a tab to another group.
- Background tabs keep running (like background workspaces do today).
- Tabs, their order and the active tab survive restart via the session file.
- Tabs are scriptable through IPC / `oxi`.

## Non-goals (this round)

- Browser, diff, or file-preview tabs. Tabs hold terminals only.
- Dropping a tab on a pane's edge to create a new split (Orca does this). Moving a tab onto another
  group's strip is in scope; edge-drop splitting can come later.
- Chat UI for agents, worktree creation, orchestration, remote hosts.
- Agent brand logos (licensing); we use our own markers.

## Anti-goals (would make this a failure even if it "works")

- Recreating VTE terminals when tabs move or layouts change. Processes must survive every tab and
  layout operation, as they do today.
- Guessing. If we don't know the model, show nothing; if the title is the shell's own title, it is not
  a summary.
- Breaking existing sessions: a session file from 0.3.0 must load with every pane intact.
- Noticeable cost: no extra polling per pane; startup and idle CPU stay within docs/PERFORMANCE.md.

## Constraints from the current code

- `model.rs` is the source of truth; `ui/mod.rs::rebuild_workspace` re-parents existing pane frames into
  a fresh `gtk::Paned` tree. Tab groups must follow the same rule.
- Agent state arrives via `oxi agent-event` → `agent.event` IPC → `App::agent_event`
  (`ui/mod.rs:1195`), stored in `Pane.agent: (agent, state, message)`.
- `Pane.fg_agent` identifies agents with no hooks from the foreground process.
- VTE's `window-title-changed` is already connected (`ui/pane.rs:455`) but only triggers a sidebar
  rebuild; the title is never shown.
- Session file is `session.rs` `Snapshot { version: 1, state: AppState, panes }`;
  layout leaf serializes as `{"type":"leaf","pane":N}`.
- GTK4 + libadwaita 1.5, Rust. Linux and macOS shortcut tables both exist in `config.rs`.

## Approaches considered

**Tabs**

1. **Tab groups in the model** *(chosen)*. `Node::Leaf` holds a group (`tabs: Vec<PaneId>`,
   `active`). Each group renders as our own tab strip + a `gtk::Stack` of pane frames. Persistence,
   IPC and rebuilds all see tabs. Old sessions migrate (one tab per leaf).
2. **libadwaita `AdwTabView`/`AdwTabBar` per slot.** Free drag-reorder and overflow scrolling, but the
   view owns its pages, which fights "model is truth, rebuild re-parents frames", and its styling fights
   our Warp themes. Rejected.
3. **UI-only tabs outside the model.** Tabs would vanish on restart and be invisible to `oxi`. Rejected.

**Sidebar summary source**

1. **Terminal title set by the agent, else first line of the last prompt** *(chosen)*. Claude Code
   already sets a task-summary title; `UserPromptSubmit` payloads (Claude, Codex) carry `prompt`.
2. Last assistant message (already stored). Too long and describes the result, not the task; it stays
   in notifications.
3. Parsing agent transcripts. Slow, agent-specific, fragile. Rejected.

The user picked tabs approach 1 and approved the sidebar line below.

## Design

### Part 1: agent sessions in the sidebar

```
▌ api                                   2  ✕
  ⎇ feature/auth ●2 · ~/projects/api
  ports :8020
  ● Claude  Fix token refresh race     opus   2m
  ● Codex   Add EU tax regression tests gpt-5  now
  ● Gemini  Needs input: approve rm     —      5s
```

**Per-pane agent info** (extend `Pane`):

- `agent_title: Option<String>`: task summary from the terminal title (see rules below).
- `agent_prompt: Option<String>`: first line of the last submitted prompt, ≤ 120 chars, sanitized
  with `notify::sanitize`.
- `agent_model: Option<String>`: short model name when an agent reports it.
- `agent_activity: Option<Instant>`: time of the last hook event or title change.

**Where the data comes from**

- Prompt: `parse_hook_payload` also returns `prompt` (Claude/Codex `UserPromptSubmit`). New optional
  `prompt` param on `agent.event` IPC, sanitized like `message`.
- Model: Codex hook payloads' `model`; Claude's status-line payload `model.display_name` (we already
  receive it through the usage status line → `agent.usage`). New optional `model` param on
  `agent.event` and carried on `agent.usage`. Shortened for display (e.g. `claude-opus-5-5` → `opus`,
  `gpt-5-codex` → `gpt-5`); unknown → shown as given, truncated.
- Title: VTE `window_title()` when it qualifies as an agent title:
  - the pane has an agent (hook state not `Ended`, or `fg_agent` set), and
  - the title changed after the agent was first seen in this pane (so the shell's own
    `user@host: ~/dir` title is never used), and
  - after stripping leading spinner/status glyphs and whitespace it is non-empty and is not just the
    agent's name (e.g. "Claude Code").
  When the agent ends (state `Ended` or `fg_agent` cleared), title, prompt and model are cleared.

**Summary shown** = `agent_title` → else `agent_prompt` → else state label (`Running`, `Needs input`,
`Done`). For `Waiting`, prefix "Needs input: " when the summary is not already the state label.

**State dot colour**: Running = accent blue, Waiting = amber, Idle/Done = green, foreground-only
(no hooks) = grey. Colours come from the theme's existing ANSI palette so all 41 themes work.

**Lines**: one per pane whose agent is live, in layout order then tab order, across all tab groups.
Up to 5 lines; then `+N more`. A line is a button: click → select workspace, select the tab, focus the
pane. Tooltip: full summary, agent display name, pane's cwd.

**Time since activity**: `now`, `12s`, `4m`, `2h`, `3d`. Re-rendered on the existing refresh cadence;
add a 30 s timer only if nothing else refreshes the sidebar that often.

**Replaces** the single `agent` label in `sidebar.rs::Row`. The `attention` line stays but shows
only non-agent notifications (bell, finished command, `oxi notify`), since agent state now has its own
lines.

**Persistence**: summary/prompt/model are not saved (they describe a live process). Agent session ids
continue to be saved for resume, unchanged.

### Part 2: tabs in each pane slot

```
┌ ● Claude: Fix token… ✕ │ ~/api ✕ │ + ┐ ✥ 🐳 ┌ ● Codex: tax tests ✕ │ + ┐ ✥ 🐳
│ [~/projects/api] [⎇ feature/auth ●2]   │       │ [~/projects/api] [⎇ main]          │
│ claude …                               │       │ codex …                            │
```

**Model** (`model.rs`)

```rust
Node::Leaf { tabs: Vec<PaneId>, active: usize }   // invariant: tabs non-empty, active < tabs.len()
```

- `Node::panes()` keeps returning *every* pane (all tabs), so ports, notifications, agents, session
  save, `pane.list` keep covering background tabs.
- Add `Node::visible_panes()` (active tab per leaf). Geometry users switch to it: `rects()`,
  `neighbor()` (Alt+arrows), corner resize. Every caller of `panes()` must be audited and put into one
  of the two buckets.
- `Workspace.focused` stays a `PaneId` and is always the active tab of some group.
- New operations (all `Result<_, ModelError>`, unit-tested):
  - `new_tab(after: PaneId) -> PaneId`: insert after `after`, make active, focus.
  - `select_tab(pane)`: make `pane` its group's active tab and focus it.
  - `cycle_tab(pane, delta)`: next/previous with wrap-around.
  - `move_tab(pane, target_group_pane, index)`: reorder within a group or move into another group in
    the same workspace; an emptied source group is removed like a closed pane.
  - `close_pane(pane)`: removes the tab; if it was the group's last tab, the group's slot is removed
    from the split tree exactly as closing a pane works today. Focus goes to the neighbouring tab.
  - `split(pane, orient)`: splits the *group* containing `pane`; the new slot is a one-tab group.
  - `swap_panes(x, y)` (the ✥ handle): swaps whole groups.
- `validate()` checks the group invariants.

**Session** (`session.rs`)

- Leaf serializes as `{"type":"leaf","tabs":[2,5],"active":1}`.
- Reading accepts the old `{"type":"leaf","pane":N}` → `tabs:[N], active:0` (custom `Deserialize`
  or a serde-untagged helper). Bump `VERSION` to 2; version-1 files load via the migration.
- Test: the existing v1 fixture in `session.rs` tests loads into one-tab groups.

**UI**

- Each group = `gtk::Box(vertical)` holding a tab strip and a `gtk::Stack` whose pages are the
  existing pane frames (named by pane id). `rebuild_workspace` builds groups instead of placing frames
  directly; frames are detached/re-parented as now, never recreated. Group widgets may be rebuilt on
  every layout rebuild (simplest); keeping them is an optimisation for later.
- Tab strip (new `ui/tabs.rs`): horizontal scrolled box of tab buttons + `+` button, then the existing
  ✥ move handle and compose button moved from the pane header to the strip's right end. The per-pane
  ✕ becomes each tab's ✕. Chips stay inside the page above the terminal.
- Tab label: agent marker + agent summary (same text as the sidebar) when the pane has an agent;
  otherwise the foreground program if busy (`npm`, `cargo`…) else the cwd's basename. Ellipsized,
  max ~28 chars; full text in tooltip.
- Tab state: active tab styled like the focused pane; unread notifications in a hidden tab show a dot
  on its tab; the blue attention ring applies to the group when any of its tabs needs attention.
- Interactions: click selects; middle-click closes; drag reorders within the strip; dropping on another
  group's strip moves the tab there; double-click on empty strip area opens a new tab.
- Single-tab groups still show the strip (consistent with Orca, and it hosts ✕/✥/compose).
- New tab starts in the source pane's cwd, like split does.
- Notification jump / `jump-unread` / desktop-notification click selects the tab before focusing.

**Shortcuts** (`config.rs`, both platform tables, all rebindable)

| Action | Linux | macOS |
|---|---|---|
| `new-tab` | `<Ctrl><Shift>t` | `<Meta><Shift>t` |
| `next-tab` | `<Ctrl>Tab` | `<Ctrl>Tab` |
| `prev-tab` | `<Ctrl><Shift>Tab` | `<Ctrl><Shift>Tab` |
| `close-pane` | unchanged; closes the active tab | unchanged |

macOS `<Meta>t` stays New workspace to avoid changing an existing binding. Must pass
`shortcut_conflicts()`. Add palette entries for each action.

**IPC / CLI**

- `pane.list`: each pane gains `group` (id of the group's first pane or a stable index), `tab_index`,
  `active_tab: bool`, `visible: bool`, plus `agent_summary`, `agent_model`.
- New `pane.new-tab {pane?, cwd?} → {pane}`, `pane.select {pane}` (select its tab + focus; may
  extend `pane.focus` instead), `pane.move-tab {pane, to, index?}`.
- `oxi tab new|select|move` subcommands mirroring them; documented in docs/CLI.md.

### Docs

README features table (Tabs row, sidebar description), docs/AGENTS.md (summary/model sources),
docs/CLI.md, docs/CONFIGURATION.md (shortcuts), docs/ARCHITECTURE.md (Node::Leaf groups).

## Testing

- `cargo test --lib`: group operations and invariants, `visible_panes` vs `panes`, neighbor/rects
  using only active tabs, session v1→v2 migration and round-trip, title qualification + glyph
  stripping, model-name shortening, prompt extraction from Claude/Codex payload fixtures, time-ago
  formatting.
- `tests/e2e.rs` via IPC: create tabs, write to a background tab and read its output, close last tab
  collapses the slot, move tab between groups, restart restores tabs/active tab, agent events with
  `prompt`/`model` show up in `pane.list`.
- `tests/interactions.rs`: tab click/close/shortcut behaviour and sidebar session-line click jumping
  to a background tab.
- Re-run `scripts/bench.sh`; startup and idle CPU must stay within docs/PERFORMANCE.md.

## Left to the builder

- Exact widget structure and CSS for the tab strip and session lines (match existing chips/sidebar
  style and theme tokens).
- Spinner glyph list for title stripping (start with Claude Code's `✳ ✶ ✻ ✽ ✢ · ⠋…⠿` family).
- Model-name shortening table.
- Whether `pane.select` is new or an extension of `pane.focus`.
- Order of implementation; suggested: Part 1 first (independent, smaller), then model groups, session
  migration, UI, shortcuts, IPC, docs.

## Calls made without explicit user input

- Shortcuts above (esp. macOS `<Meta><Shift>t` to keep `<Meta>t` = New workspace).
- Strip shown even with one tab.
- Edge-drop splitting and non-terminal tabs deferred.
- Own agent markers instead of brand logos.
- Sidebar capped at 5 session lines per workspace, then "+N more".
- Attention line kept only for non-agent notifications.

---

## Revision (2026-11-18, after the user saw the build): tabs belong to workspaces

Part 2 above (a tab strip on every split slot) was the wrong reading of the request. The user's
reference is Orca's sidebar: a workspace (`eu-promo-checkout`) is the top-level item and its sessions
are listed under it. Approved replacement:

```
┌────────────────────────┬─ ● Fix ACME-412 × │ ● tax-rules × │ + ─────┐  ← ONE tab bar per workspace
│ ▌eu-promo-checkout   ✕ │   (the selected tab's own split layout;    │
│   ● Fix ACME-412  opus │    split panes have NO tab strips)         │
│   ● tax-rules     gpt-5│                                            │
│   ○ promo-tests        │                                            │
└────────────────────────┴────────────────────────────────────────────┘
```

- **Model:** `Workspace { tabs: Vec<Tab>, active_tab }`, `Tab { id, layout: Node, focused }`. Each tab
  has its own split tree (tmux-window style). `Node::Leaf { pane }` is back to one pane per leaf.
  Ids for workspaces, tabs and panes share one id space.
- **Tab bar:** one per workspace, above its terminal area: tab label (agent + summary, else busy
  program, else folder), state dot, unread dot, ✕; `+` and double-click on empty bar open a tab;
  middle-click closes; drag reorders within the bar. Shortcuts as before (`new-tab`, `next-tab`,
  `prev-tab`); `close-pane` closes a pane, and closing a tab's last pane closes the tab (its
  workspace's last tab closes the workspace, as today).
- **Sidebar:** under each workspace, one line per tab (shown when the workspace has more than one
  tab or any agent): state dot, agent, summary, model, age — or the tab label for a plain shell tab.
  The active tab's line is highlighted. Click selects the tab.
- **Split panes:** exactly as in 0.3.0: ✕, ✥ swap handle (within the same tab) and compose button
  in the pane's corner, focus/attention ring on the pane.
- **Session:** version 2 stores tabs; a version-1 workspace (`layout`, `focused`) loads as one tab.
- **IPC/CLI:** `tab.new {workspace?, cwd?} → {tab, pane}`, `tab.select {tab}`, `tab.move {tab, index}`,
  `tab.close {tab, force?}`, `tab.list {workspace?}`; `pane.list` adds `tab`, `tab_index`,
  `active_tab`, `visible`; `workspace.list` adds `tabs`. `oxi tab new|list|select|move|close`.
- **Non-goals:** moving tabs between workspaces; per-pane tab strips.
- Part 1 (agent summary/model/age, title rules) is unchanged.
