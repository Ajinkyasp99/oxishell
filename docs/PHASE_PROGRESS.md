# Phase Progress

Status rules: **Completed** only with test evidence; **In Progress** = implemented but not fully
verified; **Blocked** = cannot be verified in this environment (reason given).

Evidence keys: **U** = unit tests (`cargo test --lib`, 37 tests), **E** = end-to-end tests
(`tests/e2e.rs`, real app + real shells; passed on Xvfb/X11 and on headless GNOME Shell Wayland),
**K** = real keyboard/mouse events via xdotool on X11, **S** = screenshot reviewed, **M** = manual CLI run.

| Phase | Feature | Status | Tests | Notes |
|---|---|---|---|---|
| 0.1 | Reference analysis | Completed | Paths/licenses verified vs clone `be5e243` | cmux-tui license inconsistency (MIT metadata vs GPL root) open |
| 0.2 | Terminal feasibility | Completed | Spike: VTE spawned bash under Xvfb | libghostty not hands-on tested (Zig missing) |
| 0.3 | GUI stack | Completed | GTK 4.14/libadwaita 1.5 build + run | |
| 0.4 | Architecture | Completed | n/a | ARCHITECTURE.md |
| 0.5 | Docs | Completed | n/a | |
| 1.1 | Project initialization | Completed | `cargo build` | |
| 1.2 | Native window | Completed | E, S | |
| 1.3 | Main layout | Completed | S | header, sidebar, panes, status bar |
| 1.4 | Dark/light appearance | Completed | S (dark), U (config) | light mode switch not screenshot-verified |
| 1.5 | Desktop launcher | Completed | desktop-file-validate | |
| 1.6 | Open/close | Completed | E (start, SIGTERM exit, socket removed) | |
| 2.1–2.3 | Engine, PTY shell, live output | Completed | E | VTE owns the PTY |
| 2.4 | Keyboard input | Completed | K (typed text runs), E | |
| 2.5 | Resize | Completed | S (bash rewraps after splits) | |
| 2.6 | Colors, Unicode | Completed | E (`é中`, ANSI stripped in reads), S (emoji, colors) | |
| 2.7 | Copy/paste, selection, scrollback | Completed | E (scrollback read past 10k lines), K (search bar) | clipboard copy/paste not asserted automatically |
| 2.8 | vim/top/ssh/completion | Completed | M (vi saved file, top, tab completion, ssh password prompt) | full ssh login not tested (would use your keys) |
| 2.9 | Exit & cleanup | Completed | E (exit closes pane, pids reaped), M (SIGHUP-ignoring shell SIGKILLed after 3 s) | |
| 3.1 | Workspace model | Completed | U | |
| 3.2–3.5 | Sidebar, create, switch, rename | Completed | E, K, S | |
| 3.6 | Safe close | Completed | E (refuses with running programs unless forced) | |
| 3.7 | Active indicators | Completed | S | |
| 3.8 | Keyboard navigation | Completed | K (Ctrl+PageUp, Alt+2), U (config) | |
| 4.1 | Pane layout model | Completed | U | |
| 4.2–4.4 | Splits, nested | Completed | E, K | |
| 4.5 | Draggable resize | In Progress | S | dividers render; drag→ratio persistence not exercised by a test |
| 4.6 | Pane focus navigation | Completed | U, K (Alt+Left) | |
| 4.7 | Safe pane close | Completed | E, K (Ctrl+Shift+W) | |
| 4.8 | Independent shells | Completed | E (distinct pids, output isolated) | |
| 5.1 | Toolbar/sidebar styling | Completed | S | |
| 5.2 | Theme selection | In Progress | U (validation), S (dialog) | live theme switch not screenshot-verified |
| 5.3 | Font settings | In Progress | S (dialog) | font change not exercised by a test |
| 5.4 | Sidebar collapse | Completed | S (auto-hide < 640 px), K | |
| 5.5 | Context menus | Completed | K (workspace Close/Move Down, pane Split Right executed) | found and fixed a hang (ListBox row removal) |
| 5.6 | Shortcut settings | Completed | U (parse, conflicts), M (invalid config reported) | |
| 5.7 | Focus states, spacing | Completed | S | |
| 5.8 | Small/large windows | Completed | S (1200×760 and 560×380) | |
| 6.1 | cwd detection | Completed | E (`/tmp` pane), U | via /proc of the foreground process |
| 6.2–6.4 | Git repo, branch, status | Completed | U (real `git init` repo), S | |
| 6.5 | Directory display | Completed | S | |
| 6.6 | Listening ports | Completed | U, E (port 38419 detected, background workspace too) | own processes only |
| 6.7 | Efficient refresh | Completed | PERFORMANCE.md (idle CPU 1.2 % at 20 panes) | |
| 7.1–7.2 | Notification model, CLI notify | Completed | U, E | |
| 7.3 | Desktop notifications | In Progress | code path only | disabled in tests to avoid popping on the host desktop; not visually verified |
| 7.4–7.5 | Badges, attention rings | Completed | E (unread counts), S | |
| 7.6 | Notifications panel | Completed | K (Ctrl+Shift+I), S (lists title, workspace, time, source, body) | |
| 7.7 | Jump to unread | Completed | E | |
| 7.8 | OSC 9/99/777 | Completed (evaluation) | symbol check of libvte | Ubuntu VTE lacks `notification-received`; documented, bell used instead |
| 8.1 | Agent session identification | Completed | U (process matching) | process name only |
| 8.2 | Agent interface | Completed | E (`agent.event`) | |
| 8.3 | Codex | Completed | U (config.toml notify install), E (payload) | not run against a real Codex binary |
| 8.4 | Claude Code | Completed | U (settings.json merge), E (stdin payload) | not run against a real Claude Code session |
| 8.5 | OpenCode | In Progress | U (plugin file) | plugin JS not executed by OpenCode here |
| 8.6 | Gemini CLI | Completed | U (install/uninstall) | not run against real Gemini |
| 8.7 | Generic hooks | Completed | E (`agent-event` for any agent) | |
| 8.8 | Agent metadata display | Completed | S ("Claude Code — Needs input") | |
| 8.9 | Simultaneous agents | Completed | E (claude + codex states on different panes) | |
| 9.1–9.3 | Command palette & commands | Completed | K (fuzzy "splitdown" + Enter) | |
| 9.4–9.7 | Socket server, CLI, list/create/split | Completed | U (server), E | |
| 9.8 | Send input with safety | Completed | E (disabled by default; enabled works) | |
| 9.9 | notify & navigation | Completed | E | |
| 9.10 | Socket security | Completed | U, E (0600/0700, peer uid, method allow-list, size limits) | |
| 10.1–10.2 | Config schema, persistence | Completed | U | |
| 10.3–10.6 | Save/restore workspaces, layout, cwd | Completed | E (restart restores names, layout, `/tmp` cwd) | |
| 10.7 | Scrollback restoration | Completed | E (display-only, ran once, escapes stripped) | |
| 10.8 | tmux/zellij backend | Blocked | — | tmux backend implemented but tmux is not installed here; zellij not implemented |
| 10.9 | Agent resume | In Progress | U (command validation), M (restore toast) | resume action not exercised end to end |
| 10.10 | Interrupted shutdown, corrupt files | Completed | U, E (SIGTERM save, corrupt file set aside) | |
| 11.1–11.3 | Startup, idle CPU, memory | Completed | scripts/bench.sh → PERFORMANCE.md | Xvfb/software rendering |
| 11.4–11.7 | Output volume, 16–20 panes, switching, churn | Completed | E (stress test) | |
| 11.8 | Leak/lifecycle fixes | Completed | E regression tests | fixed: text read range, input before shell ready, background-workspace ports, sidebar busy-loop hang, stale focus marking notifications read, per-pane /proc scans |
| 11.9 | Signals & PTY cleanup | Completed | E, M | |
| 11.10 | Wayland and X11 | Completed | E on Xvfb and headless GNOME Shell 46 Wayland | no physical multi-monitor / HiDPI test |
| 12.1 | Reproducible builds | Completed | two clean builds → identical .deb (same machine) | cross-machine not verified |
| 12.2 | .deb | Completed | `apt install` on the dev machine (Ubuntu 24.04): installed, binaries on PATH, desktop entry + icon present | apt prints a harmless note when the .deb is in a home dir (`_apt` sandbox) |
| 12.3 | Icons & metadata | Completed | appstreamcli, desktop-file-validate | |
| 12.4 | Installation guide | Completed | docs/INSTALL.md | |
| 12.5 | Clean uninstall | Blocked | — | needs sudo to run `apt remove`; package lists every file it installs |
| 12.6 | Flatpak/AppImage evaluation | Completed | docs/PACKAGING.md | |
| 12.7 | GitHub Actions | In Progress | every step run locally | workflows not executed on GitHub yet |
| 12.8 | Release notes & troubleshooting | Completed | docs/RELEASE_NOTES.md, docs/TROUBLESHOOTING.md | |
