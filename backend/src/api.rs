//! REST API. One surface for both the web app (session bearer) and
//! automation/LLMs (API-token bearer) — the auth extractor accepts either.

use std::sync::Arc;

use axum::extract::{Multipart, Path as AxPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use futures::stream::Stream;
use serde::Deserialize;
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::auth;
use crate::config::Config;
use crate::db::{Db, User};
use crate::events::EventHub;
use crate::mail;
use crate::scriber::{ScriberClient, ScriberError};
use crate::store::{clean_path, sort_entries, Access, Entry, Store};

pub struct AppState {
    pub cfg: Config,
    pub db: Db,
    pub store: Store,
    pub hub: EventHub,
    pub scriber: Option<ScriberClient>,
}

pub type SharedState = Arc<AppState>;

// ---------------------------------------------------------------------------
// Errors & auth
// ---------------------------------------------------------------------------

pub struct ApiError(pub StatusCode, pub String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "detail": self.1 }))).into_response()
    }
}

fn err(code: StatusCode, msg: &str) -> ApiError {
    ApiError(code, msg.to_string())
}

#[derive(Clone)]
pub struct Authed {
    pub user: User,
    /// Some(scope) when authenticated with an API token instead of a session.
    pub token_scope: Option<String>,
}

impl Authed {
    pub fn can_write(&self) -> bool {
        match &self.token_scope {
            Some(scope) => scope == "readwrite",
            None => true,
        }
    }
}

/// Authenticate from `Authorization: Bearer` (session or API token) or, for
/// SSE/EventSource which cannot set headers, `?access_token=`.
pub fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    query_token: Option<&str>,
    ip: Option<&str>,
) -> Result<Authed, ApiError> {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .or_else(|| query_token.map(str::to_string));
    let Some(token) = bearer else {
        return Err(err(StatusCode::UNAUTHORIZED, "Not authenticated"));
    };
    // Session token?
    if let Some(user_id) = auth::verify_session(&token, &state.cfg.web_secret) {
        if let Some(user) = state.db.user_by_id(&user_id) {
            return Ok(Authed { user, token_scope: None });
        }
    }
    // API token?
    if let Some((tok, user_id)) = state.db.api_token_by_hash(&auth::hash_token(&token)) {
        if let Some(user) = state.db.user_by_id(&user_id) {
            state.db.touch_api_token(&tok.id, ip);
            return Ok(Authed { user, token_scope: Some(tok.scope) });
        }
    }
    Err(err(StatusCode::UNAUTHORIZED, "Invalid or expired token"))
}

fn require(
    state: &AppState,
    headers: &HeaderMap,
    ip: Option<&str>,
) -> Result<Authed, ApiError> {
    authenticate(state, headers, None, ip)
}

fn require_admin(a: &Authed) -> Result<(), ApiError> {
    if a.user.is_admin {
        Ok(())
    } else {
        Err(err(StatusCode::FORBIDDEN, "Admin only"))
    }
}

fn require_write_scope(a: &Authed) -> Result<(), ApiError> {
    if a.can_write() {
        Ok(())
    } else {
        Err(err(StatusCode::FORBIDDEN, "This API token is read-only"))
    }
}

fn grants(state: &AppState, a: &Authed) -> Vec<(String, String)> {
    let gids = state.db.user_group_ids(&a.user.id);
    state.db.grants_for_user(&a.user.id, &gids)
}

fn parse_path(p: &str) -> Result<Vec<String>, ApiError> {
    clean_path(p).ok_or_else(|| err(StatusCode::BAD_REQUEST, "Invalid path"))
}

/// Best-effort event record + broadcast.
fn record(state: &AppState, kind: &str, segs: &[String], new_path: Option<&str>, actor: &str) {
    if segs.is_empty() {
        return;
    }
    let event = state
        .db
        .record_event(&segs[0], kind, &segs.join("/"), new_path, actor);
    state.hub.publish(event);
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router(state: SharedState) -> Router {
    Router::new()
        // auth & profile
        .route("/api/login", post(login))
        .route("/api/forgot", post(forgot))
        .route("/api/reset", post(reset_password))
        .route("/api/me", get(me).put(update_me))
        .route("/api/me/avatar", post(upload_avatar).delete(delete_avatar))
        .route("/api/users/{id}/avatar", get(user_avatar))
        .route("/api/directory", get(directory))
        // api tokens
        .route("/api/tokens", get(list_tokens).post(create_token))
        .route("/api/tokens/{id}", delete(delete_token))
        // admin
        .route("/api/admin/users", get(admin_list_users).post(admin_create_user))
        .route(
            "/api/admin/users/{id}",
            put(admin_update_user).delete(admin_delete_user),
        )
        .route("/api/admin/users/{id}/reset-link", post(admin_reset_link))
        .route("/api/admin/groups", get(admin_list_groups).post(admin_create_group))
        .route(
            "/api/admin/groups/{id}",
            put(admin_update_group).delete(admin_delete_group),
        )
        // file system
        .route("/api/fs/list", get(fs_list))
        .route("/api/fs/file", get(fs_read).put(fs_write))
        .route("/api/fs/mkdir", post(fs_mkdir))
        .route("/api/fs/delete", post(fs_delete))
        .route("/api/fs/rename", post(fs_rename))
        .route("/api/fs/move", post(fs_move))
        .route("/api/fs/copy", post(fs_copy))
        // sharing
        .route("/api/shares", get(shares_list).post(shares_create))
        .route("/api/shares/{id}", delete(shares_delete))
        // events
        .route("/api/events", get(events_poll))
        .route("/api/events/stream", get(events_stream))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Auth & profile handlers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
}

async fn login(
    State(state): State<SharedState>,
    Json(body): Json<LoginBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = state
        .db
        .user_by_username(body.username.trim())
        .filter(|u| auth::verify_password(&body.password, &u.password_hash))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "Invalid username or password"))?;
    let token = auth::create_session(&user.id, &state.cfg.web_secret, 86400 * 7);
    Ok(Json(json!({ "token": token, "user": user })))
}

#[derive(Deserialize)]
struct ForgotBody {
    username_or_email: String,
}

async fn forgot(
    State(state): State<SharedState>,
    Json(body): Json<ForgotBody>,
) -> Json<serde_json::Value> {
    let needle = body.username_or_email.trim().to_string();
    let user = state
        .db
        .list_users()
        .into_iter()
        .find(|u| u.username == needle || (!u.email.is_empty() && u.email == needle));
    if let Some(user) = user {
        let (link, _) = make_reset_link(&state, &user);
        let mailed = mail::send_reset_link(&state.cfg, &user.email, &link).await;
        if !mailed {
            tracing::info!("Password reset link for {}: {}", user.username, link);
        }
    }
    // Always the same answer — do not leak account existence.
    Json(json!({ "ok": true }))
}

fn make_reset_link(state: &AppState, user: &User) -> (String, String) {
    let token = auth::generate_api_token();
    let expires = (chrono::Utc::now() + chrono::Duration::hours(1))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    state
        .db
        .create_password_reset(&user.id, &auth::hash_token(&token), &expires);
    (
        format!("{}/#/reset?token={}", state.cfg.public_url.trim_end_matches('/'), token),
        token,
    )
}

#[derive(Deserialize)]
struct ResetBody {
    token: String,
    password: String,
}

async fn reset_password(
    State(state): State<SharedState>,
    Json(body): Json<ResetBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.password.len() < 8 {
        return Err(err(StatusCode::BAD_REQUEST, "Password must be at least 8 characters"));
    }
    let user_id = state
        .db
        .consume_password_reset(&auth::hash_token(body.token.trim()))
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Invalid or expired reset link"))?;
    state
        .db
        .update_user(&user_id, None, None, Some(&auth::hash_password(&body.password)), None)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    Ok(Json(json!({ "ok": true })))
}

async fn me(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    Ok(Json(json!({ "user": a.user, "via_api_token": a.token_scope })))
}

#[derive(Deserialize)]
struct UpdateMeBody {
    username: Option<String>,
    email: Option<String>,
    password: Option<String>,
    current_password: Option<String>,
}

async fn update_me(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<UpdateMeBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    if a.token_scope.is_some() {
        return Err(err(StatusCode::FORBIDDEN, "Profile edits need a browser session"));
    }
    if let Some(pw) = &body.password {
        let current = body.current_password.clone().unwrap_or_default();
        if !auth::verify_password(&current, &a.user.password_hash) {
            return Err(err(StatusCode::BAD_REQUEST, "Current password is incorrect"));
        }
        if pw.len() < 8 {
            return Err(err(StatusCode::BAD_REQUEST, "Password must be at least 8 characters"));
        }
    }
    let username = body.username.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if let Some(name) = username {
        if let Some(other) = state.db.user_by_username(name) {
            if other.id != a.user.id {
                return Err(err(StatusCode::CONFLICT, "Username already taken"));
            }
        }
    }
    let hash = body.password.as_deref().map(auth::hash_password);
    state
        .db
        .update_user(
            &a.user.id,
            username,
            body.email.as_deref().map(str::trim),
            hash.as_deref(),
            None,
        )
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    Ok(Json(json!({ "ok": true, "user": state.db.user_by_id(&a.user.id) })))
}

// ---- avatars --------------------------------------------------------------

const AVATAR_TYPES: &[(&str, &str)] = &[
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/gif", "gif"),
    ("image/webp", "webp"),
];

fn avatar_path(state: &AppState, user_id: &str, ext: &str) -> std::path::PathBuf {
    state.cfg.data_dir.join("avatars").join(format!("{user_id}.{ext}"))
}

async fn upload_avatar(
    State(state): State<SharedState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Bad upload"))?
    {
        let content_type = field.content_type().unwrap_or("").to_string();
        let Some((_, ext)) = AVATAR_TYPES.iter().find(|(t, _)| *t == content_type) else {
            continue;
        };
        let data = field
            .bytes()
            .await
            .map_err(|_| err(StatusCode::BAD_REQUEST, "Bad upload"))?;
        if data.len() > 5 * 1024 * 1024 {
            return Err(err(StatusCode::PAYLOAD_TOO_LARGE, "Avatar exceeds the 5 MB limit"));
        }
        // Remove any prior avatar under a different extension.
        for (_, other) in AVATAR_TYPES {
            let _ = std::fs::remove_file(avatar_path(&state, &a.user.id, other));
        }
        let path = avatar_path(&state, &a.user.id, ext);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&path, &data)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
        state.db.set_avatar_ext(&a.user.id, Some(ext));
        return Ok(Json(json!({ "ok": true })));
    }
    Err(err(StatusCode::BAD_REQUEST, "Unsupported image type"))
}

async fn delete_avatar(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    for (_, ext) in AVATAR_TYPES {
        let _ = std::fs::remove_file(avatar_path(&state, &a.user.id, ext));
    }
    state.db.set_avatar_ext(&a.user.id, None);
    Ok(Json(json!({ "ok": true })))
}

async fn user_avatar(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxPath(id): AxPath<String>,
) -> Result<Response, ApiError> {
    require(&state, &headers, None)?;
    let ext = state
        .db
        .avatar_ext(&id)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "No avatar"))?;
    let path = avatar_path(&state, &id, &ext);
    let bytes =
        std::fs::read(&path).map_err(|_| err(StatusCode::NOT_FOUND, "No avatar"))?;
    let mime = AVATAR_TYPES
        .iter()
        .find(|(_, e)| *e == ext)
        .map(|(t, _)| *t)
        .unwrap_or("application/octet-stream");
    Ok(([(axum::http::header::CONTENT_TYPE, mime)], bytes).into_response())
}

/// Users + groups directory for the share dialog (id + display names only).
async fn directory(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    require(&state, &headers, None)?;
    let users: Vec<_> = state
        .db
        .list_users()
        .into_iter()
        .map(|u| json!({ "id": u.id, "username": u.username, "has_avatar": u.has_avatar }))
        .collect();
    let groups: Vec<_> = state
        .db
        .list_groups()
        .into_iter()
        .map(|g| json!({ "id": g.id, "name": g.name }))
        .collect();
    Ok(Json(json!({ "users": users, "groups": groups })))
}

// ---------------------------------------------------------------------------
// API tokens
// ---------------------------------------------------------------------------

async fn list_tokens(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    Ok(Json(json!({ "tokens": state.db.list_api_tokens(&a.user.id) })))
}

#[derive(Deserialize)]
struct CreateTokenBody {
    name: String,
    #[serde(default)]
    scope: Option<String>,
}

async fn create_token(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<CreateTokenBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    if a.token_scope.is_some() {
        return Err(err(StatusCode::FORBIDDEN, "Token management needs a browser session"));
    }
    let name = body.name.trim();
    if name.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Token name is required"));
    }
    let scope = match body.scope.as_deref().unwrap_or("read") {
        "read" => "read",
        "readwrite" => "readwrite",
        _ => return Err(err(StatusCode::BAD_REQUEST, "scope must be read or readwrite")),
    };
    let token = auth::generate_api_token();
    let row = state
        .db
        .create_api_token(
            &a.user.id,
            name,
            &auth::token_prefix(&token),
            &auth::hash_token(&token),
            scope,
        )
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    Ok(Json(json!({ "token": token, "api_token": row })))
}

async fn delete_token(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxPath(id): AxPath<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    if state.db.delete_api_token(&a.user.id, &id) {
        Ok(Json(json!({ "ok": true })))
    } else {
        Err(err(StatusCode::NOT_FOUND, "Token not found"))
    }
}

// ---------------------------------------------------------------------------
// Admin: users & groups
// ---------------------------------------------------------------------------

async fn admin_list_users(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    require_admin(&a)?;
    Ok(Json(json!({ "users": state.db.list_users() })))
}

#[derive(Deserialize)]
struct AdminCreateUserBody {
    username: String,
    #[serde(default)]
    email: String,
    password: String,
    #[serde(default)]
    is_admin: bool,
}

async fn admin_create_user(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<AdminCreateUserBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    require_admin(&a)?;
    let username = body.username.trim();
    if username.is_empty() || username.contains(['/', '\\']) || username.starts_with('.') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid username"));
    }
    if body.password.len() < 8 {
        return Err(err(StatusCode::BAD_REQUEST, "Password must be at least 8 characters"));
    }
    if state.db.user_by_username(username).is_some() {
        return Err(err(StatusCode::CONFLICT, "Username already taken"));
    }
    let user = state
        .db
        .create_user(username, body.email.trim(), &auth::hash_password(&body.password), body.is_admin)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    Ok(Json(json!({ "ok": true, "user": user })))
}

#[derive(Deserialize)]
struct AdminUpdateUserBody {
    username: Option<String>,
    email: Option<String>,
    password: Option<String>,
    is_admin: Option<bool>,
}

async fn admin_update_user(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxPath(id): AxPath<String>,
    Json(body): Json<AdminUpdateUserBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    require_admin(&a)?;
    if state.db.user_by_id(&id).is_none() {
        return Err(err(StatusCode::NOT_FOUND, "User not found"));
    }
    if let Some(pw) = &body.password {
        if pw.len() < 8 {
            return Err(err(StatusCode::BAD_REQUEST, "Password must be at least 8 characters"));
        }
    }
    let username = body.username.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if let Some(name) = username {
        if let Some(other) = state.db.user_by_username(name) {
            if other.id != id {
                return Err(err(StatusCode::CONFLICT, "Username already taken"));
            }
        }
    }
    let hash = body.password.as_deref().map(auth::hash_password);
    state
        .db
        .update_user(&id, username, body.email.as_deref().map(str::trim), hash.as_deref(), body.is_admin)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    Ok(Json(json!({ "ok": true, "user": state.db.user_by_id(&id) })))
}

async fn admin_delete_user(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxPath(id): AxPath<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    require_admin(&a)?;
    if id == a.user.id {
        return Err(err(StatusCode::BAD_REQUEST, "You cannot delete your own account"));
    }
    state.db.delete_user(&id);
    Ok(Json(json!({ "ok": true })))
}

async fn admin_reset_link(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxPath(id): AxPath<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    require_admin(&a)?;
    let user = state
        .db
        .user_by_id(&id)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "User not found"))?;
    let (link, _) = make_reset_link(&state, &user);
    let mailed = mail::send_reset_link(&state.cfg, &user.email, &link).await;
    Ok(Json(json!({ "ok": true, "link": link, "mailed": mailed })))
}

async fn admin_list_groups(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    require_admin(&a)?;
    Ok(Json(json!({ "groups": state.db.list_groups() })))
}

#[derive(Deserialize)]
struct GroupBody {
    name: Option<String>,
    members: Option<Vec<String>>,
}

async fn admin_create_group(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<GroupBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    require_admin(&a)?;
    let name = body.name.as_deref().map(str::trim).unwrap_or("");
    if name.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Group name is required"));
    }
    let id = state
        .db
        .create_group(name)
        .map_err(|_| err(StatusCode::CONFLICT, "Group name already taken"))?;
    if let Some(members) = &body.members {
        state.db.set_group_members(&id, members);
    }
    Ok(Json(json!({ "ok": true, "id": id })))
}

async fn admin_update_group(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxPath(id): AxPath<String>,
    Json(body): Json<GroupBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    require_admin(&a)?;
    if let Some(name) = body.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        state
            .db
            .rename_group(&id, name)
            .map_err(|_| err(StatusCode::CONFLICT, "Group name already taken"))?;
    }
    if let Some(members) = &body.members {
        state.db.set_group_members(&id, members);
    }
    Ok(Json(json!({ "ok": true })))
}

async fn admin_delete_group(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxPath(id): AxPath<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    require_admin(&a)?;
    state.db.delete_group(&id);
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// File system
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PathQuery {
    #[serde(default)]
    path: String,
}

fn is_scriber(state: &AppState, segs: &[String]) -> bool {
    state.scriber.is_some() && !segs.is_empty() && segs[0] == state.cfg.scriber_mount
}

async fn fs_list(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    let segs = parse_path(&q.path)?;
    let g = grants(&state, &a);

    // Virtual scriber project.
    if is_scriber(&state, &segs) {
        let client = state.scriber.as_ref().unwrap();
        let entries = scriber_list(client, &segs, &a).await?;
        return Ok(Json(json!({ "path": segs.join("/"), "entries": entries })));
    }

    if !state.store.visible(&a.user.username, a.user.is_admin, &g, &segs) {
        return Err(err(StatusCode::FORBIDDEN, "No access to this path"));
    }
    let mut entries = state
        .store
        .list(&segs, &a.user.username, a.user.is_admin, &g, &state.db)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // At root, expose the virtual scriber project to every signed-in user.
    if segs.is_empty() {
        if let Some(client) = &state.scriber {
            entries.push(Entry {
                name: state.cfg.scriber_mount.clone(),
                path: state.cfg.scriber_mount.clone(),
                is_dir: true,
                size: 0,
                modified_at: None,
                access: if a.can_write() { "readwrite" } else { "read" }.into(),
                shared: false,
            });
            let _ = client; // client used above for presence only
            sort_entries(&mut entries);
        }
    }
    Ok(Json(json!({ "path": segs.join("/"), "entries": entries })))
}

async fn scriber_list(
    client: &ScriberClient,
    segs: &[String],
    a: &Authed,
) -> Result<Vec<Entry>, ApiError> {
    let access = if a.can_write() { "readwrite" } else { "read" };
    client
        .mount_entries(segs, access)
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("scriber: {e}")))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Not found"))
}

async fn fs_read(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    let segs = parse_path(&q.path)?;
    if segs.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Missing path"));
    }
    if is_scriber(&state, &segs) {
        let file = ScriberClient::classify_file(&segs)
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "Not found"))?;
        let content = state
            .scriber
            .as_ref()
            .unwrap()
            .read_mount_file(&file)
            .await
            .map_err(|e| match e {
                ScriberError::NotFound => err(StatusCode::NOT_FOUND, "File not found"),
                ScriberError::Upstream(e) => err(StatusCode::BAD_GATEWAY, &format!("scriber: {e}")),
            })?;
        return Ok(Json(json!({ "path": segs.join("/"), "content": content })));
    }
    let g = grants(&state, &a);
    if Store::access_for(&a.user.username, a.user.is_admin, &g, &segs) == Access::None {
        return Err(err(StatusCode::FORBIDDEN, "No access to this path"));
    }
    let content = state
        .store
        .read_file(&segs)
        .map_err(|_| err(StatusCode::NOT_FOUND, "File not found"))?;
    Ok(Json(json!({ "path": segs.join("/"), "content": content })))
}

#[derive(Deserialize)]
struct WriteBody {
    path: String,
    content: String,
}

async fn fs_write(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<WriteBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    require_write_scope(&a)?;
    let segs = parse_path(&body.path)?;
    if segs.len() < 2 {
        return Err(err(StatusCode::BAD_REQUEST, "Files live inside a project"));
    }
    if is_scriber(&state, &segs) {
        let file = ScriberClient::classify_file(&segs).ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                &format!(
                    "Only memory.md and meeting summary files can be written in {}",
                    state.cfg.scriber_mount
                ),
            )
        })?;
        state
            .scriber
            .as_ref()
            .unwrap()
            .write_mount_file(&file, &body.content)
            .await
            .map_err(|e| match e {
                ScriberError::NotFound => err(StatusCode::NOT_FOUND, "File not found"),
                ScriberError::Upstream(e) => err(StatusCode::BAD_GATEWAY, &format!("scriber: {e}")),
            })?;
        record(&state, "updated", &segs, None, &a.user.username);
        return Ok(Json(json!({ "ok": true })));
    }
    let g = grants(&state, &a);
    if Store::access_for(&a.user.username, a.user.is_admin, &g, &segs) != Access::ReadWrite {
        return Err(err(StatusCode::FORBIDDEN, "No write access to this path"));
    }
    let created = state
        .store
        .write_file(&segs, &body.content)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    record(
        &state,
        if created { "created" } else { "updated" },
        &segs,
        None,
        &a.user.username,
    );
    Ok(Json(json!({ "ok": true, "created": created })))
}

#[derive(Deserialize)]
struct MkdirBody {
    path: String,
}

async fn fs_mkdir(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<MkdirBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    require_write_scope(&a)?;
    let mut segs = parse_path(&body.path)?;
    if segs.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Missing path"));
    }
    if is_scriber(&state, &segs) {
        return Err(err(StatusCode::BAD_REQUEST, "The scriber project is managed externally"));
    }
    // Creating a project (top-level folder) implicitly creates your own space.
    if segs.len() == 1 && !a.user.is_admin {
        segs.push(a.user.username.clone());
    }
    let g = grants(&state, &a);
    if Store::access_for(&a.user.username, a.user.is_admin, &g, &segs) != Access::ReadWrite {
        return Err(err(StatusCode::FORBIDDEN, "No write access to this path"));
    }
    state
        .store
        .mkdir(&segs)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    record(&state, "created", &segs, None, &a.user.username);
    Ok(Json(json!({ "ok": true, "path": segs.join("/") })))
}

#[derive(Deserialize)]
struct PathsBody {
    paths: Vec<String>,
}

async fn fs_delete(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<PathsBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    require_write_scope(&a)?;
    let g = grants(&state, &a);
    let mut deleted = Vec::new();
    for p in &body.paths {
        let segs = parse_path(p)?;
        if segs.is_empty() || is_scriber(&state, &segs) {
            return Err(err(StatusCode::BAD_REQUEST, "This item cannot be deleted"));
        }
        if Store::access_for(&a.user.username, a.user.is_admin, &g, &segs) != Access::ReadWrite {
            return Err(err(StatusCode::FORBIDDEN, &format!("No write access to {p}")));
        }
        state
            .store
            .delete(&segs)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
        state.db.delete_shares_under(&segs.join("/"));
        record(&state, "deleted", &segs, None, &a.user.username);
        deleted.push(segs.join("/"));
    }
    Ok(Json(json!({ "ok": true, "deleted": deleted })))
}

#[derive(Deserialize)]
struct RenameBody {
    path: String,
    new_name: String,
}

async fn fs_rename(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<RenameBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    require_write_scope(&a)?;
    let segs = parse_path(&body.path)?;
    let new_name = body.new_name.trim();
    if segs.is_empty() || is_scriber(&state, &segs) {
        return Err(err(StatusCode::BAD_REQUEST, "This item cannot be renamed"));
    }
    if clean_path(new_name).map(|v| v.len()) != Some(1) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid name"));
    }
    let g = grants(&state, &a);
    if Store::access_for(&a.user.username, a.user.is_admin, &g, &segs) != Access::ReadWrite {
        return Err(err(StatusCode::FORBIDDEN, "No write access to this path"));
    }
    let mut to = segs.clone();
    *to.last_mut().unwrap() = new_name.to_string();
    if state.store.exists(&to) {
        return Err(err(StatusCode::CONFLICT, "An item with that name already exists"));
    }
    state
        .store
        .rename(&segs, &to)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    state.db.rewrite_share_paths(&segs.join("/"), &to.join("/"));
    record(&state, "renamed", &segs, Some(&to.join("/")), &a.user.username);
    Ok(Json(json!({ "ok": true, "path": to.join("/") })))
}

#[derive(Deserialize)]
struct TransferBody {
    paths: Vec<String>,
    dest: String,
}

async fn transfer(
    state: &AppState,
    a: &Authed,
    body: &TransferBody,
    move_items: bool,
) -> Result<Vec<String>, ApiError> {
    let dest = parse_path(&body.dest)?;
    if is_scriber(state, &dest) {
        return Err(err(StatusCode::BAD_REQUEST, "The scriber project is managed externally"));
    }
    let g = grants(state, a);
    if !dest.is_empty()
        && Store::access_for(&a.user.username, a.user.is_admin, &g, &dest) != Access::ReadWrite
    {
        return Err(err(StatusCode::FORBIDDEN, "No write access to the destination"));
    }
    if !dest.is_empty() && !state.store.is_dir(&dest) {
        return Err(err(StatusCode::BAD_REQUEST, "Destination is not a folder"));
    }
    let mut results = Vec::new();
    for p in &body.paths {
        let segs = parse_path(p)?;
        if segs.is_empty() || is_scriber(state, &segs) {
            return Err(err(StatusCode::BAD_REQUEST, "This item cannot be transferred"));
        }
        let needed = if move_items { Access::ReadWrite } else { Access::Read };
        let have = Store::access_for(&a.user.username, a.user.is_admin, &g, &segs);
        let allowed = match needed {
            Access::ReadWrite => have == Access::ReadWrite,
            _ => have != Access::None,
        };
        if !allowed {
            return Err(err(StatusCode::FORBIDDEN, &format!("No access to {p}")));
        }
        let mut to = dest.clone();
        to.push(segs.last().unwrap().clone());
        // Moving into a project root would escape the per-user convention —
        // only allow destinations at least one level deep (inside a project),
        // except for admins who may reorganize projects.
        if to.len() < 2 && !a.user.is_admin {
            return Err(err(StatusCode::BAD_REQUEST, "Destination must be inside a project"));
        }
        // No-op / cycle guards.
        let src_joined = segs.join("/");
        let dst_joined = to.join("/");
        if src_joined == dst_joined {
            continue;
        }
        if dst_joined.starts_with(&format!("{src_joined}/")) {
            return Err(err(StatusCode::BAD_REQUEST, "Cannot move a folder into itself"));
        }
        if state.store.exists(&to) {
            return Err(err(
                StatusCode::CONFLICT,
                &format!("\"{}\" already exists in the destination", to.last().unwrap()),
            ));
        }
        if move_items {
            state
                .store
                .rename(&segs, &to)
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
            state.db.rewrite_share_paths(&src_joined, &dst_joined);
            record(state, "moved", &segs, Some(&dst_joined), &a.user.username);
            if segs[0] != to[0] {
                // Also visible as a creation in the destination project.
                record(state, "created", &to, None, &a.user.username);
            }
        } else {
            state
                .store
                .copy(&segs, &to)
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
            record(state, "created", &to, None, &a.user.username);
        }
        results.push(dst_joined);
    }
    Ok(results)
}

async fn fs_move(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<TransferBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    require_write_scope(&a)?;
    let moved = transfer(&state, &a, &body, true).await?;
    Ok(Json(json!({ "ok": true, "paths": moved })))
}

async fn fs_copy(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<TransferBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    require_write_scope(&a)?;
    let copied = transfer(&state, &a, &body, false).await?;
    Ok(Json(json!({ "ok": true, "paths": copied })))
}

// ---------------------------------------------------------------------------
// Shares
// ---------------------------------------------------------------------------

async fn shares_list(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    let segs = parse_path(&q.path)?;
    let g = grants(&state, &a);
    if Store::access_for(&a.user.username, a.user.is_admin, &g, &segs) == Access::None {
        return Err(err(StatusCode::FORBIDDEN, "No access to this path"));
    }
    Ok(Json(json!({ "shares": state.db.shares_for_path(&segs.join("/")) })))
}

#[derive(Deserialize)]
struct ShareBody {
    path: String,
    grantee_type: String,
    grantee_id: String,
    permission: String,
}

async fn shares_create(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<ShareBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    require_write_scope(&a)?;
    let segs = parse_path(&body.path)?;
    if segs.is_empty() || is_scriber(&state, &segs) {
        return Err(err(StatusCode::BAD_REQUEST, "This item cannot be shared"));
    }
    if !matches!(body.grantee_type.as_str(), "user" | "group") {
        return Err(err(StatusCode::BAD_REQUEST, "grantee_type must be user or group"));
    }
    if !matches!(body.permission.as_str(), "read" | "readwrite") {
        return Err(err(StatusCode::BAD_REQUEST, "permission must be read or readwrite"));
    }
    let g = grants(&state, &a);
    // Only someone with write access (owner/admin/rw-grantee) may share.
    if Store::access_for(&a.user.username, a.user.is_admin, &g, &segs) != Access::ReadWrite {
        return Err(err(StatusCode::FORBIDDEN, "Only items you can write can be shared"));
    }
    let valid = if body.grantee_type == "user" {
        state.db.user_by_id(&body.grantee_id).is_some()
    } else {
        state.db.list_groups().iter().any(|gr| gr.id == body.grantee_id)
    };
    if !valid {
        return Err(err(StatusCode::NOT_FOUND, "Grantee not found"));
    }
    state
        .db
        .upsert_share(&segs.join("/"), &body.grantee_type, &body.grantee_id, &body.permission, &a.user.id)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    Ok(Json(json!({ "ok": true, "shares": state.db.shares_for_path(&segs.join("/")) })))
}

async fn shares_delete(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxPath(id): AxPath<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = require(&state, &headers, None)?;
    require_write_scope(&a)?;
    let share = state
        .db
        .share_by_id(&id)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Share not found"))?;
    let segs = parse_path(&share.path)?;
    let g = grants(&state, &a);
    if Store::access_for(&a.user.username, a.user.is_admin, &g, &segs) != Access::ReadWrite {
        return Err(err(StatusCode::FORBIDDEN, "No write access to this path"));
    }
    state.db.delete_share(&id);
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Events (poll + SSE)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct EventsQuery {
    project: String,
    #[serde(default)]
    since: i64,
    #[serde(default)]
    access_token: Option<String>,
}

async fn events_poll(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(q): Query<EventsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = authenticate(&state, &headers, q.access_token.as_deref(), None)?;
    let segs = parse_path(&q.project)?;
    if segs.len() != 1 {
        return Err(err(StatusCode::BAD_REQUEST, "project must be a top-level name"));
    }
    let g = grants(&state, &a);
    if !state.store.visible(&a.user.username, a.user.is_admin, &g, &segs) && !is_scriber(&state, &segs) {
        return Err(err(StatusCode::FORBIDDEN, "No access to this project"));
    }
    let events = state.db.events_since(&segs[0], q.since, 500);
    let latest = events.last().map(|e| e.seq).unwrap_or_else(|| state.db.latest_seq().max(q.since));
    Ok(Json(json!({ "events": events, "latest_seq": latest })))
}

async fn events_stream(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(q): Query<EventsQuery>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, std::convert::Infallible>>>, ApiError> {
    let a = authenticate(&state, &headers, q.access_token.as_deref(), None)?;
    let segs = parse_path(&q.project)?;
    if segs.len() != 1 {
        return Err(err(StatusCode::BAD_REQUEST, "project must be a top-level name"));
    }
    let g = grants(&state, &a);
    if !state.store.visible(&a.user.username, a.user.is_admin, &g, &segs) && !is_scriber(&state, &segs) {
        return Err(err(StatusCode::FORBIDDEN, "No access to this project"));
    }
    let project = segs[0].clone();
    let rx = state.hub.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(move |item| match item {
        Ok(ev) if ev.project == project => Some(Ok(SseEvent::default()
            .event("change")
            .id(ev.seq.to_string())
            .data(serde_json::to_string(&ev).unwrap_or_default()))),
        _ => None,
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
