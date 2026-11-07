## What & why

## How I tested
- [ ] `cargo fmt` and `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --lib`
- [ ] `xvfb-run -a cargo test --test e2e -- --test-threads=1`
- [ ] Tried it in the app (X11 / Wayland)
