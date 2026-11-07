# Reference Analysis: manaflow-ai/cmux (Phase 0.1)

Analyzed commit: `be5e24339c6eabb8e929a7029c4e6f834e257c97` (main). Method: GitHub REST API listing of the
full tree (25,074 files, not truncated) plus a sparse shallow clone of `cmux-tui/`, `docs/agent-hooks.md`
and three Swift packages. Other directories were inspected via listings and READMEs only, not read in depth.

## 1. What cmux is
A macOS-only app (Swift/AppKit + SwiftPM, Xcode project) embedding a **Ghostty fork** (`ghostty/`, Metal
renderer) for terminal rendering. Vertical workspace sidebar, splits, notification rings for coding agents,
in-app browser, SSH workspaces, CLI + Unix-socket API, `cmux.json` custom commands.

File mix: ~14k Swift, ~1.9k TS, ~430 Rust, plus Go (`daemon/remote`), Python, Java, shell.

## 2. Architecture by area (paths)
| Area | Location |
|---|---|
| macOS app shell, AppDelegate, windowing | `Sources/` (`AppDelegate+*.swift`), `Packages/macOS/CmuxWindowing` |
| Terminal embedding | `ghostty/` (submodule fork), `ghostty.h`, `Packages/Shared/CmuxGhosttyKit`, `CmuxTerminal*` |
| Workspaces / sidebar | `Packages/macOS/CmuxWorkspaces`, `CmuxSidebar`, `CmuxSidebarGit` |
| Splits | `Packages/macOS/CmuxPanes` (`PaneLayoutService`, `SessionSplitContainerLayoutCodec`), `vendor/bonsplit` |
| Notifications | `Packages/macOS/CmuxNotifications`, `Sources/AgentNotification*.swift` |
| Agent hooks / sessions | `CLI/CMUXCLI+AgentHook*.swift`, `Sources/Agent*.swift`, `docs/agent-hooks.md`, `Resources/opencode-plugin.js` |
| Control socket + CLI | `Packages/macOS/CmuxControlSocket`, `CLI/`, `docs/cli-contract.md` |
| Git / ports | `Packages/macOS/CmuxGit`, `Sources/AgentPort*.swift` |
| Command palette | `Packages/macOS/CmuxCommandPalette`, `Native/CommandPaletteNucleoFFI` |
| Settings / config | `Packages/macOS/CmuxSettings`, `docs/configuration.md`, `config/` |
| Rust multiplexer (portable) | `cmux-tui/` |
| Remote daemon (Go) | `daemon/remote/` |
| Out of scope | `web/`, `workers/`, `services/`, `ios/`, `Packages/macOS/CmuxCloud*`, `CmuxBrowser`, `CmuxComputerUse`, `TunnelExtension` |

**Key discovery:** `cmux-tui/` is a separate, Unix-capable Rust workspace (rust 1.91): `cmux-pty`
(portable-pty), `cmux-tui-core` (~133k lines: model, layout, journal, agent_hooks, resource API),
`cmux-remote-protocol`, `cmux-terminal-client`, `ghostty-vt(-sys)` (needs Zig 0.16 to build
`libghostty-vt.a`), plus specs in `cmux-tui/spec/` (cli, events, session-journal, terminal-host,
native-frontend). It is a TUI, not a GTK GUI, but its model, protocol and PTY code are the most reusable assets.

## 3. Licensing (verified from root `LICENSE`)
- Default: **GPL-3.0-or-later**. macOS app, CLI and `cmux-tui` remain under that unless a file/notice says otherwise.
- **BUSL-1.1 (not reusable)**: `web/`, `workers/ci-artifacts/`, `workers/iroh-v2/`, `workers/presence/`,
  `services/iroh-relay-minter/`, `cmux-tui/relays/cloudflare-do/`.
- **Inconsistency to resolve:** `cmux-tui/Cargo.toml` sets workspace `license = "MIT"`, and every crate
  inherits it, but the root LICENSE says cmux-tui stays under the default GPL. No LICENSE file exists in
  `cmux-tui/`. Until clarified, treat as GPL-3.0-or-later (safe if our project is GPL-3.0-or-later).
- Third-party: Ghostty (MIT), Bonsplit, herdr detection manifests (Apache-2.0, pinned commits in
  `cmux-tui/ATTRIBUTIONS.md`), Sparkle, Sentry, PostHog, WireGuard, Markdown/Mermaid/Vega web assets, icons
  (Lobe, Octicons). Per-file notices in `THIRD_PARTY_LICENSES.md` must be honored on any copy.
- Decision: the Linux project is **GPL-3.0-or-later**; keep upstream copyright headers and a NOTICE.
  Not legal advice; confirm with maintainers before shipping copied code.

## 4. Portability matrix
Category A = reusable, B = adapt, C = rewrite, D = optional/defer.

| Feature | Cat | Source | Linux plan |
|---|---|---|---|
| PTY spawn/lifecycle | A | `cmux-tui/crates/cmux-pty` | Reuse or re-implement on `portable-pty`; with VTE, PTY is owned by the widget, so likely only for headless/restore |
| Resource/topology model, layout tree | A/B | `cmux-tui-core/src/{model,layout}.rs`, `Packages/macOS/CmuxPanes` | Borrow concepts; own small Rust tree for GUI |
| Control protocol / CLI contract | A | `cmux-tui/spec/*`, `docs/cli-contract.md`, `docs/protocol.md` | Adopt noun-first CLI and JSON style; own minimal subset |
| Unix socket security policy | B | `CmuxControlSocket` README (0600, peer UID check, path fallback) | Re-implement in Rust with `SO_PEERCRED` |
| Agent hooks (claude/codex/opencode/gemini/qoder + ~14 more) | A/B | `docs/agent-hooks.md`, `CLI/CMUXCLI+AgentHook*`, `Resources/opencode-plugin.js` | Reuse hook file locations/schemas and JS plugin; reimplement `hooks setup` |
| Agent state detection manifests | A | herdr manifests (Apache-2.0) | Optional; only claim states hooks truly expose |
| Notifications model | B | `CmuxNotifications`, `AgentNotification*` | Rust model + D-Bus (`org.freedesktop.Notifications`) |
| OSC 9/99/777 | B | Ghostty behavior | Verify VTE support in Phase 7.8 |
| Git branch / status / ports | B | `CmuxGit`, `AgentPort*` | `git` CLI; `/proc/net/tcp` or `ss` for ports |
| Session journal / restore | A/B | `spec/session-journal.md`, `AgentSessionRecovery` | Simplified JSON; no process-restore promises |
| Config / `cmux.json` custom commands | B | `docs/configuration.md` | TOML + optional reading of ghostty config |
| Shell integration, terminfo overlay | A | `Resources/shell-integration`, `terminfo-overlay` | Check licenses, then reuse (OSC 7 cwd etc.) |
| Sidebar / workspace UI | C | `CmuxSidebar`, `CmuxWorkspaces` (SwiftUI/AppKit) | GTK4/libadwaita |
| Split UI | C | `vendor/bonsplit` | `gtk::Paned` tree |
| Command palette UI | C | `CmuxCommandPalette` (+Nucleo FFI) | GTK; `nucleo` Rust crate for fuzzy match is reusable |
| Windowing, updater, keychain, entitlements, Xcode | C | `Sources/`, Sparkle | Drop; use package manager |
| Terminal rendering | C | Ghostty Metal fork | VTE-GTK4 (see 5) |
| In-app browser, cloud, WireGuard/iroh, iOS, billing, computer-use | D | `CmuxBrowser`, `CmuxCloud*`, `ios/`, ... | Out of scope per brief |
| Remote daemon (Go) | D | `daemon/remote` | Plain `ssh` first |

## 5. Platform challenges
1. **Terminal widget:** Ghostty's embeddable Linux piece is `libghostty-vt` (parser/screen state, no
   renderer or GTK widget) and requires Zig. cmux's rendering patches are Metal-specific. A GPU Ghostty
   widget in GTK is not available from this repo; VTE-GTK4 is the pragmatic engine (to confirm in 0.2).
2. **Shell/agent plumbing** is Swift-heavy (`CLI/CMUXCLI+*.swift`); protocol is reusable, code is not.
3. **Notifications and focus:** macOS `UNUserNotificationCenter`/Dock badge map to D-Bus + in-app badges; Wayland blocks focus stealing.
4. **Dev environment gaps (this machine):** Ubuntu 24.04, X11, Rust present; GTK4/libadwaita/VTE dev
   packages and Zig are missing (apt approval needed; verify VTE-GTK4 availability on 24.04).
5. **Licensing ambiguity** for cmux-tui (section 3).
6. **Scale:** upstream is enormous; scope must stay at the brief's feature list.

## 6. Recommendation
Build a new Rust + GTK4/libadwaita app with VTE-GTK4 behind a `TerminalBackend` trait; use cmux's specs
(CLI contract, hook formats, socket policy, journal concepts) as the design reference; reuse
`cmux-pty`/agent-hook assets only where license-verified; keep GPL-3.0-or-later. Defer browser/cloud/remote.
Decisions on engine and GUI stack are finalized in Phases 0.2 and 0.3.

## 7. Known limitations of this analysis
Most Swift sources were not read, only listed; behavior claims about them come from READMEs/docs. Commit
pinned above; upstream moves fast.
