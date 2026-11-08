# Roadmap

Phases 0–12 from the project brief are tracked in [PHASE_PROGRESS.md](PHASE_PROGRESS.md). This file lists
what remains, in priority order.

## To close out 0.1

1. Manual uninstall check by the maintainer: `sudo apt remove oxishell` (12.5).
2. Run the GitHub Actions workflows on GitHub (12.7).
3. Turn the manual audit checks (divider drag, theme/font, D-Bus notification, agent resume) into
   automated e2e tests.
4. Exercise hooks with real agent binaries (Claude Code, Codex, Gemini CLI, Qoder, OpenCode).
5. Physical-display checks: HiDPI/fractional scaling, multiple monitors.

## Next features

- Flatpak with host shell spawning (see PACKAGING.md).
- Per-project command files (e.g. `.oxishell/commands.toml`) in addition to global custom commands.
- OSC 9/777 notifications when a VTE build that exposes them is available.
- Pane zoom (temporarily maximize one pane), pane swap, equalize splits.
- Optional `oxi ssh host` convenience that opens a workspace running ssh.

Out of scope (per brief): embedded browser, cloud services, remote dashboards.
