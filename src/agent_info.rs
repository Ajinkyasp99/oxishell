//! Text shown for an agent session: task summary, short model name and time
//! since activity. GTK-free so it can be unit-tested.

use crate::agents::AgentState;
use std::time::Duration;

/// Longest summary or prompt line we keep.
pub const MAX_SUMMARY: usize = 120;

/// One line of sanitized text, at most [`MAX_SUMMARY`] chars (`…` when cut).
fn one_line(s: &str) -> Option<String> {
    let s = crate::notify::sanitize(s, 4 * MAX_SUMMARY);
    let line = s.lines().map(str::trim).find(|l| !l.is_empty())?;
    if line.chars().count() > MAX_SUMMARY {
        let mut cut: String = line.chars().take(MAX_SUMMARY - 1).collect();
        cut.push('…');
        Some(cut)
    } else {
        Some(line.to_string())
    }
}

/// Spinner and status glyphs agents put before their terminal title.
fn is_title_glyph(c: char) -> bool {
    c.is_whitespace()
        || ('\u{2800}'..='\u{28FF}').contains(&c) // braille spinners
        || "✳✶✻✽✢✱✷✸✹✺·*⏺●○◐◑◒◓".contains(c)
}

/// The task summary in an agent-set terminal title, or `None` when the title
/// is empty or only names the agent (e.g. "✳ Claude Code").
pub fn title_summary(title: &str, agent: &str) -> Option<String> {
    let rest = title.trim_start_matches(is_title_glyph);
    let line = one_line(rest)?;
    let lower = line.to_lowercase();
    if lower == agent.to_lowercase() || lower == crate::agents::display_name(agent).to_lowercase() {
        return None;
    }
    Some(line)
}

/// First non-empty line of a submitted prompt, sanitized and capped.
pub fn prompt_line(prompt: &str) -> Option<String> {
    one_line(prompt)
}

/// Short display name for a model id or display name ("claude-opus-5-5" → "opus").
pub fn short_model(model: &str) -> Option<String> {
    let model = crate::notify::sanitize(model, 64);
    if model.is_empty() {
        return None;
    }
    let lower = model.to_lowercase();
    for family in ["opus", "sonnet", "haiku", "fable"] {
        if lower.contains(family) {
            return Some(family.to_string());
        }
    }
    for prefix in ["gpt-", "gemini-"] {
        if lower.starts_with(prefix) {
            let parts: Vec<&str> = lower.splitn(3, '-').collect();
            if parts.len() >= 2 && !parts[1].is_empty() {
                return Some(format!("{}-{}", parts[0], parts[1]));
            }
        }
    }
    Some(model.chars().take(12).collect())
}

/// Short agent name for sidebar lines and tabs ("claude" → "Claude").
pub fn short_agent(agent: &str) -> String {
    match agent {
        "claude" => "Claude",
        "codex" => "Codex",
        "gemini" => "Gemini",
        "qoder" => "Qoder",
        "opencode" => "OpenCode",
        "cursor" => "Cursor",
        "amp" => "Amp",
        "aider" => "Aider",
        other => return crate::agents::display_name(other),
    }
    .to_string()
}

/// Tab label: the agent and what it is doing, else the busy program, else
/// the folder name.
pub fn tab_text(agent: Option<&str>, summary: Option<&str>, program: Option<&str>, cwd: Option<&std::path::Path>) -> String {
    if let Some(agent) = agent {
        return format!("{}: {}", short_agent(agent), summary.unwrap_or(""));
    }
    if let Some(p) = program.filter(|p| !p.is_empty()) {
        return p.to_string();
    }
    match cwd {
        Some(c) => c
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| c.display().to_string()),
        None => "Terminal".to_string(),
    }
}

/// Compact age: `now`, `42s`, `4m`, `2h`, `3d`.
pub fn time_ago(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..5 => "now".to_string(),
        5..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// What an agent session is doing: its title, else its last prompt, else its state.
pub fn summary(title: Option<&str>, prompt: Option<&str>, state: Option<AgentState>) -> String {
    let label = state.map(|s| s.label()).unwrap_or(AgentState::Running.label());
    match (title.or(prompt), state) {
        (Some(text), Some(AgentState::Waiting)) => format!("{label}: {text}"),
        (Some(text), _) => text.to_string(),
        (None, _) => label.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_qualification() {
        assert_eq!(
            title_summary("✳ Fix token refresh race", "claude").as_deref(),
            Some("Fix token refresh race")
        );
        assert_eq!(title_summary("⠙ Add tests", "codex").as_deref(), Some("Add tests"));
        assert_eq!(title_summary("✳ Claude Code", "claude"), None);
        assert_eq!(title_summary("claude", "claude"), None);
        assert_eq!(title_summary("  ✳  ", "claude"), None);
    }

    #[test]
    fn prompt_first_line_is_sanitized_and_capped() {
        assert_eq!(prompt_line("\n  fix the bug\nsecond line").as_deref(), Some("fix the bug"));
        let l = prompt_line(&format!("\x1b]0;evil\x07{}", "x".repeat(10_000))).unwrap();
        assert!(!l.contains('\x1b') && !l.contains('\x07'));
        assert!(l.chars().count() <= 120 && l.ends_with('…'));
        assert_eq!(prompt_line("   \n\n"), None);
    }

    #[test]
    fn model_names_are_shortened() {
        assert_eq!(short_model("claude-opus-5-5").as_deref(), Some("opus"));
        assert_eq!(short_model("Opus 5.5").as_deref(), Some("opus"));
        assert_eq!(short_model("gpt-5-codex").as_deref(), Some("gpt-5"));
        assert_eq!(short_model("gemini-3-pro").as_deref(), Some("gemini-3"));
        assert_eq!(short_model("some-very-long-model-name").as_deref(), Some("some-very-lo"));
        assert_eq!(short_model(""), None);
    }

    #[test]
    fn time_ago_buckets() {
        use std::time::Duration as D;
        assert_eq!(time_ago(D::from_secs(2)), "now");
        assert_eq!(time_ago(D::from_secs(42)), "42s");
        assert_eq!(time_ago(D::from_secs(4 * 60 + 5)), "4m");
        assert_eq!(time_ago(D::from_secs(2 * 3600)), "2h");
        assert_eq!(time_ago(D::from_secs(3 * 86400)), "3d");
    }

    #[test]
    fn summary_prefers_title_then_prompt_then_state() {
        use crate::agents::AgentState::*;
        assert_eq!(summary(Some("T"), Some("P"), Some(Running)), "T");
        assert_eq!(summary(None, Some("P"), Some(Running)), "P");
        assert_eq!(summary(None, None, Some(Idle)), "Done");
        assert_eq!(summary(None, Some("P"), Some(Waiting)), "Needs input: P");
        assert_eq!(summary(None, None, Some(Waiting)), "Needs input");
        assert_eq!(summary(None, None, None), "Running");
    }

    #[test]
    fn short_agent_names() {
        assert_eq!(short_agent("claude"), "Claude");
        assert_eq!(short_agent("codex"), "Codex");
        assert_eq!(short_agent("gemini"), "Gemini");
        assert_eq!(short_agent("qoder"), "Qoder");
        assert_eq!(short_agent("opencode"), "OpenCode");
        assert_eq!(short_agent("cursor"), "Cursor");
        assert_eq!(short_agent("mytool"), "mytool");
    }

    #[test]
    fn tab_text_prefers_agent_then_program_then_folder() {
        use std::path::Path;
        assert_eq!(
            tab_text(Some("claude"), Some("Fix A"), Some("node"), Some(Path::new("/x/api"))),
            "Claude: Fix A"
        );
        assert_eq!(tab_text(None, None, Some("cargo"), Some(Path::new("/x/api"))), "cargo");
        assert_eq!(tab_text(None, None, None, Some(Path::new("/home/u/api"))), "api");
        assert_eq!(tab_text(None, None, None, Some(Path::new("/"))), "/");
        assert_eq!(tab_text(None, None, None, None), "Terminal");
    }
}
