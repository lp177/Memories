//! Privacy / access-control regression suite — the deploy gate for the
//! per-user, per-group and admin isolation of the memory store.
//!
//! Layout on disk is `memories/<project>/<user>/<path...>`; the depth-2 folder
//! `<project>/<username>` is the *ownership anchor* whose name decides which
//! user owns everything beneath it. The attacks these tests pin down:
//!
//!   * a non-admin renaming their own anchor to another username to INJECT
//!     memory into that user's space (or pre-position it for a future user);
//!   * a non-admin renaming / deleting / moving *another* user's anchor (even
//!     with a readwrite share) to hijack or destroy their memories;
//!   * a non-admin forging a foreign anchor via move/copy into a shared project;
//!   * a group grant leaking write (it must give visibility, not control);
//!   * reading memories one is not entitled to, path traversal, admin-only
//!     endpoints, token scopes and unauthenticated access;
//!   * reaching the scriber mount (private meeting minutes, participant
//!     memories) without being an admin or holding a share inside it, or
//!     writing through it while it is read-only.
//!
//! Each test drives the *real* `api::router` over HTTP via reqwest, so it
//! exercises authentication, the handlers and the store together — the exact
//! surface an attacker would reach. If any of these regress, a privacy leak is
//! shipping; keep them green before every deploy.

use std::sync::Arc;

use memories::api::{self, AppState};
use memories::auth;
use memories::config::Config;
use memories::db::{Db, User};
use memories::events::EventHub;
use memories::scriber::ScriberClient;
use memories::store::{clean_path, Store};

const SECRET: &str = "privacy-test-secret";

struct World {
    base: String,
    admin: String,
    alice: String,
    bob: String,
    carol: String, // in the "readers" group with alice
    dave: String,  // not in any group
    alice_id: String,
    bob_id: String,
    carol_id: String,
    group_id: String,
    state: Arc<AppState>,
    _tmp: tempfile::TempDir,
}

fn test_config(dir: &std::path::Path) -> Config {
    Config {
        bind: "127.0.0.1:0".into(),
        data_dir: dir.to_path_buf(),
        web_secret: SECRET.into(),
        admin_username: "admin".into(),
        admin_password: "unused".into(),
        public_url: "http://localhost".into(),
        smtp_host: None,
        smtp_port: 587,
        smtp_username: None,
        smtp_password: None,
        smtp_from: "memories@localhost".into(),
        scriber_url: None,
        scriber_token: None,
        scriber_mount: "scriber".into(),
        scriber_writable: false,
    }
}

fn seg(p: &str) -> Vec<String> {
    clean_path(p).unwrap()
}

async fn spawn() -> World {
    spawn_with(None, false).await
}

/// A world with the scriber mount configured. The upstream address is dead on
/// purpose: an access check must refuse BEFORE any upstream call (403), while
/// an allowed request gets past it and fails upstream (502/404).
async fn spawn_mounted(writable: bool) -> World {
    spawn_with(Some(ScriberClient::new("http://127.0.0.1:9", "unused")), writable).await
}

async fn spawn_with(scriber: Option<ScriberClient>, writable: bool) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let db = Db::open(&tmp.path().join("memories.db")).unwrap();
    let hash = auth::hash_password("password123");
    let admin = db.create_user("admin", "", &hash, true).unwrap();
    let alice = db.create_user("alice", "", &hash, false).unwrap();
    let bob = db.create_user("bob", "", &hash, false).unwrap();
    let carol = db.create_user("carol", "", &hash, false).unwrap();
    let dave = db.create_user("dave", "", &hash, false).unwrap();

    // "readers" holds alice + carol; dave is deliberately left out.
    let group_id = db.create_group("readers").unwrap();
    db.set_group_members(&group_id, &[alice.id.clone(), carol.id.clone()]);

    let store = Store::new(tmp.path());
    store.write_file(&seg("proj/alice/notes.md"), "alice private").unwrap();
    store.write_file(&seg("proj/bob/secret.md"), "bob private").unwrap();

    let mut cfg = test_config(tmp.path());
    cfg.scriber_writable = writable;
    let state = Arc::new(AppState {
        cfg,
        db,
        store,
        hub: EventHub::new(),
        scriber,
    });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = api::router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let tok = |u: &User| auth::create_session(&u.id, SECRET, 3600);
    World {
        base: format!("http://{addr}"),
        admin: tok(&admin),
        alice: tok(&alice),
        bob: tok(&bob),
        carol: tok(&carol),
        dave: tok(&dave),
        alice_id: alice.id,
        bob_id: bob.id,
        carol_id: carol.id,
        group_id,
        state,
        _tmp: tmp,
    }
}

// ---- HTTP helpers ---------------------------------------------------------

fn http() -> reqwest::Client {
    reqwest::Client::new()
}

async fn get(w: &World, tok: &str, path: &str) -> reqwest::Response {
    http()
        .get(format!("{}{}", w.base, path))
        .bearer_auth(tok)
        .send()
        .await
        .unwrap()
}

async fn post(w: &World, tok: &str, path: &str, body: serde_json::Value) -> reqwest::Response {
    http()
        .post(format!("{}{}", w.base, path))
        .bearer_auth(tok)
        .json(&body)
        .send()
        .await
        .unwrap()
}

async fn put(w: &World, tok: &str, path: &str, body: serde_json::Value) -> reqwest::Response {
    http()
        .put(format!("{}{}", w.base, path))
        .bearer_auth(tok)
        .json(&body)
        .send()
        .await
        .unwrap()
}

async fn rename(w: &World, tok: &str, path: &str, new_name: &str) -> reqwest::Response {
    post(w, tok, "/api/fs/rename", serde_json::json!({ "path": path, "new_name": new_name })).await
}

async fn write_file(w: &World, tok: &str, path: &str, content: &str) -> reqwest::Response {
    put(w, tok, "/api/fs/file", serde_json::json!({ "path": path, "content": content })).await
}

async fn read_file(w: &World, tok: &str, path: &str) -> reqwest::Response {
    get(w, tok, &format!("/api/fs/file?path={path}")).await
}

async fn delete_paths(w: &World, tok: &str, paths: &[&str]) -> reqwest::Response {
    post(w, tok, "/api/fs/delete", serde_json::json!({ "paths": paths })).await
}

async fn mv(w: &World, tok: &str, paths: &[&str], dest: &str) -> reqwest::Response {
    post(w, tok, "/api/fs/move", serde_json::json!({ "paths": paths, "dest": dest })).await
}

async fn copy(w: &World, tok: &str, paths: &[&str], dest: &str) -> reqwest::Response {
    post(w, tok, "/api/fs/copy", serde_json::json!({ "paths": paths, "dest": dest })).await
}

fn exists(w: &World, path: &str) -> bool {
    w.state.store.exists(&seg(path))
}

// ===========================================================================
// Baseline isolation
// ===========================================================================

#[tokio::test]
async fn owner_has_full_access_to_own_space() {
    let w = spawn().await;
    assert_eq!(read_file(&w, &w.alice, "proj/alice/notes.md").await.status(), 200);
    assert_eq!(write_file(&w, &w.alice, "proj/alice/new.md", "hi").await.status(), 200);
    assert!(exists(&w, "proj/alice/new.md"));
}

#[tokio::test]
async fn cannot_read_another_users_memory() {
    let w = spawn().await;
    // No grant: bob's private file is invisible to alice and dave.
    assert_eq!(read_file(&w, &w.alice, "proj/bob/secret.md").await.status(), 403);
    assert_eq!(read_file(&w, &w.dave, "proj/bob/secret.md").await.status(), 403);
    // The owner and an admin can read it.
    assert_eq!(read_file(&w, &w.bob, "proj/bob/secret.md").await.status(), 200);
    assert_eq!(read_file(&w, &w.admin, "proj/bob/secret.md").await.status(), 200);
}

#[tokio::test]
async fn cannot_write_or_mkdir_in_another_users_space() {
    let w = spawn().await;
    assert_eq!(write_file(&w, &w.alice, "proj/bob/inject.md", "x").await.status(), 403);
    let mk = post(&w, &w.alice, "/api/fs/mkdir", serde_json::json!({ "path": "proj/bob/evil" })).await;
    assert_eq!(mk.status(), 403);
    assert!(!exists(&w, "proj/bob/inject.md"));
    assert!(!exists(&w, "proj/bob/evil"));
}

// ===========================================================================
// The core attack: renaming the ownership anchor to inject memory
// ===========================================================================

#[tokio::test]
async fn cannot_rename_own_anchor_to_prefill_a_future_user() {
    let w = spawn().await;
    // alice tries to rename proj/alice -> proj/mallory (no such user yet). If
    // this were allowed, her authored content would be waiting inside the
    // namespace of whoever is later provisioned as "mallory".
    let r = rename(&w, &w.alice, "proj/alice", "mallory").await;
    assert_eq!(r.status(), 403, "renaming an ownership anchor must be forbidden");
    assert!(exists(&w, "proj/alice/notes.md"), "alice's space must be untouched");
    assert!(!exists(&w, "proj/mallory"), "no foreign anchor may be created");
}

#[tokio::test]
async fn cannot_rename_own_anchor_onto_another_existing_user() {
    let w = spawn().await;
    // Renaming proj/alice -> proj/bob would drop alice's content into bob's
    // namespace. Forbidden regardless of the collision.
    let r = rename(&w, &w.alice, "proj/alice", "bob").await;
    assert_eq!(r.status(), 403);
    assert_eq!(read_file(&w, &w.bob, "proj/bob/secret.md").await.status(), 200);
}

#[tokio::test]
async fn cannot_rename_another_users_anchor() {
    let w = spawn().await;
    // alice has no access to bob's anchor at all.
    assert_eq!(rename(&w, &w.alice, "proj/bob", "hijacked").await.status(), 403);
    assert!(exists(&w, "proj/bob"));
}

#[tokio::test]
async fn readwrite_share_does_not_grant_anchor_control() {
    let w = spawn().await;
    // bob deliberately shares his whole space readwrite with alice.
    w.state
        .db
        .upsert_share("proj/bob", "user", &w.alice_id, "readwrite", &w.bob_id)
        .unwrap();

    // The share lets alice write *content* inside bob's space...
    assert_eq!(write_file(&w, &w.alice, "proj/bob/from_alice.md", "hi").await.status(), 200);
    // ...but it must NOT let her rename, move or delete bob's anchor, which
    // would tear down bob's own-space access entirely.
    assert_eq!(rename(&w, &w.alice, "proj/bob", "gone").await.status(), 403);
    assert_eq!(delete_paths(&w, &w.alice, &["proj/bob"]).await.status(), 403);
    assert_eq!(mv(&w, &w.alice, &["proj/bob"], "proj/alice").await.status(), 403);
    assert!(exists(&w, "proj/bob/secret.md"), "bob keeps his memories");
}

// ===========================================================================
// Deleting / moving anchors
// ===========================================================================

#[tokio::test]
async fn cannot_delete_another_users_anchor() {
    let w = spawn().await;
    assert_eq!(delete_paths(&w, &w.alice, &["proj/bob"]).await.status(), 403);
    assert!(exists(&w, "proj/bob/secret.md"));
}

#[tokio::test]
async fn can_delete_own_anchor() {
    let w = spawn().await;
    assert_eq!(delete_paths(&w, &w.alice, &["proj/alice"]).await.status(), 200);
    assert!(!exists(&w, "proj/alice"));
}

#[tokio::test]
async fn cannot_move_own_anchor() {
    let w = spawn().await;
    // Relocating a whole user namespace is admin-only, even for your own.
    assert_eq!(mv(&w, &w.alice, &["proj/alice"], "").await.status(), 403);
    assert!(exists(&w, "proj/alice/notes.md"));
}

#[tokio::test]
async fn cannot_forge_foreign_anchor_via_move_in_shared_project() {
    let w = spawn().await;
    // Admin shares an entire project readwrite with alice (a legitimate setup).
    w.state
        .db
        .upsert_share("team", "user", &w.alice_id, "readwrite", &w.bob_id)
        .unwrap();
    w.state.store.mkdir(&seg("team/alice")).unwrap();
    // alice stages a folder literally named "bob" inside her own space...
    assert_eq!(post(&w, &w.alice, "/api/fs/mkdir", serde_json::json!({ "path": "team/alice/bob" })).await.status(), 200);
    write_file(&w, &w.alice, "team/alice/bob/payload.md", "injected").await;

    // ...and tries to promote it to a top-level anchor `team/bob`. Denied:
    // that would inject memory into bob's namespace within the shared project.
    assert_eq!(mv(&w, &w.alice, &["team/alice/bob"], "team").await.status(), 403);
    assert_eq!(copy(&w, &w.alice, &["team/alice/bob"], "team").await.status(), 403);
    assert!(!exists(&w, "team/bob"), "no foreign anchor may be forged");
}

#[tokio::test]
async fn normal_moves_within_own_space_still_work() {
    let w = spawn().await;
    w.state.store.mkdir(&seg("proj/alice/dst")).unwrap();
    write_file(&w, &w.alice, "proj/alice/note.md", "x").await;
    assert_eq!(mv(&w, &w.alice, &["proj/alice/note.md"], "proj/alice/dst").await.status(), 200);
    assert!(exists(&w, "proj/alice/dst/note.md"));
    assert!(!exists(&w, "proj/alice/note.md"));
}

// ===========================================================================
// Groups: visibility, not control
// ===========================================================================

#[tokio::test]
async fn group_read_share_grants_visibility_only() {
    let w = spawn().await;
    // bob shares his space READ with the "readers" group (alice + carol).
    w.state
        .db
        .upsert_share("proj/bob", "group", &w.group_id, "read", &w.bob_id)
        .unwrap();

    // carol (a group member) can now read bob's memory...
    assert_eq!(read_file(&w, &w.carol, "proj/bob/secret.md").await.status(), 200);
    // ...but cannot write, rename or delete anything through a read grant.
    assert_eq!(write_file(&w, &w.carol, "proj/bob/secret.md", "tampered").await.status(), 403);
    assert_eq!(rename(&w, &w.carol, "proj/bob", "x").await.status(), 403);
    assert_eq!(delete_paths(&w, &w.carol, &["proj/bob"]).await.status(), 403);
    assert_eq!(delete_paths(&w, &w.carol, &["proj/bob/secret.md"]).await.status(), 403);

    // dave is not in the group and sees nothing.
    assert_eq!(read_file(&w, &w.dave, "proj/bob/secret.md").await.status(), 403);
}

#[tokio::test]
async fn group_membership_drives_access() {
    let w = spawn().await;
    w.state
        .db
        .upsert_share("proj/bob", "group", &w.group_id, "read", &w.bob_id)
        .unwrap();
    // carol starts as a member and can read.
    assert_eq!(read_file(&w, &w.carol, "proj/bob/secret.md").await.status(), 200);
    // Remove carol from the group -> access is revoked immediately.
    w.state.db.set_group_members(&w.group_id, &[w.alice_id.clone()]);
    assert_eq!(read_file(&w, &w.carol, "proj/bob/secret.md").await.status(), 403);
    // alice, still a member, retains access.
    assert_eq!(read_file(&w, &w.alice, "proj/bob/secret.md").await.status(), 200);
}

#[tokio::test]
async fn only_writers_can_share_a_path() {
    let w = spawn().await;
    // alice cannot share bob's space (she has no write access to it).
    let bad = post(
        &w,
        &w.alice,
        "/api/shares",
        serde_json::json!({ "path": "proj/bob", "grantee_type": "user", "grantee_id": w.carol_id, "permission": "read" }),
    )
    .await;
    assert_eq!(bad.status(), 403);
    // bob (the owner) can.
    let ok = post(
        &w,
        &w.bob,
        "/api/shares",
        serde_json::json!({ "path": "proj/bob", "grantee_type": "user", "grantee_id": w.carol_id, "permission": "read" }),
    )
    .await;
    assert_eq!(ok.status(), 200);
}

// ===========================================================================
// Admins may reorganize anchors
// ===========================================================================

#[tokio::test]
async fn admin_can_rename_move_and_delete_anchors() {
    let w = spawn().await;
    // Rename an anchor.
    assert_eq!(rename(&w, &w.admin, "proj/bob", "bob2").await.status(), 200);
    assert!(exists(&w, "proj/bob2/secret.md"));
    // Move an anchor into another (existing) project (creates a new anchor).
    w.state.store.mkdir(&seg("archive/keep")).unwrap();
    assert_eq!(mv(&w, &w.admin, &["proj/alice"], "archive").await.status(), 200);
    assert!(exists(&w, "archive/alice/notes.md"));
    // Delete an anchor.
    assert_eq!(delete_paths(&w, &w.admin, &["proj/bob2"]).await.status(), 200);
    assert!(!exists(&w, "proj/bob2"));
}

// ===========================================================================
// Path traversal, admin surface, token scope, unauthenticated
// ===========================================================================

#[tokio::test]
async fn path_traversal_is_rejected() {
    let w = spawn().await;
    assert_eq!(read_file(&w, &w.alice, "../../etc/passwd").await.status(), 400);
    assert_eq!(read_file(&w, &w.alice, "proj/../bob/secret.md").await.status(), 400);
    // A traversal payload as a rename target is rejected too.
    assert_eq!(rename(&w, &w.alice, "proj/alice/notes.md", "../evil").await.status(), 400);
}

#[tokio::test]
async fn admin_endpoints_reject_non_admins() {
    let w = spawn().await;
    assert_eq!(get(&w, &w.alice, "/api/admin/users").await.status(), 403);
    assert_eq!(get(&w, &w.alice, "/api/admin/groups").await.status(), 403);
    let create = post(
        &w,
        &w.alice,
        "/api/admin/users",
        serde_json::json!({ "username": "evil", "password": "password123", "is_admin": true }),
    )
    .await;
    assert_eq!(create.status(), 403);
    // Admin is allowed.
    assert_eq!(get(&w, &w.admin, "/api/admin/users").await.status(), 200);
}

#[tokio::test]
async fn read_scope_api_token_cannot_write() {
    let w = spawn().await;
    // Mint a read-only API token for alice and try to write with it.
    let token = auth::generate_api_token();
    w.state
        .db
        .create_api_token(&w.alice_id, "ro", &auth::token_prefix(&token), &auth::hash_token(&token), "read")
        .unwrap();
    // Reading her own space works.
    assert_eq!(read_file(&w, &token, "proj/alice/notes.md").await.status(), 200);
    // Writing is refused by the token scope even though she owns the path.
    assert_eq!(write_file(&w, &token, "proj/alice/x.md", "x").await.status(), 403);
    assert_eq!(rename(&w, &token, "proj/alice/notes.md", "y.md").await.status(), 403);
}

#[tokio::test]
async fn unauthenticated_requests_are_rejected() {
    let w = spawn().await;
    let r = http().get(format!("{}/api/fs/list", w.base)).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let r = http().get(format!("{}/api/fs/list", w.base)).bearer_auth("garbage").send().await.unwrap();
    assert_eq!(r.status(), 401);
}

// ===========================================================================
// Scriber mount: admins and explicit shares only, read-only by default
// ===========================================================================

const MEETING: &str = "scriber/meetings/2026-10-01 board (m-1).md";
const MEMORY: &str = "scriber/Bob (222)/memory.md";

async fn root_names(w: &World, tok: &str) -> Vec<String> {
    let body: serde_json::Value = get(w, tok, "/api/fs/list?path=").await.json().await.unwrap();
    body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn scriber_mount_is_hidden_from_non_admins() {
    let w = spawn_mounted(false).await;
    assert!(root_names(&w, &w.admin).await.contains(&"scriber".to_string()));
    for tok in [&w.alice, &w.dave] {
        assert!(!root_names(&w, tok).await.contains(&"scriber".to_string()));
        assert_eq!(get(&w, tok, "/api/fs/list?path=scriber").await.status(), 403);
        assert_eq!(read_file(&w, tok, MEETING).await.status(), 403);
        assert_eq!(read_file(&w, tok, MEMORY).await.status(), 403);
        assert_eq!(write_file(&w, tok, MEMORY, "poisoned").await.status(), 403);
        assert_eq!(get(&w, tok, "/api/events?project=scriber").await.status(), 403);
    }
    // The admin gets past the check (then fails on the dead upstream).
    assert_ne!(read_file(&w, &w.admin, MEETING).await.status(), 403);
}

#[tokio::test]
async fn scriber_mount_is_read_only_unless_enabled() {
    let w = spawn_mounted(false).await;
    let resp = write_file(&w, &w.admin, MEMORY, "edited").await;
    assert_eq!(resp.status(), 403);
    assert!(resp.text().await.unwrap().contains("read-only"));
    let w = spawn_mounted(true).await;
    assert_ne!(write_file(&w, &w.admin, MEMORY, "edited").await.status(), 403);
}

#[tokio::test]
async fn scriber_share_opens_only_the_shared_part() {
    let w = spawn_mounted(true).await;
    // Only an admin may share inside the mount.
    let share = serde_json::json!({
        "path": "scriber/meetings", "grantee_type": "user",
        "grantee_id": w.alice_id, "permission": "read",
    });
    assert_eq!(post(&w, &w.bob, "/api/shares", share.clone()).await.status(), 403);
    assert_eq!(post(&w, &w.admin, "/api/shares", share).await.status(), 200);

    assert!(root_names(&w, &w.alice).await.contains(&"scriber".to_string()));
    assert_ne!(read_file(&w, &w.alice, MEETING).await.status(), 403);
    // Read share: no write, and nothing outside the shared folder.
    assert_eq!(write_file(&w, &w.alice, MEETING, "edited").await.status(), 403);
    assert_eq!(read_file(&w, &w.alice, MEMORY).await.status(), 403);
    // A non-admin grantee cannot see or touch the mount's shares.
    assert_eq!(get(&w, &w.alice, "/api/shares?path=scriber/meetings").await.status(), 403);
    // Others still see nothing.
    assert!(!root_names(&w, &w.dave).await.contains(&"scriber".to_string()));
    assert_eq!(read_file(&w, &w.dave, MEETING).await.status(), 403);
}
