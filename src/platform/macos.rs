//! macOS replacements for the procfs reads used on Linux.
//!
//! Only processes of the current user are readable, which is all oxishell
//! needs (the shells and programs it started). Every function returns
//! None/empty on failure, like the procfs versions.

use std::collections::HashMap;
use std::ffi::{c_int, c_void, CStr};
use std::mem::{size_of, zeroed};
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// Working directory of `pid` (`PROC_PIDVNODEPATHINFO`).
pub fn cwd_of(pid: i32) -> Option<PathBuf> {
    // SAFETY: plain-old-data struct, filled by the kernel up to `size` bytes.
    let mut info: libc::proc_vnodepathinfo = unsafe { zeroed() };
    let size = size_of::<libc::proc_vnodepathinfo>() as c_int;
    // SAFETY: info is a writable buffer of exactly `size` bytes.
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, &mut info as *mut _ as *mut c_void, size) };
    if n != size {
        return None;
    }
    // vip_path is a MAXPATHLEN char array split into [[c_char; 32]; 32].
    let raw = &info.pvi_cdir.vip_path;
    // SAFETY: the 32x32 array is one contiguous 1024-byte buffer.
    let bytes = unsafe { std::slice::from_raw_parts(raw.as_ptr() as *const u8, size_of_val(raw)) };
    let path = CStr::from_bytes_until_nul(bytes).ok()?;
    use std::os::unix::ffi::OsStrExt;
    (!path.is_empty()).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes())))
}

/// Process name (`proc_name`), the equivalent of `/proc/<pid>/comm`.
pub fn comm(pid: i32) -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: buf is writable for its full length.
    let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

/// Raw `KERN_PROCARGS2` buffer of `pid` (argv and environment).
fn procargs(pid: i32) -> Option<Vec<u8>> {
    let mut size = argmax();
    let mut buf = vec![0u8; size];
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    // SAFETY: mib has 3 valid entries; buf is writable for `size` bytes and
    // the kernel updates `size` to the bytes written.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            buf.as_mut_ptr() as *mut c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    buf.truncate(size);
    Some(buf)
}

/// `kern.argmax`: the largest possible argv + environment block.
fn argmax() -> usize {
    static ARGMAX: OnceLock<usize> = OnceLock::new();
    *ARGMAX.get_or_init(|| sysctl_by_name::<c_int>(c"kern.argmax").map_or(1 << 20, |n| n.max(4096) as usize))
}

pub fn cmdline(pid: i32) -> Vec<String> {
    procargs(pid)
        .and_then(|b| crate::platform::parse_procargs2(&b))
        .map(|(argv, _)| {
            argv.into_iter()
                .filter(|a| !a.is_empty())
                .map(|a| String::from_utf8_lossy(&a).into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Environment of `pid` as raw `KEY=value` entries.
pub fn environ(pid: i32) -> Option<Vec<Vec<u8>>> {
    procargs(pid).and_then(|b| crate::platform::parse_procargs2(&b)).map(|(_, env)| env)
}

/// Map pid -> parent pid for every process we can inspect.
pub fn ppid_table() -> HashMap<i32, i32> {
    let mut map = HashMap::new();
    // SAFETY: a null buffer asks only for the current number of pids.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return map;
    }
    // Room for processes started between the two calls.
    let mut pids = vec![0 as libc::pid_t; count as usize + 64];
    let bytes = (pids.len() * size_of::<libc::pid_t>()) as c_int;
    // SAFETY: pids is writable for `bytes` bytes.
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr() as *mut c_void, bytes) };
    if n <= 0 {
        return map;
    }
    for &pid in &pids[..(n as usize).min(pids.len())] {
        if pid <= 0 {
            continue;
        }
        // SAFETY: plain-old-data struct, filled by the kernel.
        let mut info: libc::proc_bsdinfo = unsafe { zeroed() };
        let size = size_of::<libc::proc_bsdinfo>() as c_int;
        // SAFETY: info is a writable buffer of exactly `size` bytes.
        let r = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut c_void, size) };
        if r == size {
            map.insert(pid, info.pbi_ppid as i32);
        }
    }
    map
}

/// TCP ports in LISTEN state per owning pid, via one system `lsof` for all
/// `pids` (libc has no bindings for the socket fdinfo structs).
pub fn listening_ports_by_pid(pids: &[i32]) -> std::collections::HashMap<i32, Vec<u16>> {
    if pids.is_empty() {
        return Default::default();
    }
    let list = pids.iter().map(i32::to_string).collect::<Vec<_>>().join(",");
    Command::new("/usr/sbin/lsof")
        .args(["-nP", "-w", "-a", "-iTCP", "-sTCP:LISTEN", "-p", &list, "-Fpn"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| crate::platform::parse_lsof_listen_by_pid(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

fn sysctl_by_name<T: Copy>(name: &CStr) -> Option<T> {
    // SAFETY: T is a plain integer; the kernel writes at most `len` bytes.
    let mut value: T = unsafe { zeroed() };
    let mut len = size_of::<T>();
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut value as *mut T as *mut c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0 && len == size_of::<T>()).then_some(value)
}

/// The host port. `mach_host_self` adds a send right per call, so take one.
fn host() -> libc::host_t {
    static HOST: OnceLock<libc::host_t> = OnceLock::new();
    // SAFETY: no preconditions. Deprecated in libc only in favour of the
    // mach2 crate; the system call itself is current.
    #[allow(deprecated)]
    *HOST.get_or_init(|| unsafe { libc::mach_host_self() })
}

/// Aggregate CPU ticks across all cores: (idle, total).
pub fn cpu_ticks() -> Option<(u64, u64)> {
    // SAFETY: plain-old-data struct, filled by the kernel.
    let mut info: libc::host_cpu_load_info = unsafe { zeroed() };
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: info has room for HOST_CPU_LOAD_INFO_COUNT integers.
    let rc = unsafe {
        libc::host_statistics(
            host(),
            libc::HOST_CPU_LOAD_INFO,
            &mut info as *mut _ as libc::host_info_t,
            &mut count,
        )
    };
    if rc != libc::KERN_SUCCESS {
        return None;
    }
    let ticks = info.cpu_ticks.map(u64::from);
    Some((ticks[libc::CPU_STATE_IDLE as usize], ticks.iter().sum()))
}

/// Memory in KiB: (used, total). "Used" follows Activity Monitor: app memory
/// (anonymous pages minus purgeable) + wired + compressed.
pub fn memory_kib() -> Option<(u64, u64)> {
    let total = sysctl_by_name::<u64>(c"hw.memsize")?;
    // SAFETY: plain-old-data struct, filled by the kernel.
    let mut vm: libc::vm_statistics64 = unsafe { zeroed() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: vm has room for HOST_VM_INFO64_COUNT integers.
    let rc = unsafe { libc::host_statistics64(host(), libc::HOST_VM_INFO64, &mut vm as *mut _ as libc::host_info64_t, &mut count) };
    if rc != libc::KERN_SUCCESS {
        return None;
    }
    // SAFETY: sysconf has no preconditions.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
    let pages = u64::from(vm.internal_page_count).saturating_sub(u64::from(vm.purgeable_count))
        + u64::from(vm.wire_count)
        + u64::from(vm.compressor_page_count);
    Some(((pages * page / 1024).min(total / 1024), total / 1024))
}

/// Marks around the PATH printed by the login shell, so text that the user's
/// startup files print is ignored.
const PATH_MARK: &str = "__OXISHELL_PATH__";

/// When started outside a terminal (Finder, Dock, `open`), replace launchd's
/// minimal PATH with the one the user's interactive login shell sets up.
/// Gives up after 3 s, so a slow or broken shell setup never blocks startup.
pub fn import_login_path() {
    if std::env::var_os("TERM").is_some() || std::env::var_os("OXISHELL_KEEP_PATH").is_some() {
        return;
    }
    let shell = crate::config::login_shell();
    let script = format!("printf '{PATH_MARK}%s{PATH_MARK}' \"$PATH\"");
    let Ok(mut child) = Command::new(&shell)
        .args(["-l", "-i", "-c", &script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return;
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
    }
    let Ok(out) = child.wait_with_output() else { return };
    if let Some(path) = crate::platform::extract_marked(&String::from_utf8_lossy(&out.stdout), PATH_MARK) {
        // SAFETY (edition 2021: safe fn): called from main before any thread starts.
        std::env::set_var("PATH", path);
    }
}

/// `Contents/Resources` when this binary runs from inside an `.app` bundle.
pub fn bundle_resources() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let contents = exe.parent().filter(|d| d.ends_with("Contents/MacOS"))?.parent()?;
    Some(contents.join("Resources")).filter(|r| r.is_dir())
}

/// Inside Oxishell.app, point GLib/GTK at the data copied into the bundle by
/// scripts/build-macos.sh (schemas, icon themes, pixbuf loaders, fontconfig).
/// Variables the user already set are kept. Call before any thread starts.
pub fn setup_bundle_env() {
    let Some(res) = bundle_resources() else { return };
    let set_default = |key: &str, value: &std::path::Path| {
        if std::env::var_os(key).is_none() && value.exists() {
            std::env::set_var(key, value);
        }
    };
    let share = res.join("share");
    let data_dirs = match std::env::var("XDG_DATA_DIRS") {
        Ok(v) if !v.is_empty() => format!("{}:{v}", share.display()),
        _ => share.display().to_string(),
    };
    std::env::set_var("XDG_DATA_DIRS", data_dirs);
    set_default("GSETTINGS_SCHEMA_DIR", &share.join("glib-2.0/schemas"));
    set_default("FONTCONFIG_PATH", &res.join("etc/fonts"));
    set_default("FONTCONFIG_FILE", &res.join("etc/fonts/fonts.conf"));
    // Keep GIO from loading modules out of a Homebrew install, if any.
    if std::env::var_os("GIO_MODULE_DIR").is_none() {
        std::env::set_var("GIO_MODULE_DIR", res.join("lib/gio/modules"));
    }
    if std::env::var_os("GDK_PIXBUF_MODULE_FILE").is_none() {
        if let Some(cache) = pixbuf_loaders_cache(&res) {
            std::env::set_var("GDK_PIXBUF_MODULE_FILE", cache);
        }
    }
}

/// Write the bundle's pixbuf loader cache with its real location filled in.
fn pixbuf_loaders_cache(res: &std::path::Path) -> Option<PathBuf> {
    let template = std::fs::read_to_string(res.join("lib/gdk-pixbuf-2.0/2.10.0/loaders.cache.in")).ok()?;
    let text = template.replace("@RESOURCES@", &res.to_string_lossy());
    let dir = crate::paths::runtime_dir();
    crate::paths::ensure_private_dir(&dir).ok()?;
    let path = dir.join("pixbuf-loaders.cache");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        crate::paths::write_atomic(&path, text.as_bytes(), 0o600).ok()?;
    }
    Some(path)
}
