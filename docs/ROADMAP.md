# Roadmap

Phases 0–12 from the project brief are tracked in [PHASE_PROGRESS.md](PHASE_PROGRESS.md). This file lists
what remains, in priority order.

## To close out 0.1

1. Install/uninstall the .deb on a clean Ubuntu 24.04 machine or VM (`apt install`, launch from the menu,
   `apt remove`) — needs sudo (12.2, 12.5).
2. Run the GitHub Actions workflows on GitHub (12.7).
3. Install tmux and add an e2e test for the tmux backend surviving an app restart (10.8).
4. Automated tests for: divider drag → saved ratio (4.5), theme/font changes (5.2, 5.3), desktop
   notifications against a test notification daemon (7.3), notifications panel contents (7.6), agent
   resume end to end (10.9).
5. Exercise hooks with real agent binaries (Claude Code, Codex, Gemini CLI, Qoder, OpenCode).
6. Physical-display checks: HiDPI/fractional scaling, multiple monitors.

## Next features

- Flatpak with host shell spawning (see PACKAGING.md).
- zellij backend alongside tmux.
- Per-project command files (e.g. `.cmux/commands.toml`) in addition to global custom commands.
- OSC 9/777 notifications when a VTE build that exposes them is available.
- Pane zoom (temporarily maximize one pane), pane swap, equalize splits.
- Optional `cmux ssh host` convenience that opens a workspace running ssh.

Out of scope (per brief): embedded browser, cloud services, remote dashboards.
