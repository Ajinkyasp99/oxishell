//! App-local Hack fonts, also used by Warp. No system font installation.

use std::ffi::c_void;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::OnceLock;

// Linux: Fontconfig is already part of the GTK/Pango stack. Register before
// creating widgets so Pango resolves Hack without a system-wide install.
#[cfg(not(target_os = "macos"))]
#[link(name = "fontconfig")]
unsafe extern "C" {
    fn FcConfigGetCurrent() -> *mut c_void;
    fn FcConfigAppFontAddFile(config: *mut c_void, file: *const u8) -> libc::c_int;
}

// macOS: Pango renders through CoreText there, so the fonts are registered
// with CoreText for this process only.
#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFURLCreateFromFileSystemRepresentation(allocator: *const c_void, buffer: *const u8, len: isize, is_directory: u8) -> *const c_void;
    fn CFRelease(cf: *const c_void);
}

#[cfg(target_os = "macos")]
#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTFontManagerRegisterFontsForURL(font_url: *const c_void, scope: u32, error: *mut *const c_void) -> u8;
}

#[cfg(not(target_os = "macos"))]
fn register_file(path: &Path) -> Result<(), String> {
    // SAFETY: Fontconfig owns the borrowed current configuration. We neither
    // free it nor retain any borrowed data; font registration is done once.
    let config = unsafe { FcConfigGetCurrent() };
    if config.is_null() {
        return Err("Fontconfig configuration is unavailable".into());
    }
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    // SAFETY: config is a live Fontconfig configuration, and c_path
    // is a NUL-terminated filename kept alive throughout the call.
    if unsafe { FcConfigAppFontAddFile(config, c_path.as_ptr().cast()) } == 0 {
        return Err("Fontconfig rejected the file".into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn register_file(path: &Path) -> Result<(), String> {
    /// kCTFontManagerScopeProcess: visible to this process only, gone at exit.
    const SCOPE_PROCESS: u32 = 1;
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: bytes is a valid buffer of the given length for the whole call;
    // the returned CFURL is owned by us and released below.
    let url = unsafe { CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize, 0) };
    if url.is_null() {
        return Err("could not build a file URL".into());
    }
    // SAFETY: url is a live CFURL; a null error out-pointer is allowed.
    let ok = unsafe { CTFontManagerRegisterFontsForURL(url, SCOPE_PROCESS, std::ptr::null_mut()) };
    // SAFETY: url came from a Create function, so we own one reference.
    unsafe { CFRelease(url) };
    if ok == 0 {
        return Err("CoreText rejected the file".into());
    }
    Ok(())
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
            register_file(&path).map_err(|e| format!("could not register bundled {name}: {e}"))?;
        }
    }
    Ok(())
}
