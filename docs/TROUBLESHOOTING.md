# Troubleshooting

**`cmux: cannot connect to cmux-linux`** — the app is not running, or it runs with a different
`XDG_RUNTIME_DIR`. Inside a cmux-linux pane `CMUX_SOCKET` points at the right socket. If the runtime
directory path is longer than ~100 bytes, the socket falls back to `/tmp/cmux-linux-<uid>/<hash>.sock`.

**"Control socket disabled: … in use by another instance"** — another cmux-linux owns the socket.
Normally a second launch just focuses the first window. Close the other instance, or use
`CMUX_SOCKET=/path/to/other.sock` for a deliberate second instance with `CMUX_NON_UNIQUE=1`.

**`cmux pane send` says "sending input is disabled"** — intentional default; set `[ipc] allow_send = true`.

**Agent notifications do not appear**
1. `cmux hooks list` — is the agent on `PATH`? Then `cmux hooks install <agent>`.
2. Start the agent *inside* a cmux-linux pane (`echo $CMUX_PANE_ID` must print a number).
3. Restart the agent after installing hooks.
4. `CMUX_DEBUG=1 cmux agent-event --agent claude --state waiting` prints connection errors.
5. Desktop pop-ups need a notification daemon (GNOME/KDE provide one). In-app rings, badges and the
   notifications panel work without one.

**Config errors** are shown as a banner at startup; the app uses defaults until the file is fixed.
`[shortcuts]` conflicts are listed too.

**Wrong shell** — set `[terminal] shell = "/usr/bin/fish"`. Otherwise `$SHELL` is used.

**Restored panes show old output** — that is the read-only replay of the last session (marked
"restored output above (not live)"). Disable with `[session] scrollback_lines = 0`, or disable restore
entirely with `restore = false`. A broken session file is moved to `session.json.corrupt`.

**Processes are gone after restarting the app** — expected with the default backend: closing the
app ends its shells. Use `[session] backend = "tmux"` for sessions that survive restarts.

**Notifications from `OSC 9 / 777` escape sequences are ignored** — Ubuntu's VTE build does not expose
them (see docs/TECH_STACK.md). Use `cmux notify`, hooks, or the terminal bell.

**Slow first start in unusual sessions** (containers, bare `dbus-run-session`) — GTK waits for the
accessibility bus. `GTK_A11Y=none cmux-linux` avoids that; normal desktop sessions are not affected.

**Blank or garbled rendering on old GPUs/VMs** — try `GSK_RENDERER=cairo cmux-linux`.

**Logs** — run `cmux-linux` from a terminal; errors are printed to stderr as well as shown in the window.

**`N: Download is performed unsandboxed as root ... couldn't be accessed by user '_apt'`** when
installing a local .deb — harmless; apt could not read the file as its sandbox user because it is inside
your home directory, so it read it as root. Copy the .deb to `/tmp` first to avoid the note.
