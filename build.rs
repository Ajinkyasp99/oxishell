//! Build script for oxishell.
//!
//! On macOS, `ld` keeps every listed dylib even when no symbol is used from it,
//! so the `oxi` CLI would load GTK/VTE/libadwaita at startup. `-dead_strip_dylibs`
//! drops the unused ones. Linux already links with `--as-needed`, so nothing is
//! added there. The target is read from `CARGO_CFG_TARGET_OS` because `cfg!` in a
//! build script reflects the host, not the target.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-bin=oxi=-Wl,-dead_strip_dylibs");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
