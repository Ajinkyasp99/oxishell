//! Subscription quota snapshots reported by the CLIs. No credentials or network requests.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub used_percent: f64,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub five_hour: Option<Window>,
    pub weekly: Option<Window>,
}

/// Status lines can redraw cached data while another session is working.
/// Repeated snapshots and API-key sessions must not replace newer quota data.
#[derive(Default)]
pub struct ClaudeReports {
    pub latest: Usage,
    /// When `latest` was observed (unix seconds); 0 = never.
    latest_at: i64,
    reports: std::collections::HashMap<crate::model::PaneId, Usage>,
}

impl ClaudeReports {
    pub fn update(&mut self, pane: crate::model::PaneId, usage: Usage) {
        if self.reports.get(&pane) == Some(&usage) {
            return;
        }
        if usage != Usage::default() {
            self.latest = usage.clone();
            self.latest_at = now();
        }
        self.reports.insert(pane, usage);
    }

    /// Claude Code's own usage cache (see [`read_claude_cache`]): used when it
    /// is newer than any status-line report, e.g. right after a session starts
    /// and before its first response, or after cmux-linux restarts.
    pub fn merge_cache(&mut self, fetched_at: i64, usage: Usage) {
        if usage != Usage::default() && fetched_at > self.latest_at {
            self.latest = usage;
            self.latest_at = fetched_at;
        }
    }

    pub fn retain_panes(&mut self, mut exists: impl FnMut(crate::model::PaneId) -> bool) {
        self.reports.retain(|pane, _| exists(*pane));
    }
}

fn window(value: &Value, percentage_key: &str) -> Option<Window> {
    let used_percent = value.get(percentage_key)?.as_f64()?;
    if !used_percent.is_finite() || !(0.0..=100.0).contains(&used_percent) {
        return None;
    }
    Some(Window {
        used_percent,
        resets_at: value.get("resets_at").and_then(Value::as_i64).filter(|t| *t > 0),
    })
}

pub fn claude(payload: &Value) -> Usage {
    let limits = &payload["rate_limits"];
    Usage {
        five_hour: window(&limits["five_hour"], "used_percentage"),
        weekly: window(&limits["seven_day"], "used_percentage"),
    }
}

fn codex(limits: &Value) -> Option<Usage> {
    // Other buckets (e.g. code review) must not replace the main Codex quota.
    if limits.get("limit_id").and_then(Value::as_str).is_some_and(|id| id != "codex") {
        return None;
    }
    let mut usage = Usage::default();
    for key in ["primary", "secondary"] {
        let value = &limits[key];
        match value.get("window_minutes").and_then(Value::as_u64) {
            Some(300) => usage.five_hour = window(value, "used_percent"),
            Some(10080) => usage.weekly = window(value, "used_percent"),
            _ => {}
        }
    }
    (usage != Usage::default()).then_some(usage)
}

/// Claude Code's global config file (`$CLAUDE_CONFIG_DIR/.claude.json`, else `~/.claude.json`).
pub fn claude_config_file() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(|d| PathBuf::from(d).join(".claude.json"))
        .unwrap_or_else(|| crate::paths::home().join(".claude.json"))
}

/// Claude Code caches the subscription usage it last fetched in its config
/// file (`cachedUsageUtilization`: `utilization` percentages, ISO `resets_at`
/// times and `fetchedAtMs`). Status lines only carry `rate_limits` after a
/// session's first response, so this is what can be shown before that.
/// Only these fields are read. Returns (fetched at, unix seconds; usage).
pub fn read_claude_cache(path: &Path) -> Option<(i64, Usage)> {
    let text = std::fs::read(path).ok()?;
    let config: Value = serde_json::from_slice(&text).ok()?;
    parse_claude_cache(&config["cachedUsageUtilization"])
}

pub fn parse_claude_cache(cache: &Value) -> Option<(i64, Usage)> {
    let fetched_at = cache.get("fetchedAtMs")?.as_f64()? as i64 / 1000;
    let windows = &cache["utilization"];
    let window = |key: &str| {
        let w = &windows[key];
        let used_percent = w.get("utilization")?.as_f64()?;
        if !used_percent.is_finite() || !(0.0..=100.0).contains(&used_percent) {
            return None;
        }
        Some(Window {
            used_percent,
            resets_at: w.get("resets_at").and_then(Value::as_str).and_then(parse_iso8601),
        })
    };
    let usage = Usage {
        five_hour: window("five_hour"),
        weekly: window("seven_day"),
    };
    (usage != Usage::default()).then_some((fetched_at, usage))
}

/// `2026-11-07T16:00:00.175985+00:00` (or `…Z`) → unix seconds.
pub fn parse_iso8601(s: &str) -> Option<i64> {
    let s = s.trim();
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    if s.len() < 19 || &s[4..5] != "-" || &s[7..8] != "-" || !matches!(&s[10..11], "T" | " ") {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, sec) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    // Skip fractional seconds, then read the zone.
    let rest = s[19..].trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match rest {
        "" | "Z" | "z" => 0,
        z if (z.starts_with('+') || z.starts_with('-')) && z.len() >= 6 => {
            let sign = if z.starts_with('-') { -1 } else { 1 };
            let oh: i64 = z.get(1..3)?.parse().ok()?;
            let om: i64 = z.get(4..6)?.parse().ok()?;
            sign * (oh * 3600 + om * 60)
        }
        _ => return None,
    };
    // Days since 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + sec - offset)
}

pub fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::paths::home().join(".codex"))
}

/// Read bounded tails of the most recently modified rollouts. This also works
/// when an old session is resumed. Never deserialize conversation content.
pub fn read_codex(root: &Path) -> Option<Usage> {
    fn collect(dir: &Path, depth: usize, out: &mut Vec<(SystemTime, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() && depth > 0 {
                collect(&path, depth - 1, out);
            } else if kind.is_file()
                && path.extension().is_some_and(|e| e == "jsonl")
                && entry.file_name().to_string_lossy().starts_with("rollout-")
            {
                if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                    out.push((modified, path));
                }
            }
        }
    }
    let mut files = Vec::new();
    collect(&root.join("sessions"), 3, &mut files);
    files.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.0));
    let mut latest: Option<(String, Usage)> = None;
    for (_, path) in files.into_iter().take(12) {
        let Ok(mut file) = std::fs::File::open(path) else { continue };
        let Ok(len) = file.metadata().map(|m| m.len()) else { continue };
        let offset = len.saturating_sub(512 * 1024);
        if file.seek(SeekFrom::Start(offset)).is_err() {
            continue;
        }
        let mut bytes = Vec::new();
        if file.take(512 * 1024).read_to_end(&mut bytes).is_err() {
            continue;
        }
        // When seeking into a record, discard its partial first line. A partial
        // final record is ignored until the CLI finishes writing it.
        for (index, line) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
            if (offset > 0 && index == 0) || !line.ends_with(b"\n") || !line.windows(13).any(|w| w == b"\"rate_limits\"") {
                continue;
            }
            let Ok(event) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            if event["type"] != "event_msg" || event["payload"]["type"] != "token_count" {
                continue;
            }
            let Some(usage) = codex(&event["payload"]["rate_limits"]) else {
                continue;
            };
            let Some(timestamp) = event["timestamp"].as_str() else { continue };
            if latest.as_ref().is_none_or(|(t, _)| timestamp > t.as_str()) {
                latest = Some((timestamp.to_string(), usage));
            }
        }
    }
    latest.map(|(_, usage)| usage)
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64
}

impl Window {
    pub fn expired(&self, now: i64) -> bool {
        self.resets_at.is_some_and(|reset| reset <= now)
    }

    pub fn remaining(&self) -> f64 {
        100.0 - self.used_percent
    }

    pub fn reset_text(&self, now: i64) -> String {
        let Some(reset) = self.resets_at else {
            return "Reset time unavailable".into();
        };
        let seconds = reset.saturating_sub(now);
        if seconds <= 0 {
            "Awaiting updated limits".into()
        } else if seconds < 60 {
            "Resets in <1m".into()
        } else {
            let minutes = seconds / 60;
            if minutes >= 1440 {
                format!("Resets in {}d {}h", minutes / 1440, (minutes % 1440) / 60)
            } else if minutes >= 60 {
                format!("Resets in {}h {}m", minutes / 60, minutes % 60)
            } else {
                format!("Resets in {minutes}m")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_both_claude_windows_without_confusing_context_usage() {
        let usage = claude(&json!({"context_window": {"used_percentage": 99}, "rate_limits": {
            "five_hour": {"used_percentage": 23.5, "resets_at": 2000},
            "seven_day": {"used_percentage": 80, "resets_at": 9000}
        }}));
        assert_eq!(usage.five_hour.unwrap().remaining(), 76.5);
        assert_eq!(usage.weekly.unwrap().remaining(), 20.0);
        assert_eq!(claude(&json!({})), Usage::default());
        assert!(claude(&json!({"rate_limits": {"five_hour": {"used_percentage": 101}}}))
            .five_hour
            .is_none());
        assert!(claude(&json!({"rate_limits": {"seven_day": {"used_percentage": -1}}}))
            .weekly
            .is_none());
    }

    #[test]
    fn idle_claude_redraws_and_missing_quotas_do_not_overwrite_another_session() {
        let old = claude(&json!({"rate_limits": {"five_hour": {"used_percentage": 20}}}));
        let new = claude(&json!({"rate_limits": {"five_hour": {"used_percentage": 40}}}));
        let mut reports = ClaudeReports::default();
        reports.update(1, old.clone());
        reports.update(2, new.clone());
        reports.update(1, old);
        assert_eq!(reports.latest, new);
        reports.update(3, Usage::default());
        assert_eq!(reports.latest, new);
        reports.retain_panes(|pane| pane == 2);
        assert_eq!(reports.reports.len(), 1);
    }

    #[test]
    fn parses_iso8601_times() {
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601("2026-11-07T16:00:00.175985+00:00"), Some(1_794_067_200));
        assert_eq!(parse_iso8601("2026-11-07T21:30:00+05:30"), Some(1_794_067_200));
        assert_eq!(parse_iso8601("2024-02-29T12:00:00Z"), Some(1_709_208_000));
        assert_eq!(parse_iso8601("2026-13-07T16:00:00Z"), None);
        assert_eq!(parse_iso8601("not a date"), None);
    }

    #[test]
    fn claude_cache_fills_in_until_a_newer_status_line_report() {
        let cache = json!({"fetchedAtMs": 1_794_050_636_467_i64, "accountUuid": "x", "utilization": {
            "five_hour": {"utilization": 7, "resets_at": "2026-11-07T16:00:00.175985+00:00"},
            "seven_day": {"utilization": 51, "resets_at": "2026-11-09T20:00:00.176003+00:00"},
            "seven_day_opus": null
        }});
        let (fetched, usage) = parse_claude_cache(&cache).unwrap();
        assert_eq!(fetched, 1_794_050_636);
        assert_eq!(usage.five_hour.as_ref().unwrap().used_percent, 7.0);
        assert_eq!(usage.five_hour.as_ref().unwrap().resets_at, Some(1_794_067_200));
        assert_eq!(usage.weekly.as_ref().unwrap().used_percent, 51.0);
        assert!(parse_claude_cache(&json!({})).is_none());
        assert!(parse_claude_cache(&json!({"fetchedAtMs": 1, "utilization": {}})).is_none());

        let mut reports = ClaudeReports::default();
        reports.merge_cache(fetched, usage.clone());
        assert_eq!(reports.latest, usage, "shown before any status-line report");
        // A live report replaces it; an older cache never overrides a live report.
        let live = claude(&json!({"rate_limits": {"five_hour": {"used_percentage": 9}}}));
        reports.update(1, live.clone());
        assert_eq!(reports.latest, live);
        reports.merge_cache(fetched, usage);
        assert_eq!(reports.latest, live);
    }

    #[test]
    fn codex_windows_follow_duration_and_ignore_other_buckets() {
        let limits = json!({"limit_id": "codex", "primary": {"window_minutes": 10080, "used_percent": 70},
            "secondary": {"window_minutes": 300, "used_percent": 5}});
        let usage = codex(&limits).unwrap();
        assert_eq!(usage.five_hour.unwrap().remaining(), 95.0);
        assert_eq!(usage.weekly.unwrap().remaining(), 30.0);
        let mut other = limits;
        other["limit_id"] = json!("codex_review");
        assert!(codex(&other).is_none());
        assert!(codex(&json!({"primary": {"window_minutes": 15, "used_percent": 5}})).is_none());
    }

    #[test]
    fn reset_countdown_never_claims_a_new_allowance() {
        let mut w = Window {
            used_percent: 100.0,
            resets_at: Some(1000),
        };
        assert_eq!(w.remaining(), 0.0);
        assert_eq!(w.reset_text(1000), "Awaiting updated limits");
        assert!(w.expired(1000));
        assert_eq!(w.reset_text(999), "Resets in <1m");
        w.resets_at = Some(1000 + 90060);
        assert_eq!(w.reset_text(1000), "Resets in 1d 1h");
        w.resets_at = None;
        assert_eq!(w.reset_text(1000), "Reset time unavailable");
    }

    #[test]
    fn rollout_reader_uses_latest_quota_event_and_ignores_partial_records() {
        let root = std::env::temp_dir().join(format!("cmux-usage-{}", std::process::id()));
        let day = root.join("sessions/2026/11/07");
        std::fs::create_dir_all(&day).unwrap();
        let event = |timestamp: &str, used: u8| {
            json!({"timestamp": timestamp, "type": "event_msg",
            "payload": {"type": "token_count", "rate_limits": {
                "limit_id": "codex", "primary": {"used_percent": used, "window_minutes": 300},
                "secondary": {"used_percent": 20, "window_minutes": 10080}
            }}})
            .to_string()
        };
        let newer = event("2026-11-07T10:00:00.000Z", 40);
        let older = event("2026-11-07T09:00:00.000Z", 10);
        std::fs::write(day.join("rollout-new.jsonl"), format!("{newer}\n{{\"partial\":")).unwrap();
        std::fs::write(day.join("rollout-old.jsonl"), format!("{older}\n")).unwrap();
        let usage = read_codex(&root).unwrap();
        assert_eq!(usage.five_hour.unwrap().remaining(), 60.0);
        assert_eq!(usage.weekly.unwrap().remaining(), 80.0);
        assert!(read_codex(&root.join("missing")).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
