# Release notes

## 0.1.4 — 2026-11-14

- **File links:** `src/main.rs:42:7` (or Python's `File "app.py", line 12`) in any output opens your editor at
  that line when the file exists. Click for **Open in Editor / Copy Path**; Ctrl+click opens it directly.
  Uses Settings → Terminal → Editor, else `$VISUAL`, `$EDITOR` or an installed editor; vim, nano and other
  terminal editors open in a new pane.
- **Usage limit alerts:** one notification when Codex or Claude Code passes 80% and 95% of a 5-hour or weekly
  limit, and a note when that window resets. Once per window, also across restarts. Settings →
  Notifications → Usage limit alerts.
- **Workspace colour and icon:** right-click a workspace → Colour / Icon, or
  `oxi workspace set ID --color red --icon server`. Colours follow the current theme.
- **Background opacity:** Settings → Appearance → Background opacity (50–100%). Backgrounds become
  see-through, text stays solid. Needs a compositor; themes with a background image stay opaque.
- **Update notice (off by default):** Settings → Notifications → Check for updates asks GitHub at most
  once a day and shows a notice with the release notes link. Nothing is downloaded or installed, and with
  the setting off oxishell makes no network requests.
- Fixed: command block headers (`~/folder ⎇ branch ✓ 0.01s`) could be drawn on top of a running program's
  output, such as Claude Code's input box, after it cleared the scrollback or passed the scrollback limit.
- Fixed: notifications that belong to no workspace no longer say "closed" in the notifications panel.

## 0.1.3 — 2026-11-14

- Swap panes by dragging: every pane has a ✥ move handle left of its ✕ close button. Drop it on another
  pane in the same workspace and the two trade places, with short animations while you drag.
  Shells, scrollback and running agents keep going.
- Drag where two dividers meet (┤ ├ ┬ ┴ ┼) to resize in both directions at once.
- History panel: the newest command is no longer cut off at the bottom.

## 0.1.1 — 2026-11-11

- **Renamed from cmux-linux to oxishell.** The package, binary and app are now `oxishell`, and the
  download is `oxishell_amd64.deb`. The CLI is `oxi`.
- Codex lifecycle hooks report running, needs input (approvals and explicit questions), completion,
  interruption and session end. Installation preserves custom hooks and notification programs and
  migrates the old oxishell notifier. Restart Codex and review and trust the hooks in `/hooks`.
- Codex and Claude Code 5-hour and weekly usage limits in the sidebar.
- Bundled Hack font, font zoom shortcuts, improved link detection, and scroll and crash fixes.

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
desktop notifications; hooks for Claude Code, Codex, Gemini CLI, Qoder CLI and OpenCode (`oxi hooks
install`); bell-as-attention; foreground agent detection; explicit, user-confirmed agent session resume.

**Themes** — 41 built-in Warp themes (Apache-2.0, warpdotdev/themes) that colour the whole window,
including gradient backgrounds and accents; a visual theme gallery in Settings; "Theme: …" entries in the
command palette; any Warp `.yaml` theme (with background image) can be added to `~/.config/oxishell/themes/`.

**Suggestions & chips** — Warp/fish-style inline command suggestions from bash/zsh/fish history
(accept with →), a command-history search popup (Ctrl+Shift+H), and context chips above each terminal
showing the active Python venv/conda env, folder and git branch.

**Prompt & cursor** — compact `❯` prompt inside oxishell panes (green, red after a failed command;
user configs untouched); cursor shape block / vertical line / horizontal line, with optional blinking.

**Productivity** — command palette with custom commands, configurable shortcuts, light/dark/system mode
for the default theme, settings dialog, auto-hiding sidebar in narrow windows.

**Automation** — `oxi` CLI and newline-JSON Unix socket API (0600, peer-uid checked); text injection
disabled unless enabled.

**Sessions** — workspaces, layouts, directories and agent session ids restored on start, each terminal
clean at the top; optional display-only replay of previous output (`replay_lines`, off by default); graceful save on SIGTERM/SIGINT/SIGHUP; corrupt session files are set aside.

### Known limitations
- OSC 9/99/777 desktop-notification sequences are not available with Ubuntu's VTE build.
- Running programs are not restored after the app restarts (layouts, directories and output are).
- Codex and Qoder hooks do not report a "running" state.
- Tested on X11 (Xvfb) and headless GNOME Shell Wayland; not yet on physical multi-monitor/HiDPI setups.
- In-app browser, cloud features and remote daemons from cmux are out of scope.
