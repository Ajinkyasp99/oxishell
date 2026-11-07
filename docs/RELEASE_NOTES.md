# Release notes

## Unreleased

- Codex lifecycle hooks report running, needs input (approvals and explicit questions), completion,
  interruption and session end. Installation preserves custom hooks and notification programs and
  migrates the old cmux notifier. Restart Codex and review and trust the hooks in `/hooks`.

## 0.1.0 — 2026-11-07 (first release)

Native GTK4/libadwaita terminal workspace manager for Linux, inspired by cmux.

**Terminal** — VTE-based: real PTYs, your login shell (bash, zsh, fish, …), 256-color/truecolor, Unicode
and emoji, mouse, hyperlinks, copy/paste (Ctrl+Shift+C/V), scrollback search (Ctrl+Shift+F), font
zoom, full-screen programs (vi/vim, top), interactive ssh prompts. Closing a pane with a running program asks first; shells
that ignore SIGHUP are killed after 3 s.

**Workspaces & splits** — sidebar with git branch/dirty count, cwd, listening ports, agent status and
unread badges; create, rename (double-click), reorder (drag), close with the ✕ button (always confirms); nested right/down splits with
draggable dividers and Alt+Arrow navigation.

**Notifications & agents** — attention rings, workspace badges, notifications panel, jump-to-unread,
desktop notifications; hooks for Claude Code, Codex, Gemini CLI, Qoder CLI and OpenCode (`cmux hooks
install`); bell-as-attention; foreground agent detection; explicit, user-confirmed agent session resume.

**Themes** — 41 built-in Warp themes (Apache-2.0, warpdotdev/themes) that colour the whole window,
including gradient backgrounds and accents; a visual theme gallery in Settings; "Theme: …" entries in the
command palette; any Warp `.yaml` theme (with background image) can be added to `~/.config/cmux-linux/themes/`.

**Suggestions & chips** — Warp/fish-style inline command suggestions from bash/zsh/fish history
(accept with →), a command-history search popup (Ctrl+Shift+H), and context chips above each terminal
showing the active Python venv/conda env, folder and git branch.

**Prompt & cursor** — compact `❯` prompt inside cmux-linux panes (green, red after a failed command;
user configs untouched); cursor shape block / vertical line / horizontal line, with optional blinking.

**Productivity** — command palette with custom commands, configurable shortcuts, light/dark/system mode
for the default theme, settings dialog, auto-hiding sidebar in narrow windows.

**Automation** — `cmux` CLI and newline-JSON Unix socket API (0600, peer-uid checked); text injection
disabled unless enabled.

**Sessions** — workspaces, layouts, directories and agent session ids restored on start, each terminal
clean at the top; optional display-only replay of previous output (`replay_lines`, off by default); graceful save on SIGTERM/SIGINT/SIGHUP; corrupt session files are set aside.

### Known limitations
- OSC 9/99/777 desktop-notification sequences are not available with Ubuntu's VTE build.
- Running programs are not restored after the app restarts (layouts, directories and output are).
- Codex and Qoder hooks do not report a "running" state.
- Tested on X11 (Xvfb) and headless GNOME Shell Wayland; not yet on physical multi-monitor/HiDPI setups.
- In-app browser, cloud features and remote daemons from cmux are out of scope.
