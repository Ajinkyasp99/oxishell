# Configuration

`~/.config/cmux-linux/config.toml` (or `$XDG_CONFIG_HOME/cmux-linux/config.toml`). Every key is
optional. The Settings dialog (Ctrl+,) edits the same file. Unknown keys and invalid values are reported
in the window and the app falls back to defaults instead of failing.

```toml
[appearance]
mode = "system"          # system | light | dark
theme = "default"        # "default" or any theme id, e.g. warp_dark, cyber_wave, tokyo_night (see THEMES.md)
font = "Monospace 11"    # any Pango font description
sidebar_width = 230
sidebar_visible = true   # also hidden automatically below 640 px window width

[terminal]
shell = ""               # empty: $SHELL, then your passwd shell, then /bin/sh
scrollback_lines = 10000
confirm_close_running = true

[notifications]
desktop = true           # desktop notification when the window/pane is not focused
bell_attention = true    # terminal bell marks the pane for attention (max once per 5 s)

[session]
restore = true           # restore workspaces, layouts and directories on start
scrollback_lines = 500   # lines of output saved per pane, shown read-only on restore (0 = off)

[ipc]
enabled = true
allow_send = false       # allow `cmux pane send` to type into panes

[shortcuts]              # action = GTK accelerator; "" disables
split-right = "<Ctrl><Shift>d"

[[commands]]             # shown in the command palette as "Run: <name>"
name = "Test"
command = "cargo test"
```

Shortcut actions: `new-workspace`, `close-workspace`, `rename-workspace`, `next-workspace`,
`prev-workspace`, `split-right`, `split-down`, `close-pane`, `focus-left|right|up|down`, `copy`, `paste`,
`search`, `command-palette`, `toggle-sidebar`, `jump-unread`, `notifications`, `zoom-in`, `zoom-out`,
`zoom-reset`, `settings`. Conflicting bindings are reported when the app starts.

Environment variables: `CMUX_SOCKET` (socket path), `CMUX_GEOMETRY=WxH` (initial window size),
`CMUX_NON_UNIQUE=1` (allow a second independent instance, used by tests).

## Files

| Path | Contents |
|---|---|
| `~/.config/cmux-linux/config.toml` | settings (mode 0600) |
| `~/.local/state/cmux-linux/session.json` | workspaces, layouts, pane directories, agent session ids |
| `~/.local/state/cmux-linux/scrollback/*.txt` | sanitized plain-text output per pane |
| `$XDG_RUNTIME_DIR/cmux-linux/cmux.sock` | control socket (0600, directory 0700) |

A corrupt `session.json` is renamed to `session.json.corrupt` and the app starts fresh. Restoring never
runs commands: saved output is stripped of escape sequences and displayed only.
