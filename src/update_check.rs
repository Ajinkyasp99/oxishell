//! Optional check for a newer oxishell release (Settings → Notifications →
//! Check for updates; off by default). At most one request a day to GitHub's
//! "latest release" endpoint; nothing is downloaded or installed, and every
//! failure is silent.

use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use std::time::Duration;

/// GitHub API endpoint for the newest release.
pub const RELEASES_URL: &str = "https://api.github.com/repos/Ajinkyasp99/oxishell/releases/latest";

/// Minimum time between two checks.
pub const INTERVAL_SECS: i64 = 24 * 3600;

/// Where to ask: `$OXISHELL_UPDATE_URL` (used by tests) or GitHub.
pub fn url() -> String {
    std::env::var("OXISHELL_UPDATE_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| RELEASES_URL.to_string())
}

/// Body of a GET request to `url`: short timeouts, a generic user agent, at
/// most 256 KiB read. Blocking; call it off the main thread.
pub fn fetch(url: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(20)))
        .max_redirects(3)
        .user_agent(concat!("oxishell/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let response = agent
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    let mut body = String::new();
    response
        .into_body()
        .into_reader()
        .take(256 * 1024)
        .read_to_string(&mut body)
        .map_err(|e| e.to_string())?;
    Ok(body)
}

/// A published release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Version without a leading `v`, e.g. "0.1.4".
    pub version: String,
    /// Release page with the notes.
    pub url: String,
}

/// What is remembered between runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Unix seconds of the last request (successful or not); 0 = never.
    pub last_check: i64,
    /// Newest version already announced, so each one is shown once.
    pub announced: Option<String>,
}

/// `major.minor.patch` of a version string like `v0.1.4`; None if it is not one.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.split('.').map(|p| p.parse::<u64>().ok());
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

/// `latest` is a newer version than `current`.
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

/// The release described by GitHub's JSON reply, skipping drafts and pre-releases.
pub fn parse_release(json: &str) -> Option<Release> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return None;
    }
    let tag = v["tag_name"].as_str()?;
    parse_version(tag)?;
    let url = v["html_url"].as_str().filter(|u| u.starts_with("https://"))?;
    Some(Release {
        version: tag.trim().trim_start_matches('v').to_string(),
        url: url.to_string(),
    })
}

impl State {
    /// A check is due: never checked, a day has passed, or the clock went back.
    pub fn due(&self, now: i64) -> bool {
        self.last_check == 0 || now < self.last_check || now - self.last_check >= INTERVAL_SECS
    }

    /// The release to announce, if it is newer than `current` and not yet announced.
    /// Records it as announced.
    pub fn announce(&mut self, current: &str, release: &Release) -> bool {
        let seen = self.announced.as_deref().is_some_and(|a| !is_newer(a, &release.version));
        if !is_newer(current, &release.version) || seen {
            return false;
        }
        self.announced = Some(release.version.clone());
        true
    }

    pub fn load(path: &Path) -> State {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(v: &str) -> Release {
        Release {
            version: v.into(),
            url: format!("https://github.com/x/releases/tag/v{v}"),
        }
    }

    #[test]
    fn parses_versions_with_or_without_a_v() {
        assert_eq!(parse_version("0.1.3"), Some((0, 1, 3)));
        assert_eq!(parse_version("v0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version(" v1.10.2 "), Some((1, 10, 2)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("latest"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.1.3", "0.1.4"));
        assert!(is_newer("0.1.3", "v0.2.0"));
        assert!(is_newer("0.1.9", "0.1.10"));
        assert!(!is_newer("0.1.3", "0.1.3"));
        assert!(!is_newer("0.2.0", "0.1.9"));
        assert!(!is_newer("0.1.3", "garbage"));
    }

    #[test]
    fn reads_githubs_latest_release_reply() {
        let json = r#"{"tag_name":"v0.1.4","html_url":"https://github.com/a/b/releases/tag/v0.1.4","draft":false,"prerelease":false,"name":"0.1.4"}"#;
        assert_eq!(
            parse_release(json),
            Some(Release {
                version: "0.1.4".into(),
                url: "https://github.com/a/b/releases/tag/v0.1.4".into()
            })
        );
        assert_eq!(
            parse_release(r#"{"tag_name":"v0.2.0","html_url":"https://x","prerelease":true}"#),
            None
        );
        assert_eq!(parse_release(r#"{"tag_name":"v0.2.0","html_url":"https://x","draft":true}"#), None);
        assert_eq!(parse_release(r#"{"tag_name":"nightly","html_url":"https://x"}"#), None);
        assert_eq!(
            parse_release(r#"{"tag_name":"v0.2.0","html_url":"javascript:alert(1)"}"#),
            None,
            "only https links"
        );
        assert_eq!(parse_release(r#"{"message":"API rate limit exceeded"}"#), None);
        assert_eq!(parse_release("<html>"), None);
    }

    #[test]
    fn checks_at_most_once_a_day() {
        let now = 1_000_000;
        assert!(State::default().due(now));
        let s = State {
            last_check: now,
            announced: None,
        };
        assert!(!s.due(now + 3600));
        assert!(s.due(now + INTERVAL_SECS));
        assert!(s.due(now - 10), "clock moved back");
    }

    #[test]
    fn announces_each_newer_version_once() {
        let mut s = State::default();
        assert!(!s.announce("0.1.3", &rel("0.1.3")), "same version");
        assert!(s.announce("0.1.3", &rel("0.1.4")));
        assert_eq!(s.announced.as_deref(), Some("0.1.4"));
        assert!(!s.announce("0.1.3", &rel("0.1.4")), "already announced");
        assert!(s.announce("0.1.3", &rel("0.1.5")), "a newer one is announced again");
    }

    #[test]
    fn state_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("oxishell-update-{}", std::process::id()));
        let path = dir.join("update-check.json");
        assert_eq!(State::load(&path), State::default());
        let s = State {
            last_check: 42,
            announced: Some("0.1.4".into()),
        };
        s.save(&path).unwrap();
        assert_eq!(State::load(&path), s);
        std::fs::write(&path, "{").unwrap();
        assert_eq!(State::load(&path), State::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
