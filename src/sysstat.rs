//! System-wide CPU and memory usage read from `/proc` for the status bar.
//! Parsing is pure and unit-tested; `Sampler` keeps the previous CPU reading
//! because CPU usage is a delta between two samples.

use std::fs;

/// Aggregate CPU jiffies from the first `cpu` line of `/proc/stat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTimes {
    pub idle: u64,
    pub total: u64,
}

/// Parse the aggregate `cpu ` line of `/proc/stat`.
pub fn parse_cpu(stat: &str) -> Option<CpuTimes> {
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let nums: Vec<u64> = line.split_whitespace().skip(1).filter_map(|s| s.parse().ok()).collect();
    if nums.len() < 4 {
        return None;
    }
    // user nice system idle iowait irq softirq steal [guest guest_nice]
    // guest time is already counted in user/nice, so stop at steal.
    let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
    let total = nums.iter().take(8).sum();
    Some(CpuTimes { idle, total })
}

/// CPU busy percentage between two samples.
pub fn cpu_percent(prev: CpuTimes, cur: CpuTimes) -> Option<f64> {
    let total = cur.total.checked_sub(prev.total)?;
    let idle = cur.idle.checked_sub(prev.idle)?;
    if total == 0 {
        return None;
    }
    Some((total.saturating_sub(idle)) as f64 * 100.0 / total as f64)
}

/// Memory in kibibytes: (used, total). Used = MemTotal - MemAvailable.
pub fn parse_mem(meminfo: &str) -> Option<(u64, u64)> {
    let field = |name: &str| -> Option<u64> {
        meminfo
            .lines()
            .find_map(|l| l.strip_prefix(name)?.strip_prefix(':'))?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    };
    let total = field("MemTotal")?;
    let avail = field("MemAvailable").or_else(|| Some(field("MemFree")? + field("Cached").unwrap_or(0)))?;
    Some((total.saturating_sub(avail), total))
}

/// Format kibibytes as GiB with one decimal place.
fn gib(kib: u64) -> String {
    format!("{:.1}G", kib as f64 / (1024.0 * 1024.0))
}

/// Status-bar text, e.g. `CPU 12%  |  Mem 6.3G/15.5G (41%)`.
pub fn format(cpu: Option<f64>, mem: Option<(u64, u64)>) -> String {
    let mut parts = Vec::new();
    parts.push(match cpu {
        Some(c) => format!("CPU {:.0}%", c.clamp(0.0, 100.0)),
        None => "CPU —".into(),
    });
    if let Some((used, total)) = mem.filter(|(_, t)| *t > 0) {
        let pct = used as f64 * 100.0 / total as f64;
        parts.push(format!("Mem {}/{} ({:.0}%)", gib(used), gib(total), pct));
    }
    parts.join("  |  ")
}

/// Keeps the last CPU sample so each `sample()` reports usage since the previous call.
#[derive(Default)]
pub struct Sampler {
    prev: Option<CpuTimes>,
}

impl Sampler {
    pub fn sample(&mut self) -> String {
        let cur = fs::read_to_string("/proc/stat").ok().and_then(|s| parse_cpu(&s));
        let cpu = match (self.prev, cur) {
            (Some(p), Some(c)) => cpu_percent(p, c),
            _ => None,
        };
        if cur.is_some() {
            self.prev = cur;
        }
        let mem = fs::read_to_string("/proc/meminfo").ok().and_then(|s| parse_mem(&s));
        format(cpu, mem)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "cpu  100 10 50 800 40 0 0 0 0 0\ncpu0 50 5 25 400 20 0 0 0 0 0\nintr 1\n";

    #[test]
    fn parses_aggregate_cpu_line() {
        assert_eq!(parse_cpu(STAT), Some(CpuTimes { idle: 840, total: 1000 }));
        assert_eq!(parse_cpu("intr 1\n"), None);
        assert_eq!(parse_cpu("cpu 1 2\n"), None);
    }

    #[test]
    fn cpu_percent_is_busy_share_of_delta() {
        let a = CpuTimes { idle: 800, total: 1000 };
        let b = CpuTimes { idle: 850, total: 1200 };
        assert_eq!(cpu_percent(a, b), Some(75.0));
        assert_eq!(cpu_percent(a, a), None);
        assert_eq!(cpu_percent(b, a), None);
    }

    #[test]
    fn parses_meminfo() {
        let m = "MemTotal:       16000000 kB\nMemFree:  1000000 kB\nMemAvailable:   10000000 kB\n";
        assert_eq!(parse_mem(m), Some((6000000, 16000000)));
        let old = "MemTotal: 1000 kB\nMemFree: 300 kB\nCached: 200 kB\n";
        assert_eq!(parse_mem(old), Some((500, 1000)));
        assert_eq!(parse_mem("nothing"), None);
    }

    #[test]
    fn formats_status_text() {
        let s = format(Some(12.4), Some((4 * 1024 * 1024, 16 * 1024 * 1024)));
        assert_eq!(s, "CPU 12%  |  Mem 4.0G/16.0G (25%)");
        assert_eq!(format(None, None), "CPU —");
    }
}
