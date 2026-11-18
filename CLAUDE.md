# oxishell — project rules

## Every feature targets Linux and macOS

oxishell ships on Linux (Ubuntu/Debian, X11 and Wayland) and macOS (Apple silicon and Intel). A feature is
not done until it is designed, built and checked for both.

- **Design:** say in the design/plan how the feature behaves on each platform, including shortcuts
  (Linux is Ctrl-based, macOS is ⌘ / `<Meta>`-based so Ctrl stays with the shell), process and file
  APIs, notifications, packaging, and anything the OS does differently.
- **Code:**
  - OS-specific code goes behind `crate::platform` / `crate::meta` (procfs on Linux, libproc/sysctl on
    macOS) or `#[cfg(target_os = "...")]`. Never call `/proc`, X11-only or other single-OS APIs from
    shared code.
  - New shortcuts go in **both** `DEFAULT_SHORTCUTS` tables in `src/config.rs` (the
    `cfg(not(target_os = "macos"))` one and the `cfg(target_os = "macos")` one), must not conflict in
    either table, and need a test for each table's values.
  - Linux-only workarounds (e.g. `ui::vte_fix`) stay `#[cfg(target_os = "linux")]`, with the macOS
    behaviour stated next to them.
- **Checks before calling a feature done:**
  - Linux: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --lib --bins`,
    `dbus-run-session -- xvfb-run -a cargo test --test e2e --test interactions -- --test-threads=1`.
  - macOS: on a Mac, `cargo clippy --all-targets -- -D warnings`, `cargo test --lib --bins` and
    `scripts/build-macos.sh`. On Linux, at least a macOS compile check:
    `PKG_CONFIG_ALLOW_CROSS=1 CARGO_TARGET_DIR=target/macos-check cargo clippy --target aarch64-apple-darwin --all-targets -- -D warnings`
    (compile only; it cannot run tests or link). The CI `macos` job runs the real checks on push.
  - The GUI suites (`tests/e2e.rs`, `tests/interactions.rs`) are Linux-only (Xvfb + xdotool). Say so in
    the report: macOS UI behaviour of a new feature is verified only by hand on a Mac.
- **Docs:** README and `docs/` describe both platforms, including the macOS shortcut paragraph in the
  README's "Keyboard shortcuts" section.
- **Report:** when a feature is handed over, state per platform what was verified and what was not.
