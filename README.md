<div align="center">

<img src="data/icons/dev.cmux.Linux.svg" width="96" alt="cmux-linux icon">

# cmux-linux

**A native Linux terminal for running many shells and AI coding agents side by side.**

Workspaces in a sidebar · split panes · notification rings when an agent needs you · 41 Warp themes

[![CI](https://github.com/Ajinkyasp99/cmux_linux/actions/workflows/ci.yml/badge.svg)](https://github.com/Ajinkyasp99/cmux_linux/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Ajinkyasp99/cmux_linux?display_name=tag)](https://github.com/Ajinkyasp99/cmux_linux/releases/latest)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)
![Platform: Linux](https://img.shields.io/badge/platform-Linux%20(X11%20%2B%20Wayland)-informational)

<img src="docs/assets/hero.png" alt="cmux-linux with three workspaces, split panes, git branch and port info in the sidebar, and unread notification badges" width="900">

</div>

cmux-linux brings the workflow of [cmux](https://github.com/manaflow-ai/cmux) (a macOS app) to Linux as a
real native app: **Rust + GTK4/libadwaita**, with terminals rendered by **VTE**, the engine behind GNOME
Terminal. No Electron, no browser, no web terminal.

## Why

When you run Claude Code, Codex or Gemini in several terminals, you keep checking each one to see whether it
finished or is waiting for you. cmux-linux keeps every project in its own **workspace**, shows each one's git
branch, open ports and agent status in a **sidebar**, and **lights up the pane** the moment an agent needs
input.

## Features

| | |
|---|---|
| 🗂 **Workspaces** | Vertical sidebar with git branch, uncommitted-change count, working directory, listening ports, agent status and unread badges. Background workspaces keep running. Drag to reorder, double-click to rename, ✕ to close (asks first). |
| ⊞ **Split panes** | Split right or down, nest them, drag dividers, move focus with <kbd>Alt</kbd>+arrows. Each pane is an independent shell. |
| 🔔 **Agent notifications** | Hooks for **Claude Code, Codex, Gemini CLI, Qoder, OpenCode**. A blue ring on the pane, a badge on the workspace, a desktop notification, and <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>U</kbd> jumps to it. |
| 🎨 **Warp themes** | 41 built-in themes from [Warp](https://github.com/warpdotdev/themes), including gradients. They colour the whole window, not just the text. Drop any Warp `.yaml` theme in to add your own. |
| 💡 **Command suggestions** | As you type, the newest matching command from your bash/zsh/fish history appears dimmed after the cursor. <kbd>→</kbd> accepts it. <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>H</kbd> searches your whole history. |
| 🏷 **Context chips** | Warp-style chips above each terminal: active Python venv/conda env, folder, git branch and changes. The prompt becomes a clean `❯` (red after a failed command). |
| ▮ **Cursor** | Block, vertical line or horizontal line, blinking or steady. |
| ⌘ **Command palette** | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd>: every action, workspace, theme and your own project commands, with fuzzy search. |
| 🤖 **Scriptable** | The `cmux` CLI controls the running app over a private local socket: create workspaces, split, read output, send notifications. |
| 💾 **Sessions** | Workspaces, layouts, directories and agent sessions come back after a restart, each terminal clean at the top. Commands are never re-run. |
| 🐧 **Native** | GTK4, X11 and Wayland, light/dark aware, ~180 ms startup, ~1 % idle CPU with 20 panes ([measured](docs/PERFORMANCE.md)). |

## Install

### Ubuntu / Debian (.deb)

Requires Ubuntu 24.04+ or Debian 13+ (GTK ≥ 4.12, libadwaita ≥ 1.5, VTE-GTK4 ≥ 0.76).

```bash
wget https://github.com/Ajinkyasp99/cmux_linux/releases/latest/download/cmux-linux_amd64.deb
sudo apt install ./cmux-linux_amd64.deb
```

Then open **cmux** from your app menu, or run `cmux-linux`.

> apt may print `N: Download is performed unsandboxed as root…` when the file is in your home folder.
> That note is harmless.

### Other distributions: build from source

```bash
# Ubuntu:  sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev libvte-2.91-gtk4-dev
git clone https://github.com/Ajinkyasp99/cmux_linux.git
cd cmux_linux
cargo build --release          # needs Rust 1.87+ (https://rustup.rs)
./target/release/cmux-linux
```

On Fedora (`gtk4-devel libadwaita-devel vte291-gtk4-devel`) or Arch (`gtk4 libadwaita vte4`), install the
equivalent packages. These distros haven't been tested yet; reports are welcome.
Full guide, including uninstalling: [docs/INSTALL.md](docs/INSTALL.md).

## Quick start

1. **Open a workspace per project:** <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>N</kbd>, then rename it with a double-click.
2. **Split:** <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>E</kbd> (right) and <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>O</kbd> (down). Start your dev server in one pane and an agent in another.
3. **Turn on agent notifications** (one time):
   ```bash
   cmux hooks install --all     # sets up every supported agent found on your PATH
   ```
4. **Pick a theme:** <kbd>Ctrl</kbd>+<kbd>,</kbd> → Appearance, or <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> and type `theme`.

When an agent finishes or asks for permission, its workspace gets a badge. Press <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>U</kbd> to jump straight to it.

<table>
<tr>
<td width="50%"><img src="docs/assets/palette.png" alt="Command palette searching themes"><br><sub><b>Command palette:</b> actions, workspaces, themes, custom commands</sub></td>
<td width="50%"><img src="docs/assets/notifications.png" alt="Notifications panel with a Claude Code permission request"><br><sub><b>Notifications:</b> which agent needs you, and where</sub></td>
</tr>
</table>

## Suggestions and context chips

<table>
<tr>
<td width="50%"><img src="docs/assets/suggest.png" alt="Inline suggestion: typing git ch shows eckout -b feature/chips dimmed; chips show CommonVenv, ~/proj, main"><br><sub>Type <code>git ch</code> → the rest comes from your history; press <kbd>→</kbd> to accept</sub></td>
<td width="50%"><img src="docs/assets/history.png" alt="Command history search popup"><br><sub><kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>H</kbd>: search history. Enter puts it on the prompt, Ctrl+Enter runs it</sub></td>
</tr>
</table>

Suggestions only appear when you're at a shell prompt (never inside vim, Python and so on), and only while
cmux-linux can follow your typing exactly (after <kbd>Tab</kbd>, arrow keys or paste they pause until the
next line). The **environment chip** works automatically in bash; for zsh/fish add one line:
`cmux shell-integration zsh >> ~/.zshrc` (or `fish >> ~/.config/fish/config.fish`). Both features can be
turned off in Settings.

Because the chips already show where you are, panes use a compact **`❯` prompt** (green, red after a
failed command) instead of `user@host:~/path$`. This only applies inside cmux-linux; your `~/.bashrc`,
other terminals and SSH sessions keep their own prompt. Turn it off in Settings → Compact prompt.

## Themes

Every theme colours the whole window (background, sidebar, header, badges, focus rings and the terminal),
just like in Warp. Gradient themes such as *Cyber Wave* and *Willow Dream* paint behind the terminal too.

<img src="docs/assets/themes.png" alt="Six themes: Warp Dark, Cyber Wave, Willow Dream, Catppuccin, Synthwave 84, GitHub Light" width="900">

Built in: Warp Dark/Light, Cyber Wave, Willow Dream, Dark City, Fancy Dracula, Jellyfish, Koi, Leafy, Marble,
Pink City, Red Rock, Snowy, Paper Botanical, Dracula, Catppuccin, Nord, Tokyo Night, One Dark, GitHub
Dark/Light, Gruvbox, Solarized, Monokai Pro, Night Owl, Synthwave 84, Ayu, Everforest, Kanagawa, Material,
Cobalt 2, Outrun, Shades of Purple, Poimandres, Vitesse, Palenight, and more.

**Add your own:** put any [Warp theme file](https://github.com/warpdotdev/themes) into
`~/.config/cmux-linux/themes/`. It shows up in the gallery right away, background images included.
See [docs/THEMES.md](docs/THEMES.md).

<img src="docs/assets/theme-gallery.png" alt="Theme gallery in Settings" width="420">

## Keyboard shortcuts

| Action | Shortcut |
|---|---|
| New / close / rename workspace | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>N</kbd> / <kbd>Q</kbd> / <kbd>R</kbd> |
| Next / previous workspace | <kbd>Ctrl</kbd>+<kbd>PgDn</kbd> / <kbd>Ctrl</kbd>+<kbd>PgUp</kbd>, or <kbd>Alt</kbd>+<kbd>1</kbd>…<kbd>9</kbd> |
| Split right / down | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>E</kbd> / <kbd>O</kbd> |
| Close pane | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>W</kbd> |
| Move focus | <kbd>Alt</kbd>+<kbd>←</kbd><kbd>→</kbd><kbd>↑</kbd><kbd>↓</kbd> |
| Command palette | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> |
| Copy / paste | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>C</kbd> / <kbd>V</kbd> |
| Search scrollback | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>F</kbd> |
| Accept suggestion | <kbd>→</kbd> or <kbd>End</kbd> |
| Search command history | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>H</kbd> |
| Jump to latest unread | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>U</kbd> |
| Notifications panel | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>I</kbd> |
| Toggle sidebar | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>B</kbd> |
| Font size | <kbd>Ctrl</kbd>+<kbd>=</kbd> / <kbd>Ctrl</kbd>+<kbd>-</kbd> / <kbd>Ctrl</kbd>+<kbd>0</kbd> |
| Settings | <kbd>Ctrl</kbd>+<kbd>,</kbd> |

All shortcuts can be changed in Settings → Shortcuts. Right-click a workspace or a terminal for more actions.

## Command line

```bash
cmux workspace new --name api --cwd ~/src/api   # new workspace
cmux pane split --direction down                # split the current pane
cmux notify -t "Deploy done" -b "prod is green" # ring this pane
cmux workspace list                             # see everything
cmux pane read --lines 50                       # last 50 lines of output
```

Use it from scripts, git hooks or your own agent tooling. Reference: [docs/CLI.md](docs/CLI.md).

## Supported agents

| Agent | Running | Needs input | Finished |
|---|:-:|:-:|:-:|
| Claude Code | ✓ | ✓ | ✓ |
| Gemini CLI | ✓ | — | ✓ |
| OpenCode¹ | ✓ | ✓ | ✓ |
| Codex CLI | — | — | ✓ |
| Qoder CLI | — | — | ✓ |
| Anything else | `cmux agent-event` / `cmux notify` from your own scripts | | |

¹ OpenCode support is implemented as a plugin but has not yet been tested against a live OpenCode.
After a restart, sessions whose hooks reported a session id (Claude Code, Gemini, Qoder, and Codex when it
sends a thread id) can be resumed from the pane menu or the palette.

cmux-linux only shows states that the agent's own hooks report; it never guesses. Hooks are silent no-ops
outside cmux-linux, so installing them never breaks your agents. Details: [docs/AGENTS.md](docs/AGENTS.md).

## Configuration

Everything is in Settings (<kbd>Ctrl</kbd>+<kbd>,</kbd>), saved to `~/.config/cmux-linux/config.toml`:

```toml
[appearance]
theme = "cyber_wave"
font = "JetBrains Mono 12"

[[commands]]                 # appear in the command palette as "Run: …"
name = "Tests"
command = "cargo test"
```

All options: [docs/CONFIGURATION.md](docs/CONFIGURATION.md).

## Privacy & security

- Everything stays on your machine. No accounts, telemetry, cloud or network listeners.
- The control socket is private to your user (mode 0600, peer-uid checked).
- Typing into panes from the CLI (`cmux pane send`) is **off by default**.
- Restoring a session never re-runs commands; saved output is shown as plain text.

Report vulnerabilities privately: [SECURITY.md](SECURITY.md).

## Documentation

| | |
|---|---|
| [INSTALL](docs/INSTALL.md) · [TROUBLESHOOTING](docs/TROUBLESHOOTING.md) | installing, removing, fixing problems |
| [CLI](docs/CLI.md) · [CONFIGURATION](docs/CONFIGURATION.md) · [AGENTS](docs/AGENTS.md) · [THEMES](docs/THEMES.md) | using it |
| [ARCHITECTURE](docs/ARCHITECTURE.md) · [TECH_STACK](docs/TECH_STACK.md) · [PERFORMANCE](docs/PERFORMANCE.md) | how it works |
| [ROADMAP](docs/ROADMAP.md) · [RELEASE_NOTES](docs/RELEASE_NOTES.md) · [PHASE_PROGRESS](docs/PHASE_PROGRESS.md) | where it's going |

## Contributing

Bug reports, theme fixes and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for setup,
tests and guidelines.

## Credits & license

- Inspired by [cmux](https://github.com/manaflow-ai/cmux) by Manaflow. This is an independent Linux
  implementation and contains no cmux source code.
- Themes from [warpdotdev/themes](https://github.com/warpdotdev/themes) (Apache-2.0), with credit to Warp and
  the original theme authors ([NOTICE](data/themes/NOTICE.md)).
- Built on [GTK](https://gtk.org), [libadwaita](https://gnome.pages.gitlab.gnome.org/libadwaita/) and
  [VTE](https://gitlab.gnome.org/GNOME/vte).

cmux-linux is free software under the **GNU GPL v3.0 or later**. See [LICENSE](LICENSE).
