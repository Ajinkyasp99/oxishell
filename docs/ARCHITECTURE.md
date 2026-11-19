# Architecture

```
 oxi (CLI, src/bin/oxi.rs)
   │ newline-delimited JSON over $XDG_RUNTIME_DIR/oxishell/oxishell.sock (0600, SO_PEERCRED)
   ▼
 ipc.rs  ── accept thread ─ per-connection thread ─ validate ─▶ async channel ─▶ ui/ipc_handler.rs (GTK thread)
                                                                                  │
 ┌────────────────────────────── GTK main thread (ui/) ─────────────────────────┐ │
 │ mod.rs      App: window, actions, key handler, workspace/pane operations     │◀┘
 │ sidebar.rs  workspace rows, agent session lines, drag-reorder, context menu  │
 │ tabs.rs     workspace tab bar + stack of tab layouts, tab DnD reorder         │
 │ pane.rs     VTE terminal per pane (owns PTY + child), search, lifecycle      │
 │ dialogs.rs  palette, settings, notifications panel, rename/confirm           │
 │ meta_worker.rs ── jobs ─▶ worker thread (procfs, git) ── updates ─▶ back      │
 └──────────────┬───────────────────────────────────────────────────────────────┘
                │ reads/mutates
 ┌──────────────▼──────────────── GTK-free library (unit-tested) ───────────────┐
 │ model.rs   AppState { workspaces: [Workspace { layout: Node tree, focused }] }│
 │ config.rs  TOML config + shortcut table      theme.rs   color themes          │
 │ notify.rs  notification center (unread, history, sanitizing)                  │
 │ session.rs snapshot save/load, scrollback sanitizer                           │
 │ meta.rs    cwd / foreground process / ports from /proc, git porcelain parser  │
 │ agents.rs  hook install/uninstall per agent, payload parsing, resume commands │
 │ agent_info.rs  session summary / model / age / tab label text                 │
 │ paths.rs   XDG paths, private dirs, atomic writes                             │
 └──────────────────────────────────────────────────────────────────────────────┘
```

## Key decisions

- **Model is the source of truth.** `Workspace { tabs: [Tab { id, layout: Node, focused }], active_tab }`,
  `Node = Leaf { pane } | Split { orient, ratio, a, b }`. Each tab has its own split tree; workspaces, tabs
  and panes share one id space. Every structural change mutates the model, then `rebuild_workspace`
  re-parents the existing pane frames into the workspace page: a tab bar (`ui/tabs.rs`) over a
  `gtk::Stack` with one `gtk::Paned` tree per tab. Terminals are never recreated, so processes survive
  layout and tab changes; hidden tabs keep running. Session files from before tabs (version 1, a
  workspace with `layout` + `focused`) load as one tab.
- **Workspaces are `gtk::Stack` pages.** Hidden workspaces stay alive and keep processing output.
- **VTE owns the PTY.** We pass env (`OXISHELL_PANE_ID`, `OXISHELL_WORKSPACE_ID`, `OXISHELL_SOCKET`,
  `TERM_PROGRAM=oxishell`) at spawn. Input sent before the shell is ready is queued until the tty
  leaves canonical mode and output is quiet (VTE drops input sent before the PTY exists).
- **Reading text** uses absolute VTE row numbers anchored on the cursor, because the scroll adjustment is
  relative to retained scrollback.
- **Re-entrancy:** VTE/GTK signal handlers defer to idle callbacks, so they never run while the model's
  `RefCell` is borrowed.
- **Process lifecycle:** close = SIGHUP to the process group, SIGKILL after 3 s if still alive; a shell
  that exits removes its pane; SIGTERM/SIGINT/SIGHUP to the app save the session first.
- **Metadata off the main thread:** one worker thread, jobs coalesced per pane, one process-table read
  and one batched listening-ports lookup per batch (`/proc` on Linux, `lsof` on macOS). Per-pane back-off
  schedules (`src/meta_schedule.rs`) slow ports and git checks while nothing changes; Compose scans are
  cached. Details in PERFORMANCE.md.
- **Security:** socket and state files are owner-only; IPC methods are allow-listed and typed; text
  injection is opt-in; notification and scrollback text is sanitized; restore never executes commands;
  agent session ids are validated before being placed in a resume command, which is typed but not run.

## Testing layers

- Library: `cargo test --lib` (model, config, IPC server/permissions, notifications, sessions, hooks, git, procfs).
- End to end: `tests/e2e.rs` starts the real app with isolated XDG dirs and drives it through the socket,
  asserting on real shell output and real process state. Runs under Xvfb or Wayland.
- Benchmarks: `scripts/bench.sh`.

## Extension points

- Another terminal engine (e.g. a future GTK-embeddable libghostty) would replace `ui/pane.rs`; the
  model, IPC, sessions and metadata do not depend on VTE.
- New agents: add an entry to `agents.rs` (`nested_events`, `settings_path`) and `meta::AGENTS`.
