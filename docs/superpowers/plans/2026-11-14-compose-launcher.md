# Compose Launcher Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional, on-by-default pane button that lists the repo's Docker Compose files, runs them in the
pane, and shows a stack bar with rebuild/restart/stop, a logs line, and crash notifications.

**Architecture:** All logic that does not touch GTK lives in a new `src/compose.rs` (scan, YAML, commands, `ps`
parsing, crash detection), unit-tested like the rest of the library. The meta worker thread runs the blocking
parts (scan, compose command detection, `ps`) and reports through its existing `Update`. A new
`src/ui/compose_ui.rs` owns the pane button, the picker popover, the stack bar and the logs line, and is
synced from `App::refresh`.

**Tech Stack:** Rust 2021 (rust-version 1.87), gtk4 0.9, libadwaita 0.7, vte4 0.8, yaml-rust2, serde_json,
async-channel. No new crates.

**Spec:** `docs/COMPOSE_LAUNCHER.md` (mockup: https://claude.ai/artifact/RZUTJ6wBYP43r5kZgNkhna)

## Global Constraints

- **No git commits.** The user commits everything. Never run `git commit`, `git commit --amend` or `git push`.
  Each task ends with a checkpoint where the work is left uncommitted for review.
- `src/compose.rs` must not import gtk4, glib or vte4 (`src/lib.rs`: "Everything except `ui` is GTK-free and
  unit-tested").
- No new dependencies. YAML via `yaml_rust2::YamlLoader` (as in `src/theme.rs`), JSON via `serde_json`.
- `Config` uses `#[serde(default, deny_unknown_fields)]`. `Compose` gets a hand-written `Default`:
  `enabled = true`, `run_immediately = true`, `notify_on_crash = true`, `default_action = "up"`, `command = ""`.
- Shortcut entry, verbatim: `("compose-files", "<Ctrl><Shift>l", "Docker Compose files")`.
- Exact copy (from the spec):
  - Button tooltip: `Compose files in this repo (Ctrl+Shift+L)`
  - Busy tooltip: `Busy: wait for the command to finish`
  - Settings: `Docker Compose launcher` / `Show a button in each pane that lists the repo's compose files and
    runs them.`; `Run the command right away` / `Off: type the command and wait for Enter.`;
    `Notify when a service crashes` / `Desktop notification for a non-zero exit or an unhealthy service.`
  - Bar buttons: `rebuild`, `restart`, `stop`; logs line button: `stop logs`
  - Crash titles: `<service> exited with code <n>`, `<service> is unhealthy`; notification action label:
    `Restart <service>`
- Scan limits: depth 4, at most 50 files. Skip `.git`, `node_modules`, `target`, `vendor`, `.venv` and every
  folder whose name starts with `.`.
- Turning the feature off hides the button, the bars and the shortcut. It never stops containers and never
  sends input to a pane.
- Verify with: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test --lib`,
  `xvfb-run -a cargo test --test interactions -- --test-threads=1`.

## Review Focus

1. **`ps --format json` comes in two shapes.** Compose before 2.21 prints one JSON array; 2.21 and later print
   one object per line. Both must parse to the same services (Task 4).
2. **Pane opened in `$HOME` or `/` (no git repo).** The scan must stop after a bounded number of folders instead
   of walking the whole disk (Task 2, `max_dirs`).
3. **Symlinked folders, including loops.** The scan must not follow symlinked directories, so a link back to
   a parent cannot hang it (Task 2).
4. **Pane is in a subfolder of the repo** (`cd services/api`). The typed `-f` path must work from the pane's
   folder, not from the repo root (Task 3).
5. **First `up` pulls or builds for minutes with no containers yet.** The stack bar must stay while the pane
   is busy and only hide once nothing runs (Task 4 `stack_should_hide`, Task 7 e2e).

---

## File Structure

| File | Responsibility |
|---|---|
| `src/compose.rs` (new) | Compose file matching and scan, `services:` names, repo root, `Action`, quoting and command building, compose command detection, `ps` parsing, crash detection, bar hide rule |
| `src/config.rs` | `Compose` section, validation of `default_action`, shortcut entry |
| `src/lib.rs` | `pub mod compose;` |
| `src/ui/meta_worker.rs` | `busy`, scan per repo root with cache, command detection cache, `ps` polling |
| `src/ui/compose_ui.rs` (new) | Per-pane state and widgets: button, picker popover and keys, stack bar, logs line, running commands, rebuild |
| `src/ui/pane.rs` | `busy` and `compose` fields on `Pane`; insert the button and bars when the pane is built |
| `src/ui/mod.rs` | `compose-files` action, sync from `refresh`, meta job fields, crash notification with an action |
| `src/ui/desktop_notify.rs` | Optional extra action button on a notification |
| `src/ui/settings.rs` | "Docker Compose" group on the Workflow page; shortcut in the "Panes" group |
| `src/ui/icons.rs`, `data/icons/oxishell-compose-symbolic.svg` | Compose icon with fallback |
| `src/ui/style.css` | Picker, stack bar and logs line styles in the terminal's font |
| `tests/interactions.rs` | e2e tests with a fake `docker` script |
| `docs/CONFIGURATION.md`, `README.md` | Document the feature |

---

### Task 1: `[compose]` config section and shortcut

**Files:**
- Modify: `src/config.rs` (struct list near line 13, `impl Default` block near line 196, `DEFAULT_SHORTCUTS`
  near line 210, `validate` near line 255, tests module)
- Modify: `src/ui/settings.rs` ("Panes" shortcut group near line 46)

**Interfaces:**
- Produces: `pub struct Compose { pub enabled: bool, pub run_immediately: bool, pub notify_on_crash: bool,
  pub default_action: String, pub command: String }` and field `pub compose: Compose` on `Config`.
- Produces: shortcut action name `"compose-files"`.

- [ ] **Step 1: Write the failing tests** in the `tests` module of `src/config.rs`

```rust
#[test]
fn compose_launcher_is_on_by_default_and_can_be_turned_off() {
    let d = Config::default().compose;
    assert!(d.enabled && d.run_immediately && d.notify_on_crash);
    assert_eq!(d.default_action, "up");
    assert_eq!(d.command, "");
    assert!(Config::parse("[appearance]\nmode = \"dark\"\n").unwrap().compose.enabled);
    assert!(!Config::parse("[compose]\nenabled = false\n").unwrap().compose.enabled);
    assert!(Config::parse("[compose]\nenable = false\n").is_err());
}

#[test]
fn compose_default_action_is_checked() {
    for ok in ["up", "up -d", "down", "logs -f"] {
        assert!(Config::parse(&format!("[compose]\ndefault_action = \"{ok}\"\n")).is_ok(), "{ok}");
    }
    assert!(Config::parse("[compose]\ndefault_action = \"explode\"\n").is_err());
}

#[test]
fn compose_shortcut_is_bound_without_conflicts() {
    let c = Config::default();
    assert!(c.effective_shortcuts().contains(&("compose-files".into(), "<Ctrl><Shift>l".into())));
    assert!(c.shortcut_conflicts().is_empty());
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test --lib config::tests::compose`
Expected: compile error, no field `compose` on `Config`.

- [ ] **Step 3: Implement**

Add `Compose` with `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] #[serde(default,
deny_unknown_fields)]` and the hand-written `Default` from Global Constraints. In `validate`, reject a
`default_action` outside `["up", "up -d", "down", "logs -f"]` with
`compose.default_action must be up, up -d, down or logs -f (got "…")`. Append the shortcut tuple to
`DEFAULT_SHORTCUTS` and `"compose-files"` to the "Panes" group in `settings.rs`.

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib config`
Expected: PASS, including the existing `defaults_are_valid_and_roundtrip`.

- [ ] **Step 5: Checkpoint.** `cargo clippy --all-targets -- -D warnings` is clean. Leave the changes
  uncommitted for the user.

---

### Task 2: Scan for compose files

**Files:**
- Create: `src/compose.rs`
- Modify: `src/lib.rs` (add `pub mod compose;`)

**Interfaces:**
- Produces:
  - `pub struct ComposeFile { pub path: PathBuf /* absolute */, pub rel: PathBuf /* relative to root */,
    pub services: Option<Vec<String>> /* None = YAML did not parse */ }` (derive `Debug, Clone, PartialEq`)
  - `pub struct Limits { pub max_depth: usize, pub max_files: usize, pub max_dirs: usize }`, `Default` =
    `4, 50, 5000`
  - `pub fn is_compose_file(name: &str) -> bool`
  - `pub fn service_names(yaml: &str) -> Option<Vec<String>>`
  - `pub fn repo_root(dir: &Path) -> PathBuf`
  - `pub fn scan(root: &Path) -> Vec<ComposeFile>` (= `scan_limited(root, &Limits::default())`)
  - `pub fn scan_limited(root: &Path, limits: &Limits) -> Vec<ComposeFile>`

- [ ] **Step 1: Write the failing tests** (in `src/compose.rs`, `#[cfg(test)] mod tests`, temp dirs under
  `std::env::temp_dir()` named with the test name and `std::process::id()`)

```rust
#[test]
fn compose_file_names() {
    for n in ["compose.yaml", "compose.yml", "docker-compose.yml", "docker-compose.yaml",
              "docker-compose.override.yml", "docker-compose.prod.yaml"] {
        assert!(is_compose_file(n), "{n}");
    }
    for n in ["compose.json", "docker-compose", "my-compose.yml", "docker-compose.yml.bak", "Dockerfile"] {
        assert!(!is_compose_file(n), "{n}");
    }
}

#[test]
fn service_names_from_yaml() {
    assert_eq!(service_names("services:\n  api: {}\n  web: {}\n"), Some(vec!["api".into(), "web".into()]));
    assert_eq!(service_names("version: '3'\n"), Some(vec![]));
    assert_eq!(service_names("services: [unclosed\n"), None);
}

#[test]
fn scan_finds_root_and_nested_files_root_first() {
    // root/docker-compose.yml, root/b/compose.yaml, root/a/docker-compose.yml
    let files = scan(&root);
    let rels: Vec<_> = files.iter().map(|f| f.rel.to_string_lossy().into_owned()).collect();
    assert_eq!(rels, ["docker-compose.yml", "a/docker-compose.yml", "b/compose.yaml"]);
    assert!(files[0].path.is_absolute());
}

#[test]
fn scan_respects_depth_and_skips() {
    // 1/2/3/4/compose.yml is found; 1/2/3/4/5/compose.yml is not;
    // node_modules/x/compose.yml, .git/compose.yml, .hidden/compose.yml, target/compose.yml are not.
    let rels = rels(scan(&root));
    assert_eq!(rels, ["1/2/3/4/compose.yml"]);
}

#[test]
fn scan_caps_files_and_folders() {
    // 60 folders each with compose.yml
    assert_eq!(scan(&root).len(), 50);
    let few = scan_limited(&root, &Limits { max_depth: 4, max_files: 50, max_dirs: 10 });
    assert!(few.len() <= 10, "stops after max_dirs folders");
}

#[test]
fn scan_does_not_follow_symlinked_folders() {
    // root/loop -> root ; root/linked -> other dir containing compose.yml
    let rels = rels(scan(&root)); // returns, does not hang
    assert!(!rels.iter().any(|r| r.starts_with("loop") || r.starts_with("linked")));
}

#[test]
fn repo_root_is_git_toplevel_or_the_folder() {
    // `git init root` then root/sub
    assert_eq!(repo_root(&root.join("sub")), root.canonicalize().unwrap());
    assert_eq!(repo_root(&not_a_repo), not_a_repo);
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test --lib compose::`
Expected: compile errors for the missing functions.

- [ ] **Step 3: Implement** in `src/compose.rs`

Breadth-first walk with `std::fs::read_dir`, using `DirEntry::file_type()` (which does not follow symlinks) so
symlinked directories are skipped. Count visited folders and stop at `max_dirs`. `max_depth` is the folder depth
below the root; files directly in the root are depth 0. Sort: files whose `rel` has no parent first, then by `rel`.
Truncate to `max_files`. `repo_root` runs `git -C <dir> rev-parse --show-toplevel` with stdin/stderr null and
`GIT_OPTIONAL_LOCKS=0`, like `meta::git_info`, and returns the folder unchanged on failure. `service_names`
returns the keys of the top-level `services` hash in file order.

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib compose::`
Expected: PASS.

- [ ] **Step 5: Checkpoint.** Clippy clean, changes left uncommitted.

---

### Task 3: Actions, quoting and the compose command

**Files:**
- Modify: `src/compose.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `pub enum Action { Up, UpDetached, Down, Logs, Restart, Stop, UpBuild, RestartService(String) }`
    (derive `Debug, Clone, PartialEq`)
  - `impl Action { pub fn args(&self) -> Vec<String>; pub fn from_setting(s: &str) -> Option<Action>;
    pub fn holds_pane(&self) -> bool }`
  - `pub fn shell_quote(s: &str) -> String`
  - `pub fn file_arg(file: &Path, cwd: &Path) -> String`
  - `pub fn typed_command(cmd: &[String], file: &Path, cwd: &Path, action: &Action) -> String`
  - `pub fn argv(cmd: &[String], file: &Path, action: &Action) -> Vec<String>` (background runs, absolute path)
  - `pub fn detect_command(setting: &str, works: impl Fn(&[&str]) -> bool) -> Option<Vec<String>>`
  - `pub fn command_works(cmd: &[&str]) -> bool` (runs `<cmd> version`, quiet, true on exit 0)

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn action_args() {
    assert_eq!(Action::Up.args(), ["up"]);
    assert_eq!(Action::UpDetached.args(), ["up", "-d"]);
    assert_eq!(Action::Logs.args(), ["logs", "-f"]);
    assert_eq!(Action::UpBuild.args(), ["up", "--build"]);
    assert_eq!(Action::RestartService("api".into()).args(), ["restart", "api"]);
    assert_eq!(Action::from_setting("up -d"), Some(Action::UpDetached));
    assert_eq!(Action::from_setting("nope"), None);
    assert!(Action::Up.holds_pane() && Action::Logs.holds_pane() && Action::UpBuild.holds_pane());
    assert!(!Action::UpDetached.holds_pane() && !Action::Down.holds_pane());
}

#[test]
fn quoting() {
    assert_eq!(shell_quote("services/api/compose.yaml"), "services/api/compose.yaml");
    assert_eq!(shell_quote("my app/compose.yml"), "'my app/compose.yml'");
    assert_eq!(shell_quote("it's/compose.yml"), "'it'\\''s/compose.yml'");
}

#[test]
fn file_path_is_relative_to_the_panes_folder_when_inside_it() {
    let f = Path::new("/r/services/api/compose.yaml");
    assert_eq!(file_arg(f, Path::new("/r")), "services/api/compose.yaml");
    assert_eq!(file_arg(f, Path::new("/r/services/api")), "compose.yaml");
    assert_eq!(file_arg(f, Path::new("/r/web")), "/r/services/api/compose.yaml");
}

#[test]
fn typed_command_text() {
    let cmd = vec!["docker".to_string(), "compose".to_string()];
    assert_eq!(
        typed_command(&cmd, Path::new("/r/my app/compose.yml"), Path::new("/r"), &Action::Up),
        "docker compose -f 'my app/compose.yml' up"
    );
}

#[test]
fn compose_command_detection() {
    let only = |ok: &'static [&'static str]| move |c: &[&str]| c == ok;
    assert_eq!(detect_command("", only(&["docker", "compose"])), Some(vec!["docker".into(), "compose".into()]));
    assert_eq!(detect_command("", only(&["docker-compose"])), Some(vec!["docker-compose".into()]));
    assert_eq!(detect_command("", only(&["podman", "compose"])), Some(vec!["podman".into(), "compose".into()]));
    assert_eq!(detect_command("", |_| false), None);
    assert_eq!(detect_command("/opt/d compose", |_| false), Some(vec!["/opt/d".into(), "compose".into()]));
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test --lib compose::`
Expected: compile errors for the missing items.

- [ ] **Step 3: Implement** the items listed under Produces. `shell_quote` leaves a string bare only when every
  char is in `A-Za-z0-9_./-`. Detection order: `docker compose`, `docker-compose`, `podman compose`. A non-empty
  setting is split on whitespace and used without probing.

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib compose::`
Expected: PASS.

- [ ] **Step 5: Checkpoint.** Clippy clean, changes left uncommitted.

---

### Task 4: `ps` parsing, crash detection and the bar hide rule

**Files:**
- Modify: `src/compose.rs`

**Interfaces:**
- Produces:
  - `pub enum State { Running, Restarting, Exited, Other }`
  - `pub struct ServiceStatus { pub service: String, pub state: State, pub health: String, pub exit_code: i32,
    pub ports: Vec<u16> }` (derive `Debug, Clone, PartialEq`)
  - `pub fn ps_argv(cmd: &[String], file: &Path) -> Vec<String>` → `… -f <abs file> ps --all --format json`
  - `pub fn parse_ps(text: &str) -> Result<Vec<ServiceStatus>, String>`, sorted by `service`
  - `pub enum CrashReason { Exited(i32), Unhealthy }`,
    `pub struct Crash { pub service: String, pub reason: CrashReason }`, `impl Crash { pub fn title(&self) -> String }`
  - `pub fn crashes(prev: &[ServiceStatus], now: &[ServiceStatus]) -> Vec<Crash>`
  - `pub fn stack_should_hide(seen_running: bool, now: &[ServiceStatus], pane_busy: bool) -> bool`

- [ ] **Step 1: Write the failing tests**

```rust
const ONE: &str = r#"{"Service":"api","State":"running","Health":"","ExitCode":0,"Publishers":[{"URL":"0.0.0.0","TargetPort":8000,"PublishedPort":8000,"Protocol":"tcp"}]}"#;
const TWO: &str = r#"{"Service":"db","State":"exited","Health":"","ExitCode":1,"Publishers":[]}"#;

#[test]
fn ps_parses_array_and_line_formats_alike() {
    let lines = parse_ps(&format!("{ONE}\n{TWO}\n")).unwrap();
    let array = parse_ps(&format!("[{ONE},{TWO}]")).unwrap();
    assert_eq!(lines, array);
    assert_eq!(lines[0].service, "api");
    assert_eq!(lines[0].ports, [8000]);
    assert_eq!(lines[1].state, State::Exited);
    assert_eq!(lines[1].exit_code, 1);
    assert_eq!(parse_ps("").unwrap(), []);
    assert!(parse_ps("not json").is_err());
}

#[test]
fn crash_detection() {
    let s = |state, health: &str, code| ServiceStatus { service: "api".into(), state, health: health.into(), exit_code: code, ports: vec![] };
    let up = [s(State::Running, "", 0)];
    assert_eq!(crashes(&up, &[s(State::Exited, "", 1)])[0].title(), "api exited with code 1");
    assert!(crashes(&up, &[s(State::Exited, "", 0)]).is_empty(), "clean exit");
    assert!(crashes(&[s(State::Exited, "", 1)], &[s(State::Exited, "", 1)]).is_empty(), "once per crash");
    assert_eq!(crashes(&up, &[s(State::Running, "unhealthy", 0)])[0].title(), "api is unhealthy");
    assert!(crashes(&[s(State::Running, "unhealthy", 0)], &[s(State::Running, "unhealthy", 0)]).is_empty());
}

#[test]
fn bar_stays_while_starting_and_hides_when_nothing_runs() {
    let running = [ServiceStatus { service: "api".into(), state: State::Running, health: String::new(), exit_code: 0, ports: vec![] }];
    assert!(!stack_should_hide(false, &[], true), "first up still pulling: pane busy, no containers yet");
    assert!(!stack_should_hide(true, &running, false));
    assert!(stack_should_hide(true, &[], false), "stopped outside oxishell");
    assert!(stack_should_hide(false, &[], false), "up -d failed and returned to the prompt");
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test --lib compose::`
Expected: compile errors.

- [ ] **Step 3: Implement.** `parse_ps` trims the text; if it starts with `[`, parse an array, otherwise parse
  each non-empty line as an object. Map `State` from `"running"`, `"restarting"`, `"exited"`, anything else →
  `Other`. `ports` = distinct `PublishedPort` values > 0, sorted. A service counts as running for
  `stack_should_hide` when its state is `Running` or `Restarting`. Hide when nothing runs and
  (`seen_running` or the pane is not busy).

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib compose::`
Expected: PASS.

- [ ] **Step 5: Checkpoint.** Clippy clean, changes left uncommitted.

---

### Task 5: Meta worker reports busy state, compose files and `ps`

**Files:**
- Modify: `src/ui/meta_worker.rs` (`Job`, `Update`, worker loop, `attach`)
- Modify: `src/ui/mod.rs` (Job construction near line 1695; extract it into `fn meta_job(&self, pid: PaneId,
  ports: bool) -> Option<meta_worker::Job>` so Task 6 can send one on demand)
- Modify: `src/ui/pane.rs` (`Pane` fields)
- Create: `src/ui/compose_ui.rs` (state types only in this task) and add `pub mod compose_ui;` in
  `src/ui/mod.rs` (public, because the e2e tests call into it)
- Test: `tests/interactions.rs`

**Interfaces:**
- Consumes: `compose::{repo_root, scan, detect_command, command_works, ps_argv, parse_ps, ComposeFile,
  ServiceStatus}`.
- Produces:
  - `Job` gains `compose_cmd: Option<String>` (None = feature off, Some = `config.compose.command`),
    `rescan: bool`, `ps_file: Option<PathBuf>`.
  - `Update` gains `busy: bool`, `compose: Option<ComposeScan>`, `ps: Option<Result<Vec<ServiceStatus>, String>>`.
  - In `src/compose.rs`: `pub struct ComposeScan { pub root: PathBuf, pub files: Vec<ComposeFile>,
    pub command: Option<Vec<String>> }`.
  - `Pane` gains `pub busy: bool` and `pub compose: compose_ui::PaneCompose`.
  - `compose_ui::PaneCompose { pub scan: Option<ComposeScan>, pub stack: Option<Stack>,
    pub logs: Option<PathBuf>, pub rescan: bool }` (widget handles are added in Task 6).
  - `compose_ui::Stack { pub file: PathBuf, pub attached: bool, pub started: Instant,
    pub services: Vec<ServiceStatus>, pub seen_running: bool, pub note: Option<String> }`.

- [ ] **Step 1: Write the failing e2e test** and a fake `docker` helper in `tests/interactions.rs`

```rust
/// A `docker` stand-in: records its arguments, answers `compose version`, prints `ps.json` for `ps`,
/// and stays in the foreground for `up`, `up --build` and `logs -f` until Ctrl+C.
fn fake_docker(h: &Harness) -> (PathBuf, PathBuf, PathBuf) {
    let bin = h.root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let record = h.root.join("docker-args");
    let ps = h.root.join("ps.json");
    std::fs::write(&ps, "").unwrap();
    let docker = bin.join("docker");
    std::fs::write(&docker, format!(
        "#!/bin/sh\necho \"$*\" >> '{rec}'\ncase \"$*\" in\n  'compose version') exit 0;;\n  *' ps --all --format json') cat '{ps}'; exit 0;;\n  *' up'|*' up --build'|*' logs -f') trap 'exit 130' INT; while :; do sleep 0.1; done;;\nesac\nexit 0\n",
        rec = record.display(), ps = ps.display())).unwrap();
    std::fs::set_permissions(&docker, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    h.app.inner.borrow_mut().config.compose.command = format!("{} compose", docker.display());
    (docker, record, ps)
}

#[test]
fn panes_learn_the_repos_compose_files_and_busy_state() {
    gtk4::test_synced(|| {
        let h = Harness::new("compose-scan");
        let (_, _, _) = fake_docker(&h);
        std::fs::write(h.root.join("home/docker-compose.yml"), "services:\n  api: {}\n  web: {}\n").unwrap();
        let (_, pane) = h.app.new_workspace(Some("shop"), Some(h.root.join("home"))).unwrap();
        wait_until(5_000, || h.app.inner.borrow().panes[&pane].compose.scan.is_some());
        let i = h.app.inner.borrow();
        let scan = i.panes[&pane].compose.scan.as_ref().unwrap();
        assert_eq!(scan.files.len(), 1);
        assert_eq!(scan.files[0].services, Some(vec!["api".into(), "web".into()]));
        assert!(scan.command.is_some());
        drop(i);
        let term = h.app.inner.borrow().panes[&pane].term.clone();
        term.feed_child(b"sleep 30\n");
        wait_until(5_000, || h.app.inner.borrow().panes[&pane].busy);
    });
}
```

Add `fn wait_until(ms: u64, f: impl Fn() -> bool)` next to `pump`: pump in 50 ms steps until `f()` is true,
and panic with a message after `ms`.

- [ ] **Step 2: Run it and see it fail**

Run: `xvfb-run -a cargo test --test interactions panes_learn -- --test-threads=1`
Expected: compile error for the missing `compose` and `busy` fields.

- [ ] **Step 3: Implement**

- Worker: `busy = fg.is_some_and(|p| p != job.shell_pid)`. When `compose_cmd` is `Some`, compute
  `repo_root(cwd)`. Keep two caches in the worker thread: `HashMap<PathBuf, Vec<ComposeFile>>` keyed by root
  (rescan bypasses it) and `HashMap<String, Option<Vec<String>>>` keyed by the command setting (uses
  `detect_command(setting, command_works)`). Send `compose: Some(ComposeScan)` only when the root or the file
  list changed for that pane, or when `rescan` is set. When `ps_file` is `Some`, run `ps_argv` and send `ps`.
- `attach`: copy `busy`, `compose` and `ps` into the pane and count them as changes. Apply `ps` to
  `compose.stack` (Task 7 adds crash handling and the hide rule).
- `meta_job`: `compose_cmd` is `Some(config.compose.command.clone())` only when `config.compose.enabled`;
  `rescan` takes and clears `pane.compose.rescan`; `ps_file` is the stack's file when a stack exists.

- [ ] **Step 4: Run the tests**

Run: `xvfb-run -a cargo test --test interactions panes_learn -- --test-threads=1` and `cargo test --lib`
Expected: PASS.

- [ ] **Step 5: Checkpoint.** Clippy clean, changes left uncommitted.

---

### Task 6: Pane button, picker and running a command

**Files:**
- Modify: `src/ui/compose_ui.rs`
- Modify: `src/ui/pane.rs` (`create_pane`, where `actions` gets `handle` and `close_btn`)
- Modify: `src/ui/mod.rs` (`run_action` arm `"compose-files"`, `refresh` loop calls `compose_ui::sync`)
- Modify: `src/ui/icons.rs`; create `data/icons/oxishell-compose-symbolic.svg`
- Modify: `src/ui/style.css`
- Test: `tests/interactions.rs`

**Interfaces:**
- Consumes: Task 5 state, `compose::{Action, typed_command, file_arg}`, `Pane::send_input`,
  `Pane::has_running_child`, `App::meta_job`.
- Produces:
  - `PaneCompose` gains `pub button: gtk4::Button` (CSS class `pane-compose`).
  - `pub fn attach_button(app: &AppRc, id: PaneId, actions: &gtk4::Box, before: &impl IsA<gtk4::Widget>) -> gtk4::Button`
  - `pub fn sync(app: &AppRc, id: PaneId)`
  - `pub fn open_picker(app: &AppRc, id: PaneId)`
  - `pub fn run(app: &AppRc, id: PaneId, file: &Path, action: Action)`
  - `pub fn rescan(app: &AppRc, id: PaneId)`
  - `icons::compose() -> &'static str` (bundled `oxishell-compose-symbolic`, fallback `system-run-symbolic`)

- [ ] **Step 1: Write the failing e2e test**

```rust
#[test]
fn choosing_a_compose_file_runs_it_in_the_pane_and_disables_the_button() {
    gtk4::test_synced(|| {
        let h = Harness::new("compose-run");
        let (docker, record, _) = fake_docker(&h);
        std::fs::write(h.root.join("home/docker-compose.yml"), "services:\n  api: {}\n").unwrap();
        let (_, pane) = h.app.new_workspace(Some("shop"), Some(h.root.join("home"))).unwrap();
        wait_until(5_000, || h.app.inner.borrow().panes[&pane].compose.button.is_visible());
        let button = h.app.inner.borrow().panes[&pane].compose.button.clone();
        assert_eq!(button.tooltip_text().as_deref(), Some("Compose files in this repo (Ctrl+Shift+L)"));

        let file = h.root.join("home/docker-compose.yml");
        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::Up);
        wait_until(5_000, || std::fs::read_to_string(&record).unwrap_or_default().contains(" up"));
        assert!(std::fs::read_to_string(&record).unwrap().contains("compose -f docker-compose.yml up"));
        wait_until(5_000, || !button.is_sensitive());
        assert_eq!(button.tooltip_text().as_deref(), Some("Busy: wait for the command to finish"));
        let _ = docker;
    });
}

#[test]
fn no_compose_files_means_no_button() {
    gtk4::test_synced(|| {
        let h = Harness::new("compose-none");
        fake_docker(&h);
        let (_, pane) = h.app.new_workspace(Some("empty"), Some(h.root.join("home"))).unwrap();
        wait_until(5_000, || h.app.inner.borrow().panes[&pane].compose.scan.is_some());
        pump(300);
        assert!(!h.app.inner.borrow().panes[&pane].compose.button.is_visible());
    });
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `xvfb-run -a cargo test --test interactions compose -- --test-threads=1`
Expected: compile error for the missing `button` field and `compose_ui::run`.

- [ ] **Step 3: Implement**

- Button: `gtk4::Button::from_icon_name(icons::compose())` with classes `flat`, `circular`, `pane-compose`,
  `focus_on_click(false)`, `can_focus(false)`, inserted into `actions` before the drag handle
  (`actions.insert_child_after` with no sibling, then append handle and close as today). Click → `open_picker`.
- `sync`: visible when `config.compose.enabled`, the scan has files, and `command` is `Some`. Sensitive when
  `!pane.busy`. Tooltips from Global Constraints. Called for every pane from `App::refresh`.
- Picker: a `gtk4::Popover` on the button. A `SearchEntry` with a `/` prefix label, a `ListBox` of rows (file
  name, `rel` parent, `N services · a, b`), a keys line
  (`↑↓ move  ⏎ up  d detached  x down  l logs  / filter  r rescan  esc close`), and a footer with the scan
  summary. Keys through an `EventControllerKey` on the popover: `Down`/`j`, `Up`/`k`, `Return`/`u` →
  `default_action`, `d` → `UpDetached`, `x` → `Down`, `l` → `Logs`, `/` focuses the entry, `r` → `rescan`,
  `Escape` closes. Letters act only while the entry is not focused. Each row also has `d -d`, `x down`,
  `l logs` flat buttons.
- `run`: check `has_running_child()` again first and do nothing when busy. Build
  `typed_command(cmd, file, pane.cwd, &action)`, then `send_input(text)`, and `"\n"` too when
  `run_immediately`. If `action.holds_pane()`, set `pane.busy = true` at once so the button greys out before
  the next meta tick. `Up`/`UpBuild` set `stack = Some(Stack { attached: true, .. })`, `UpDetached` sets
  `attached: false`, `Logs` sets `logs = Some(file)`, and `Down` on the stack's file clears `stack`. Then call
  `app.refresh()`.
- `run_action("compose-files")`: when the focused pane's button is visible and sensitive, call
  `open_picker`; otherwise do nothing.
- `rescan`: set `pane.compose.rescan = true` and send `app.meta_job(id, false)` right away.
- CSS: the picker and its rows use the terminal font (`font-family: monospace`), square corners, inverse
  colours for the selected row and for hovered row buttons. The button keeps the default flat look of
  `pane-close`.

- [ ] **Step 4: Run the tests**

Run: `xvfb-run -a cargo test --test interactions compose -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Check by hand.** `cargo run --bin oxishell` in a repo with a compose file: the icon shows; the
  picker opens with `Ctrl+Shift+L`; `j`/`k`, `/`, `Esc` work; a folder without compose files has no icon.

- [ ] **Step 6: Checkpoint.** Clippy clean, changes left uncommitted.

---

### Task 7: Stack bar and logs line

**Files:**
- Modify: `src/ui/compose_ui.rs`, `src/ui/pane.rs` (append the two bars to `body` after `overlay`),
  `src/ui/meta_worker.rs` (`attach`: hide rule), `src/ui/style.css`
- Test: `tests/interactions.rs`

**Interfaces:**
- Consumes: `compose::{stack_should_hide, argv, Action, ServiceStatus, State}`, `Stack`, `PaneCompose`.
- Produces:
  - `PaneCompose` gains `pub stack_bar: gtk4::Box` (class `compose-stack`) and `pub logs_bar: gtk4::Box`
    (class `compose-logs`).
  - `pub fn run_background(app: &AppRc, id: PaneId, action: Action)`
  - `pub fn stop_logs(app: &AppRc, id: PaneId)`

- [ ] **Step 1: Write the failing e2e test**

```rust
#[test]
fn stack_bar_follows_ps_and_stop_runs_in_the_background() {
    gtk4::test_synced(|| {
        let h = Harness::new("compose-bar");
        let (_, record, ps) = fake_docker(&h);
        let file = h.root.join("home/docker-compose.yml");
        std::fs::write(&file, "services:\n  api: {}\n").unwrap();
        let (_, pane) = h.app.new_workspace(Some("shop"), Some(h.root.join("home"))).unwrap();
        wait_until(5_000, || h.app.inner.borrow().panes[&pane].compose.button.is_visible());

        oxishell::ui::compose_ui::run(&h.app, pane, &file, oxishell::compose::Action::Up);
        let bar = h.app.inner.borrow().panes[&pane].compose.stack_bar.clone();
        pump(4_000);
        assert!(bar.is_visible(), "no containers yet but the pane is busy: bar stays");

        std::fs::write(&ps, r#"{"Service":"api","State":"running","Health":"","ExitCode":0,"Publishers":[{"URL":"0.0.0.0","TargetPort":8000,"PublishedPort":8000,"Protocol":"tcp"}]}"#).unwrap();
        wait_until(8_000, || h.app.inner.borrow().panes[&pane].compose.stack.as_ref().is_some_and(|s| s.seen_running));

        oxishell::ui::compose_ui::run_background(&h.app, pane, oxishell::compose::Action::Stop);
        wait_until(5_000, || std::fs::read_to_string(&record).unwrap().contains(" stop"));
        assert!(h.app.inner.borrow().panes[&pane].busy, "stop does not type into the pane");

        std::fs::write(&ps, "").unwrap();
        wait_until(8_000, || !bar.is_visible());
    });
}

#[test]
fn stop_logs_sends_ctrl_c_and_hides_the_logs_line() {
    // run(.., Action::Logs); wait until compose.logs_bar is visible and the pane is busy;
    // stop_logs(); wait until the pane is not busy and logs_bar is hidden;
    // the record file has no " stop" or " down" line.
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `xvfb-run -a cargo test --test interactions stack_bar stop_logs -- --test-threads=1`
Expected: compile errors for the missing fields and functions.

- [ ] **Step 3: Implement**

- Stack bar, monospace, one status row and one chip row:
  `● <rel>  <n>/<m> up │ <mm:ss> │ attached|detached` plus `[rebuild] [restart] [stop]` buttons
  (`rebuild` is wired in Task 9). Chips: `● api :8000`, `↻` for `Restarting`, `✗` for `Exited`. Clicking a
  chip runs `run_background(RestartService(name))`. While `note` is set (e.g. `Stopping…`), show it in place
  of the counts and make the buttons insensitive. A 1 s `glib::timeout_add_seconds_local` updates the uptime
  only while some stack bar is visible.
- `run_background`: run `compose::argv(cmd, file, &action)` on a `std::thread`, send the exit status and the
  last stderr line back through `async_channel`, and handle the result in `glib::spawn_future_local`. On
  failure, set `note` to `<action> failed (exit <n>): <last stderr line>`. Never touch the pane's input.
- Logs line: `≡ logs -f  <rel> │ following` and a `stop logs` button. Visible while `compose.logs` is `Some`
  and the pane is busy. `stop_logs` sends `b"\x03"` with `send_input` and clears `compose.logs`. Also clear
  `logs` when the pane is no longer busy.
- `attach` (meta worker): after applying `ps` to the stack, set `seen_running` once any service runs, and
  clear the stack when `stack_should_hide(seen_running, &services, pane.busy)`.
- Bars hidden when `config.compose.enabled` is false.

- [ ] **Step 4: Run the tests**

Run: `xvfb-run -a cargo test --test interactions stack_bar stop_logs -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Checkpoint.** Clippy clean, changes left uncommitted.

---

### Task 8: Crash notifications with a Restart action

**Files:**
- Modify: `src/ui/desktop_notify.rs` (`Request`, the `actions` list near line 213, `ActionInvoked` near line 155)
- Modify: `src/ui/mod.rs` (`notify_with` gains an `action` parameter; new `notify_compose_crash`; map of
  notification id → restart target; `set_on_action` hookup next to `set_on_activate` near line 365)
- Modify: `src/ui/meta_worker.rs` (`attach`: call `compose::crashes` before replacing `stack.services`)
- Test: `src/ui/desktop_notify.rs` unit test, `tests/interactions.rs`

**Interfaces:**
- Consumes: `compose::{crashes, Crash}`, `run_background`.
- Produces:
  - `Request` gains `pub action: Option<(&'a str, &'a str)>` (key, label). Existing callers pass `None`.
  - `pub(crate) fn action_list(extra: Option<(&str, &str)>) -> Vec<String>`
  - `Desktop::set_on_action(&self, f: impl Fn(u64, String) + 'static)`
  - `App::notify_compose_crash(self: &Rc<Self>, pane: PaneId, file_rel: &str, crash: &Crash)`

- [ ] **Step 1: Write the failing tests**

```rust
// src/ui/desktop_notify.rs
#[test]
fn extra_action_is_appended_after_open() {
    assert_eq!(action_list(None), ["default", "Open"]);
    assert_eq!(action_list(Some(("restart", "Restart api"))), ["default", "Open", "restart", "Restart api"]);
}
```

```rust
// tests/interactions.rs
#[test]
fn a_crashing_service_is_reported_once() {
    // As in Task 7: run Up, write a running `api` to ps.json, wait for seen_running.
    // Write {"Service":"api","State":"exited","Health":"","ExitCode":1,"Publishers":[]} to ps.json.
    // wait_until h.app.inner.borrow().notes.all() has exactly one entry titled "api exited with code 1";
    // pump(7_000) more and assert there is still exactly one.
    // Set config.compose.notify_on_crash = false, write running, then exited again: still one entry.
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test --lib desktop_notify` and `xvfb-run -a cargo test --test interactions crashing -- --test-threads=1`
Expected: compile errors.

- [ ] **Step 3: Implement**

`notify_compose_crash` calls `notify_with(Some(pane), &crash.title(), file_rel, "compose", Kind::CommandFailed,
None, Some(("restart", &format!("Restart {}", crash.service))))` and records `id → (pane, service)`. The existing
`notify_with` already sends the desktop notification only when the pane is not seen, and already puts the
workspace name in front of the body. `set_on_action` with key `restart` looks up the id and calls
`run_background(app, pane, Action::RestartService(service))`. Skip all of this when
`config.compose.notify_on_crash` is false; the red `✗` chip shows either way.

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib` and `xvfb-run -a cargo test --test interactions crashing -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Checkpoint.** Clippy clean, changes left uncommitted.

---

### Task 9: Rebuild in the pane, without `-d`

**Files:**
- Modify: `src/ui/compose_ui.rs` (wire the `rebuild` button)
- Test: `tests/interactions.rs`

**Interfaces:**
- Consumes: `run`, `Action::UpBuild`, `Pane::has_running_child`, `App::split`.
- Produces: `pub fn rebuild(app: &AppRc, id: PaneId)`

- [ ] **Step 1: Write the failing e2e test**

```rust
#[test]
fn rebuild_stops_the_attached_up_then_runs_up_build_in_the_pane() {
    // fake_docker, compose file, run(.., Action::Up); wait until busy.
    // compose_ui::rebuild(&h.app, pane);
    // wait_until record contains "up --build"; the "up --build" line comes after the first " up" line;
    // the pane is busy again and compose.stack is attached (s.attached == true).
}

#[test]
fn rebuild_with_another_program_in_the_pane_opens_a_split() {
    // run(.., Action::UpDetached) with ps.json running; then feed_child(b"sleep 30\n") and wait until busy.
    // rebuild(); a new pane appears in the workspace and the record gets "up --build";
    // the first pane is still running sleep (busy).
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `xvfb-run -a cargo test --test interactions rebuild -- --test-threads=1`
Expected: compile error for `rebuild`.

- [ ] **Step 3: Implement**

- The pane is not busy: `run(UpBuild)`.
- The pane holds this stack's attached `up` or the launcher's `logs -f`: send `b"\x03"`, then poll
  `has_running_child()` every 200 ms with a `glib::timeout_add_local`. When it is false, `run(UpBuild)`. After
  30 s, give up and set `note` to `Rebuild: the pane did not return to the prompt`.
- Any other program: `app.split(Some(id), Orient::Vertical, pane.cwd)`, then `run(new_pane, file, UpBuild)`,
  and move the stack to the new pane.
- Reset `started` and set `attached = true`.

- [ ] **Step 4: Run the tests**

Run: `xvfb-run -a cargo test --test interactions rebuild -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Checkpoint.** Clippy clean, changes left uncommitted.

---

### Task 10: Settings switches, turning the feature off, docs

**Files:**
- Modify: `src/ui/settings.rs` (`workflow_page`, new group after "Sessions")
- Modify: `src/ui/mod.rs` (`apply_config` ends with `refresh()` so the button and bars follow the setting at once)
- Modify: `docs/CONFIGURATION.md`, `README.md`
- Test: `tests/interactions.rs`

**Interfaces:**
- Consumes: `Config::compose`, `switch` helper, `compose_ui::sync`.

- [ ] **Step 1: Write the failing e2e test**

```rust
#[test]
fn turning_the_launcher_off_hides_everything_and_stops_nothing() {
    // fake_docker, compose file, run(.., Action::UpDetached), running api in ps.json, wait for the stack bar.
    // h.app.inner.borrow_mut().config.compose.enabled = false; h.app.apply_config(); pump(300);
    // button and stack_bar are hidden; h.app.run_action("compose-files") opens no popover;
    // the record file has no " stop" and no " down" line.
    // Turn it back on: the button shows again.
}
```

- [ ] **Step 2: Run it and see it fail**

Run: `xvfb-run -a cargo test --test interactions turning_the_launcher_off -- --test-threads=1`
Expected: FAIL (the button stays visible until the next meta tick, or the scan keeps running).

- [ ] **Step 3: Implement**

- Settings: `group(&p, "Docker Compose", "")` and three `switch` rows with the exact copy from Global
  Constraints. Bind the two child rows' `sensitive` to the main row's `active`, as the Sound row does in the
  notifications page.
- `apply_config` calls `self.refresh()` (if it does not already), so `compose_ui::sync` hides or shows at once.
  When disabled, `meta_job` sends `compose_cmd: None`, so no scan and no `ps` run.
- Docs: add a "Docker Compose launcher" section to `docs/CONFIGURATION.md` with the `[compose]` table from the
  spec, and one line in the README feature list. Set `docs/COMPOSE_LAUNCHER.md` status to "built".

- [ ] **Step 4: Run everything**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --lib && xvfb-run -a cargo test --test interactions -- --test-threads=1 && xvfb-run -a cargo test --test e2e -- --test-threads=1`
Expected: all pass, no warnings.

- [ ] **Step 5: Checkpoint.** Leave everything uncommitted for the user to review and commit.

---

## Spec coverage

| Spec section | Task |
|---|---|
| Settings, `[compose]`, visibility table, off never stops containers | 1, 10 |
| Pane button, no badge, busy disable, tooltips | 6 |
| File list, keys, `Ctrl+Shift+L` | 1, 6 |
| Running a command (quoting, detection, `send_input`, `run_immediately`) | 3, 6 |
| Stack bar, chips, restart/stop in the background | 4, 7 |
| Logs line and `stop logs` | 7 |
| Rebuild without `-d` | 9 |
| Status polling, hide when nothing runs | 4, 5, 7 |
| Crash notifications | 4, 8 |
| Scanning | 2, 5 |
| Tests and docs | every task, 10 |
