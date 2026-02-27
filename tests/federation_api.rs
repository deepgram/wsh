//! Integration tests for the federation server management API endpoints:
//! - GET /servers
//! - POST /servers
//! - GET /servers/{hostname}
//! - DELETE /servers/{hostname}
//!
//! And proxy/routing tests for the `?server=` query parameter:
//! - Session operations with ?server=<self> resolve locally
//! - Session operations with ?server=<unknown> return 404
//! - Session listing with no ?server= returns local sessions
//! - Session creation with server=<self> creates locally
//! - Session creation with server=<unknown> returns 404

use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use wsh::api::{router, AppState, RouterConfig, ServerConfig};
use wsh::session::SessionRegistry;
use wsh::shutdown::ShutdownCoordinator;

/// Creates a test app with an empty session registry and no backends.
/// The backends registry is shared between AppState and FederationManager.
fn create_test_app() -> axum::Router {
    let (app, _) = create_test_app_with_registry();
    app
}

/// Creates a test app and returns the shared BackendRegistry for direct manipulation.
fn create_test_app_with_registry() -> (axum::Router, wsh::federation::registry::BackendRegistry) {
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
        backends: backends.clone(),
        federation: std::sync::Arc::new(tokio::sync::Mutex::new(federation_manager)),
        hostname: "test-host".to_string(),
        federation_config_path: None,
        local_token: None,
        default_backend_token: None,
    };
    (router(state, RouterConfig::default()), backends)
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

// ═══════════════════════════════════════════════════════════════════
// ?server= query parameter proxy routing tests
// ═══════════════════════════════════════════════════════════════════

// ── Test 8: GET /sessions (no ?server=) returns local sessions ────

#[tokio::test]
async fn list_sessions_no_server_returns_local() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Create a session first
    let resp = client
        .post(format!("http://{addr}/sessions"))
        .json(&serde_json::json!({"name": "alpha"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // List sessions without ?server= (no backends registered, returns local only)
    let resp = client
        .get(format!("http://{addr}/sessions"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let sessions = body.as_array().expect("response should be an array");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["name"], "alpha");
    assert_eq!(
        sessions[0]["server"], "test-host",
        "session should include server field matching hostname"
    );
}

// ── Test 9: GET /sessions?server=<self> returns local sessions ────

#[tokio::test]
async fn list_sessions_with_self_server_returns_local() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Create a session
    let resp = client
        .post(format!("http://{addr}/sessions"))
        .json(&serde_json::json!({"name": "beta"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // List sessions with ?server=test-host (our own hostname)
    let resp = client
        .get(format!("http://{addr}/sessions?server=test-host"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let sessions = body.as_array().expect("response should be an array");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["name"], "beta");
}

// ── Test 10: GET /sessions?server=<unknown> returns 404 ───────────

#[tokio::test]
async fn list_sessions_with_unknown_server_returns_404() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{addr}/sessions?server=nonexistent"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "server_not_found");
}

// ── Test 11: GET /sessions/:name?server=<self> resolves locally ───

#[tokio::test]
async fn get_session_with_self_server_resolves_locally() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Create session
    let resp = client
        .post(format!("http://{addr}/sessions"))
        .json(&serde_json::json!({"name": "gamma"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // GET with ?server=test-host (self)
    let resp = client
        .get(format!(
            "http://{addr}/sessions/gamma?server=test-host"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "gamma");
}

// ── Test 12: GET /sessions/:name?server=<unknown> returns 404 ─────

#[tokio::test]
async fn get_session_with_unknown_server_returns_404() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "http://{addr}/sessions/whatever?server=nonexistent"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "server_not_found");
}

// ── Test 13: GET /sessions/:name/screen?server=<self> resolves locally

#[tokio::test]
async fn get_screen_with_self_server_resolves_locally() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Create session
    let resp = client
        .post(format!("http://{addr}/sessions"))
        .json(&serde_json::json!({"name": "delta"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // GET screen with ?server=test-host (self)
    let resp = client
        .get(format!(
            "http://{addr}/sessions/delta/screen?server=test-host"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

// ── Test 14: GET /sessions/:name/screen?server=<unknown> returns 404

#[tokio::test]
async fn get_screen_with_unknown_server_returns_404() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "http://{addr}/sessions/whatever/screen?server=nonexistent"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "server_not_found");
}

// ── Test 15: DELETE /sessions/:name?server=<unknown> returns 404 ──

#[tokio::test]
async fn kill_session_with_unknown_server_returns_404() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .delete(format!(
            "http://{addr}/sessions/whatever?server=nonexistent"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "server_not_found");
}

// ── Test 16: DELETE /sessions/:name?server=<self> resolves locally ─

#[tokio::test]
async fn kill_session_with_self_server_resolves_locally() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Create a session
    let resp = client
        .post(format!("http://{addr}/sessions"))
        .json(&serde_json::json!({"name": "epsilon"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Kill with ?server=test-host (self)
    let resp = client
        .delete(format!(
            "http://{addr}/sessions/epsilon?server=test-host"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    // Verify gone
    let resp = client
        .get(format!("http://{addr}/sessions/epsilon"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// ── Test 17: POST /sessions with server=<self> creates locally ────

#[tokio::test]
async fn create_session_with_self_server_creates_locally() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/sessions"))
        .json(&serde_json::json!({
            "name": "zeta",
            "server": "test-host"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "zeta");
    assert_eq!(body["server"], "test-host");
}

// ── Test 18: POST /sessions with server=<unknown> returns 404 ─────

#[tokio::test]
async fn create_session_with_unknown_server_returns_404() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/sessions"))
        .json(&serde_json::json!({
            "name": "eta",
            "server": "nonexistent"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "server_not_found");
}

// ── Test 19: GET /sessions/:name/scrollback?server=<unknown> returns 404

#[tokio::test]
async fn get_scrollback_with_unknown_server_returns_404() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "http://{addr}/sessions/whatever/scrollback?server=nonexistent"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "server_not_found");
}

// ── Test 20: POST /sessions/:name/input?server=<unknown> returns 404

#[tokio::test]
async fn send_input_with_unknown_server_returns_404() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!(
            "http://{addr}/sessions/whatever/input?server=nonexistent"
        ))
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "server_not_found");
}

// ── Test 21: ?server= with unavailable backend returns 503 ────────

#[tokio::test]
async fn server_param_with_unavailable_backend_returns_503() {
    use wsh::federation::registry::{BackendEntry, BackendHealth, BackendRole};

    let (app, backends) = create_test_app_with_registry();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Directly add a backend with a known hostname and Unavailable health.
    backends
        .add(BackendEntry {
            address: "10.99.99.99:9999".into(),
            token: None,
            hostname: Some("unavailable-host".into()),
            health: BackendHealth::Unavailable,
            role: BackendRole::Member,
        })
        .unwrap();

    // GET screen with ?server= pointing to the unavailable backend → 503
    let resp = client
        .get(format!(
            "http://{addr}/sessions/whatever/screen?server=unavailable-host"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 503);

    // GET sessions with ?server= pointing to the unavailable backend → 503
    let resp = client
        .get(format!(
            "http://{addr}/sessions?server=unavailable-host"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 503);

    // POST sessions with server in body pointing to unavailable backend → 503
    let resp = client
        .post(format!("http://{addr}/sessions"))
        .json(&serde_json::json!({
            "command": "echo hello",
            "server": "unavailable-host"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 503);
}

// ── Test 22: No ?server= always means local (security property) ───

#[tokio::test]
async fn no_server_param_always_local() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Create session
    let resp = client
        .post(format!("http://{addr}/sessions"))
        .json(&serde_json::json!({"name": "local-test"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // All session operations without ?server= work as local
    let resp = client
        .get(format!("http://{addr}/sessions/local-test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let resp = client
        .get(format!("http://{addr}/sessions/local-test/screen"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let resp = client
        .get(format!("http://{addr}/sessions/local-test/scrollback"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}
