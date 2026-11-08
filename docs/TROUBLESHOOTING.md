# Troubleshooting

**`oxi: cannot connect to oxishell`** — the app is not running, or it runs with a different
`XDG_RUNTIME_DIR`. Inside an oxishell pane `OXISHELL_SOCKET` points at the right socket. If the runtime
directory path is longer than ~100 bytes, the socket falls back to `/tmp/oxishell-<uid>/<hash>.sock`.

**"Control socket disabled: … in use by another instance"** — another oxishell owns the socket.
Normally a second launch just focuses the first window. Close the other instance, or use
`OXISHELL_SOCKET=/path/to/other.sock` for a deliberate second instance with `OXISHELL_NON_UNIQUE=1`.

**`oxi pane send` says "sending input is disabled"** — intentional default; set `[ipc] allow_send = true`.

**Agent notifications do not appear**
1. `oxi hooks list` — is the agent on `PATH`? Then `oxi hooks install <agent>`.
2. Start the agent *inside* an oxishell pane (`echo $OXISHELL_PANE_ID` must print a number).
3. Restart the agent after installing hooks. For Codex, open `/hooks` and review and trust the oxishell
   hooks. Use a CLI with lifecycle hook support and check that `[features] hooks` is not disabled.
4. `OXISHELL_DEBUG=1 oxi agent-event --agent claude --state waiting` prints connection errors.
5. Desktop pop-ups need a notification daemon (GNOME/KDE provide one). In-app rings, badges and the
   notifications panel work without one.

**Config errors** are shown as a banner at startup; the app uses defaults until the file is fixed.
`[shortcuts]` conflicts are listed too.

**Wrong shell** — set `[terminal] shell = "/usr/bin/fish"`. Otherwise `$SHELL` is used.

**Terminals start empty after a restart** — by design: workspaces, splits, folders and agent sessions
are restored, but each terminal starts clean with the prompt at the top. To also see the previous output,
set `[session] replay_lines = 500` (Settings → Replay previous output). Note that replayed output sits
above the prompt and moves down when the window is resized (that is how terminal scrollback works).

**Processes are gone after restarting the app** — expected: closing the app ends its shells. Workspaces,
layouts, directories and recent output come back; running programs do not.

**Notifications from `OSC 9 / 777` escape sequences are ignored** — Ubuntu's VTE build does not expose
them (see docs/TECH_STACK.md). Use `oxi notify`, hooks, or the terminal bell.

**Slow first start in unusual sessions** (containers, bare `dbus-run-session`) — GTK waits for the
accessibility bus. `GTK_A11Y=none oxishell` avoids that; normal desktop sessions are not affected.

**Blank or garbled rendering on old GPUs/VMs** — try `GSK_RENDERER=cairo oxishell`.

**Logs** — run `oxishell` from a terminal; errors are printed to stderr as well as shown in the window.

**`N: Download is performed unsandboxed as root ... couldn't be accessed by user '_apt'`** when
installing a local .deb — harmless; apt could not read the file as its sandbox user because it is inside
your home directory, so it read it as root. Copy the .deb to `/tmp` first to avoid the note.
