//! Integration tests for multi-session management API.
//!
//! These tests verify the full HTTP API flow for session lifecycle operations:
//! - Creating sessions (with and without explicit names)
//! - Listing sessions
//! - Getting session info
//! - Renaming sessions
//! - Deleting sessions
//! - Session isolation (input to one session does not affect another)
//! - Error cases (duplicate names, nonexistent sessions)
//! - Per-session endpoints work after creation

use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use wsh::api::{router, AppState, RouterConfig};
use wsh::session::SessionRegistry;
use wsh::shutdown::ShutdownCoordinator;

/// Creates a test app with an empty session registry (no sessions pre-created).
fn create_empty_test_app() -> axum::Router {
    let registry = SessionRegistry::new();
    let state = AppState {
        sessions: registry,
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(wsh::api::ServerConfig::new(false)),
            server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            ticket_store: std::sync::Arc::new(wsh::api::ticket::TicketStore::new()),
            backends: wsh::federation::registry::BackendRegistry::new(),
            federation: std::sync::Arc::new(tokio::sync::Mutex::new(wsh::federation::manager::FederationManager::new())),
            hostname: "test".to_string(),
            federation_config_path: None,
            local_token: None,
            default_backend_token: None,
    };
    router(state, RouterConfig::default())
}

async fn start_test_server(app: axum::Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    addr
}

// ── Test 1: Create session with explicit name ────────────────────

#[tokio::test]
async fn test_create_session_via_http() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // POST /sessions with {"name": "alpha"}
    let resp = client
        .post(format!("http://{}/sessions", addr))
        .json(&serde_json::json!({"name": "alpha"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201, "Expected 201 Created for session creation");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "alpha");

    // GET /sessions/alpha should return 200 with session info
    let resp = client
        .get(format!("http://{}/sessions/alpha", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "Expected 200 OK for session get");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "alpha");
}

// ── Test 2: Create session with auto-generated name ──────────────

#[tokio::test]
async fn test_create_session_auto_name() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // First POST /sessions with {} (no name)
    let resp = client
        .post(format!("http://{}/sessions", addr))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "0", "First auto-generated name should be '0'");

    // Second POST /sessions with {} (no name)
    let resp = client
        .post(format!("http://{}/sessions", addr))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "1", "Second auto-generated name should be '1'");
}

// ── Test 3: List sessions ────────────────────────────────────────

#[tokio::test]
async fn test_list_sessions() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Create two sessions
    let resp = client
        .post(format!("http://{}/sessions", addr))
        .json(&serde_json::json!({"name": "foo"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    let resp = client
        .post(format!("http://{}/sessions", addr))
        .json(&serde_json::json!({"name": "bar"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // GET /sessions
    let resp = client
        .get(format!("http://{}/sessions", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert_eq!(body.len(), 2, "Expected 2 sessions in list");

    let mut names: Vec<String> = body
        .iter()
        .map(|v| v["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(names, vec!["bar", "foo"]);
}

// ── Test 4: Session isolation ────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn test_session_isolation() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Create two sessions
    let resp = client
        .post(format!("http://{}/sessions", addr))
        .json(&serde_json::json!({"name": "alpha"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    let resp = client
        .post(format!("http://{}/sessions", addr))
        .json(&serde_json::json!({"name": "beta"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Send a unique marker string as input to "alpha"
    let marker = "ISOLATION_MARKER_99887";
    let cmd = format!("echo {}\n", marker);
    let resp = client
        .post(format!("http://{}/sessions/alpha/input", addr))
        .body(cmd)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    // Wait for PTY to process
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Check that "beta" session's screen does NOT contain the marker
    let resp = client
        .get(format!("http://{}/sessions/beta/screen?format=plain", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let screen_text = serde_json::to_string(&body).unwrap();
    assert!(
        !screen_text.contains(marker),
        "Beta session screen should NOT contain the marker '{}' sent to alpha. Got: {}",
        marker,
        screen_text
    );
}

// ── Test 5: Rename session ───────────────────────────────────────

#[tokio::test]
async fn test_rename_session() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Create session "old-name"
    let resp = client
        .post(format!("http://{}/sessions", addr))
        .json(&serde_json::json!({"name": "old-name"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // PATCH /sessions/old-name with {"name": "new-name"}
    let resp = client
        .patch(format!("http://{}/sessions/old-name", addr))
        .json(&serde_json::json!({"name": "new-name"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "Expected 200 OK for rename");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "new-name");

    // GET /sessions/new-name should return 200
    let resp = client
        .get(format!("http://{}/sessions/new-name", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // GET /sessions/old-name should return 404
    let resp = client
        .get(format!("http://{}/sessions/old-name", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// ── Test 6: Delete session ───────────────────────────────────────

#[tokio::test]
async fn test_delete_session() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Create session "doomed"
    let resp = client
        .post(format!("http://{}/sessions", addr))
        .json(&serde_json::json!({"name": "doomed"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // DELETE /sessions/doomed
    let resp = client
        .delete(format!("http://{}/sessions/doomed", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204, "Expected 204 No Content for delete");

    // GET /sessions/doomed should return 404
    let resp = client
        .get(format!("http://{}/sessions/doomed", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // GET /sessions should not contain "doomed"
    let resp = client
        .get(format!("http://{}/sessions", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let names: Vec<String> = body
        .iter()
        .map(|v| v["name"].as_str().unwrap().to_string())
        .collect();
    assert!(
        !names.contains(&"doomed".to_string()),
        "Session list should not contain 'doomed' after deletion, got: {:?}",
        names
    );
}

// ── Test 7: Duplicate name returns 409 ───────────────────────────

#[tokio::test]
async fn test_create_duplicate_name_returns_409() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Create session "taken"
    let resp = client
        .post(format!("http://{}/sessions", addr))
        .json(&serde_json::json!({"name": "taken"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Try to create another session with the same name
    let resp = client
        .post(format!("http://{}/sessions", addr))
        .json(&serde_json::json!({"name": "taken"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        409,
        "Expected 409 Conflict for duplicate session name"
    );
}

// ── Test 8: Nonexistent session returns 404 ──────────────────────

#[tokio::test]
async fn test_nonexistent_session_returns_404() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // GET /sessions/nonexistent returns 404
    let resp = client
        .get(format!("http://{}/sessions/nonexistent", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // DELETE /sessions/nonexistent returns 404
    let resp = client
        .delete(format!("http://{}/sessions/nonexistent", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // POST /sessions/nonexistent/input returns 404
    let resp = client
        .post(format!("http://{}/sessions/nonexistent/input", addr))
        .body("some input")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// ── Test 9: Per-session endpoints work after create ──────────────

#[tokio::test]
async fn test_per_session_endpoints_work_after_create() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Create a session
    let resp = client
        .post(format!("http://{}/sessions", addr))
        .json(&serde_json::json!({"name": "test-session"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // GET /sessions/test-session/screen should work
    let resp = client
        .get(format!(
            "http://{}/sessions/test-session/screen?format=plain",
            addr
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "Expected 200 for /screen on newly created session"
    );

    // GET /sessions/test-session/input/mode should work
    let resp = client
        .get(format!(
            "http://{}/sessions/test-session/input/mode",
            addr
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "Expected 200 for /input/mode on newly created session"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["mode"], "passthrough");

    // GET /sessions/test-session/scrollback should work
    let resp = client
        .get(format!(
            "http://{}/sessions/test-session/scrollback?format=plain",
            addr
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "Expected 200 for /scrollback on newly created session"
    );

    // POST /sessions/test-session/input should work
    let resp = client
        .post(format!(
            "http://{}/sessions/test-session/input",
            addr
        ))
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        204,
        "Expected 204 for /input on newly created session"
    );

    // GET /sessions/test-session/overlay should work (empty list)
    let resp = client
        .get(format!(
            "http://{}/sessions/test-session/overlay",
            addr
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "Expected 200 for /overlay list on newly created session"
    );
    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert!(body.is_empty(), "Overlay list should be empty initially");

    // GET /sessions/test-session/panel should work (empty list)
    let resp = client
        .get(format!(
            "http://{}/sessions/test-session/panel",
            addr
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "Expected 200 for /panel list on newly created session"
    );
    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert!(body.is_empty(), "Panel list should be empty initially");
}
