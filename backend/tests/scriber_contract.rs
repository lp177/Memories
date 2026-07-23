//! Regression tests for the Scriber integration contract.
//!
//! These tests pin the exact wire shapes the `ScriberClient` depends on —
//! the "compatible memory provider" contract documented in the README:
//!
//!   GET /api/v1/participants?limit&offset  -> {"total": N, "items": [{"id", "display_name"}]}
//!   GET /api/v1/participants/{id}/memory   -> {"content": "..."}
//!   PUT /api/v1/participants/{id}/memory   {"content"} -> {"ok": true}
//!   GET /api/v1/meetings?limit&offset      -> {"total": N, "items": [{"id", "started_at",
//!                                             "voice_channel_name", "guild_name", "has_summary"}]}
//!   GET /api/v1/meetings/{id}/summary      -> raw text/markdown minutes (NOT JSON)
//!   PUT /api/v1/meetings/{id}/summary      {"content"} -> {"ok": true}
//!   Auth: "Authorization: Bearer <token>", 401 without it.
//!
//! If Scriber (or any other provider) changes these shapes, these tests are
//! the early warning; if `ScriberClient` drifts, they fail too.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use memories::scriber::ScriberClient;

#[derive(Clone, Default)]
struct MockState {
    memories: Arc<Mutex<std::collections::HashMap<String, String>>>,
    summaries: Arc<Mutex<std::collections::HashMap<String, String>>>,
    /// Records whether every request carried the expected bearer token.
    saw_bad_auth: Arc<Mutex<bool>>,
}

const TOKEN: &str = "testtoken1234567890testtoken1234567890testtoken1";

fn check_auth(state: &MockState, headers: &HeaderMap) -> bool {
    let ok = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == format!("Bearer {TOKEN}"))
        .unwrap_or(false);
    if !ok {
        *state.saw_bad_auth.lock().unwrap() = true;
    }
    ok
}

async fn spawn_mock() -> (SocketAddr, MockState) {
    let state = MockState::default();
    state
        .memories
        .lock()
        .unwrap()
        .insert("111".into(), "# Alice\n- context".into());
    state
        .memories
        .lock()
        .unwrap()
        .insert("222".into(), "# Bob".into());
    state
        .summaries
        .lock()
        .unwrap()
        .insert("m-1".into(), "# Minutes\n- decided things".into());

    let app = Router::new()
        .route(
            "/api/v1/participants",
            get(
                |State(s): State<MockState>, headers: HeaderMap, Query(q): Query<Value>| async move {
                    if !check_auth(&s, &headers) {
                        return (StatusCode::UNAUTHORIZED, Json(json!({"detail": "Invalid API token"})));
                    }
                    // Contract: paginated {"total", "items": [{"id", "display_name", ...}]}.
                    // One item per page so the client's offset loop is actually driven.
                    let offset = q.get("offset").and_then(|v| v.as_str()).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
                    let all = vec![
                        json!({"id": "111", "display_name": "Alice", "session_count": 3}),
                        json!({"id": "222", "display_name": "Bob/Evil", "session_count": 1}),
                    ];
                    let items: Vec<_> = all.into_iter().skip(offset).take(1).collect();
                    (StatusCode::OK, Json(json!({"total": 2, "items": items})))
                },
            ),
        )
        .route(
            "/api/v1/participants/{id}/memory",
            get(
                |State(s): State<MockState>, headers: HeaderMap, Path(id): Path<String>| async move {
                    if !check_auth(&s, &headers) {
                        return (StatusCode::UNAUTHORIZED, Json(json!({"detail": "Invalid API token"})));
                    }
                    match s.memories.lock().unwrap().get(&id) {
                        Some(content) => (StatusCode::OK, Json(json!({"content": content}))),
                        None => (StatusCode::NOT_FOUND, Json(json!({"detail": "Participant not found"}))),
                    }
                },
            )
            .put(
                |State(s): State<MockState>,
                 headers: HeaderMap,
                 Path(id): Path<String>,
                 Json(body): Json<Value>| async move {
                    if !check_auth(&s, &headers) {
                        return (StatusCode::UNAUTHORIZED, Json(json!({"detail": "Invalid API token"})));
                    }
                    let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    s.memories.lock().unwrap().insert(id, content);
                    (StatusCode::OK, Json(json!({"ok": true})))
                },
            ),
        )
        .route(
            "/api/v1/meetings",
            get(
                |State(s): State<MockState>, headers: HeaderMap, Query(q): Query<Value>| async move {
                    if !check_auth(&s, &headers) {
                        return (StatusCode::UNAUTHORIZED, Json(json!({"detail": "Invalid API token"})));
                    }
                    // Contract: paginated {"total", "items": [...]}, newest first.
                    // One item per page so the client's offset loop is actually driven.
                    let offset = q.get("offset").and_then(|v| v.as_str()).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
                    let all = vec![
                        json!({"id": "m-1", "started_at": "2026-07-07T12:00:00", "voice_channel_name": "General/Weekly",
                               "guild_name": "Guild", "status": "completed", "has_summary": true}),
                        json!({"id": "m-2", "started_at": "2026-07-06T09:30:00", "voice_channel_name": null,
                               "guild_name": "Guild", "status": "error", "has_summary": false}),
                    ];
                    let items: Vec<_> = all.into_iter().skip(offset).take(1).collect();
                    (StatusCode::OK, Json(json!({"total": 2, "items": items})))
                },
            ),
        )
        .route(
            "/api/v1/meetings/{id}/summary",
            get(
                |State(s): State<MockState>, headers: HeaderMap, Path(id): Path<String>| async move {
                    if !check_auth(&s, &headers) {
                        return (StatusCode::UNAUTHORIZED, "Invalid API token".to_string()).into_response();
                    }
                    // Contract: the minutes are served as raw markdown text, not JSON.
                    match s.summaries.lock().unwrap().get(&id) {
                        Some(content) => (
                            StatusCode::OK,
                            [("content-type", "text/markdown; charset=utf-8")],
                            content.clone(),
                        )
                            .into_response(),
                        None => (StatusCode::NOT_FOUND, "File not found".to_string()).into_response(),
                    }
                },
            )
            .put(
                |State(s): State<MockState>,
                 headers: HeaderMap,
                 Path(id): Path<String>,
                 Json(body): Json<Value>| async move {
                    if !check_auth(&s, &headers) {
                        return (StatusCode::UNAUTHORIZED, Json(json!({"detail": "Invalid API token"})));
                    }
                    let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    s.summaries.lock().unwrap().insert(id, content);
                    (StatusCode::OK, Json(json!({"ok": true})))
                },
            ),
        )
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, state)
}

#[tokio::test]
async fn lists_participants_with_pagination() {
    let (addr, state) = spawn_mock().await;
    let client = ScriberClient::new(&format!("http://{addr}"), TOKEN);
    let participants = client.participants().await.expect("participants");
    assert_eq!(participants.len(), 2);
    assert_eq!(participants[0].id, "111");
    assert_eq!(participants[0].display_name.as_deref(), Some("Alice"));
    assert!(!*state.saw_bad_auth.lock().unwrap(), "client must send the bearer token");
}

#[tokio::test]
async fn memory_read_write_roundtrip() {
    let (addr, _state) = spawn_mock().await;
    let client = ScriberClient::new(&format!("http://{addr}"), TOKEN);
    assert_eq!(client.read_memory("111").await.unwrap(), "# Alice\n- context");
    client
        .write_memory("111", "# Alice\n- updated by memories")
        .await
        .expect("write");
    assert_eq!(
        client.read_memory("111").await.unwrap(),
        "# Alice\n- updated by memories"
    );
}

#[tokio::test]
async fn wrong_token_is_rejected() {
    let (addr, _state) = spawn_mock().await;
    let client = ScriberClient::new(&format!("http://{addr}"), "wrong-token");
    assert!(client.participants().await.is_err());
    assert!(client.read_memory("111").await.is_err());
    assert!(client.write_memory("111", "x").await.is_err());
    assert!(client.meetings().await.is_err());
    assert!(client.read_meeting_summary("m-1").await.is_err());
    assert!(client.write_meeting_summary("m-1", "x").await.is_err());
}

#[tokio::test]
async fn lists_meetings_with_pagination() {
    let (addr, state) = spawn_mock().await;
    let client = ScriberClient::new(&format!("http://{addr}"), TOKEN);
    let meetings = client.meetings().await.expect("meetings");
    assert_eq!(meetings.len(), 2);
    assert_eq!(meetings[0].id, "m-1");
    assert!(meetings[0].has_summary);
    assert!(!meetings[1].has_summary);
    assert!(!*state.saw_bad_auth.lock().unwrap(), "client must send the bearer token");
}

#[tokio::test]
async fn meeting_summary_read_write_roundtrip() {
    // The GET side is raw text/markdown (not JSON) — pin that shape.
    let (addr, _state) = spawn_mock().await;
    let client = ScriberClient::new(&format!("http://{addr}"), TOKEN);
    assert_eq!(
        client.read_meeting_summary("m-1").await.unwrap(),
        "# Minutes\n- decided things"
    );
    client
        .write_meeting_summary("m-1", "# Minutes\n- edited from memories")
        .await
        .expect("write");
    assert_eq!(
        client.read_meeting_summary("m-1").await.unwrap(),
        "# Minutes\n- edited from memories"
    );
}

#[tokio::test]
async fn meeting_file_names_are_safe_and_reversible() {
    let m = memories::scriber::Meeting {
        id: "m-1".into(),
        started_at: Some("2026-07-06T12:00:00".into()),
        voice_channel_name: Some("General/Weekly".into()),
        guild_name: Some("Guild".into()),
        has_summary: true,
    };
    let file = ScriberClient::meeting_file_name(&m);
    assert!(!file.contains('/'));
    assert!(file.starts_with("2026-07-06 "));
    assert!(file.ends_with(".md"));
    assert_eq!(ScriberClient::meeting_id_from_file(&file), "m-1");
    // Bare-id fallback when nothing else is known.
    let bare = memories::scriber::Meeting {
        id: "m-9".into(),
        started_at: None,
        voice_channel_name: None,
        guild_name: None,
        has_summary: true,
    };
    assert_eq!(ScriberClient::meeting_file_name(&bare), "m-9.md");
    assert_eq!(ScriberClient::meeting_id_from_file("m-9.md"), "m-9");
}

#[tokio::test]
async fn mount_entries_synthesize_the_virtual_tree() {
    let (addr, _state) = spawn_mock().await;
    let client = ScriberClient::new(&format!("http://{addr}"), TOKEN);
    let seg = |s: &str| s.to_string();

    // Mount root: the meetings folder + one folder per participant, all dirs,
    // in ascending ASCII name order ("meetings" is lowercase, so last here).
    let root = client.mount_entries(&[seg("scriber")], "readwrite").await.unwrap().unwrap();
    assert_eq!(root.len(), 3);
    assert!(root.iter().all(|e| e.is_dir));
    assert_eq!(root[0].name, "Alice (111)");
    assert_eq!(root[0].path, "scriber/Alice (111)");
    assert_eq!(root[1].name, "Bob_Evil (222)");
    assert_eq!(root[2].name, "meetings");

    // A participant folder holds exactly memory.md.
    let inside = client
        .mount_entries(&[seg("scriber"), seg("Alice (111)")], "read")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inside.len(), 1);
    assert_eq!(inside[0].name, "memory.md");
    assert_eq!(inside[0].access, "read");

    // The meetings folder lists only meetings that have minutes.
    let meetings = client
        .mount_entries(&[seg("scriber"), seg("meetings")], "readwrite")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(meetings.len(), 1);
    assert!(meetings[0].name.ends_with("(m-1).md"));
    assert!(!meetings[0].is_dir);
    assert_eq!(meetings[0].path, format!("scriber/meetings/{}", meetings[0].name));

    // Deeper paths do not exist in the virtual layout.
    assert!(client
        .mount_entries(&[seg("scriber"), seg("meetings"), seg("x.md")], "read")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn missing_files_are_not_found_not_gateway_errors() {
    use memories::scriber::ScriberError;
    let (addr, _state) = spawn_mock().await;
    let client = ScriberClient::new(&format!("http://{addr}"), TOKEN);
    assert_eq!(client.read_memory("nope").await.unwrap_err(), ScriberError::NotFound);
    assert_eq!(client.read_meeting_summary("nope").await.unwrap_err(), ScriberError::NotFound);
}

#[test]
fn odd_ids_and_labels_still_round_trip() {
    use memories::scriber::{Meeting, Participant};
    let meeting = |id: &str, started: Option<&str>, channel: Option<&str>| Meeting {
        id: id.into(),
        started_at: started.map(Into::into),
        voice_channel_name: channel.map(Into::into),
        guild_name: None,
        has_summary: true,
    };

    // Opaque ids containing parentheses (third-party providers).
    for id in ["we(ird)", "m)1", "a(b"] {
        let bare = ScriberClient::meeting_file_name(&meeting(id, None, None));
        assert_eq!(ScriberClient::meeting_id_from_file(&bare), id, "bare {id}");
        let labeled =
            ScriberClient::meeting_file_name(&meeting(id, Some("2026-07-06T12:00:00"), Some("General")));
        assert_eq!(ScriberClient::meeting_id_from_file(&labeled), id, "labeled {id}");
    }

    // Labels that themselves end in a parenthesized group.
    let p = Participant { id: "222".into(), display_name: Some("Bob (the great)".into()) };
    assert_eq!(
        ScriberClient::participant_id_from_folder(&ScriberClient::folder_name(&p)),
        "222"
    );

    // Leading dots would be rejected as hidden files by the store.
    let dot = Participant { id: "222".into(), display_name: Some(".bob".into()) };
    let folder = ScriberClient::folder_name(&dot);
    assert!(!folder.starts_with('.'), "{folder}");
    assert_eq!(ScriberClient::participant_id_from_folder(&folder), "222");

    // Long unicode labels stay under the store's 255-byte segment limit.
    let long = "会".repeat(100);
    let m = meeting("20260706-120000-abcd", Some("2026-07-06T12:00:00"), Some(&long));
    let name = ScriberClient::meeting_file_name(&m);
    assert!(name.len() < 255, "{} bytes", name.len());
    assert_eq!(ScriberClient::meeting_id_from_file(&name), "20260706-120000-abcd");
    let wide = Participant { id: "222".into(), display_name: Some(long) };
    assert!(ScriberClient::folder_name(&wide).len() < 255);

    // "meetings" is reserved at the mount root: a bare id colliding with it
    // gets a decorated (still reversible) folder label.
    let clash = Participant { id: "meetings".into(), display_name: None };
    let folder = ScriberClient::folder_name(&clash);
    assert_ne!(folder, "meetings");
    assert_eq!(ScriberClient::participant_id_from_folder(&folder), "meetings");
}

#[tokio::test]
async fn classify_file_routes_mount_paths() {
    use memories::scriber::MountFile;
    let segs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    match ScriberClient::classify_file(&segs(&["scriber", "Alice (111)", "memory.md"])) {
        Some(MountFile::ParticipantMemory(id)) => assert_eq!(id, "111"),
        _ => panic!("participant memory.md must classify"),
    }
    match ScriberClient::classify_file(&segs(&["scriber", "meetings", "2026-07-06 General (m-1).md"])) {
        Some(MountFile::MeetingSummary(id)) => assert_eq!(id, "m-1"),
        _ => panic!("meeting file must classify"),
    }
    // Wrong file name, wrong depth, non-md meeting file: not files of the mount.
    assert!(ScriberClient::classify_file(&segs(&["scriber", "Alice (111)", "notes.md"])).is_none());
    assert!(ScriberClient::classify_file(&segs(&["scriber", "meetings"])).is_none());
    assert!(ScriberClient::classify_file(&segs(&["scriber", "meetings", "notes.txt"])).is_none());
}

#[tokio::test]
async fn folder_names_are_safe_and_reversible() {
    // Display names become single safe path segments; the participant id must
    // always be recoverable from the folder label.
    let p = memories::scriber::Participant { id: "222".into(), display_name: Some("Bob/Evil".into()) };
    let folder = ScriberClient::folder_name(&p);
    assert!(!folder.contains('/'));
    assert_eq!(ScriberClient::participant_id_from_folder(&folder), "222");
    // Id-only fallback.
    let p2 = memories::scriber::Participant { id: "333".into(), display_name: None };
    assert_eq!(ScriberClient::folder_name(&p2), "333");
    assert_eq!(ScriberClient::participant_id_from_folder("333"), "333");
}
