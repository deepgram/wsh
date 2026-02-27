//! Integration tests for the federation server management API endpoints:
//! - GET /servers
//! - POST /servers
//! - GET /servers/{hostname}
//! - DELETE /servers/{hostname}

use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use wsh::api::{router, AppState, RouterConfig, ServerConfig};
use wsh::session::SessionRegistry;
use wsh::shutdown::ShutdownCoordinator;

/// Creates a test app with an empty session registry and no backends.
/// The backends registry is shared between AppState and FederationManager.
fn create_test_app() -> axum::Router {
    let registry = SessionRegistry::new();
    let federation_manager = wsh::federation::manager::FederationManager::new();
    let backends = federation_manager.registry().clone();
    let state = AppState {
        sessions: registry,
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(ServerConfig::new(false)),
        server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        ticket_store: std::sync::Arc::new(wsh::api::ticket::TicketStore::new()),
        backends,
        federation: std::sync::Arc::new(tokio::sync::Mutex::new(federation_manager)),
        hostname: "test-host".to_string(),
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

// ── Test 1: GET /servers includes self ────────────────────────────

#[tokio::test]
async fn list_servers_includes_self() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{addr}/servers"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let servers = body.as_array().expect("response should be an array");
    assert_eq!(servers.len(), 1, "should have exactly 1 server (self)");

    let self_entry = &servers[0];
    assert_eq!(self_entry["hostname"], "test-host");
    assert_eq!(self_entry["address"], "local");
    assert_eq!(self_entry["health"], "healthy");
    assert_eq!(self_entry["role"], "member");
    assert!(
        self_entry["sessions"].is_number(),
        "sessions count should be a number"
    );
}

// ── Test 2: POST /servers returns 201 ────────────────────────────

#[tokio::test]
async fn add_server_returns_created() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/servers"))
        .json(&serde_json::json!({
            "address": "10.0.1.99:8080",
            "token": "my-token"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["address"], "10.0.1.99:8080");
    assert_eq!(body["health"], "connecting");

    // GET /servers should now return 2 entries (self + new)
    let resp = client
        .get(format!("http://{addr}/servers"))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let servers = body.as_array().unwrap();
    assert_eq!(servers.len(), 2);
}

// ── Test 3: POST /servers with duplicate returns 409 ─────────────

#[tokio::test]
async fn add_duplicate_server_returns_conflict() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // First add
    let resp = client
        .post(format!("http://{addr}/servers"))
        .json(&serde_json::json!({ "address": "10.0.1.50:8080" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Duplicate add
    let resp = client
        .post(format!("http://{addr}/servers"))
        .json(&serde_json::json!({ "address": "10.0.1.50:8080" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "server_already_registered");
}

// ── Test 4: DELETE /servers/{hostname} unknown returns 404 ───────

#[tokio::test]
async fn delete_unknown_server_returns_not_found() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .delete(format!("http://{addr}/servers/nonexistent"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "server_not_found");
}

// ── Test 5: GET /servers/{hostname} self returns details ─────────

#[tokio::test]
async fn get_server_self_returns_details() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{addr}/servers/test-host"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["hostname"], "test-host");
    assert_eq!(body["address"], "local");
    assert_eq!(body["health"], "healthy");
}

// ── Test 6: GET /servers/{hostname} unknown returns 404 ──────────

#[tokio::test]
async fn get_unknown_server_returns_not_found() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{addr}/servers/does-not-exist"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "server_not_found");
}

// ── Test 7: POST /servers without address returns 400 ────────────

#[tokio::test]
async fn add_server_missing_address_returns_bad_request() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/servers"))
        .json(&serde_json::json!({ "token": "my-token" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_request");
}
