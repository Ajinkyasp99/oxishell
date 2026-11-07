# cmux-linux

A native Linux terminal for running many shells and AI coding agents side by side — a
Linux reimplementation inspired by [cmux](https://github.com/manaflow-ai/cmux) for macOS.

- **Native**: Rust + GTK4/libadwaita, terminals rendered by VTE. No Electron, no browser, no web terminal.
- **Workspaces**: a vertical sidebar of workspaces showing git branch, dirty count, working
  directory, listening ports, agent status and unread notifications. Background workspaces keep running.
- **Splits**: nested right/down splits, draggable dividers, Alt+Arrow focus navigation.
- **Agent notifications**: hooks for Claude Code, Codex CLI, Gemini CLI, Qoder CLI and OpenCode light up the
  pane (attention ring), badge the workspace, and send a desktop notification. Jump to the latest unread with Ctrl+Shift+U.
- **Automation**: the `cmux` CLI drives the running app over a private Unix socket.
- **Sessions**: workspaces, split layouts and directories are restored on restart; optional tmux backend keeps programs alive.

```text
┌──────────────────────────────────────────────────────────────┐
│ ☰ +            cmux — Backend               ▯▯  ▭  🔔2  ≡    │
├──────────────┬──────────────────────────┬────────────────────┤
│ WORKSPACES   │ $ pnpm dev               │ $ codex            │
│ Backend      │                          │                    │
│ ⎇ main ●3    │                          │                    │
│ Frontend  1  ├──────────────────────────┴────────────────────┤
│ AI Agent  1  │ $ docker compose ps                           │
│ Claude — Needs input                                         │
├──────────────┴───────────────────────────────────────────────┤
│ Workspace: Backend | ~/src/api | Branch: main ●3 | Agent: —  │
└──────────────────────────────────────────────────────────────┘
```

## Install

Ubuntu 24.04+ (or any distro with GTK ≥ 4.14, libadwaita ≥ 1.5, VTE-GTK4 ≥ 0.76):

```bash
sudo apt install ./cmux-linux_0.1.0_amd64.deb
```

See [docs/INSTALL.md](docs/INSTALL.md) for building from source and uninstalling.

## Use

| Action | Shortcut |
|---|---|
| New workspace | Ctrl+Shift+N |
| Split right / down | Ctrl+Shift+E / Ctrl+Shift+O |
| Close pane | Ctrl+Shift+W |
| Move focus between panes | Alt+Arrow |
| Next / previous workspace | Ctrl+PageDown / Ctrl+PageUp, Alt+1…9 |
| Command palette | Ctrl+Shift+P |
| Copy / paste | Ctrl+Shift+C / Ctrl+Shift+V |
| Search scrollback | Ctrl+Shift+F |
| Jump to latest unread | Ctrl+Shift+U |
| Notifications panel | Ctrl+Shift+I |
| Toggle sidebar | Ctrl+Shift+B |
| Font size | Ctrl+= / Ctrl+- / Ctrl+0 |
| Settings | Ctrl+, |

All shortcuts are configurable (Settings → Shortcuts, or `[shortcuts]` in the config file).
Right-click a workspace or a terminal for context menus; drag workspaces to reorder; double-click to rename.

### Agent notifications

```bash
cmux hooks install --all      # every supported agent found on PATH
cmux hooks install claude     # or one at a time: claude, codex, gemini, qoder, opencode
```

Hooks only report inside cmux-linux panes and are silent no-ops elsewhere. Details: [docs/AGENTS.md](docs/AGENTS.md).

### CLI

```bash
cmux workspace new --name api --cwd ~/src/api
cmux pane split --direction down
cmux notify -t "Build done" -b "all tests passed"
cmux workspace list
```

Full reference: [docs/CLI.md](docs/CLI.md). Configuration: [docs/CONFIGURATION.md](docs/CONFIGURATION.md).

## Documentation

- [Architecture](docs/ARCHITECTURE.md), [technology choices](docs/TECH_STACK.md), [reference analysis of cmux](docs/REFERENCE_ANALYSIS.md)
- [Performance measurements](docs/PERFORMANCE.md), [packaging evaluation](docs/PACKAGING.md)
- [Troubleshooting](docs/TROUBLESHOOTING.md), [release notes](docs/RELEASE_NOTES.md), [phase progress](docs/PHASE_PROGRESS.md)

## Development

```bash
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev libvte-2.91-gtk4-dev xvfb
cargo test --lib                                   # unit tests
xvfb-run -a cargo test --test e2e -- --test-threads=1   # end-to-end tests against the real app
cargo run --bin cmux-linux
scripts/build-deb.sh                               # -> dist/*.deb
```

## License

GPL-3.0-or-later (see [LICENSE](LICENSE)). This is an independent implementation; no cmux source code is
copied. cmux is © Manaflow, Inc., licensed GPL-3.0-or-later.
