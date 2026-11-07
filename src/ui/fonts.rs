//! App-local Hack fonts, also used by Warp. No system font installation.

use std::ffi::{c_void, CString};
use std::os::unix::ffi::OsStrExt;
use std::sync::OnceLock;

// Fontconfig is already part of the GTK/Pango Linux stack. Register before
// creating widgets so Pango resolves Hack without a system-wide install.
#[link(name = "fontconfig")]
unsafe extern "C" {
    fn FcConfigGetCurrent() -> *mut c_void;
    fn FcConfigAppFontAddFile(config: *mut c_void, file: *const u8) -> libc::c_int;
}

pub fn register() -> Result<(), String> {
    static REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();
    REGISTERED.get_or_init(register_fonts).clone()
}

fn register_fonts() -> Result<(), String> {
    let runtime = crate::paths::runtime_dir();
    crate::paths::ensure_private_dir(&runtime).map_err(|e| e.to_string())?;
    let dir = runtime.join("fonts-hack");
    crate::paths::ensure_private_dir(&dir).map_err(|e| e.to_string())?;
    // SAFETY: Fontconfig owns the borrowed current configuration. We neither
    // free it nor retain any borrowed data; font registration is done once.
    let config = unsafe { FcConfigGetCurrent() };
    if config.is_null() {
        return Err("Fontconfig configuration is unavailable".into());
    }
    for (name, bytes) in [
        ("Hack-Regular.ttf", include_bytes!("../../data/fonts/Hack-Regular.ttf").as_slice()),
        ("Hack-Bold.ttf", include_bytes!("../../data/fonts/Hack-Bold.ttf").as_slice()),
        ("Hack-Italic.ttf", include_bytes!("../../data/fonts/Hack-Italic.ttf").as_slice()),
        (
            "Hack-BoldItalic.ttf",
            include_bytes!("../../data/fonts/Hack-BoldItalic.ttf").as_slice(),
        ),
        ("LICENSE.md", include_bytes!("../../data/fonts/LICENSE.md").as_slice()),
        ("NOTICE.md", include_bytes!("../../data/fonts/NOTICE.md").as_slice()),
    ] {
        let path = dir.join(name);
        if std::fs::read(&path).ok().as_deref() != Some(bytes) {
            crate::paths::write_atomic(&path, bytes, 0o600).map_err(|e| format!("{name}: {e}"))?;
        }
        if name.ends_with(".ttf") {
            let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
            // SAFETY: config is a live Fontconfig configuration, and c_path
            // is a NUL-terminated filename kept alive throughout the call.
            if unsafe { FcConfigAppFontAddFile(config, c_path.as_ptr().cast()) } == 0 {
                return Err(format!("could not register bundled {name}"));
            }
        }
    }
    Ok(())
}
