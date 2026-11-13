# Configuration

`~/.config/oxishell/config.toml` (or `$XDG_CONFIG_HOME/oxishell/config.toml`). Every key is
optional. The Settings dialog (Ctrl+,) edits the same file. Unknown keys and invalid values are reported
in the window and the app falls back to defaults instead of failing.

```toml
[appearance]
mode = "system"          # system | light | dark
theme = "default"        # "default" or any theme id, e.g. warp_dark, cyber_wave, tokyo_night (see THEMES.md)
font = "Hack 11"         # bundled Hack font; any Pango font description is accepted
sidebar_width = 230
sidebar_visible = true   # also hidden automatically below 640 px window width
chips = true             # context chips (Python env, folder, git) above each terminal
watermark = true         # faint workspace name centred behind the terminals
autohide_buttons = true  # header buttons slide in on hover; window controls always show

[terminal]
shell = ""               # empty: $SHELL, then your passwd shell, then /bin/sh
scrollback_lines = 10000
confirm_close_running = true
suggestions = true       # ghost-text suggestions from shell history; accept with → or End
compact_prompt = true    # prompt is just ❯ (red after a failed command) inside oxishell; new terminals
cursor_shape = "block"   # block (solid bar) | ibeam (vertical line) | underline (horizontal line)
cursor_blink = true
command_blocks = true    # Warp-style header above each command: folder, branch, ✓/✗ exit status, duration; new terminals
highlight_commands = true # typed commands in bold cyan, output in the normal colour (needs compact_prompt); new terminals
history_panel = true     # ↑ at the shell prompt opens the history panel, filtered by what is already typed
vivid_colors = true      # brighten the theme's pastel ANSI colours so coloured output (git, ls, compilers) stands out

[notifications]
desktop = true           # desktop notification when the window/pane is not focused
bell_attention = true    # terminal bell marks the pane for attention (max once per 5 s)
sound = true             # play a sound-theme sound with desktop notifications
command_finished = true  # notify when a long shell command ends in a pane you are not looking at
command_min_seconds = 10 # ... that ran at least this long

[session]
restore = true           # restore workspaces, layouts and directories on start
replay_lines = 0         # replay N lines of each pane's old output after restart (0 = off: clean prompt at the top,
                         # nothing written to disk). Replayed output moves down when the window is resized.

[ipc]
enabled = true
allow_send = false       # allow `oxi pane send` to type into panes

[shortcuts]              # action = GTK accelerator; "" disables
split-right = "<Ctrl><Shift>d"

[[commands]]             # shown in the command palette as "Run: <name>"
name = "Test"
command = "cargo test"
```

Shortcut actions: `new-workspace`, `close-workspace`, `rename-workspace`, `next-workspace`,
`prev-workspace`, `split-right`, `split-down`, `close-pane`, `focus-left|right|up|down`, `copy`, `paste`,
`newline`, `newline-alt`, `search`, `command-palette`, `toggle-sidebar`, `jump-unread`, `notifications`, `zoom-in`, `zoom-out`,
`zoom-reset`, `command-history`, `settings`. Conflicting bindings are reported when the app starts.

Environment variables: `OXISHELL_SOCKET` (socket path), `OXISHELL_GEOMETRY=WxH` (initial window size),
`OXISHELL_NON_UNIQUE=1` (allow a second independent instance, used by tests).

## Files

| Path | Contents |
|---|---|
| `~/.config/oxishell/config.toml` | settings (mode 0600) |
| `~/.local/state/oxishell/session.json` | workspaces, layouts, pane directories, agent session ids |
| `~/.local/state/oxishell/scrollback/*.txt` | sanitized plain-text output per pane (only when `replay_lines > 0`) |
| `$XDG_RUNTIME_DIR/oxishell/oxishell.sock` | control socket (0600, directory 0700) |

A corrupt `session.json` is renamed to `session.json.corrupt` and the app starts fresh. Restoring never
runs commands: saved output is stripped of escape sequences and displayed only.
