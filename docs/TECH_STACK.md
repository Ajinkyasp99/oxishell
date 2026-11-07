# Tech Stack (Phases 0.2 + 0.3)

## Terminal engine: VTE (GTK4) — decided
Evidence (measured on this machine, Ubuntu 24.04, X11):
- A spike using crates `gtk4 0.9`, `libadwaita 0.7`, `vte4 0.8` against system GTK 4.14.5, libadwaita 1.5.0,
  VTE 0.76.0 built and ran under Xvfb; `Terminal::spawn_async` started `/bin/bash` in a real PTY ("spawn true").
- VTE provides PTY management, VT parsing, scrollback, selection/clipboard, search, mouse, IME, OSC 7 cwd,
  OSC 8 hyperlinks, and desktop-notification OSC 777 (`notification-received` signal in 0.76 builds that enable it).

libghostty (not adopted):
- Per the survey of upstream: the Linux-embeddable piece is `libghostty-vt` (parser + screen state only, no
  renderer or GTK widget) and needs Zig 0.16 (not installed here). cmux's renderer patches target Metal.
  Using it would mean writing our own GTK renderer, input, selection and IME — the work the brief says to avoid.
- Not tested hands-on (Zig missing); conclusion rests on upstream docs. Revisit behind the `TerminalBackend`
  seam if a GTK-embeddable libghostty appears.

Trade-offs: VTE is CPU-rendered via GTK (GSK), slightly slower than GPU terminals on huge output, but mature.

## GUI: GTK4 + libadwaita — decided
GTK 4.14 supports Wayland and X11 natively (backend chosen at runtime). Only X11 is testable here (no Wayland
session); Wayland testing in Phase 11.10 is limited to what can be run (e.g. a nested compositor if available).

## Other choices
| Concern | Choice |
|---|---|
| Language | Rust 2021 (cargo 1.97 installed) |
| Persistence/config | TOML (config), JSON (sessions); no SQLite |
| CLI | `clap` derive |
| IPC | Unix domain socket, newline-delimited JSON, mode 0600 in `$XDG_RUNTIME_DIR`, `SO_PEERCRED` check |
| Notifications | `gio::Notification` (D-Bus via GApplication) |
| Git | `git` CLI (porcelain v2) — avoids libgit2 build dependency |
| Ports | parse `/proc/net/tcp{,6}` + `/proc/<pid>/fd` for panes' process trees |
| Packaging | `cargo-deb` -> .deb |
| License | GPL-3.0-or-later |

## Findings during implementation

- **OSC 9/99/777 notifications:** the `vte4` binding exposes `notification-received`, but Ubuntu 24.04's
  `libvte-2.91-gtk4.so.0` (0.76.0) does not contain that signal (it comes from a downstream patch).
  Connecting it would fail at runtime, so it is not used. Attention comes from hooks, `cmux notify`, and
  the terminal bell (`bell` signal), which is available.
- **Renderer:** GTK's default GL renderer works under Xvfb via Mesa llvmpipe; `GSK_RENDERER=cairo` uses
  ~65 MB less private memory there. See PERFORMANCE.md.
- **Wayland:** verified by running the full e2e suite natively on a headless GNOME Shell 46 compositor.
