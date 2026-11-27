//! OS-specific process and system queries.
//!
//! Linux reads procfs directly (in `meta`, `sysstat`, `compose`). macOS has no
//! procfs, so the same answers come from libproc, sysctl and Mach calls in
//! [`macos`]. The parsers below are pure and compiled on every OS, so the Linux
//! test suite covers them too.

#[cfg(target_os = "macos")]
pub mod macos;

/// Keys of the `apple[]` strings the macOS kernel appends after a process's
/// environment in `KERN_PROCARGS2`. They are not environment variables.
const APPLE_KEYS: &[&str] = &[
    "executable_path=",
    "executable_file=",
    "executable_cdhash=",
    "executable_boothash=",
    "dyld_file=",
    "ptr_munge=",
    "main_stack=",
    "malloc_entropy=",
    "stack_guard=",
    "arm64e_abi=",
    "th_port=",
    "pfz=",
];

/// A process's argv and environment entries, as raw bytes.
pub type ProcArgs = (Vec<Vec<u8>>, Vec<Vec<u8>>);

/// Split a macOS `KERN_PROCARGS2` buffer into (argv, environment).
///
/// Layout: native-endian `int argc`, the executable path, NUL padding, `argc`
/// NUL-terminated arguments, then NUL-terminated `KEY=value` environment
/// entries, then the kernel's `apple[]` strings (dropped here).
pub fn parse_procargs2(buf: &[u8]) -> Option<ProcArgs> {
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?);
    let rest = &buf[4..];
    let rest = &rest[rest.iter().position(|b| *b == 0)?..]; // skip exec path
    let rest = &rest[rest.iter().position(|b| *b != 0).unwrap_or(rest.len())..]; // skip padding
    let mut parts = rest.split(|b| *b == 0);
    let argv: Vec<Vec<u8>> = parts.by_ref().take(argc.max(0) as usize).map(<[u8]>::to_vec).collect();
    let env = parts
        .take_while(|e| !e.is_empty())
        .take_while(|e| !APPLE_KEYS.iter().any(|k| e.starts_with(k.as_bytes())))
        .filter(|e| e.contains(&b'='))
        .map(<[u8]>::to_vec)
        .collect();
    Some((argv, env))
}

/// Listening ports per pid from `lsof -F pn` output: each `n` line belongs to
/// the most recent `p<pid>` line. Ports are sorted and deduplicated per pid;
/// pids without a LISTEN socket are absent.
pub fn parse_lsof_listen_by_pid(out: &str) -> std::collections::HashMap<i32, Vec<u16>> {
    let mut map: std::collections::HashMap<i32, Vec<u16>> = std::collections::HashMap::new();
    let mut pid: Option<i32> = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().ok();
        } else if let (Some(pid), Some(n)) = (pid, line.strip_prefix('n')) {
            if let Some(port) = n.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()) {
                map.entry(pid).or_default().push(port);
            }
        }
    }
    for ports in map.values_mut() {
        ports.sort_unstable();
        ports.dedup();
    }
    map
}

/// The non-empty text between the last pair of `mark`s in `out`.
pub fn extract_marked<'a>(out: &'a str, mark: &str) -> Option<&'a str> {
    let (before, _) = out.rsplit_once(mark)?;
    let (_, value) = before.rsplit_once(mark)?;
    (!value.is_empty()).then_some(value)
}

/// Output of `defaults read <domain> <bool key>`: `1` when the setting is on.
pub fn parse_defaults_bool(out: &str) -> bool {
    matches!(out.trim(), "1" | "true" | "YES")
}

/// The OS asks apps to reduce motion. Blocking: may start a process.
///
/// Linux: always false here, because GNOME's "Reduce animation" already reaches
/// GTK as `gtk-enable-animations`. macOS: System Settings > Accessibility >
/// Display > Reduce motion, which GTK does not read by itself.
pub fn reduce_motion() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::reduce_motion()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_bool_reads_only_true_values() {
        for on in ["1", "1\n", "true", "YES"] {
            assert!(parse_defaults_bool(on), "{on:?}");
        }
        for off in ["0", "0\n", "", "false", "NO", "The domain/default pair does not exist"] {
            assert!(!parse_defaults_bool(off), "{off:?}");
        }
    }

    fn procargs(argc: i32, body: &[u8]) -> Vec<u8> {
        let mut v = argc.to_ne_bytes().to_vec();
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn procargs2_splits_argv_and_env_and_drops_apple_strings() {
        let buf = procargs(
            2,
            b"/bin/sleep\0\0\0\0sleep\x005\0PATH=/usr/bin:/bin\0COMPOSE_PROJECT_NAME=x\0executable_path=/bin/sleep\0ptr_munge=\0\0\0",
        );
        let (argv, env) = parse_procargs2(&buf).unwrap();
        assert_eq!(argv, vec![b"sleep".to_vec(), b"5".to_vec()]);
        assert_eq!(env, vec![b"PATH=/usr/bin:/bin".to_vec(), b"COMPOSE_PROJECT_NAME=x".to_vec()]);
    }

    #[test]
    fn procargs2_handles_no_env_and_garbage() {
        let (argv, env) = parse_procargs2(&procargs(1, b"/bin/sh\0sh\0\0")).unwrap();
        assert_eq!(argv, vec![b"sh".to_vec()]);
        assert!(env.is_empty());
        assert!(parse_procargs2(b"\x01\x00").is_none());
        assert!(parse_procargs2(&procargs(1, b"no-nul")).is_none());
    }

    #[test]
    fn marked_value_ignores_startup_noise() {
        let m = "__M__";
        assert_eq!(
            extract_marked("motd\n__M__/opt/homebrew/bin:/usr/bin__M__", m),
            Some("/opt/homebrew/bin:/usr/bin")
        );
        assert_eq!(extract_marked("__M____M__", m), None);
        assert_eq!(extract_marked("no marks", m), None);
    }

    #[test]
    fn lsof_listen_ports_by_pid() {
        let out = "p100\nf5\nn*:3000\nf6\nn[::1]:3000\np200\nf7\nn127.0.0.1:5173\np300\n";
        let m = parse_lsof_listen_by_pid(out);
        assert_eq!(m.get(&100), Some(&vec![3000]));
        assert_eq!(m.get(&200), Some(&vec![5173]));
        assert!(!m.contains_key(&300));
        // Name lines look like `n*:8080`, `n127.0.0.1:3000` or `n[::1]:5173`;
        // ports are sorted and deduplicated per pid.
        let m = parse_lsof_listen_by_pid("p123\nf5\nn*:8080\nf6\nn127.0.0.1:3000\nf7\nn[::1]:5173\nf8\nn*:8080\n");
        assert_eq!(m.get(&123), Some(&vec![3000, 5173, 8080]));
        assert!(parse_lsof_listen_by_pid("").is_empty());
    }
}
