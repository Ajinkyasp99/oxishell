//! `oxi` — command-line control for a running oxishell.

use clap::{Parser, Subcommand, ValueEnum};
use oxishell::{agents, ipc};
use serde_json::{json, Value};
use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "oxi", version, about = "Control a running oxishell from the command line")]
struct Cli {
    /// Control socket (default: $OXISHELL_SOCKET or $XDG_RUNTIME_DIR/oxishell/oxishell.sock)
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Print raw JSON results
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Check that the app is running
    Ping,
    /// Show the pane/workspace this shell runs in
    Identify,
    /// Manage workspaces
    #[command(subcommand, alias = "ws")]
    Workspace(WsCmd),
    /// Manage panes
    #[command(subcommand)]
    Pane(PaneCmd),
    /// Post a notification (defaults to the current pane)
    Notify {
        #[arg(long, short)]
        title: Option<String>,
        #[arg(long, short)]
        body: Option<String>,
        #[arg(long)]
        pane: Option<u64>,
        /// Do not associate with the current pane
        #[arg(long)]
        global: bool,
    },
    /// Notification history
    #[command(subcommand)]
    Notifications(NotifCmd),
    /// Report agent state (used by agent hooks). Silent no-op outside oxishell.
    AgentEvent {
        #[arg(long)]
        agent: String,
        #[arg(long)]
        state: Option<String>,
        #[arg(long)]
        message: Option<String>,
        #[arg(long)]
        session: Option<String>,
        /// Read the hook's JSON payload from stdin (Claude Code, Codex, Gemini, Qoder)
        #[arg(long)]
        stdin_json: bool,
        /// Treat the last argument as a Codex `notify` JSON payload
        #[arg(long)]
        codex_json: bool,
        #[arg(long, hide = true)]
        marker: Option<String>,
        payload: Option<String>,
    },
    /// Forward Claude status-line usage to the sidebar; silent outside oxishell.
    AgentUsage {
        /// Existing status-line command to run with the original stdin and output
        #[arg(long)]
        forward_command: Option<String>,
    },
    /// Install or remove agent notification hooks
    #[command(subcommand)]
    Hooks(HooksCmd),
    /// Print the shell snippet that reports the active Python/conda env for context chips
    /// (bash: automatic; zsh/fish: add the output to your shell config)
    ShellIntegration {
        /// bash, zsh or fish
        shell: String,
    },
}

#[derive(Subcommand)]
enum WsCmd {
    List,
    New {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    Select {
        id: u64,
    },
    Next,
    Prev,
    Rename {
        id: u64,
        name: String,
    },
    Move {
        id: u64,
        index: u64,
    },
    Close {
        id: u64,
        /// Close even if programs are running
        #[arg(long)]
        force: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Dir {
    Right,
    Down,
}

#[derive(Subcommand)]
enum PaneCmd {
    List {
        #[arg(long)]
        workspace: Option<u64>,
    },
    Split {
        #[arg(long)]
        pane: Option<u64>,
        #[arg(long, short, value_enum, default_value = "right")]
        direction: Dir,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    Focus {
        id: u64,
    },
    Close {
        id: u64,
        #[arg(long)]
        force: bool,
    },
    /// Type text into a pane (requires [ipc] allow_send = true)
    Send {
        #[arg(long)]
        pane: Option<u64>,
        /// Press Enter after the text
        #[arg(long)]
        enter: bool,
        text: String,
    },
    /// Print a pane's visible text and scrollback
    Read {
        #[arg(long)]
        pane: Option<u64>,
        #[arg(long, default_value_t = 200)]
        lines: u64,
    },
}

#[derive(Subcommand)]
enum NotifCmd {
    List,
    Clear,
    /// Focus the pane with the latest unread notification
    Jump,
}

#[derive(Subcommand)]
enum HooksCmd {
    /// Install hooks for an agent (claude, codex, gemini, qoder, opencode) or --all found on PATH
    Install {
        agent: Option<String>,
        #[arg(long)]
        all: bool,
    },
    Uninstall {
        agent: String,
    },
    List,
}

fn env_pane() -> Option<u64> {
    std::env::var("OXISHELL_PANE_ID").ok().and_then(|v| v.parse().ok())
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

fn agent_binary(agent: &str) -> &str {
    match agent {
        "qoder" => "qodercli",
        a => a,
    }
}

fn main() -> ExitCode {
    // Exit quietly when output is piped into e.g. `head`.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    let cli = Cli::parse();
    let socket = cli.socket.clone().unwrap_or_else(ipc::default_socket);
    let call = |m: &str, p: Value| ipc::call_at(&socket, m, p);

    let result: Result<Value, String> = match cli.cmd {
        Cmd::Ping => call("ping", json!({})),
        Cmd::Identify => Ok(json!({
            "pane": env_pane(),
            "workspace": std::env::var("OXISHELL_WORKSPACE_ID").ok().and_then(|v| v.parse::<u64>().ok()),
            "socket": socket.display().to_string(),
        })),
        Cmd::Workspace(c) => match c {
            WsCmd::List => call("workspace.list", json!({})),
            WsCmd::New { name, cwd } => {
                let cwd = cwd.map(|c| std::fs::canonicalize(&c).unwrap_or(c).display().to_string());
                call("workspace.create", json!({"name": name, "cwd": cwd}))
            }
            WsCmd::Select { id } => call("workspace.select", json!({"workspace": id})),
            WsCmd::Next => call("workspace.next", json!({})),
            WsCmd::Prev => call("workspace.prev", json!({})),
            WsCmd::Rename { id, name } => call("workspace.rename", json!({"workspace": id, "name": name})),
            WsCmd::Move { id, index } => call("workspace.move", json!({"workspace": id, "index": index})),
            WsCmd::Close { id, force } => call("workspace.close", json!({"workspace": id, "force": force})),
        },
        Cmd::Pane(c) => match c {
            PaneCmd::List { workspace } => call("pane.list", json!({"workspace": workspace})),
            PaneCmd::Split { pane, direction, cwd } => call(
                "pane.split",
                json!({"pane": pane.or_else(env_pane), "direction": match direction { Dir::Right => "right", Dir::Down => "down" }, "cwd": cwd.map(|c| c.display().to_string())}),
            ),
            PaneCmd::Focus { id } => call("pane.focus", json!({"pane": id})),
            PaneCmd::Close { id, force } => call("pane.close", json!({"pane": id, "force": force})),
            PaneCmd::Send { pane, enter, text } => {
                let text = if enter { format!("{text}\r") } else { text };
                call("pane.send", json!({"pane": pane.or_else(env_pane), "text": text}))
            }
            PaneCmd::Read { pane, lines } => call("pane.read", json!({"pane": pane.or_else(env_pane), "lines": lines})),
        },
        Cmd::Notify { title, body, pane, global } => {
            let pane = if global { None } else { pane.or_else(env_pane) };
            call(
                "notify",
                json!({"title": title.unwrap_or_else(|| "Notification".into()), "body": body.unwrap_or_default(), "pane": pane}),
            )
        }
        Cmd::Notifications(c) => match c {
            NotifCmd::List => call("notification.list", json!({})),
            NotifCmd::Clear => call("notification.clear", json!({})),
            NotifCmd::Jump => call("notification.jump", json!({})),
        },
        Cmd::AgentUsage { forward_command } => {
            let mut input = Vec::new();
            if !std::io::stdin().is_terminal() {
                let _ = std::io::stdin().take(1 << 20).read_to_end(&mut input);
            }
            if let Some(pane) = env_pane() {
                if let Ok(payload) = serde_json::from_slice::<Value>(&input) {
                    // Only quota fields leave this process, never transcript/context data.
                    let _ = ipc::call_at_with_timeout(
                        &socket,
                        "agent.usage",
                        json!({"pane": pane, "rate_limits": payload.get("rate_limits")}),
                        std::time::Duration::from_millis(250),
                    );
                }
            }
            if let Some(command) = forward_command {
                use std::io::Write;
                use std::process::{Command, Stdio};
                if let Ok(mut child) = Command::new("/bin/sh").args(["-c", &command]).stdin(Stdio::piped()).spawn() {
                    if let Some(mut stdin) = child.stdin.take() {
                        let _ = stdin.write_all(&input);
                    }
                    return match child.wait() {
                        Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8),
                        Err(_) => ExitCode::FAILURE,
                    };
                }
                return ExitCode::FAILURE;
            }
            return ExitCode::SUCCESS;
        }
        Cmd::AgentEvent {
            agent,
            state,
            message,
            session,
            stdin_json,
            codex_json,
            marker: _,
            payload,
        } => {
            // Hooks must never break the agent: outside oxishell, or on any error, exit 0.
            let Some(pane) = env_pane() else { return ExitCode::SUCCESS };
            let mut state = state;
            let (mut session, mut message) = (session, message);
            let mut payload_v = None;
            if stdin_json && !std::io::stdin().is_terminal() {
                let mut buf = String::new();
                let _ = std::io::stdin().take(1 << 20).read_to_string(&mut buf);
                payload_v = serde_json::from_str::<Value>(&buf).ok();
            }
            if codex_json {
                payload_v = payload.as_deref().and_then(|p| serde_json::from_str(p).ok());
                match payload_v.as_ref().and_then(|v| v.get("type")).and_then(Value::as_str) {
                    Some("agent-turn-complete") => state = Some("idle".into()),
                    _ => return ExitCode::SUCCESS,
                }
            }
            if let Some(v) = &payload_v {
                let (s, m) = agents::parse_hook_payload(&agent, v);
                session = session.or(s);
                message = message.or(m.map(|m| m.chars().take(300).collect()));
            }
            let Some(state) = state else { return ExitCode::SUCCESS };
            let r = call(
                "agent.event",
                json!({"pane": pane, "agent": agent, "state": state, "message": message, "session": session}),
            );
            if let Err(e) = r {
                if std::env::var_os("OXISHELL_DEBUG").is_some() {
                    eprintln!("oxi agent-event: {e}");
                }
            }
            return ExitCode::SUCCESS;
        }
        Cmd::ShellIntegration { shell } => match oxishell::shellint::snippet(&shell) {
            Some(s) => {
                print!("{s}");
                return ExitCode::SUCCESS;
            }
            None => Err(format!("unsupported shell {shell:?}; use bash, zsh or fish")),
        },
        Cmd::Hooks(c) => {
            let home = oxishell::paths::home();
            let me = std::env::current_exe()
                .and_then(std::fs::canonicalize)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "oxi".into());
            match c {
                HooksCmd::List => Ok(json!(agents::SUPPORTED
                    .iter()
                    .map(|a| json!({"agent": a, "on_path": on_path(agent_binary(a))}))
                    .collect::<Vec<_>>())),
                HooksCmd::Install { agent, all } => {
                    let targets: Vec<String> = if all {
                        agents::SUPPORTED
                            .iter()
                            .filter(|a| on_path(agent_binary(a)))
                            .map(|a| a.to_string())
                            .collect()
                    } else {
                        match agent {
                            Some(a) => vec![a],
                            None => return fail("name an agent or pass --all"),
                        }
                    };
                    if targets.is_empty() {
                        Ok(json!({"installed": [], "note": "no supported agent found on PATH"}))
                    } else {
                        let mut out = vec![];
                        for a in targets {
                            match agents::install(&a, &home, &me) {
                                Ok(p) => {
                                    let mut installed = json!({"agent": a, "file": p.display().to_string()});
                                    if a == "codex" {
                                        installed["note"] = json!("Requires Codex lifecycle hooks. Restart Codex, then open /hooks to review and trust the oxi hooks. Existing features.hooks = false settings are preserved.");
                                    }
                                    out.push(installed);
                                }
                                Err(e) => return fail(&format!("{a}: {e}")),
                            }
                        }
                        Ok(json!({"installed": out}))
                    }
                }
                HooksCmd::Uninstall { agent } => {
                    agents::uninstall(&agent, &home).map(|p| json!({"agent": agent, "file": p.display().to_string()}))
                }
            }
        }
    };

    match result {
        Ok(v) => {
            print_result(&v, cli.json);
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("oxi: {msg}");
    ExitCode::FAILURE
}

fn print_result(v: &Value, json_out: bool) {
    if json_out {
        println!("{}", serde_json::to_string_pretty(v).unwrap());
        return;
    }
    match v {
        Value::Array(items) if items.first().is_some_and(|i| i.get("name").is_some() && i.get("panes").is_some()) => {
            for w in items {
                println!(
                    "{} {:>3}  {:<24} panes={:<3} {}{}",
                    if w["active"] == json!(true) { "*" } else { " " },
                    w["id"],
                    w["name"].as_str().unwrap_or(""),
                    w["panes"].as_array().map(|a| a.len()).unwrap_or(0),
                    w["branch"].as_str().map(|b| format!("[{b}] ")).unwrap_or_default(),
                    w["cwd"].as_str().unwrap_or("")
                );
            }
        }
        Value::Array(items) if items.first().is_some_and(|i| i.get("focused").is_some()) => {
            for p in items {
                println!(
                    "{} {:>3}  ws={:<3} {}  {}",
                    if p["focused"] == json!(true) { "*" } else { " " },
                    p["id"],
                    p["workspace"],
                    p["cwd"].as_str().unwrap_or(""),
                    p["agent"]["agent"]
                        .as_str()
                        .map(|a| format!("[{a}: {}]", p["agent"]["state"].as_str().unwrap_or("")))
                        .unwrap_or_default()
                );
            }
        }
        Value::Object(o) if o.contains_key("text") && o.len() == 2 => println!("{}", o["text"].as_str().unwrap_or("")),
        Value::Null => {}
        other => println!("{}", serde_json::to_string_pretty(other).unwrap()),
    }
}
