//! Local control API: newline-delimited JSON over a Unix domain socket.
//!
//! Security: the socket lives in a 0700 per-user directory, is itself 0600,
//! and every connection's peer uid is checked with SO_PEERCRED. There is no
//! network listener. Requests are size-limited and validated before they
//! reach the GUI thread.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MAX_REQUEST_BYTES: u64 = 1 << 20;
pub const MAX_SEND_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    #[serde(default)]
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }
    pub fn err(id: u64, e: impl std::fmt::Display) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(e.to_string()),
        }
    }
}

/// Every method the server accepts. Anything else is rejected before it
/// reaches the GUI.
pub const METHODS: &[&str] = &[
    "ping",
    "workspace.list",
    "workspace.create",
    "workspace.select",
    "workspace.rename",
    "workspace.close",
    "workspace.move",
    "pane.list",
    "pane.split",
    "pane.focus",
    "pane.close",
    "pane.send",
    "pane.read",
    "notify",
    "agent.event",
    "notification.list",
    "notification.clear",
    "notification.jump",
    "workspace.next",
    "workspace.prev",
];

/// A validated request plus the channel to answer it on.
pub struct Call {
    pub req: Request,
    pub reply: std::sync::mpsc::Sender<Response>,
}

pub fn parse_request(line: &str) -> Result<Request, String> {
    let req: Request = serde_json::from_str(line).map_err(|e| format!("invalid JSON request: {e}"))?;
    if !METHODS.contains(&req.method.as_str()) {
        return Err(format!("unknown method {:?}", req.method));
    }
    if !(req.params.is_object() || req.params.is_null()) {
        return Err("params must be an object".into());
    }
    Ok(req)
}

// ---- typed parameter helpers ------------------------------------------------

pub fn param_u64(p: &Value, key: &str) -> Result<Option<u64>, String> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| format!("{key} must be a non-negative integer")),
    }
}

pub fn param_str<'a>(p: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_str().map(Some).ok_or_else(|| format!("{key} must be a string")),
    }
}

pub fn require_str<'a>(p: &'a Value, key: &str) -> Result<&'a str, String> {
    param_str(p, key)?.ok_or_else(|| format!("missing {key}"))
}

// ---- server -----------------------------------------------------------------

fn peer_uid(s: &UnixStream) -> Option<u32> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            s.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    (rc == 0).then_some(cred.uid)
}

/// Bind the control socket. Fails if another live instance owns it; removes
/// a stale socket file left by a crash.
pub fn bind(path: &Path) -> std::io::Result<UnixListener> {
    if let Some(dir) = path.parent() {
        crate::paths::ensure_private_dir(dir)?;
    }
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("{} is in use by another instance", path.display()),
            ));
        }
        std::fs::remove_file(path)?;
    }
    let l = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(l)
}

/// Accept connections forever on a background thread, forwarding validated
/// calls to `tx`. The handler answers via `Call::reply`.
pub fn serve(listener: UnixListener, tx: async_channel::Sender<Call>) {
    std::thread::Builder::new()
        .name("cmux-ipc".into())
        .spawn(move || {
            let me = unsafe { libc::getuid() };
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                if peer_uid(&stream) != Some(me) {
                    continue; // drop connections from other users
                }
                let tx = tx.clone();
                let _ = std::thread::Builder::new()
                    .name("cmux-ipc-conn".into())
                    .spawn(move || handle_conn(stream, tx));
            }
        })
        .expect("spawn ipc thread");
}

fn handle_conn(stream: UnixStream, tx: async_channel::Sender<Call>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let Ok(mut writer) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        match (&mut reader).take(MAX_REQUEST_BYTES).read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if !line.ends_with('\n') && line.len() as u64 >= MAX_REQUEST_BYTES {
            let _ = writeln!(writer, "{}", serde_json::to_string(&Response::err(0, "request too large")).unwrap());
            return;
        }
        let resp = match parse_request(line.trim()) {
            Err(e) => Response::err(0, e),
            Ok(req) if req.method == "ping" => {
                Response::ok(req.id, serde_json::json!({"pong": true, "version": env!("CARGO_PKG_VERSION")}))
            }
            Ok(req) => {
                let id = req.id;
                let (rtx, rrx) = std::sync::mpsc::channel();
                if tx.send_blocking(Call { req, reply: rtx }).is_err() {
                    Response::err(id, "application is shutting down")
                } else {
                    rrx.recv_timeout(Duration::from_secs(10))
                        .unwrap_or_else(|_| Response::err(id, "timed out waiting for the application"))
                }
            }
        };
        if writeln!(writer, "{}", serde_json::to_string(&resp).unwrap()).is_err() {
            return;
        }
    }
}

// ---- client -----------------------------------------------------------------

pub fn call_at(path: &Path, method: &str, params: Value) -> Result<Value, String> {
    let mut s =
        UnixStream::connect(path).map_err(|e| format!("cannot connect to cmux-linux at {} ({e}). Is the app running?", path.display()))?;
    s.set_read_timeout(Some(Duration::from_secs(15))).ok();
    let req = Request {
        id: 1,
        method: method.into(),
        params,
    };
    writeln!(s, "{}", serde_json::to_string(&req).unwrap()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).map_err(|e| e.to_string())?;
    let resp: Response = serde_json::from_str(&line).map_err(|e| format!("bad response: {e}"))?;
    if resp.ok {
        Ok(resp.result.unwrap_or(Value::Null))
    } else {
        Err(resp.error.unwrap_or_else(|| "unknown error".into()))
    }
}

pub fn call(method: &str, params: Value) -> Result<Value, String> {
    call_at(&crate::paths::socket_path(), method, params)
}

pub fn default_socket() -> PathBuf {
    crate::paths::socket_path()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_validation() {
        assert!(parse_request(r#"{"method":"workspace.list"}"#).is_ok());
        assert!(parse_request(r#"{"method":"rm -rf"}"#).is_err());
        assert!(parse_request(r#"{"method":"notify","params":[1]}"#).is_err());
        assert!(parse_request("garbage").is_err());
        let p = json!({"pane": 3, "name": "x", "bad": -1});
        assert_eq!(param_u64(&p, "pane"), Ok(Some(3)));
        assert!(param_u64(&p, "bad").is_err());
        assert_eq!(param_u64(&p, "none"), Ok(None));
        assert!(require_str(&p, "missing").is_err());
    }

    #[test]
    fn server_roundtrip_and_permissions() {
        let dir = std::env::temp_dir().join(format!("cmux-ipc-{}", std::process::id()));
        let sock = dir.join("t.sock");
        let l = bind(&sock).unwrap();
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(&sock).unwrap().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        // A second instance must refuse to steal the live socket.
        assert!(bind(&sock).is_err());

        let (tx, rx) = async_channel::unbounded::<Call>();
        serve(l, tx);
        std::thread::spawn(move || {
            while let Ok(c) = rx.recv_blocking() {
                let r = if c.req.method == "workspace.list" {
                    Response::ok(c.req.id, json!([]))
                } else {
                    Response::err(c.req.id, "nope")
                };
                c.reply.send(r).unwrap();
            }
        });
        assert_eq!(call_at(&sock, "ping", json!({})).unwrap()["pong"], json!(true));
        assert_eq!(call_at(&sock, "workspace.list", json!({})).unwrap(), json!([]));
        assert_eq!(call_at(&sock, "pane.split", json!({})), Err("nope".into()));
        assert!(call_at(&sock, "evil", json!({})).unwrap_err().contains("unknown method"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stale_socket_is_replaced() {
        let dir = std::env::temp_dir().join(format!("cmux-ipc-stale-{}", std::process::id()));
        let sock = dir.join("s.sock");
        drop(bind(&sock).unwrap()); // listener closed, file remains
        assert!(sock.exists());
        assert!(bind(&sock).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
