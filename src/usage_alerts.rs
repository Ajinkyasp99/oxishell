//! One-time alerts when an agent's usage limit passes 80% and 95%.
//!
//! Each threshold fires once per limit window (5-hour, weekly) and agent. A
//! window is told apart by its reset time, so the next window can alert again,
//! and the state is saved so a restart does not repeat an alert. When a window
//! that alerted resets, one quiet note says so.

use crate::usage::{Usage, Window};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Usage percentages that alert.
pub const THRESHOLDS: [u8; 2] = [80, 95];

/// Reset times closer than this belong to the same window (reports can be a
/// few seconds apart for the same window).
const SAME_WINDOW_SECS: i64 = 600;

/// A reset older than this is not announced (oxishell was closed or the agent
/// was not running when it happened).
const RESET_NOTE_MAX_AGE_SECS: i64 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    FiveHour,
    Weekly,
}

impl Span {
    pub fn label(self) -> &'static str {
        match self {
            Span::FiveHour => "5-hour",
            Span::Weekly => "weekly",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Alert {
    /// Usage reached `threshold` percent.
    Crossed { span: Span, threshold: u8, window: Window },
    /// A window that alerted has reset.
    Reset { span: Span },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Fired {
    resets_at: Option<i64>,
    threshold: u8,
}

/// Highest threshold already announced per agent and window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Alerts {
    fired: BTreeMap<String, Fired>,
}

/// Two reset times describe the same limit window.
fn same_window(a: Option<i64>, b: Option<i64>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => (x - y).abs() < SAME_WINDOW_SECS,
        (None, None) => true,
        _ => false,
    }
}

impl Alerts {
    /// Alerts due for `agent` given its latest `usage`, recording them.
    pub fn check(&mut self, agent: &str, usage: &Usage, now: i64) -> Vec<Alert> {
        let mut out = Vec::new();
        for (span, window) in [(Span::FiveHour, &usage.five_hour), (Span::Weekly, &usage.weekly)] {
            let key = format!("{agent}:{}", span.label());
            // A window that alerted has reset.
            if let Some(reset) = self.fired.get(&key).and_then(|f| f.resets_at).filter(|r| *r <= now) {
                if now - reset <= RESET_NOTE_MAX_AGE_SECS {
                    out.push(Alert::Reset { span });
                }
                self.fired.remove(&key);
            }
            // Expired snapshots describe a window that is over: wait for new data.
            let Some(w) = window.as_ref().filter(|w| !w.expired(now)) else {
                continue;
            };
            let Some(threshold) = THRESHOLDS.iter().rev().copied().find(|t| w.used_percent >= f64::from(*t)) else {
                continue;
            };
            let announced = self
                .fired
                .get(&key)
                .filter(|f| same_window(f.resets_at, w.resets_at))
                .map_or(0, |f| f.threshold);
            if threshold > announced {
                self.fired.insert(
                    key,
                    Fired {
                        resets_at: w.resets_at,
                        threshold,
                    },
                );
                out.push(Alert::Crossed {
                    span,
                    threshold,
                    window: w.clone(),
                });
            }
        }
        out
    }

    /// Saved state, or none when the file is missing or unreadable.
    pub fn load(path: &Path) -> Alerts {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string(self)?)?;
        std::fs::rename(tmp, path)
    }
}

/// Notification title and body for `alert` about `agent_name` (e.g. "Claude Code").
pub fn message(agent_name: &str, alert: &Alert, now: i64) -> (String, String) {
    match alert {
        Alert::Crossed { span, threshold, window } => (
            format!("{agent_name} usage at {threshold}%"),
            format!(
                "{:.0}% of the {} limit used · {}",
                window.used_percent,
                span.label(),
                window.reset_text(now)
            ),
        ),
        Alert::Reset { span } => (
            format!("{agent_name} {} limit reset", span.label()),
            format!("The {} usage window has started again.", span.label()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000;
    const RESET: i64 = NOW + 3 * 3600;

    fn usage(five: Option<f64>, weekly: Option<f64>) -> Usage {
        let w = |p, r| Window {
            used_percent: p,
            resets_at: Some(r),
        };
        Usage {
            five_hour: five.map(|p| w(p, RESET)),
            weekly: weekly.map(|p| w(p, RESET + 5 * 86_400)),
        }
    }

    fn crossed(alerts: &[Alert]) -> Vec<(Span, u8)> {
        alerts
            .iter()
            .filter_map(|a| match a {
                Alert::Crossed { span, threshold, .. } => Some((*span, *threshold)),
                Alert::Reset { .. } => None,
            })
            .collect()
    }

    #[test]
    fn nothing_below_the_first_threshold() {
        let mut a = Alerts::default();
        assert!(a.check("claude", &usage(Some(79.9), Some(10.0)), NOW).is_empty());
    }

    #[test]
    fn each_threshold_alerts_once_per_window() {
        let mut a = Alerts::default();
        assert_eq!(crossed(&a.check("claude", &usage(Some(81.0), None), NOW)), [(Span::FiveHour, 80)]);
        assert!(a.check("claude", &usage(Some(85.0), None), NOW + 60).is_empty());
        assert_eq!(
            crossed(&a.check("claude", &usage(Some(95.0), None), NOW + 120)),
            [(Span::FiveHour, 95)]
        );
        assert!(a.check("claude", &usage(Some(99.0), None), NOW + 180).is_empty());
    }

    #[test]
    fn a_jump_past_both_thresholds_alerts_once_with_the_higher_one() {
        let mut a = Alerts::default();
        assert_eq!(crossed(&a.check("claude", &usage(Some(96.0), None), NOW)), [(Span::FiveHour, 95)]);
        assert!(a.check("claude", &usage(Some(97.0), None), NOW + 60).is_empty());
    }

    #[test]
    fn windows_and_agents_are_independent() {
        let mut a = Alerts::default();
        let got = crossed(&a.check("claude", &usage(Some(81.0), Some(82.0)), NOW));
        assert_eq!(got, [(Span::FiveHour, 80), (Span::Weekly, 80)]);
        assert_eq!(crossed(&a.check("codex", &usage(Some(81.0), None), NOW)), [(Span::FiveHour, 80)]);
    }

    #[test]
    fn small_differences_in_reset_time_are_the_same_window() {
        let mut a = Alerts::default();
        a.check("codex", &usage(Some(81.0), None), NOW);
        let mut u = usage(Some(82.0), None);
        u.five_hour.as_mut().unwrap().resets_at = Some(RESET + 30);
        assert!(a.check("codex", &u, NOW + 60).is_empty());
    }

    #[test]
    fn a_new_window_alerts_again_and_the_reset_is_announced_once() {
        let mut a = Alerts::default();
        a.check("claude", &usage(Some(81.0), None), NOW);
        // The window has reset; the CLI has not reported new data yet.
        let after = RESET + 60;
        assert_eq!(
            a.check("claude", &usage(Some(81.0), None), after),
            [Alert::Reset { span: Span::FiveHour }]
        );
        assert!(a.check("claude", &usage(Some(81.0), None), after + 60).is_empty());
        // New window, already past 80%.
        let mut u = usage(Some(83.0), None);
        u.five_hour.as_mut().unwrap().resets_at = Some(RESET + 5 * 3600);
        assert_eq!(crossed(&a.check("claude", &u, after + 120)), [(Span::FiveHour, 80)]);
    }

    #[test]
    fn no_reset_note_without_an_alert_or_long_after_the_reset() {
        let mut a = Alerts::default();
        a.check("claude", &usage(Some(50.0), None), NOW);
        assert!(a.check("claude", &usage(Some(50.0), None), RESET + 60).is_empty());

        let mut a = Alerts::default();
        a.check("claude", &usage(Some(81.0), None), NOW);
        assert!(a.check("claude", &usage(None, None), RESET + 2 * 3600).is_empty());
        assert!(a.check("claude", &usage(None, None), RESET + 3 * 3600).is_empty());
    }

    #[test]
    fn stale_snapshots_do_not_alert() {
        let mut a = Alerts::default();
        assert!(a.check("claude", &usage(Some(90.0), None), RESET + 1).is_empty());
    }

    #[test]
    fn state_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("oxishell-alerts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("usage-alerts.json");
        assert_eq!(Alerts::load(&path), Alerts::default(), "missing file");
        let mut a = Alerts::default();
        a.check("claude", &usage(Some(81.0), None), NOW);
        a.save(&path).unwrap();
        let mut b = Alerts::load(&path);
        assert_eq!(b, a);
        assert!(b.check("claude", &usage(Some(82.0), None), NOW + 60).is_empty());
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(Alerts::load(&path), Alerts::default(), "corrupt file");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn messages_name_the_agent_window_and_reset() {
        let alert = Alert::Crossed {
            span: Span::FiveHour,
            threshold: 80,
            window: Window {
                used_percent: 81.4,
                resets_at: Some(NOW + 90 * 60),
            },
        };
        assert_eq!(
            message("Claude Code", &alert, NOW),
            (
                "Claude Code usage at 80%".to_string(),
                "81% of the 5-hour limit used · Resets in 1h 30m".to_string()
            )
        );
        assert_eq!(
            message("Codex", &Alert::Reset { span: Span::Weekly }, NOW),
            (
                "Codex weekly limit reset".to_string(),
                "The weekly usage window has started again.".to_string()
            )
        );
    }
}
