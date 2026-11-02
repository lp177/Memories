//! MCP server: stateless streamable-HTTP JSON-RPC at POST /mcp.
//!
//! Tool-based wrapper of the file API so LLM agents can browse, read, write
//! and watch memories. Auth: `Authorization: Bearer <api token>` — the same
//! tokens as the REST API; `write_*`/`delete_*` tools need readwrite scope.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::api::{authenticate, ApiError, Authed, SharedState};
use crate::db::Db;
use crate::scriber::{ScriberClient, ScriberError};
use crate::store::{clean_path, Access, Store};

const PROTOCOL_VERSION: &str = "2025-03-26";

pub fn router(state: SharedState) -> Router {
    Router::new().route("/mcp", post(handle)).with_state(state)
}

fn rpc_result(id: Value, result: Value) -> Json<Value> {
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn rpc_error(id: Value, code: i64, message: &str) -> Json<Value> {
    Json(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }))
}

fn tool_text(text: String, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

async fn handle(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(msg): Json<Value>,
) -> Result<impl IntoResponse, ApiError> {
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");

    // Notifications need no auth handshake response.
    if method.starts_with("notifications/") {
        return Ok((StatusCode::ACCEPTED, Json(json!({}))).into_response());
    }

    let authed = match authenticate(&state, &headers, None, None) {
        Ok(a) => a,
        Err(e) => {
            // JSON-RPC transport still answers 200 with an error object for
            // protocol-level calls, but auth failures use HTTP 401 per spec.
            return Err(e);
        }
    };

    let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
    let response = match method {
        "initialize" => rpc_result(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "memories", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Shared memory store. Best practice: call get_changes for your \
                    project before starting work (and periodically) to stay in sync with \
                    coworkers, and push your own memory updates with write_memory regularly."
            }),
        ),
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(id, json!({ "tools": tool_definitions() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            match call_tool(&state, &authed, name, &args).await {
                Ok(v) => rpc_result(id, v),
                Err(text) => rpc_result(id, tool_text(text, true)),
            }
        }
        _ => rpc_error(id, -32601, &format!("Method not found: {method}")),
    };
    Ok(response.into_response())
}

fn tool_definitions() -> Vec<Value> {
    let path_prop = json!({ "type": "string", "description": "Path like 'project/user/notes.md' ('' = root)" });
    vec![
        json!({ "name": "whoami", "description": "The authenticated user and token scope.",
                "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "list_projects", "description": "List top-level projects you can access.",
                "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "list_dir", "description": "List a directory (projects, user spaces, folders, files).",
                "inputSchema": { "type": "object", "properties": { "path": path_prop },
                                  "required": [] } }),
        json!({ "name": "read_memory", "description": "Read a memory (markdown file) at a path.",
                "inputSchema": { "type": "object", "properties": { "path": path_prop },
                                  "required": ["path"] } }),
        json!({ "name": "write_memory", "description": "Create or overwrite a memory file (readwrite scope). Paths are 'project/your-username/...'.",
                "inputSchema": { "type": "object", "properties": {
                    "path": path_prop, "content": { "type": "string" } },
                    "required": ["path", "content"] } }),
        json!({ "name": "mkdir", "description": "Create a folder (creating 'project' creates project/your-username).",
                "inputSchema": { "type": "object", "properties": { "path": path_prop },
                                  "required": ["path"] } }),
        json!({ "name": "delete", "description": "Delete files/folders (readwrite scope).",
                "inputSchema": { "type": "object", "properties": {
                    "paths": { "type": "array", "items": { "type": "string" } } },
                    "required": ["paths"] } }),
        json!({ "name": "move", "description": "Move items into a destination folder (readwrite scope).",
                "inputSchema": { "type": "object", "properties": {
                    "paths": { "type": "array", "items": { "type": "string" } },
                    "dest": { "type": "string" } }, "required": ["paths", "dest"] } }),
        json!({ "name": "get_changes", "description": "Change events for a project after a sequence number. Call with since=0 first, then poll with the returned latest_seq to stay in sync with coworkers.",
                "inputSchema": { "type": "object", "properties": {
                    "project": { "type": "string" },
                    "since": { "type": "integer", "default": 0 } },
                    "required": ["project"] } }),
    ]
}

async fn call_tool(
    state: &SharedState,
    a: &Authed,
    name: &str,
    args: &Value,
) -> Result<Value, String> {
    let arg_str = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let need_write = || -> Result<(), String> {
        if a.can_write() {
            Ok(())
        } else {
            Err("This API token is read-only (readwrite scope needed).".into())
        }
    };
    let g = {
        let gids = state.db.user_group_ids(&a.user.id);
        state.db.grants_for_user(&a.user.id, &gids)
    };
    let parse = |p: &str| clean_path(p).ok_or_else(|| format!("Invalid path: {p}"));

    match name {
        "whoami" => Ok(tool_text(
            json!({
                "username": a.user.username,
                "is_admin": a.user.is_admin,
                "scope": a.token_scope.clone().unwrap_or_else(|| "session".into()),
            })
            .to_string(),
            false,
        )),
        "list_projects" | "list_dir" => {
            let path = if name == "list_projects" { String::new() } else { arg_str("path") };
            let segs = parse(&path)?;
            let entries = if scriber_path(state, &segs) {
                if !Store::mount_visible(a.user.is_admin, &g, &segs) {
                    return Err("No access to this path".into());
                }
                crate::api::scriber_entries(state, a, &g, &segs)
                    .await?
                    .ok_or_else(|| "Not found".to_string())?
            } else {
                if !state.store.visible(&a.user.username, a.user.is_admin, &g, &segs) {
                    return Err("No access to this path".into());
                }
                let mut list = state
                    .store
                    .list(&segs, &a.user.username, a.user.is_admin, &g, &state.db)
                    .map_err(|e| e.to_string())?;
                if segs.is_empty() && crate::api::scriber_visible(state, a, &g) {
                    list.push(crate::api::scriber_root_entry(state, a, &g));
                    crate::store::sort_entries(&mut list);
                }
                list
            };
            Ok(tool_text(serde_json::to_string_pretty(&entries).unwrap(), false))
        }
        "read_memory" => {
            let segs = parse(&arg_str("path"))?;
            if scriber_path(state, &segs) {
                if crate::api::scriber_access(state, a, &g, &segs) == Access::None {
                    return Err("No access to this path".into());
                }
                let file = ScriberClient::classify_file(&segs)
                    .ok_or_else(|| mount_path_hint(&state.cfg.scriber_mount))?;
                let content = state
                    .scriber
                    .as_ref()
                    .unwrap()
                    .read_mount_file(&file)
                    .await
                    .map_err(|e| match e {
                        ScriberError::NotFound => "File not found".to_string(),
                        ScriberError::Upstream(e) => format!("scriber: {e}"),
                    })?;
                return Ok(tool_text(content, false));
            }
            if Store::access_for(&a.user.username, a.user.is_admin, &g, &segs) == Access::None {
                return Err("No access to this path".into());
            }
            state
                .store
                .read_file(&segs)
                .map(|c| tool_text(c, false))
                .map_err(|_| "File not found".into())
        }
        "write_memory" => {
            need_write()?;
            let segs = parse(&arg_str("path"))?;
            let content = arg_str("content");
            if scriber_path(state, &segs) {
                if crate::api::scriber_access(state, a, &g, &segs) != Access::ReadWrite {
                    return Err(crate::api::scriber_write_denied(state).into());
                }
                let file = ScriberClient::classify_file(&segs)
                    .ok_or_else(|| mount_path_hint(&state.cfg.scriber_mount))?;
                state
                    .scriber
                    .as_ref()
                    .unwrap()
                    .write_mount_file(&file, &content)
                    .await
                    .map_err(|e| match e {
                        ScriberError::NotFound => "File not found".to_string(),
                        ScriberError::Upstream(e) => format!("scriber: {e}"),
                    })?;
                record_event(state, "updated", &segs, &a.user.username);
                return Ok(tool_text("written".into(), false));
            }
            if segs.len() < 2 {
                return Err("Files live inside a project: project/your-username/file.md".into());
            }
            if Store::access_for(&a.user.username, a.user.is_admin, &g, &segs) != Access::ReadWrite {
                return Err("No write access to this path".into());
            }
            let created = state.store.write_file(&segs, &content).map_err(|e| e.to_string())?;
            record_event(state, if created { "created" } else { "updated" }, &segs, &a.user.username);
            Ok(tool_text(if created { "created" } else { "updated" }.into(), false))
        }
        "mkdir" => {
            need_write()?;
            let mut segs = parse(&arg_str("path"))?;
            if segs.is_empty() {
                return Err("Missing path".into());
            }
            if scriber_path(state, &segs) {
                return Err("The scriber project is managed externally".into());
            }
            if segs.len() == 1 && !a.user.is_admin {
                segs.push(a.user.username.clone());
            }
            if Store::access_for(&a.user.username, a.user.is_admin, &g, &segs) != Access::ReadWrite {
                return Err("No write access to this path".into());
            }
            state.store.mkdir(&segs).map_err(|e| e.to_string())?;
            record_event(state, "created", &segs, &a.user.username);
            Ok(tool_text(segs.join("/"), false))
        }
        "delete" => {
            need_write()?;
            let paths = args
                .get("paths")
                .and_then(Value::as_array)
                .ok_or("paths must be an array")?;
            let mut out = Vec::new();
            for p in paths {
                let p = p.as_str().ok_or("paths must be strings")?;
                let segs = parse(p)?;
                if segs.is_empty() || scriber_path(state, &segs) {
                    return Err(format!("{p} cannot be deleted"));
                }
                if Store::access_for(&a.user.username, a.user.is_admin, &g, &segs) != Access::ReadWrite {
                    return Err(format!("No write access to {p}"));
                }
                state.store.delete(&segs).map_err(|e| e.to_string())?;
                state.db.delete_shares_under(&segs.join("/"));
                record_event(state, "deleted", &segs, &a.user.username);
                out.push(p.to_string());
            }
            Ok(tool_text(format!("deleted: {}", out.join(", ")), false))
        }
        "move" => {
            need_write()?;
            let dest = parse(&arg_str("dest"))?;
            if scriber_path(state, &dest) {
                return Err("The scriber project is managed externally".into());
            }
            if !dest.is_empty()
                && Store::access_for(&a.user.username, a.user.is_admin, &g, &dest) != Access::ReadWrite
            {
                return Err("No write access to the destination".into());
            }
            let paths = args
                .get("paths")
                .and_then(Value::as_array)
                .ok_or("paths must be an array")?;
            let mut moved = Vec::new();
            for p in paths {
                let p = p.as_str().ok_or("paths must be strings")?;
                let segs = parse(p)?;
                if segs.is_empty() || scriber_path(state, &segs) {
                    return Err(format!("{p} cannot be moved"));
                }
                if Store::access_for(&a.user.username, a.user.is_admin, &g, &segs) != Access::ReadWrite {
                    return Err(format!("No write access to {p}"));
                }
                let mut to = dest.clone();
                to.push(segs.last().unwrap().clone());
                if to.len() < 2 && !a.user.is_admin {
                    return Err("Destination must be inside a project".into());
                }
                let (src_j, dst_j) = (segs.join("/"), to.join("/"));
                if dst_j.starts_with(&format!("{src_j}/")) {
                    return Err("Cannot move a folder into itself".into());
                }
                if state.store.exists(&to) {
                    return Err(format!("{dst_j} already exists"));
                }
                state.store.rename(&segs, &to).map_err(|e| e.to_string())?;
                state.db.rewrite_share_paths(&src_j, &dst_j);
                record_event(state, "moved", &segs, &a.user.username);
                moved.push(dst_j);
            }
            Ok(tool_text(format!("moved to: {}", moved.join(", ")), false))
        }
        "get_changes" => {
            let project = arg_str("project");
            let since = args.get("since").and_then(Value::as_i64).unwrap_or(0);
            let segs = parse(&project)?;
            if segs.len() != 1 {
                return Err("project must be a top-level name".into());
            }
            let visible = if scriber_path(state, &segs) {
                Store::mount_visible(a.user.is_admin, &g, &segs)
            } else {
                state.store.visible(&a.user.username, a.user.is_admin, &g, &segs)
            };
            if !visible {
                return Err("No access to this project".into());
            }
            let events = state.db.events_since(&segs[0], since, 200);
            let latest = events
                .last()
                .map(|e| e.seq)
                .unwrap_or_else(|| state.db.latest_seq().max(since));
            Ok(tool_text(
                json!({ "events": events, "latest_seq": latest }).to_string(),
                false,
            ))
        }
        other => Err(format!("Unknown tool: {other}")),
    }
}

fn scriber_path(state: &SharedState, segs: &[String]) -> bool {
    state.scriber.is_some() && !segs.is_empty() && segs[0] == state.cfg.scriber_mount
}

fn mount_path_hint(mount: &str) -> String {
    format!("{mount} paths look like {mount}/<participant>/memory.md or {mount}/meetings/<meeting>.md")
}

fn record_event(state: &SharedState, kind: &str, segs: &[String], actor: &str) {
    if segs.is_empty() {
        return;
    }
    let event = state.db.record_event(&segs[0], kind, &segs.join("/"), None, actor);
    state.hub.publish(event);
}

// Re-export used by api.rs auth (Db unused warning guard).
#[allow(unused)]
fn _t(_: &Db) {}
