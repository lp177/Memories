//! Regression tests for the Scriber integration contract.
//!
//! These tests pin the exact wire shapes the `ScriberClient` depends on —
//! the "compatible memory provider" contract documented in the README:
//!
//!   GET /api/v1/participants?limit&offset  -> {"total": N, "items": [{"id", "display_name"}]}
//!   GET /api/v1/participants/{id}/memory   -> {"content": "..."}
//!   PUT /api/v1/participants/{id}/memory   {"content"} -> {"ok": true}
//!   Auth: "Authorization: Bearer <token>", 401 without it.
//!
//! If Scriber (or any other provider) changes these shapes, these tests are
//! the early warning; if `ScriberClient` drifts, they fail too.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use memories::scriber::ScriberClient;

#[derive(Clone, Default)]
struct MockState {
    memories: Arc<Mutex<std::collections::HashMap<String, String>>>,
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

    let app = Router::new()
        .route(
            "/api/v1/participants",
            get(
                |State(s): State<MockState>, headers: HeaderMap, Query(q): Query<Value>| async move {
                    if !check_auth(&s, &headers) {
                        return (StatusCode::UNAUTHORIZED, Json(json!({"detail": "Invalid API token"})));
                    }
                    // Contract: paginated {"total", "items": [{"id", "display_name", ...}]}
                    let offset = q.get("offset").and_then(|v| v.as_str()).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
                    let all = vec![
                        json!({"id": "111", "display_name": "Alice", "session_count": 3}),
                        json!({"id": "222", "display_name": "Bob/Evil", "session_count": 1}),
                    ];
                    let items: Vec<_> = all.into_iter().skip(offset).collect();
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
