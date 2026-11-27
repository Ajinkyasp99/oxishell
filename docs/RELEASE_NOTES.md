# Release notes

## 0.7.0 — 2026-11-27

- **Project Explorer and file editor:** open the explorer from the folder button in the header, the main menu
  or the command palette. It lists the focused terminal's Git worktree (or its folder outside Git) beside the
  terminal; opening a text file brings up an editor there. UTF-8 files up to 1 MiB, autosave one second after
  you stop typing (Settings → Terminal → Editor changes the delay or turns it off), Ctrl+S on Linux and ⌘S on
  macOS save at once. A file changed by another program refreshes a clean editor; an editor with unsaved edits
  keeps them, pauses autosave and offers **Reload from Disk**. Clicking a file path in terminal output offers
  **Edit File**, **Preview**, **Open in External Editor** and **Copy Path**.
- **File icons and Git letters:** explorer entries show a coloured icon for their type (Rust, TypeScript,
  Markdown, config, images, lock files…) and a Git letter when they changed: **M** modified, **A** added,
  **U** untracked, **D** deleted, **R** renamed, **!** conflict. Folders with changes inside get a dot. Git runs
  only when the explorer opens or refreshes, after a save, when an agent finishes its turn and when the window
  comes back to the front, never on a timer.
- **Diff view:** a changed file has a **Diff** button in the editor header. It shows the file's changes since
  the last commit (staged and unstaged together) with added lines on green and removed lines on red, without
  the file headers. Untracked files show as entirely added. Select **Diff** again to return to the editor.
- **Terminal padding and line spacing:** text now starts 8 px from the pane edge instead of 1 px. Settings →
  Appearance has **Padding** (0 to 32 px) and **Line spacing** (1.0 to 2.0, default 1.0 keeps today's look);
  in the config file they are `[terminal] padding` and `line_height`. Command-block headers, history
  suggestions and the history panel follow the padding.
- **Smoother panels:** the sidebar and the file panel fade in and out, the explorer slides to a file preview and
  back, a workspace's session list slides open, and new sidebar rows fade in, each in 120 to 160 ms. A pane
  that takes focus from a neighbour flashes its border once, and pane dividers show the accent colour on
  hover. Mouse-wheel scrolling through scrollback glides instead of jumping a page. All motion turns off with
  GNOME's "Reduce animation" on Linux and with System Settings → Accessibility → Display → "Reduce motion" on
  macOS.
- **Less CPU while an agent works in the background:** the pulsing dot in the header stops while the window is
  behind other windows. With an agent working and oxishell in the background, CPU use went from about 31% to
  under 1% (Linux, software rendering). Idle CPU and memory are otherwise unchanged; numbers are in
  `docs/PERFORMANCE.md`.
- Checked on Linux: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, the unit tests, and the e2e
  and interaction suites under Xvfb (three new GUI tests cover the explorer's icons, Git letters and diff, the
  padding and line spacing, and the focus flash). Two tests that also fail on 0.6.0 still fail
  (`usage_footer` GTK initialisation, and `switching_back_before_a_new_tab_is_shown_settles`). macOS: the
  clippy cross-compile check passes, but nothing was run on a Mac. "Reduce motion", the fades and the padding
  there are untested until someone tries them by hand.

## 0.6.0 — 2026-11-24

- **Compose picker redesigned:** the file list now works like a command bar. The filter has the keyboard from
  the moment the picker opens, so you just type; it matches file paths and service names (`redis` finds every
  file with a redis service and underlines the match), and the services of each file are no longer cut off by
  buttons. Files stay grouped by folder, and while you filter each folder header shows how many of its files
  match (`▾ repo root · 3 of 8 match`). The picker is wider (540 px) and no longer grows with the longest line;
  long lines end in an ellipsis.
- **Last starts:** under the filter, the last three `up` or `up -d` runs of files in the current repo appear as
  pills (`↺ 1 local-infra · up -d`) that sit side by side and wrap to the next row when a row is full. Click
  one, or press Ctrl+1 to Ctrl+3 (⌘1 to ⌘3 on macOS), to run it again. Only starts are remembered, never
  `down`, `logs` or `restart`, and the list is kept across restarts in `~/.local/state/oxishell/compose-starts.json`.
- **Actions on the selected file only:** the `d -d`, `x down` and `l logs` buttons that repeated on every row
  are gone. The selected file shows its actions as chips: `up -d`, `up`, `down`, `logs`, `restart`. Enter runs
  the default action, Tab and Shift+Tab walk the chips, Esc steps back out of them, and every chip can also
  be clicked.
- **New picker chords:** Ctrl+X runs `down`, Ctrl+L follows logs, Ctrl+R rescans. On macOS they are ⌘X, ⌘L and
  ⌘R, so Ctrl stays with the shell. Because the filter takes typing, the old single keys (`d`, `x`, `l`, `u`,
  `j`, `k`, `r`, `/`) no longer act as shortcuts. On macOS ⌘X therefore no longer cuts text in the filter box.
- **Run a stopped Compose stack again:** when a stack started from the launcher stops ([stop], Ctrl+C or
  `down`), its bar shrinks to one line: `↺ docker-compose.yml  stopped just now · 4 services  [run up] [up -d]
  [×]`. One click starts the same stack again in the pane. The line stays while you run up to two more
  commands, then goes away. Same on Linux and macOS.
- Checked on Linux: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, the compose unit tests and
  the GUI suites under Xvfb. macOS: the picker code is shared and the chords use ⌘ there, but it is not checked
  on a Mac yet (the cross-compile check could not build a C dependency on the Linux machine), so the macOS
  picker is only covered by the CI `macos` job until someone tries it by hand.

## 0.5.0 — 2026-11-24

- **New usage meters:** the sidebar footer shows each agent's 5-hour and weekly limit as a row of cells: five
  for the hours of the 5-hour window, seven for the days of the week. Cells fill as you use the limit, a
  white underline marks the hours and days already gone, and the reset time and date sit under each row
  (`resets 3:10 PM · 24 Nov`). The row turns amber at 90% used. The footer is taller than before (about
  220 px with both agents).
- **Official Claude and Codex logos:** the agent marks in the sidebar are now the real Claude and OpenAI
  (Codex) marks instead of hand-drawn look-alikes. Gemini keeps a simple star, and other agents get a terminal
  mark. The marks are one colour and follow the theme.
- **Agent sessions in the sidebar:** each session line now shows the agent's logo instead of its name, with a
  state glyph: a blue braille spinner while working, an amber blinking cursor when it needs you, a green check
  when done. The workspace name carries the glyph of its most urgent agent. An agent that the hooks report as
  running but whose terminal has been silent for 15 seconds is shown as done, because hooks do not report an
  interrupted turn (Esc). Spacing is looser and the folder path is no longer shown.
- **Every agent in a split tab is listed:** the sidebar now shows one session line per agent pane, not one
  per tab, so three agents in a tab split three ways get three lines. Clicking a line focuses that pane.
- **Needs input no longer blinks:** the sidebar marks an agent that needs you with a steady amber `?`
  instead of a blinking cursor.
- **Tab names no longer carry the agent's state:** a tab reads `Claude: Fix login bug` whether the agent is
  working, needs input or is done. The state is now the sidebar's glyph on the tab's left (spinner, steady
  `?`, `✓`) instead of a plain dot, for the most urgent agent in the tab.
- **Tab headers in the sidebar:** when agents run in more than one tab of a workspace, each tab's agents are
  listed under a header with the tab's name (or `Tab N` if it has none); clicking the header shows that tab.
- **Default theme is now Cyber Wave** for new installs and configs that do not set `theme`. Set
  `theme = "default"` for plain libadwaita.
- **History panel** opens at the width of the terminal, right under the prompt line, and is reused instead
  of rebuilt on every ↑.
- Checked on Linux (unit tests, the GUI suites under Xvfb, and the usage meters rendered in a test window).
  macOS: the code is shared and the footer uses no OS-specific calls, but it is only compile-checked by CI
  and has not been tried on a Mac.

## 0.4.0 — 2026-11-19

- **Command history no longer misses commands:** oxishell used to rebuild each command from the keys you
  pressed, and gave up on any line built with Tab completion, the arrow keys (including ↑ recall), Ctrl+R,
  Ctrl+A/E/W or a paste, so those commands never reached the history popup or the inline suggestions. The
  shell itself now reports every command it runs (bash, zsh, and fish with the `oxi shell-integration fish`
  line), so history is exact whatever keys built the line, and commands from one pane show up in the other
  panes straight away. Lines the shell does not keep (a leading space with `HISTCONTROL=ignorespace`, empty
  Enter) are not added, and multi-line commands are still left out. Shells without the integration (a
  `PROMPT_COMMAND` your own config replaces, other shells) keep the old key-following behaviour.
  Checked on Linux (bash and zsh by tests, the GUI suites, and by hand); the fish hook is untested, and
  macOS is only compile-checked by CI, not yet tried on a Mac.

## 0.3.1 — 2026-11-19

- **Lighter in the background:** listening ports and git status are checked less often while nothing
  changes (ports every 9 s, backing off to 30 s; git every 5 s, backing off to 30 s), and a change in the
  pane (a command starting or finishing, a new prompt, switching workspace) is picked up on the next 3 s tick, plus two quick follow-ups. Docker
  Compose detection and file scans are cached. While the window is minimized the CPU/RAM readout pauses and
  the 3 s tick only serves panes running a Compose stack, while the 15 s pass keeps running for all panes. A window the compositor marks as suspended (for example on another virtual desktop on some systems) counts as hidden too, not only a minimized one. On Linux an idle repo pane starts about 3 helper
  processes a minute instead of 14 (3 instead of 31 with four panes).
- **Faster history import** (20,000 entries: about 16 ms instead of 400 ms) and slightly less memory per pane.
- **Smaller macOS app:** the disk image drops the unused cursor theme and ICU (VTE is now built without it).
  The new size and UTF-8 text, cursors and minimize behaviour on macOS are not yet checked on a Mac.
- **Homebrew installs the app:** `brew install Ajinkyasp99/oxishell/oxishell` now installs the cask, i.e.
  `Oxishell.app` in Applications (Launchpad, Spotlight) with `oxi` linked. In 0.3.0 that command picked the
  same-named formula, which only builds command-line binaries, so no app appeared. The CLI-only formula is
  now `oxishell-cli`. If you installed the 0.3.0 formula: `brew uninstall --formula oxishell && brew install
  --cask Ajinkyasp99/oxishell/oxishell`.
- **Website:** the marketing site now covers workspace tabs, agent sessions in the sidebar, history search and
  inline suggestions, context chips and the CPU/memory readout. It links to the Medium article about oxishell
  and notes that what a downloaded package includes depends on its release, with a link to these notes.

## 0.3.0 — 2026-11-18

- **macOS support** (new, please report issues): a self-contained `Oxishell.app` for Apple silicon and Intel
  as a `.dmg`, and a Homebrew formula (`brew install Ajinkyasp99/oxishell/oxishell`). Shortcuts use ⌘
  (⌘T, ⌘D, ⌘W, ⌘C/⌘V, ⌘1…9, ⇧⌘P …), ⌘-click opens links, notifications go to Notification Center with
  macOS sounds, panes start login shells, and the sidebar's folder, git, ports, agent and CPU/memory
  readouts work as on Linux.
- **zsh integration is automatic** (environment chip, ❯ prompt, command blocks and "command finished"
  notifications), as it already was for bash. A line added earlier with `oxi shell-integration zsh` does no harm.
- Releases: one tag now builds the Ubuntu .deb and both macOS disk images at the same version and publishes
  them together, with one `SHA256SUMS`.

## 0.2.1 — 2026-11-15

- Fixed: the Compose stack bar listed services from other compose files in the same folder (they share
  one Compose project, so `docker compose -f <file> ps` reports all of them). It now shows only the
  services the chosen file declares, as `docker compose config --services` resolves them.
- **Stack bar colours:** a running service's chip is green, yellow while its healthcheck is starting,
  red when it crashed, is unhealthy or keeps restarting, and dim once a one-shot job finished or you
  stopped it. The status dot shows the whole stack's state; **rebuild** uses the accent colour and
  **restart** yellow, next to the red **stop**. The colours follow the theme's palette.

## 0.2.0 — 2026-11-15

- **Docker Compose launcher:** a button in each pane (or Ctrl+Shift+L) lists the repo's compose files.
  Pick one to run `docker compose -f <file> up` in the pane (or `-d`, `down`, `logs -f` from the row).
  While the stack runs, a bar under the terminal shows its services with **rebuild**, **restart** and
  **stop**, and a desktop notification with **Restart** reports a crashed service. Works with
  `docker compose`, `docker-compose` and `podman compose`; Settings → Workflow → Docker Compose launcher turns it off.
- **Privacy:** what you type into a program or a hidden prompt (`sudo`, `read -s`, password prompts)
  no longer ends up in the command history, its suggestions or command block notifications.
- Fixed: `oxi pane split --cwd <relative path>` opened the pane in your home folder; the path is now
  relative to where `oxi` runs, and a folder that does not exist is refused (also for `workspace new`).
- Fixed: a malformed colour in a theme file (e.g. `#aé123`) could crash oxishell; it is now rejected.
- Fixed: installing the OpenCode hooks could replace your own `plugins/oxishell.js`; oxishell now
  refuses to overwrite a plugin it did not create, and uninstall removes only its own.
- Docs: the development-build socket path in CONTRIBUTING.md works (a socket directly in `/tmp` is
  refused by design).

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
