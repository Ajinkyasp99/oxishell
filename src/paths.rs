//! XDG locations and safe file writing.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() && Path::new(&v).is_absolute() => PathBuf::from(v),
        _ => home().join(fallback),
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("oxishell")
}

pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("oxishell")
}

/// Directory for the control socket: `$XDG_RUNTIME_DIR/oxishell`, or
/// `/tmp/oxishell-<uid>` when no runtime dir exists.
pub fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(v) if !v.is_empty() => PathBuf::from(v).join("oxishell"),
        _ => PathBuf::from(format!("/tmp/oxishell-{}", unsafe { libc::getuid() })),
    }
}

/// Unix socket paths are limited to 108 bytes (`sun_path`).
const SUN_PATH_MAX: usize = 100;

pub fn socket_path() -> PathBuf {
    match std::env::var_os("OXISHELL_SOCKET") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => {
            let p = runtime_dir().join("oxishell.sock");
            if p.as_os_str().len() < SUN_PATH_MAX {
                return p;
            }
            // Runtime dir too long for a socket: use a short per-user path keyed by it.
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            p.hash(&mut h);
            PathBuf::from(format!("/tmp/oxishell-{}/{:016x}.sock", unsafe { libc::getuid() }, h.finish()))
        }
    }
}

/// Create `dir` with mode 0700 and verify it is owned by us and not group/world accessible.
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    use std::os::unix::fs::MetadataExt;
    if !meta.is_dir() || meta.uid() != unsafe { libc::getuid() } {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} is not a directory owned by the current user", dir.display()),
        ));
    }
    if meta.mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Write via a temp file + rename so a crash never leaves a half-written file.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let tmp = dir.join(format!(
        ".{}.tmp{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_and_private_dir() {
        let dir = std::env::temp_dir().join(format!("oxishell-paths-{}", std::process::id()));
        ensure_private_dir(&dir).unwrap();
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        let f = dir.join("x.json");
        write_atomic(&f, b"one", 0o600).unwrap();
        write_atomic(&f, b"two", 0o600).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"two");
        assert_eq!(std::fs::metadata(&f).unwrap().mode() & 0o777, 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
