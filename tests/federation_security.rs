//! Security integration tests for the federation API.
//!
//! Tests SSRF prevention, invalid address rejection, token leak prevention,
//! and invalid hostname rejection.

use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use wsh::api::{router, AppState, RouterConfig, ServerConfig};
use wsh::federation::registry::{BackendEntry, BackendHealth, BackendRole};
use wsh::session::SessionRegistry;
use wsh::shutdown::ShutdownCoordinator;

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

fn create_test_app() -> axum::Router {
    let (app, _) = create_test_app_with_registry();
    app
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

// ══════════════════════════════════════════════════════════════════
// Test 1: SSRF prevention -- reject localhost/loopback backends
// ══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn ssrf_reject_ipv4_loopback() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/servers"))
        .json(&serde_json::json!({"address": "127.0.0.1:8080"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "127.0.0.1 should be rejected");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_request");
}

#[tokio::test]
async fn ssrf_reject_localhost() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/servers"))
        .json(&serde_json::json!({"address": "localhost:8080"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "localhost should be rejected");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_request");
}

#[tokio::test]
async fn ssrf_reject_ipv6_loopback() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/servers"))
        .json(&serde_json::json!({"address": "[::1]:8080"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "[::1] should be rejected");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_request");
}

#[tokio::test]
async fn ssrf_reject_unspecified() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/servers"))
        .json(&serde_json::json!({"address": "0.0.0.0:8080"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "0.0.0.0 should be rejected");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_request");
}

// ══════════════════════════════════════════════════════════════════
// Test 2: Invalid address format rejected
// ══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn reject_address_with_scheme() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/servers"))
        .json(&serde_json::json!({"address": "http://example.com:8080"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "address with scheme should be rejected");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_request");
}

#[tokio::test]
async fn reject_address_missing_port() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/servers"))
        .json(&serde_json::json!({"address": "example.com"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "address without port should be rejected");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_request");
}

#[tokio::test]
async fn reject_address_empty_host() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/servers"))
        .json(&serde_json::json!({"address": ":8080"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "address with empty host should be rejected");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_request");
}

// ══════════════════════════════════════════════════════════════════
// Test 3: Token not leaked in GET /servers
// ══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn token_not_leaked_in_list_servers() {
    let (app, backends) = create_test_app_with_registry();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Add a backend directly to the registry with a secret token.
    backends
        .add(BackendEntry {
            address: "10.0.1.50:8080".into(),
            token: Some("super-secret-token-12345".into()),
            hostname: Some("backend-1".into()),
            health: BackendHealth::Healthy,
            role: BackendRole::Member,
        })
        .unwrap();

    // GET /servers should NOT contain the token value.
    let resp = client
        .get(format!("http://{addr}/servers"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body_text = resp.text().await.unwrap();
    assert!(
        !body_text.contains("super-secret-token-12345"),
        "GET /servers must NOT leak backend tokens in the response. Body: {}",
        body_text
    );
}

#[tokio::test]
async fn token_not_leaked_in_get_server() {
    let (app, backends) = create_test_app_with_registry();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Add a backend directly to the registry with a secret token.
    backends
        .add(BackendEntry {
            address: "10.0.1.51:8080".into(),
            token: Some("another-secret-token".into()),
            hostname: Some("backend-2".into()),
            health: BackendHealth::Healthy,
            role: BackendRole::Member,
        })
        .unwrap();

    // GET /servers/backend-2 should NOT contain the token value.
    let resp = client
        .get(format!("http://{addr}/servers/backend-2"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body_text = resp.text().await.unwrap();
    assert!(
        !body_text.contains("another-secret-token"),
        "GET /servers/{{hostname}} must NOT leak backend tokens. Body: {}",
        body_text
    );
}

// ══════════════════════════════════════════════════════════════════
// Test 4: Invalid hostname rejected
// ══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn registry_rejects_invalid_hostname_on_add() {
    // This tests the registry directly (since hostname is set via connection,
    // not via POST /servers body). The registry should reject an entry with
    // an invalid hostname.
    let registry = wsh::federation::registry::BackendRegistry::new();
    let result = registry.add(BackendEntry {
        address: "10.0.1.60:8080".into(),
        token: None,
        hostname: Some("-invalid".into()),
        health: BackendHealth::Connecting,
        role: BackendRole::Member,
    });
    assert!(result.is_err(), "hostname starting with hyphen should be rejected");
}

#[tokio::test]
async fn registry_rejects_hostname_with_spaces() {
    let registry = wsh::federation::registry::BackendRegistry::new();
    let result = registry.add(BackendEntry {
        address: "10.0.1.61:8080".into(),
        token: None,
        hostname: Some("invalid host".into()),
        health: BackendHealth::Connecting,
        role: BackendRole::Member,
    });
    assert!(result.is_err(), "hostname with spaces should be rejected");
}

#[tokio::test]
async fn valid_address_accepted() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // A valid non-loopback address should be accepted.
    let resp = client
        .post(format!("http://{addr}/servers"))
        .json(&serde_json::json!({"address": "10.0.1.100:8080"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201, "valid address should be accepted");
}
