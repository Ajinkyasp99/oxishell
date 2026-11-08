//! End-to-end tests against the real GTK app, real PTYs and real shells.
//!
//! Needs a display. Run with:  xvfb-run -a cargo test --test e2e -- --nocapture
//! Without DISPLAY/WAYLAND_DISPLAY the tests are skipped (and say so).

use oxishell::ipc::call_at;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Env {
    root: PathBuf,
    socket: PathBuf,
    /// App processes to kill if a test panics before stopping them.
    apps: std::cell::RefCell<Vec<u32>>,
}

impl Env {
    fn new(name: &str) -> Env {
        // Keep paths short: Unix socket paths are limited to 108 bytes.
        let root = PathBuf::from(format!("/tmp/oxishell-e2e-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["config/oxishell", "state", "run", "home"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::set_permissions(root.join("run"), std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = root.join("run/oxishell/oxishell.sock");
        let e = Env {
            root,
            socket,
            apps: Default::default(),
        };
        e.write_config("allow_send = true");
        e
    }

    fn write_config(&self, ipc_extra: &str) {
        std::fs::write(
            self.root.join("config/oxishell/config.toml"),
            format!(
                "[ipc]\n{ipc_extra}\n[notifications]\ndesktop = false\n[terminal]\nshell = \"/bin/bash\"\n[session]\nreplay_lines = 200\n"
            ),
        )
        .unwrap();
    }

    fn start(&self) -> Child {
        let child = Command::new(env!("CARGO_BIN_EXE_oxishell"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("HOME", self.root.join("home"))
            .env("OXISHELL_NON_UNIQUE", "1")
            .env("GSETTINGS_BACKEND", "memory")
            .env("PS1", "$ ")
            .env_remove("OXISHELL_SOCKET")
            .env_remove("OXISHELL_PANE_ID")
            .current_dir(self.root.join("home"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        self.apps.borrow_mut().push(child.id());
        let t = Instant::now();
        while call_at(&self.socket, "ping", json!({})).is_err() {
            // Generous: the first GTK start in a fresh D-Bus session may wait on portal activation.
            assert!(t.elapsed() < Duration::from_secs(40), "app did not start");
            std::thread::sleep(Duration::from_millis(100));
        }
        child
    }

    fn call(&self, m: &str, p: Value) -> Result<Value, String> {
        call_at(&self.socket, m, p)
    }

    fn ok(&self, m: &str, p: Value) -> Value {
        self.call(m, p).unwrap_or_else(|e| panic!("{m} failed: {e}"))
    }

    fn cli(&self, args: &[&str], pane: Option<u64>) -> (bool, String) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_oxi"));
        c.args(args).env("OXISHELL_SOCKET", &self.socket).env_remove("OXISHELL_PANE_ID");
        if let Some(p) = pane {
            c.env("OXISHELL_PANE_ID", p.to_string());
        }
        let o = c.output().unwrap();
        (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr),
        )
    }

    fn read(&self, pane: u64) -> String {
        self.ok("pane.read", json!({"pane": pane, "lines": 500}))["text"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Wait until the pane's text contains `needle`.
    fn wait_text(&self, pane: u64, needle: &str) -> String {
        self.wait_text_for(pane, needle, 10)
    }

    fn wait_text_for(&self, pane: u64, needle: &str, secs: u64) -> String {
        let t = Instant::now();
        loop {
            let text = self.read(pane);
            if text.contains(needle) {
                return text;
            }
            assert!(
                t.elapsed() < Duration::from_secs(secs),
                "pane {pane} never showed {needle:?}; text:\n{text}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn panes(&self) -> Vec<Value> {
        self.ok("pane.list", json!({})).as_array().unwrap().clone()
    }

    fn pid_of(&self, pane: u64) -> i32 {
        let t = Instant::now();
        loop {
            if let Some(pid) = self.panes().iter().find(|p| p["id"] == pane).and_then(|p| p["pid"].as_i64()) {
                return pid as i32;
            }
            assert!(t.elapsed() < Duration::from_secs(10), "pane {pane} has no pid");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn stop(&self, mut child: Child) {
        unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        let t = Instant::now();
        while child.try_wait().unwrap().is_none() {
            assert!(t.elapsed() < Duration::from_secs(10), "app ignored SIGTERM");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for pid in self.apps.borrow().iter() {
            unsafe { libc::kill(*pid as i32, libc::SIGKILL) };
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn alive(pid: i32) -> bool {
    // Zombies count as dead.
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .is_ok_and(|s| !s.rsplit_once(')').is_some_and(|(_, r)| r.trim_start().starts_with('Z')))
}

fn wait_dead(pid: i32) -> bool {
    let t = Instant::now();
    while alive(pid) {
        if t.elapsed() > Duration::from_secs(6) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

fn have_display() -> bool {
    let ok = std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
    if !ok {
        eprintln!("SKIPPED: no display; run under `xvfb-run -a cargo test --test e2e`");
    }
    ok
}

fn first_pane(e: &Env) -> u64 {
    e.panes()[0]["id"].as_u64().unwrap()
}

#[test]
fn terminal_workspaces_splits_notifications_and_restore() {
    if !have_display() {
        return;
    }
    let e = Env::new("main");
    let app = e.start();

    // Socket is private.
    let mode = std::fs::metadata(&e.socket).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let dmode = std::fs::metadata(e.socket.parent().unwrap()).unwrap().permissions().mode() & 0o777;
    assert_eq!(dmode, 0o700);

    // --- Phase 2: a real shell; input sent before the PTY exists is not lost.
    let p1 = first_pane(&e);
    e.ok(
        "pane.send",
        json!({"pane": p1, "text": "echo RESULT=$((6*7)) PANE=$OXISHELL_PANE_ID\r"}),
    );
    e.wait_text(p1, &format!("RESULT=42 PANE={p1}"));
    e.ok(
        "pane.send",
        json!({"pane": p1, "text": "printf '\\033[31mred\\033[0m \\u00e9\\u4e2d\\n'\r"}),
    );
    let t = e.wait_text(p1, "red é中");
    assert!(!t.contains('\u{1b}'), "read text must be plain");
    // Tab completion through the real readline.
    e.ok("pane.send", json!({"pane": p1, "text": "ech\t COMPLETED\r"}));
    e.wait_text(p1, "\nCOMPLETED");

    // --- Phase 4: splits with independent shells.
    let p2 = e.ok("pane.split", json!({"pane": p1, "direction": "right"}))["pane"]
        .as_u64()
        .unwrap();
    let p3 = e.ok("pane.split", json!({"pane": p2, "direction": "down"}))["pane"]
        .as_u64()
        .unwrap();
    let (pid1, pid2, pid3) = (e.pid_of(p1), e.pid_of(p2), e.pid_of(p3));
    assert!(pid1 != pid2 && pid2 != pid3);
    let ws = e.ok("workspace.list", json!({}));
    assert_eq!(ws[0]["panes"], json!([p1, p2, p3]));
    assert_eq!(ws[0]["layout"]["type"], "split");
    e.ok("pane.send", json!({"pane": p3, "text": "echo in-p3\r"}));
    e.wait_text(p3, "\nin-p3");
    assert!(!e.read(p2).contains("in-p3"));
    assert!(e.call("pane.split", json!({"pane": p1, "direction": "diagonal"})).is_err());

    // --- Phase 2.9 / 4.7: running programs block close without force; force cleans up.
    e.ok("pane.send", json!({"pane": p3, "text": "sleep 300\r"}));
    std::thread::sleep(Duration::from_millis(500));
    let err = e.call("pane.close", json!({"pane": p3})).unwrap_err();
    assert!(err.contains("running"), "{err}");
    e.ok("pane.close", json!({"pane": p3, "force": true}));
    assert!(wait_dead(pid3), "shell of closed pane still alive");
    // Shell exiting on its own closes its pane.
    e.ok("pane.send", json!({"pane": p2, "text": "exit\r"}));
    let t = Instant::now();
    while e.panes().iter().any(|p| p["id"] == p2) {
        assert!(t.elapsed() < Duration::from_secs(5), "pane did not close after exit");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(wait_dead(pid2));

    // --- Phase 3: workspaces stay alive in the background.
    let w1 = ws[0]["id"].as_u64().unwrap();
    let created = e.ok("workspace.create", json!({"name": "Frontend", "cwd": "/tmp"}));
    let (w2, p4) = (created["workspace"].as_u64().unwrap(), created["pane"].as_u64().unwrap());
    // Regression: right after switching workspaces, a notification for a pane in the
    // now-hidden workspace must stay unread (stale GTK focus events used to mark it read).
    e.ok("notify", json!({"pane": p1, "title": "background", "body": ""}));
    std::thread::sleep(Duration::from_millis(800));
    assert!(e.panes().into_iter().find(|p| p["id"] == p1).unwrap()["unread"].as_u64().unwrap() >= 1);
    e.ok("pane.send", json!({"pane": p1, "text": "echo background-ok\r"}));
    e.wait_text(p1, "\nbackground-ok");
    e.ok("pane.send", json!({"pane": p4, "text": "pwd\r"}));
    e.wait_text(p4, "\n/tmp");
    e.ok("workspace.rename", json!({"workspace": w1, "name": "Backend"}));
    assert!(e.call("workspace.rename", json!({"workspace": w1, "name": "   "})).is_err());
    e.ok("workspace.move", json!({"workspace": w2, "index": 0}));
    let ws = e.ok("workspace.list", json!({}));
    assert_eq!(ws[0]["name"], "Frontend");
    assert_eq!(ws[1]["name"], "Backend");
    e.ok("workspace.select", json!({"workspace": w1}));
    assert_eq!(e.ok("workspace.list", json!({}))[1]["active"], true);
    let w3 = e.ok("workspace.create", json!({"name": "Temp"}))["workspace"].as_u64().unwrap();
    e.ok("workspace.close", json!({"workspace": w3}));
    assert_eq!(e.ok("workspace.list", json!({})).as_array().unwrap().len(), 2);

    // --- Phases 7/8: notifications and agent events via the real CLI.
    let (ok, out) = e.cli(&["notify", "-t", "Build done", "-b", "all green"], Some(p4));
    assert!(ok, "{out}");
    let mut sh = Command::new(env!("CARGO_BIN_EXE_oxi"))
        .args(["agent-event", "--agent", "claude", "--state", "waiting", "--stdin-json"])
        .env("OXISHELL_SOCKET", &e.socket)
        .env("OXISHELL_PANE_ID", p4.to_string())
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    sh.stdin
        .take()
        .unwrap()
        .write_all(br#"{"session_id":"sess-42","message":"Claude needs your permission"}"#)
        .unwrap();
    assert!(sh.wait().unwrap().success());
    let (ok, _) = e.cli(
        &[
            "agent-event",
            "--agent",
            "codex",
            "--codex-json",
            r#"{"type":"agent-turn-complete","last-assistant-message":"done"}"#,
        ],
        Some(p1),
    );
    assert!(ok);
    // Outside oxishell the hook is a silent success.
    let (ok, out) = e.cli(&["agent-event", "--agent", "claude", "--state", "idle"], None);
    assert!(ok && out.is_empty());
    let notes = e.ok("notification.list", json!({}));
    let titles: Vec<&str> = notes.as_array().unwrap().iter().map(|n| n["title"].as_str().unwrap()).collect();
    assert!(
        titles.contains(&"Build done") && titles.contains(&"Claude Code needs input") && titles.contains(&"Codex finished"),
        "{titles:?}"
    );
    let p4info = e.panes().into_iter().find(|p| p["id"] == p4).unwrap();
    assert_eq!(p4info["agent"]["state"], "waiting");
    // p4 is not focused (Backend is active), so it has unread items.
    assert!(p4info["unread"].as_u64().unwrap() >= 1);
    // Jump goes to the newest unread notification's pane and marks it read.
    // (Whether the focused pane's own notification is unread depends on window focus.)
    let expected = notes
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|n| n["read"] == false && n["pane"].is_u64())
        .unwrap()["pane"]
        .clone();
    assert_eq!(e.ok("notification.jump", json!({}))["pane"], expected);
    assert_eq!(e.panes().into_iter().find(|p| p["id"] == expected).unwrap()["unread"], 0);

    // --- Phase 6: metadata (port of a listening process in the pane).
    e.ok(
        "pane.send",
        json!({"pane": p4, "text": "python3 -m http.server 38419 --bind 127.0.0.1\r"}),
    );
    let t = Instant::now();
    loop {
        let ports = e.panes().into_iter().find(|p| p["id"] == p4).unwrap()["ports"].clone();
        if ports.as_array().is_some_and(|a| a.contains(&json!(38419))) {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(15), "port never detected: {ports}");
        std::thread::sleep(Duration::from_millis(250));
    }
    e.ok("pane.send", json!({"pane": p4, "text": "\u{3}"})); // Ctrl+C

    // --- Phase 10: SIGTERM saves; restart restores layout, names, scrollback, agent session.
    e.ok("pane.send", json!({"pane": p1, "text": "echo SAVED-MARKER\r"}));
    e.wait_text(p1, "\nSAVED-MARKER");
    let p1_pid = e.pid_of(p1);
    e.stop(app);
    assert!(wait_dead(p1_pid), "shell survived app exit");
    assert!(!e.socket.exists(), "socket left behind");
    let app = e.start();
    let ws = e.ok("workspace.list", json!({}));
    let names: Vec<&str> = ws.as_array().unwrap().iter().map(|w| w["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["Frontend", "Backend"]);
    let text = e.wait_text(p1, "restored output above");
    assert!(text.contains("SAVED-MARKER"), "{text}");
    // The restored text is display-only: the old command did not re-run.
    assert_eq!(text.matches("SAVED-MARKER").count(), 2, "echo line + output, once each:\n{text}");
    e.ok("pane.send", json!({"pane": p4, "text": "pwd\r"}));
    e.wait_text(p4, "\n/tmp");

    // --- Phase 9.8: send can be disabled.
    e.stop(app);
    e.write_config("allow_send = false");
    let app = e.start();
    let err = e.call("pane.send", json!({"pane": p1, "text": "x"})).unwrap_err();
    assert!(err.contains("disabled"), "{err}");
    e.stop(app);

    // --- Phase 10.10: corrupt session file -> fresh start, file set aside.
    let sess = e.root.join("state/oxishell/session.json");
    std::fs::write(&sess, "{\"version\":1,\"state\":{\"worksp").unwrap();
    let app = e.start();
    assert_eq!(e.ok("workspace.list", json!({})).as_array().unwrap().len(), 1);
    assert!(e.root.join("state/oxishell/session.json.corrupt").exists());
    e.stop(app);
}

#[test]
fn codex_hooks_install_lifecycle_and_uninstall() {
    if !have_display() {
        return;
    }
    let e = Env::new("codex");
    let codex_dir = e.root.join("home/custom-codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let config = codex_dir.join("config.toml");
    let original = "model = \"custom-model\"\nnotify = [\"user-notifier\"]\n";
    std::fs::write(&config, original).unwrap();
    let manage = |action: &str| {
        Command::new(env!("CARGO_BIN_EXE_oxi"))
            .args(["hooks", action, "codex"])
            .env("CODEX_HOME", &codex_dir)
            .output()
            .unwrap()
    };
    let installed = manage("install");
    assert!(installed.status.success(), "{:?}", installed);
    let report: Value = serde_json::from_slice(&installed.stdout).unwrap();
    assert!(report["installed"][0]["note"].as_str().unwrap().contains("/hooks"));
    assert_eq!(report["installed"][0]["file"], config.to_string_lossy().as_ref());
    assert_eq!(
        std::fs::read_to_string(config.with_extension("toml.oxishell.bak")).unwrap(),
        original
    );
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(manage("install").status.success());
    assert_eq!(std::fs::read_to_string(&config).unwrap(), text);
    let doc: toml::Table = toml::from_str(&text).unwrap();

    let app = e.start();
    let pane = first_pane(&e);
    let send = |event: &str, extra: Value, inside: bool| {
        let command = doc["hooks"][event][0]["hooks"][0]["command"].as_str().unwrap();
        let mut payload = json!({"session_id": "codex-session-42", "hook_event_name": event});
        payload.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", command])
            .env("OXISHELL_SOCKET", &e.socket)
            .env_remove("OXISHELL_PANE_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if inside {
            cmd.env("OXISHELL_PANE_ID", pane.to_string());
        }
        let mut child = cmd.spawn().unwrap();
        use std::io::Write;
        // Outside oxishell the hook can exit before reading stdin.
        let _ = child.stdin.take().unwrap().write_all(payload.to_string().as_bytes());
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{out:?}");
        assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
    };
    let assert_state = |state: &str| {
        let info = e.panes().into_iter().find(|p| p["id"] == pane).unwrap();
        assert_eq!(info["agent"]["agent"], "codex");
        assert_eq!(info["agent"]["state"], state);
    };

    send("UserPromptSubmit", json!({}), true);
    assert_state("running");
    assert!(e.ok("notification.list", json!({})).as_array().unwrap().is_empty());
    send(
        "PermissionRequest",
        json!({"tool_name": "Bash", "tool_input": {"description": "Allow this command?"}}),
        true,
    );
    assert_state("waiting");
    let notes = e.ok("notification.list", json!({}));
    assert_eq!(notes[0]["title"], "Codex needs input");
    assert_eq!(notes[0]["body"], "Allow this command?");
    send("PostToolUse", json!({"tool_name": "Bash"}), true);
    assert_state("running");
    send(
        "PreToolUse",
        json!({"tool_name": "request_user_input", "tool_input": {"questions": [{"question": "Which option?"}]}}),
        true,
    );
    assert_state("waiting");
    assert_eq!(e.ok("notification.list", json!({}))[1]["body"], "Which option?");
    send("PreModelRequest", json!({}), true);
    assert_state("running");
    send("Stop", json!({"last_assistant_message": "Implemented the change"}), true);
    assert_state("idle");
    let notes = e.ok("notification.list", json!({}));
    assert_eq!(notes.as_array().unwrap().len(), 3);
    assert_eq!(notes[2]["title"], "Codex finished");
    assert_eq!(notes[2]["body"], "Implemented the change");
    send("UserPromptSubmit", json!({}), false);
    assert_state("idle");
    send("UserPromptSubmit", json!({}), true);
    send("Interrupt", json!({}), true);
    assert_state("ended");
    send("SessionEnd", json!({}), true);
    assert_state("ended");
    assert_eq!(e.ok("notification.list", json!({})).as_array().unwrap().len(), 3);
    e.stop(app);
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(e.root.join("state/oxishell/session.json")).unwrap()).unwrap();
    assert_eq!(saved["panes"][pane.to_string()]["agent_session"], "codex-session-42");

    assert!(manage("uninstall").status.success());
    assert_eq!(
        toml::from_str::<toml::Table>(&std::fs::read_to_string(&config).unwrap()).unwrap(),
        toml::from_str::<toml::Table>(original).unwrap()
    );
    std::fs::write(&config, "{broken").unwrap();
    assert!(!manage("install").status.success());
    assert_eq!(std::fs::read_to_string(&config).unwrap(), "{broken");
}

#[test]
fn claude_usage_bridge_preserves_output_and_does_not_post_notifications() {
    if !have_display() {
        return;
    }
    let e = Env::new("usage");
    let claude_dir = e.root.join("home/custom-claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let settings = claude_dir.join("settings.json");
    let original = json!({"model": "custom", "statusLine": {"type": "command", "command": "cat\nprintf ''", "padding": 2}});
    std::fs::write(&settings, original.to_string()).unwrap();
    let manage = |action: &str| {
        Command::new(env!("CARGO_BIN_EXE_oxi"))
            .args(["hooks", action, "claude"])
            .env("CLAUDE_CONFIG_DIR", &claude_dir)
            .output()
            .unwrap()
    };
    assert!(manage("install").status.success());
    let installed: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    let command = installed["statusLine"]["command"].as_str().unwrap();
    let app = e.start();
    let pane = first_pane(&e);
    let payload = json!({"model": {"display_name": "Claude"}, "rate_limits": {
        "five_hour": {"used_percentage": 25.5, "resets_at": oxishell::usage::now() + 3600},
        "seven_day": {"used_percentage": 70, "resets_at": oxishell::usage::now() + 86400}
    }})
    .to_string();
    for inside in [false, true] {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", command])
            .env("OXISHELL_SOCKET", &e.socket)
            .env_remove("OXISHELL_PANE_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if inside {
            cmd.env("OXISHELL_PANE_ID", pane.to_string());
        }
        let mut child = cmd.spawn().unwrap();
        use std::io::Write;
        child.stdin.take().unwrap().write_all(payload.as_bytes()).unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(result.status.success(), "{result:?}");
        assert_eq!(result.stdout, payload.as_bytes());
        assert!(result.stderr.is_empty());
    }
    assert!(e.ok("notification.list", json!({})).as_array().unwrap().is_empty());
    assert!(e.panes()[0]["agent"].is_null(), "usage must not change lifecycle state");
    assert!(e
        .call("agent.usage", json!({"pane": 999999, "rate_limits": {}}))
        .unwrap_err()
        .contains("no such pane"));
    e.ok("agent.usage", json!({"pane": pane, "rate_limits": null}));
    e.stop(app);
    assert!(manage("uninstall").status.success());
    let restored: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(restored, original);
}

#[test]
fn usage_forwarder_preserves_exit_status_without_a_running_app() {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_oxi"))
        .args(["agent-usage", "--forward-command", "cat; exit 7"])
        .env_remove("OXISHELL_PANE_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"{\"rate_limits\":null}\n").unwrap();
    let result = child.wait_with_output().unwrap();
    assert_eq!(result.status.code(), Some(7));
    assert_eq!(result.stdout, b"{\"rate_limits\":null}\n");
    assert!(result.stderr.is_empty());
}

#[test]
fn usage_forwarder_keeps_status_line_responsive_when_app_stalls() {
    use std::io::Write;
    use std::os::unix::net::UnixListener;
    let e = Env::new("usage-timeout");
    std::fs::create_dir_all(e.socket.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(&e.socket).unwrap();
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let (_connection, _) = listener.accept().unwrap();
        let _ = wait.recv_timeout(Duration::from_secs(10));
    });
    let start = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_oxi"))
        .args(["agent-usage", "--forward-command", "cat"])
        .env("OXISHELL_SOCKET", &e.socket)
        .env("OXISHELL_PANE_ID", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let input = b"{\"rate_limits\":null}\n";
    child.stdin.take().unwrap().write_all(input).unwrap();
    let result = child.wait_with_output().unwrap();
    let elapsed = start.elapsed();
    let _ = release.send(());
    server.join().unwrap();
    assert!(elapsed < Duration::from_secs(3), "status line delayed by {elapsed:?}");
    assert!(result.status.success());
    assert_eq!(result.stdout, input);
    assert!(result.stderr.is_empty());
}

#[test]
fn keyboard_and_ctrl_wheel_zoom_resize_terminal_grid() {
    if !have_display() || std::env::var_os("DISPLAY").is_none() || Command::new("xdotool").arg("version").output().is_err() {
        eprintln!("skipped zoom input test: requires X11 and xdotool");
        return;
    }
    let e = Env::new("zoom");
    let app = e.start();
    let pane = first_pane(&e);
    let output = Command::new("xdotool")
        .args(["search", "--sync", "--onlyvisible", "--pid", &app.id().to_string()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let window = String::from_utf8(output.stdout).unwrap().lines().next().unwrap().to_string();
    let xdo = |args: &[&str]| {
        let result = Command::new("xdotool").args(args).output().unwrap();
        assert!(result.status.success(), "xdotool {args:?}: {result:?}");
    };
    xdo(&["windowfocus", "--sync", &window]);
    let mut sequence = 0;
    let mut grid = || -> (u32, u32) {
        sequence += 1;
        let marker = format!("ZOOM-SIZE-{sequence}");
        e.ok(
            "pane.send",
            json!({"pane": pane, "text": format!("printf '\\n{marker} '; stty size\r")}),
        );
        let start = Instant::now();
        loop {
            for line in e.read(pane).lines() {
                if let Some(size) = line.trim().strip_prefix(&format!("{marker} ")) {
                    let numbers: Vec<u32> = size.split_whitespace().filter_map(|n| n.parse().ok()).collect();
                    if numbers.len() == 2 && numbers[0] > 0 && numbers[1] > 0 {
                        return (numbers[0], numbers[1]);
                    }
                }
            }
            assert!(start.elapsed() < Duration::from_secs(5), "terminal grid probe timed out");
            std::thread::sleep(Duration::from_millis(30));
        }
    };
    let original = grid();
    let mut wait_grid = |check: &dyn Fn((u32, u32)) -> bool| {
        let start = Instant::now();
        loop {
            let size = grid();
            if check(size) {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "zoom did not resize grid: {size:?}, original {original:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    xdo(&["key", "ctrl+shift+equal"]);
    wait_grid(&|size| size.0 < original.0);
    xdo(&["key", "ctrl+0"]);
    wait_grid(&|size| size == original);
    xdo(&["key", "ctrl+minus"]);
    wait_grid(&|size| size.0 > original.0);
    xdo(&["key", "ctrl+equal"]);
    wait_grid(&|size| size == original);
    xdo(&[
        "mousemove",
        "--window",
        &window,
        "500",
        "300",
        "keydown",
        "ctrl",
        "click",
        "4",
        "keyup",
        "ctrl",
    ]);
    wait_grid(&|size| size.0 < original.0);
    xdo(&["key", "ctrl+0"]);
    wait_grid(&|size| size == original);
    xdo(&["keydown", "ctrl", "click", "5", "keyup", "ctrl"]);
    wait_grid(&|size| size.0 > original.0);
    xdo(&["key", "ctrl+0"]);
    wait_grid(&|size| size == original);
    xdo(&["click", "4"]);
    assert_eq!(grid(), original, "ordinary scrolling must not zoom");
    e.stop(app);
}

#[test]
fn stress_many_panes_and_heavy_output() {
    if !have_display() {
        return;
    }
    let e = Env::new("stress");
    let app = e.start();
    let first = first_pane(&e);
    // 16 panes across 4 workspaces.
    let mut panes = vec![first];
    for i in 0..3 {
        let c = e.ok("workspace.create", json!({"name": format!("WS{i}")}));
        panes.push(c["pane"].as_u64().unwrap());
    }
    for base in panes.clone() {
        for dir in ["right", "down", "right"] {
            let p = e.ok("pane.split", json!({"pane": base, "direction": dir}))["pane"]
                .as_u64()
                .unwrap();
            panes.push(p);
        }
    }
    assert_eq!(e.panes().len(), 16);
    let pids: Vec<i32> = panes.iter().map(|p| e.pid_of(*p)).collect();

    // Heavy output in every pane at once; the app must stay responsive.
    for p in &panes {
        e.ok("pane.send", json!({"pane": p, "text": "seq 1 200000; echo FLOOD-DONE\r"}));
    }
    let t = Instant::now();
    for p in &panes {
        e.wait_text_for(*p, "\nFLOOD-DONE", 180);
    }
    eprintln!("16 panes x 200k lines finished in {:?}", t.elapsed());
    let t = Instant::now();
    e.ok("workspace.list", json!({}));
    assert!(t.elapsed() < Duration::from_secs(2), "app unresponsive after flood");

    // Rapid workspace switching.
    let ws: Vec<u64> = e
        .ok("workspace.list", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["id"].as_u64().unwrap())
        .collect();
    for i in 0..200 {
        e.ok("workspace.select", json!({"workspace": ws[i % ws.len()]}));
    }
    // Repeated pane creation/destruction.
    for _ in 0..30 {
        let p = e.ok("pane.split", json!({"pane": first, "direction": "down"}))["pane"]
            .as_u64()
            .unwrap();
        let pid = e.pid_of(p);
        e.ok("pane.close", json!({"pane": p, "force": true}));
        assert!(wait_dead(pid), "leaked shell {pid}");
    }
    assert_eq!(e.panes().len(), 16);
    e.stop(app);
    for pid in pids {
        assert!(wait_dead(pid), "shell {pid} survived shutdown");
    }
}
