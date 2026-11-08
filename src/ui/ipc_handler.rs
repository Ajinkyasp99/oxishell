//! Executes validated IPC requests on the GTK thread.

use super::AppRc;
use crate::agents::AgentState;
use crate::ipc::{self, param_str, param_u64, require_str, Call, Response};
use crate::model::Orient;
use gtk4::glib;
use serde_json::{json, Value};
use std::path::PathBuf;
use vte4::prelude::*;

pub fn start(app: &AppRc) {
    if !app.inner.borrow().config.ipc.enabled {
        return;
    }
    let path = crate::paths::socket_path();
    let listener = match ipc::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            app.toast(&format!("Control socket disabled: {e}"));
            return;
        }
    };
    let (tx, rx) = async_channel::unbounded::<Call>();
    ipc::serve(listener, tx);
    let w = app.weak();
    glib::spawn_future_local(async move {
        while let Ok(call) = rx.recv().await {
            let Some(a) = w.upgrade() else { return };
            let id = call.req.id;
            let resp = match handle(&a, &call.req.method, &call.req.params) {
                Ok(v) => Response::ok(id, v),
                Err(e) => Response::err(id, e),
            };
            let _ = call.reply.send(resp);
        }
    });
    // Remove the socket file on exit.
    app.app.connect_shutdown(move |_| {
        let _ = std::fs::remove_file(&path);
    });
}

fn pane_json(a: &AppRc, pane: u64) -> Value {
    let i = a.inner.borrow();
    let wid = i.state.workspace_of(pane);
    let focused = wid.and_then(|w| i.state.workspace(w)).map(|w| w.focused == pane).unwrap_or(false);
    match i.panes.get(&pane) {
        Some(p) => json!({
            "id": pane,
            "workspace": wid,
            "focused": focused,
            "pid": p.pid.get(),
            "cwd": p.cwd.as_ref().map(|c| c.display().to_string()),
            "title": p.term.window_title().map(|t| t.to_string()),
            "git": p.git,
            "ports": p.ports,
            "agent": p.agent.as_ref().map(|(a, s, m)| json!({"agent": a, "state": s, "message": m})),
            "detected_agent": p.fg_agent,
            "python_env": p.python_env,
            "unread": i.notes.unread_for_pane(pane),
        }),
        None => json!({"id": pane, "workspace": wid}),
    }
}

fn resolve_pane(a: &AppRc, p: &Value) -> Result<u64, String> {
    match param_u64(p, "pane")? {
        Some(id) => {
            if a.inner.borrow().panes.contains_key(&id) {
                Ok(id)
            } else {
                Err(format!("no such pane: {id}"))
            }
        }
        None => a.focused_pane().ok_or_else(|| "no focused pane".into()),
    }
}

fn resolve_ws(a: &AppRc, p: &Value) -> Result<u64, String> {
    match param_u64(p, "workspace")? {
        Some(id) => Ok(id),
        None => a.inner.borrow().state.active.ok_or_else(|| "no active workspace".into()),
    }
}

fn handle(a: &AppRc, method: &str, p: &Value) -> Result<Value, String> {
    match method {
        "workspace.list" => {
            let i = a.inner.borrow();
            let list: Vec<Value> = i
                .state
                .workspaces
                .iter()
                .enumerate()
                .map(|(idx, w)| {
                    let fp = i.panes.get(&w.focused);
                    json!({
                        "id": w.id,
                        "index": idx,
                        "name": w.name,
                        "active": i.state.active == Some(w.id),
                        "panes": w.layout.panes(),
                        "focused_pane": w.focused,
                        "layout": w.layout,
                        "cwd": fp.and_then(|p| p.cwd.as_ref()).map(|c| c.display().to_string()),
                        "branch": fp.and_then(|p| p.git.as_ref()).map(|g| g.branch.clone()),
                        "unread": i.notes.unread_for_workspace(w.id),
                    })
                })
                .collect();
            Ok(Value::Array(list))
        }
        "workspace.create" => {
            let cwd = param_str(p, "cwd")?.map(PathBuf::from);
            if let Some(c) = &cwd {
                if !c.is_dir() {
                    return Err(format!("not a directory: {}", c.display()));
                }
            }
            let (w, pane) = a.new_workspace(param_str(p, "name")?, cwd)?;
            Ok(json!({"workspace": w, "pane": pane}))
        }
        "workspace.select" => {
            let w = resolve_ws(a, p)?;
            if a.inner.borrow().state.workspace(w).is_none() {
                return Err(format!("no such workspace: {w}"));
            }
            a.select_workspace(w);
            Ok(json!({"workspace": w}))
        }
        "workspace.next" | "workspace.prev" => {
            a.run_action(if method == "workspace.next" {
                "next-workspace"
            } else {
                "prev-workspace"
            });
            Ok(json!({"workspace": a.inner.borrow().state.active}))
        }
        "workspace.rename" => {
            let w = resolve_ws(a, p)?;
            a.rename_workspace(w, require_str(p, "name")?)?;
            Ok(json!({"workspace": w}))
        }
        "workspace.move" => {
            let w = resolve_ws(a, p)?;
            let to = param_u64(p, "index")?.ok_or("missing index")? as usize;
            a.move_workspace(w, to)?;
            Ok(json!({"workspace": w}))
        }
        "workspace.close" => {
            let w = resolve_ws(a, p)?;
            let force = p.get("force").and_then(Value::as_bool).unwrap_or(false);
            a.close_workspace(w, force, false)?;
            Ok(json!({"closed": w}))
        }
        "pane.list" => {
            let ws = param_u64(p, "workspace")?;
            let panes: Vec<u64> = a
                .inner
                .borrow()
                .state
                .all_panes()
                .into_iter()
                .filter(|(w, _)| ws.is_none_or(|x| x == *w))
                .map(|(_, p)| p)
                .collect();
            Ok(Value::Array(panes.into_iter().map(|id| pane_json(a, id)).collect()))
        }
        "pane.split" => {
            let pane = resolve_pane(a, p)?;
            let orient = match param_str(p, "direction")?.unwrap_or("right") {
                "right" => Orient::Horizontal,
                "down" => Orient::Vertical,
                d => return Err(format!("direction must be right or down, not {d:?}")),
            };
            let cwd = param_str(p, "cwd")?.map(PathBuf::from);
            let new = a.split(Some(pane), orient, cwd)?;
            Ok(json!({"pane": new}))
        }
        "pane.focus" => {
            let pane = resolve_pane(a, p)?;
            a.focus_pane(pane);
            Ok(json!({"pane": pane}))
        }
        "pane.close" => {
            let pane = resolve_pane(a, p)?;
            let force = p.get("force").and_then(Value::as_bool).unwrap_or(false);
            a.close_pane(pane, force, false)?;
            Ok(json!({"closed": pane}))
        }
        "pane.send" => {
            if !a.inner.borrow().config.ipc.allow_send {
                return Err(
                    "sending input is disabled; enable [ipc] allow_send = true in ~/.config/oxishell/config.toml or in Settings".into(),
                );
            }
            let pane = resolve_pane(a, p)?;
            let text = require_str(p, "text")?;
            if text.len() > ipc::MAX_SEND_BYTES {
                return Err(format!("text exceeds {} bytes", ipc::MAX_SEND_BYTES));
            }
            a.inner
                .borrow()
                .panes
                .get(&pane)
                .ok_or("pane vanished")?
                .send_input(text.as_bytes());
            Ok(json!({"pane": pane, "bytes": text.len()}))
        }
        "pane.read" => {
            let pane = resolve_pane(a, p)?;
            let lines = param_u64(p, "lines")?.unwrap_or(200).min(100_000) as usize;
            let text = a
                .inner
                .borrow()
                .panes
                .get(&pane)
                .map(|p| p.scrollback_text(lines))
                .unwrap_or_default();
            Ok(json!({"pane": pane, "text": text}))
        }
        "notify" => {
            let pane = match param_u64(p, "pane")? {
                Some(id) if a.inner.borrow().panes.contains_key(&id) => Some(id),
                Some(id) => return Err(format!("no such pane: {id}")),
                None => None,
            };
            let title = param_str(p, "title")?.unwrap_or("Notification");
            let body = param_str(p, "body")?.unwrap_or("");
            let id = a.notify(pane, title, body, "cli");
            Ok(json!({"notification": id}))
        }
        "agent.usage" => {
            let pane = param_u64(p, "pane")?.ok_or("missing pane")?;
            if !a.inner.borrow().panes.contains_key(&pane) {
                return Err(format!("no such pane: {pane}"));
            }
            a.usage_footer.claude.borrow_mut().update(pane, crate::usage::claude(p));
            super::usage_footer::refresh(a);
            Ok(json!({"pane": pane}))
        }
        "agent.event" => {
            let pane = param_u64(p, "pane")?.ok_or("missing pane (run inside an oxishell pane)")?;
            let agent = require_str(p, "agent")?;
            if agent.is_empty() || agent.len() > 32 || !agent.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return Err("invalid agent name".into());
            }
            let state = AgentState::parse(require_str(p, "state")?).ok_or("state must be running, waiting, idle or ended")?;
            let message = param_str(p, "message")?
                .map(|m| crate::notify::sanitize(m, crate::notify::MAX_BODY))
                .filter(|m| !m.is_empty());
            let session = param_str(p, "session")?
                .map(str::to_string)
                .filter(|s| crate::agents::resume_command(agent, s).is_some());
            a.agent_event(pane, agent, state, message, session)?;
            Ok(json!({"pane": pane}))
        }
        "notification.list" => {
            let i = a.inner.borrow();
            Ok(serde_json::to_value(i.notes.all()).unwrap())
        }
        "notification.clear" => {
            a.inner.borrow_mut().notes.clear();
            a.refresh();
            Ok(json!({}))
        }
        "notification.jump" => Ok(json!({"pane": a.jump_to_unread()})),
        other => Err(format!("unhandled method {other}")),
    }
}
