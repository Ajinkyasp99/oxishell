# Contributing to oxishell

Thanks for helping! Bug reports, theme fixes, docs and code are all welcome.

## Reporting bugs

Open an [issue](https://github.com/Ajinkyasp99/oxishell/issues/new/choose) with:
- your distro and version, X11 or Wayland (`echo $XDG_SESSION_TYPE`), and the oxishell version (`oxi --version`);
- what you did, what you expected and what happened;
- output from running `oxishell` in another terminal, if relevant.

Security problems: please follow [SECURITY.md](SECURITY.md) instead of opening a public issue.

## Development setup

```bash
# Ubuntu 24.04
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev libvte-2.91-gtk4-dev xvfb
curl https://sh.rustup.rs -sSf | sh     # Rust 1.87+
git clone https://github.com/Ajinkyasp99/oxishell.git && cd oxishell
cargo run --bin oxishell
```

Running a development build next to an installed oxishell:
`OXISHELL_NON_UNIQUE=1 OXISHELL_SOCKET=/tmp/oxishell-dev.sock cargo run --bin oxishell`.

## Before you open a pull request

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test --lib                                         # unit tests
xvfb-run -a cargo test --test e2e -- --test-threads=1    # end-to-end: real app + real shells
```

CI runs the same checks. The end-to-end tests start the real app with isolated config/state directories
and drive it through the `oxi` socket. Please add or extend a test with your change.

## Where things live

| Path | What |
|---|---|
| `src/model.rs` | workspaces and the split-pane tree (no GTK) |
| `src/config.rs`, `src/theme.rs` | settings, Warp theme parsing and CSS generation |
| `src/ipc.rs`, `src/bin/oxi.rs` | socket protocol and the CLI |
| `src/agents.rs`, `src/notify.rs` | agent hooks, notifications |
| `src/session.rs`, `src/meta.rs` | save/restore, git/ports/cwd from `/proc` |
| `src/ui/` | GTK front end (window, panes, sidebar, dialogs, theme gallery) |
| `data/themes/` | bundled Warp themes |
| `tests/e2e.rs` | end-to-end tests |

Everything outside `src/ui/` is GTK-free and unit-tested. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Guidelines

- Keep it native and small: no web views, no network services, no telemetry.
- Never run commands the user didn't ask for. Restore and automation must stay explicit.
- Only show agent states that the agent itself reports.
- Match the existing style (`cargo fmt`, `rustfmt.toml`) and keep modules focused.

## Adding a theme

Bundled themes come from [warpdotdev/themes](https://github.com/warpdotdev/themes). To add one, copy the
`.yaml` into `data/themes/`, add a line to `src/theme_builtin.rs`, and run `cargo test --lib theme`.

## Releasing (maintainers)

1. Bump `version` in `Cargo.toml`, update `docs/RELEASE_NOTES.md` and the `<releases>` entry in
   `data/dev.oxishell.Oxishell.metainfo.xml`.
2. `git tag v0.x.y && git push origin v0.x.y`. The Release workflow builds the `.deb` and publishes it.

By contributing you agree that your contributions are licensed under GPL-3.0-or-later.
