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
//! | Codex CLI   | ~/.codex/config.toml (`hooks`)    | UserPromptSubmit, PermissionRequest, Stop, ...     |
//! | OpenCode    | ~/.config/opencode/plugins/*.js   | session.idle, session.status, permission.*        |
//!
//! Event names follow upstream cmux's hook catalog (CLI/CMUXCLI+AgentHookCatalog.swift).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const MARKER: &str = "cmux-linux-hook";
const CLAUDE_USAGE_MARKER: &str = " # cmux-linux-hook statusLine=";

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
        "codex" => &[
            ("UserPromptSubmit", "running"),
            ("PreModelRequest", "running"),
            ("PermissionRequest", "waiting"),
            ("PreToolUse", "waiting"),
            ("PostToolUse", "running"),
            ("Stop", "idle"),
            ("Interrupt", "ended"),
            ("SessionEnd", "ended"),
        ],
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

/// Executable name of a supported agent.
pub fn binary(agent: &str) -> &str {
    match agent {
        "qoder" => "qodercli",
        a => a,
    }
}

/// The agent's own settings file that `install` writes our hooks into.
pub fn hook_file(home: &Path, agent: &str) -> Option<PathBuf> {
    settings_path(home, agent)
}

/// Our hooks are present in the agent's settings (`agent-event` commands
/// tagged with [`MARKER`]; the Claude usage status line alone does not count).
pub fn hooks_installed(home: &Path, agent: &str) -> bool {
    settings_path(home, agent)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|t| t.contains(MARKER) && t.contains("agent-event"))
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

/// Add our hooks to a settings object. Idempotent:
/// existing cmux-linux entries are replaced, user entries are preserved.
pub fn install_nested(settings: &mut Value, agent: &str, cmux: &str) -> Result<(), String> {
    uninstall_nested(settings);
    let obj = settings.as_object_mut().ok_or("settings file is not a JSON object")?;
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or("\"hooks\" is not a JSON object")?;
    for (event, state) in nested_events(agent) {
        let groups = hooks.entry(event.to_string()).or_insert_with(|| json!([]));
        let groups = groups.as_array_mut().ok_or_else(|| format!("hooks.{event} is not an array"))?;
        let timeout = if agent == "codex" && matches!(*event, "Interrupt" | "SessionEnd") {
            3
        } else {
            10
        };
        let mut group = json!({"hooks": [{"type": "command", "command": hook_command(cmux, agent, state), "timeout": timeout}]});
        if agent == "codex" && *event == "PreToolUse" {
            // Only an explicit question means waiting; other tools are still work.
            group["matcher"] = json!("^request_user_input$");
        }
        groups.push(group);
    }
    if agent == "claude" {
        install_claude_usage(settings, cmux)?;
    }
    Ok(())
}

/// Wrap the user's status line so the same stdin and output are preserved.
/// Keep its full settings object for lossless uninstall/reinstall.
fn install_claude_usage(settings: &mut Value, cmux: &str) -> Result<(), String> {
    let original = settings.get("statusLine").cloned().unwrap_or(Value::Null);
    let mut command = format!("{} agent-usage", shell_quote(cmux));
    if !original.is_null() {
        if original["type"] != "command" {
            return Err("cannot forward a Claude statusLine whose type is not command".into());
        }
        let forward = original["command"].as_str().ok_or("statusLine.command is not a string")?;
        command.push_str(&format!(" --forward-command {}", shell_quote(forward)));
    }
    // Store restoration data in a shell comment, keeping Claude's settings
    // object within its documented schema. JSON escapes embedded newlines.
    command.push_str(CLAUDE_USAGE_MARKER);
    command.push_str(&serde_json::to_string(&original).unwrap());
    let mut status = if original.is_object() { original.clone() } else { json!({}) };
    status["type"] = json!("command");
    status["command"] = json!(command);
    settings["statusLine"] = status;
    Ok(())
}

/// Remove every hook entry whose command carries our marker.
pub fn uninstall_nested(settings: &mut Value) {
    let original = settings["statusLine"]["command"].as_str().and_then(|command| {
        if !command.contains(" agent-usage") {
            return None;
        }
        command
            .match_indices(CLAUDE_USAGE_MARKER)
            .find_map(|(index, _)| serde_json::from_str::<Value>(&command[index + CLAUDE_USAGE_MARKER.len()..]).ok())
    });
    if let Some(original) = original {
        if original.is_null() {
            settings.as_object_mut().unwrap().remove("statusLine");
        } else {
            settings["statusLine"] = original;
        }
    }
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

/// Codex lifecycle hooks use the same nested format in an inline TOML table.
/// Preserve user hooks and notify programs, migrating our legacy notify entry
/// so completion isn't reported twice.
pub fn install_codex(toml_text: &str, cmux: &str) -> Result<String, String> {
    let mut doc: toml::Table = toml::from_str(toml_text).map_err(|e| format!("config.toml: {e}"))?;
    let mut settings = json!({});
    if let Some(hooks) = doc.get("hooks") {
        settings["hooks"] = serde_json::to_value(hooks).map_err(|e| e.to_string())?;
    }
    install_nested(&mut settings, "codex", cmux)?;
    doc.insert(
        "hooks".into(),
        toml::Value::try_from(&settings["hooks"]).map_err(|e| e.to_string())?,
    );
    remove_codex_notify(&mut doc);
    toml::to_string(&doc).map_err(|e| e.to_string())
}

pub fn uninstall_codex(toml_text: &str) -> Result<String, String> {
    let mut doc: toml::Table = toml::from_str(toml_text).map_err(|e| format!("config.toml: {e}"))?;
    if let Some(hooks) = doc.get("hooks") {
        let mut settings = json!({"hooks": serde_json::to_value(hooks).map_err(|e| e.to_string())?});
        uninstall_nested(&mut settings);
        if let Some(hooks) = settings.get("hooks") {
            doc.insert("hooks".into(), toml::Value::try_from(hooks).map_err(|e| e.to_string())?);
        } else {
            doc.remove("hooks");
        }
    }
    remove_codex_notify(&mut doc);
    toml::to_string(&doc).map_err(|e| e.to_string())
}

fn remove_codex_notify(doc: &mut toml::Table) {
    if doc
        .get("notify")
        .and_then(|v| v.as_array())
        .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(MARKER)))
    {
        doc.remove("notify");
    }
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
        "codex" => s("last_assistant_message")
            .or_else(|| s("last-assistant-message"))
            .or_else(|| s("message"))
            .or_else(|| v.pointer("/tool_input/description").and_then(Value::as_str).map(str::to_string))
            .or_else(|| {
                v.pointer("/tool_input/questions/0/question")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }),
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
    fn hooks_installed_follows_install_and_uninstall() {
        let home = std::env::temp_dir().join(format!("cmux-hooks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        std::fs::create_dir_all(home.join(".config/opencode/plugins")).unwrap();
        for agent in ["gemini", "opencode"] {
            assert!(!hooks_installed(&home, agent), "{agent}");
            install(agent, &home, "/usr/bin/cmux").unwrap();
            assert!(hooks_installed(&home, agent), "{agent}");
            uninstall(agent, &home).unwrap();
            assert!(!hooks_installed(&home, agent), "{agent}");
        }
        assert_eq!(binary("qoder"), "qodercli");
        assert_eq!(binary("claude"), "claude");
        let _ = std::fs::remove_dir_all(&home);
    }

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
    fn claude_usage_preserves_status_line_and_restores_it_on_uninstall() {
        let original = json!({"statusLine": {"type": "command", "command": "printf '%s' \"$(cat)\"", "padding": 2}, "model": "custom"});
        let mut settings = original.clone();
        install_nested(&mut settings, "claude", "/my dir/cmux").unwrap();
        let installed = settings.clone();
        assert_eq!(settings["statusLine"]["padding"], 2);
        assert!(settings["statusLine"]["command"].as_str().unwrap().contains("--forward-command"));
        install_nested(&mut settings, "claude", "/my dir/cmux").unwrap();
        assert_eq!(settings, installed);
        uninstall_nested(&mut settings);
        assert_eq!(settings, original);
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
    fn codex_install_preserves_user_config_and_hooks() {
        let original = r#"
model = "custom-model"
notify = ["my-notifier"]
[features]
hooks = false
[[hooks.Stop]]
matcher = ""
[[hooks.Stop.hooks]]
type = "command"
command = "say done"
timeout = 20
"#;
        let t = install_codex(original, "/my dir/it's/cmux").unwrap();
        let doc: toml::Table = toml::from_str(&t).unwrap();
        assert_eq!(doc["model"].as_str(), Some("custom-model"));
        assert_eq!(doc["notify"].as_array().unwrap()[0].as_str(), Some("my-notifier"));
        assert_eq!(doc["features"]["hooks"].as_bool(), Some(false));
        let hooks = &doc["hooks"];
        assert_eq!(hooks["Stop"].as_array().unwrap().len(), 2);
        for (event, state) in nested_events("codex") {
            let group = hooks[event].as_array().unwrap().last().unwrap();
            let command = group["hooks"][0]["command"].as_str().unwrap();
            assert_eq!(command, hook_command("/my dir/it's/cmux", "codex", state));
            let timeout = group["hooks"][0]["timeout"].as_integer().unwrap();
            assert_eq!(timeout, if matches!(*event, "SessionEnd" | "Interrupt") { 3 } else { 10 });
        }
        assert_eq!(hooks["PreToolUse"][0]["matcher"].as_str(), Some("^request_user_input$"));
        let again = install_codex(&t, "/my dir/it's/cmux").unwrap();
        assert_eq!(again, t);
        let removed = uninstall_codex(&t).unwrap();
        assert_eq!(
            toml::from_str::<toml::Table>(&removed).unwrap(),
            toml::from_str::<toml::Table>(original).unwrap()
        );
        assert!(install_codex("hooks = false", "cmux").is_err());
        assert!(install_codex("[hooks]\nStop = false", "cmux").is_err());
        assert!(install_codex("{broken", "cmux").is_err());
    }

    #[test]
    fn codex_migrates_legacy_notify_without_duplicate_completion() {
        let legacy =
            format!("model = \"custom-model\"\nnotify = [\"cmux\", \"agent-event\", \"--marker\", \"{MARKER}\", \"--codex-json\"]\n");
        let t = install_codex(&legacy, "cmux").unwrap();
        let doc: toml::Table = toml::from_str(&t).unwrap();
        assert!(!doc.contains_key("notify"));
        assert_eq!(doc["hooks"]["Stop"].as_array().unwrap().len(), 1);
        let removed: toml::Table = toml::from_str(&uninstall_codex(&t).unwrap()).unwrap();
        assert!(!removed.contains_key("notify") && !removed.contains_key("hooks"));
        assert_eq!(removed["model"].as_str(), Some("custom-model"));
        let old_removed: toml::Table = toml::from_str(&uninstall_codex(&legacy).unwrap()).unwrap();
        assert_eq!(old_removed, removed);
        let empty = install_codex("", "cmux").unwrap();
        assert!(toml::from_str::<toml::Table>(&uninstall_codex(&empty).unwrap()).unwrap().is_empty());
    }

    #[test]
    fn codex_payloads_include_completion_approvals_and_questions() {
        for (payload, session, message) in [
            (
                json!({"thread-id": "legacy-1", "last-assistant-message": "Legacy done"}),
                "legacy-1",
                "Legacy done",
            ),
            (json!({"session_id": "sess-1", "last_assistant_message": "Done"}), "sess-1", "Done"),
            (
                json!({"session_id": "sess-1", "tool_input": {"description": "Allow a command?"}}),
                "sess-1",
                "Allow a command?",
            ),
            (
                json!({"session_id": "sess-1", "tool_input": {"questions": [{"question": "Which option?"}]}}),
                "sess-1",
                "Which option?",
            ),
        ] {
            let (s, m) = parse_hook_payload("codex", &payload);
            assert_eq!(s.as_deref(), Some(session));
            assert_eq!(m.as_deref(), Some(message));
        }
        assert_eq!(
            parse_hook_payload("codex", &json!({"tool_input": {"command": "private command"}})),
            (None, None)
        );
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
