//! Coding-agent hook integration.
//!
//! Hooks run `cmux agent-event ...` inside panes started by cmux-linux
//! (`CMUX_PANE_ID` set). Outside cmux-linux the command exits 0 silently, so
//! installed hooks never break an agent. We only report states the agent's
//! own hook system emits:
//!
//! | Agent       | File                              | Events used                                       |
//! |-------------|-----------------------------------|---------------------------------------------------|
//! | Claude Code | ~/.claude/settings.json           | UserPromptSubmit, Notification, Stop, SessionEnd |
//! | Gemini CLI  | ~/.gemini/settings.json           | BeforeAgent, AfterAgent, SessionEnd               |
//! | Qoder CLI   | ~/.qoder/settings.json            | Stop, SessionEnd                                  |
//! | Codex CLI   | ~/.codex/config.toml (`notify`)   | agent-turn-complete                               |
//! | OpenCode    | ~/.config/opencode/plugins/*.js   | session.idle, session.status, permission.*        |
//!
//! Event names follow upstream cmux's hook catalog (CLI/CMUXCLI+AgentHookCatalog.swift).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const MARKER: &str = "cmux-linux-hook";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    /// Working on a prompt.
    Running,
    /// Explicitly asked the user for input/permission.
    Waiting,
    /// Finished its turn.
    Idle,
    /// Session ended.
    Ended,
}

impl AgentState {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "running" => Self::Running,
            "waiting" => Self::Waiting,
            "idle" | "done" => Self::Idle,
            "ended" => Self::Ended,
            _ => return None,
        })
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Waiting => "Needs input",
            Self::Idle => "Done",
            Self::Ended => "Ended",
        }
    }
    /// States that should raise a notification.
    pub fn notifies(&self) -> bool {
        matches!(self, Self::Waiting | Self::Idle)
    }
}

pub const SUPPORTED: &[&str] = &["claude", "gemini", "qoder", "codex", "opencode"];

fn display(agent: &str) -> &str {
    match agent {
        "claude" => "Claude Code",
        "gemini" => "Gemini CLI",
        "qoder" => "Qoder CLI",
        "codex" => "Codex",
        "opencode" => "OpenCode",
        other => other,
    }
}

pub fn display_name(agent: &str) -> String {
    display(agent).to_string()
}

/// (agent event, state) pairs for JSON-settings agents ("nested" hook format).
fn nested_events(agent: &str) -> &'static [(&'static str, &'static str)] {
    match agent {
        "claude" => &[
            ("UserPromptSubmit", "running"),
            ("Notification", "waiting"),
            ("Stop", "idle"),
            ("SessionEnd", "ended"),
        ],
        "gemini" => &[("BeforeAgent", "running"), ("AfterAgent", "idle"), ("SessionEnd", "ended")],
        "qoder" => &[("Stop", "idle"), ("SessionEnd", "ended")],
        _ => &[],
    }
}

fn settings_path(home: &Path, agent: &str) -> Option<PathBuf> {
    Some(match agent {
        "claude" => std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".claude"))
            .join("settings.json"),
        "gemini" => home.join(".gemini/settings.json"),
        "qoder" => std::env::var_os("QODER_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".qoder"))
            .join("settings.json"),
        "codex" => std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"))
            .join("config.toml"),
        "opencode" => std::env::var_os("OPENCODE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config/opencode"))
            .join("plugins/cmux-linux.js"),
        _ => return None,
    })
}

fn hook_command(cmux: &str, agent: &str, state: &str) -> String {
    format!(
        "{} agent-event --agent {agent} --state {state} --stdin-json # {MARKER}",
        shell_quote(cmux)
    )
}

pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Add our hooks to a Claude/Gemini/Qoder settings object. Idempotent:
/// existing cmux-linux entries are replaced, user entries are preserved.
pub fn install_nested(settings: &mut Value, agent: &str, cmux: &str) -> Result<(), String> {
    uninstall_nested(settings);
    let obj = settings.as_object_mut().ok_or("settings file is not a JSON object")?;
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or("\"hooks\" is not a JSON object")?;
    for (event, state) in nested_events(agent) {
        let groups = hooks.entry(event.to_string()).or_insert_with(|| json!([]));
        let groups = groups.as_array_mut().ok_or_else(|| format!("hooks.{event} is not an array"))?;
        groups.push(json!({"hooks": [{"type": "command", "command": hook_command(cmux, agent, state), "timeout": 10}]}));
    }
    Ok(())
}

/// Remove every hook entry whose command carries our marker.
pub fn uninstall_nested(settings: &mut Value) {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };
    for groups in hooks.values_mut() {
        if let Some(groups) = groups.as_array_mut() {
            for g in groups.iter_mut() {
                if let Some(list) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                    list.retain(|h| !h.get("command").and_then(Value::as_str).is_some_and(|c| c.contains(MARKER)));
                }
            }
            groups.retain(|g| g.get("hooks").and_then(Value::as_array).is_none_or(|l| !l.is_empty()));
        }
    }
    hooks.retain(|_, v| v.as_array().is_none_or(|a| !a.is_empty()));
    if hooks.is_empty() {
        settings.as_object_mut().unwrap().remove("hooks");
    }
}

/// Codex: set top-level `notify` in config.toml. Refuses to overwrite a
/// user-defined notify program.
pub fn install_codex(toml_text: &str, cmux: &str) -> Result<String, String> {
    let mut doc: toml::Table = toml::from_str(toml_text).map_err(|e| format!("config.toml: {e}"))?;
    if let Some(existing) = doc.get("notify") {
        let ours = existing.as_array().is_some_and(|a| a.iter().any(|v| v.as_str() == Some(MARKER)));
        if !ours {
            return Err(
                "config.toml already has a `notify` program; remove it first or call `cmux agent-event --agent codex --codex-json` from it"
                    .into(),
            );
        }
    }
    doc.insert(
        "notify".into(),
        toml::Value::Array(
            [cmux, "agent-event", "--agent", "codex", "--marker", MARKER, "--codex-json"]
                .iter()
                .map(|s| toml::Value::String(s.to_string()))
                .collect(),
        ),
    );
    toml::to_string(&doc).map_err(|e| e.to_string())
}

pub fn uninstall_codex(toml_text: &str) -> Result<String, String> {
    let mut doc: toml::Table = toml::from_str(toml_text).map_err(|e| format!("config.toml: {e}"))?;
    if doc
        .get("notify")
        .and_then(|v| v.as_array())
        .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(MARKER)))
    {
        doc.remove("notify");
    }
    toml::to_string(&doc).map_err(|e| e.to_string())
}

pub fn opencode_plugin(cmux: &str) -> String {
    let cmux_js = serde_json::to_string(cmux).unwrap();
    format!(
        r#"// {MARKER}: reports OpenCode session state to cmux-linux. Managed by `cmux hooks`.
export const CmuxLinux = async ({{ $ }}) => {{
  if (!process.env.CMUX_PANE_ID) return {{}};
  const send = async (state, message) => {{
    try {{ await $`${{{cmux_js}}} agent-event --agent opencode --state ${{state}} --message ${{message || ""}}`.quiet(); }} catch {{}}
  }};
  return {{
    event: async ({{ event }}) => {{
      const t = event?.type;
      const p = event?.properties || event?.data || {{}};
      if (t === "session.idle") await send("idle", "");
      else if (t === "session.status" && p?.status?.type === "idle") await send("idle", "");
      else if (t === "session.status" && p?.status?.type === "busy") await send("running", "");
      else if (t === "permission.updated" || t === "permission.asked") await send("waiting", p?.title || "Permission requested");
    }},
  }};
}};
"#
    )
}

/// Install hooks for `agent`. Returns the file written.
pub fn install(agent: &str, home: &Path, cmux: &str) -> Result<PathBuf, String> {
    let path = settings_path(home, agent).ok_or_else(|| format!("unsupported agent {agent:?}; supported: {}", SUPPORTED.join(", ")))?;
    let existing = match std::fs::read_to_string(&path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let text = match agent {
        "codex" => install_codex(existing.as_deref().unwrap_or(""), cmux)?,
        "opencode" => opencode_plugin(cmux),
        _ => {
            let mut v: Value = match &existing {
                Some(t) if !t.trim().is_empty() => serde_json::from_str(t).map_err(|e| format!("{}: {e}", path.display()))?,
                _ => json!({}),
            };
            install_nested(&mut v, agent, cmux)?;
            serde_json::to_string_pretty(&v).unwrap() + "\n"
        }
    };
    if let Some(old) = &existing {
        // Keep one backup of the user's original file.
        let bak = path.with_extension(format!("{}.cmux-linux.bak", path.extension().unwrap_or_default().to_string_lossy()));
        if !bak.exists() {
            std::fs::write(&bak, old).map_err(|e| e.to_string())?;
        }
    }
    crate::paths::write_atomic(&path, text.as_bytes(), 0o600).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

pub fn uninstall(agent: &str, home: &Path) -> Result<PathBuf, String> {
    let path = settings_path(home, agent).ok_or_else(|| format!("unsupported agent {agent:?}"))?;
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(path),
        Err(e) => return Err(e.to_string()),
    };
    match agent {
        "opencode" => {
            if text.contains(MARKER) {
                std::fs::remove_file(&path).map_err(|e| e.to_string())?;
            }
        }
        "codex" => crate::paths::write_atomic(&path, uninstall_codex(&text)?.as_bytes(), 0o600).map_err(|e| e.to_string())?,
        _ => {
            let mut v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            uninstall_nested(&mut v);
            crate::paths::write_atomic(&path, (serde_json::to_string_pretty(&v).unwrap() + "\n").as_bytes(), 0o600)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(path)
}

/// Extract a session id and a human message from a hook's stdin payload.
pub fn parse_hook_payload(agent: &str, v: &Value) -> (Option<String>, Option<String>) {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let session = s("session_id").or_else(|| s("sessionId")).or_else(|| s("thread-id"));
    let message = match agent {
        "codex" => s("last-assistant-message"),
        _ => s("message"),
    };
    (session, message)
}

/// Command that resumes a session; only ever typed into a pane on explicit user action.
pub fn resume_command(agent: &str, session: &str) -> Option<String> {
    if session.is_empty() || !session.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None; // never build commands from unexpected characters
    }
    Some(match agent {
        "claude" => format!("claude --resume {session}"),
        "codex" => format!("codex resume {session}"),
        "gemini" => format!("gemini --resume {session}"),
        "qoder" => format!("qodercli --resume {session}"),
        "opencode" => format!("opencode --session {session}"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_install_is_idempotent_and_preserves_user_hooks() {
        let mut v = json!({"model": "x", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "say done"}]}]}});
        install_nested(&mut v, "claude", "/usr/bin/cmux").unwrap();
        install_nested(&mut v, "claude", "/usr/bin/cmux").unwrap();
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "user hook kept, ours added once: {v}");
        assert!(v["hooks"]["Notification"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("--state waiting"));
        uninstall_nested(&mut v);
        assert_eq!(
            v,
            json!({"model": "x", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "say done"}]}]}})
        );
    }

    #[test]
    fn gemini_and_qoder_events() {
        let mut v = json!({});
        install_nested(&mut v, "gemini", "cmux").unwrap();
        assert!(v["hooks"]["AfterAgent"].is_array() && v["hooks"]["BeforeAgent"].is_array());
        let mut q = json!({});
        install_nested(&mut q, "qoder", "cmux").unwrap();
        assert!(q["hooks"]["Stop"].is_array());
        uninstall_nested(&mut q);
        assert_eq!(q, json!({}));
    }

    #[test]
    fn codex_notify_install_respects_user_program() {
        let t = install_codex("model = \"o3\"\n", "/usr/bin/cmux").unwrap();
        assert!(t.contains("notify") && t.contains("model"));
        let again = install_codex(&t, "/usr/bin/cmux").unwrap();
        assert_eq!(again, t);
        assert!(install_codex("notify = [\"my-notifier\"]\n", "cmux").is_err());
        let removed = uninstall_codex(&t).unwrap();
        assert!(!removed.contains("notify") && removed.contains("model"));
    }

    #[test]
    fn install_and_uninstall_files() {
        let home = std::env::temp_dir().join(format!("cmux-agents-{}", std::process::id()));
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        std::fs::write(home.join(".gemini/settings.json"), "{\"theme\":\"x\"}").unwrap();
        let p = install("gemini", &home, "cmux").unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["theme"], "x");
        assert!(home.join(".gemini/settings.json.cmux-linux.bak").exists());
        uninstall("gemini", &home).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v, json!({"theme": "x"}));
        let op = install("opencode", &home, "cmux").unwrap();
        assert!(std::fs::read_to_string(&op).unwrap().contains("session.idle"));
        uninstall("opencode", &home).unwrap();
        assert!(!op.exists());
        // Corrupt settings are not overwritten.
        std::fs::write(home.join(".gemini/settings.json"), "{broken").unwrap();
        assert!(install("gemini", &home, "cmux").is_err());
        assert!(install("nope", &home, "cmux").is_err());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn payload_and_resume() {
        let (s, m) = parse_hook_payload("claude", &json!({"session_id": "abc-1", "message": "Claude needs your permission"}));
        assert_eq!(s.as_deref(), Some("abc-1"));
        assert_eq!(m.as_deref(), Some("Claude needs your permission"));
        assert_eq!(resume_command("claude", "abc-1").unwrap(), "claude --resume abc-1");
        assert_eq!(resume_command("claude", "x; rm -rf ~"), None);
        assert_eq!(resume_command("unknown", "abc"), None);
        assert_eq!(shell_quote("/usr/bin/cmux"), "/usr/bin/cmux");
        assert_eq!(shell_quote("/my dir/it's"), "'/my dir/it'\\''s'");
    }
}
