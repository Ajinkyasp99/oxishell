# Architecture

```
 cmux (CLI, src/bin/cmux.rs)
   │ newline-delimited JSON over $XDG_RUNTIME_DIR/cmux-linux/cmux.sock (0600, SO_PEERCRED)
   ▼
 ipc.rs  ── accept thread ─ per-connection thread ─ validate ─▶ async channel ─▶ ui/ipc_handler.rs (GTK thread)
                                                                                  │
 ┌────────────────────────────── GTK main thread (ui/) ─────────────────────────┐ │
 │ mod.rs      App: window, actions, key handler, workspace/pane operations     │◀┘
 │ sidebar.rs  workspace rows, drag-reorder, context menu                       │
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
 │ paths.rs   XDG paths, private dirs, atomic writes                             │
 └──────────────────────────────────────────────────────────────────────────────┘
```

## Key decisions

- **Model is the source of truth.** `Node = Leaf { pane } | Split { orient, ratio, a, b }`. Every
  structural change mutates the model, then `rebuild_workspace` re-parents the existing pane widgets into a
  fresh `gtk::Paned` tree. Terminals are never recreated, so processes survive layout changes.
- **Workspaces are `gtk::Stack` pages.** Hidden workspaces stay alive and keep processing output.
- **VTE owns the PTY.** We pass env (`CMUX_PANE_ID`, `CMUX_WORKSPACE_ID`, `CMUX_SOCKET`,
  `TERM_PROGRAM=cmux-linux`) at spawn. Input sent before the shell is ready is queued until the tty
  leaves canonical mode and output is quiet (VTE drops input sent before the PTY exists).
- **Reading text** uses absolute VTE row numbers anchored on the cursor, because the scroll adjustment is
  relative to retained scrollback.
- **Re-entrancy:** VTE/GTK signal handlers defer to idle callbacks, so they never run while the model's
  `RefCell` is borrowed.
- **Process lifecycle:** close = SIGHUP to the process group, SIGKILL after 3 s if still alive; a shell
  that exits removes its pane; SIGTERM/SIGINT/SIGHUP to the app save the session first.
- **Metadata off the main thread:** one worker thread, jobs coalesced per pane, one `/proc` scan per
  batch, git results cached 5 s per directory.
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
