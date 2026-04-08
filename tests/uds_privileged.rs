//! Integration tests for UDS-privileged endpoint access control.
//!
//! These tests verify that endpoints restricted to Unix domain socket access
//! (`/server/shutdown`, `/server/token`, `/server/reload-config`) correctly:
//! - Allow access when no transport extension is present (backward compat / unit tests)
//! - Allow access when Transport::Uds is present
//! - Reject access with 403 when Transport::Tcp is present

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;
use wsh::api::{self, AppState, RouterConfig};
use wsh::session::SessionRegistry;
use wsh::shutdown::ShutdownCoordinator;

/// Construct a minimal AppState for testing privileged endpoints.
fn make_state() -> AppState {
    AppState {
        sessions: SessionRegistry::new(),
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(api::ServerConfig::new(false)),
        server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        ticket_store: std::sync::Arc::new(wsh::api::ticket::TicketStore::new()),
        backends: wsh::federation::registry::BackendRegistry::new(),
        federation: std::sync::Arc::new(tokio::sync::Mutex::new(
            wsh::federation::manager::FederationManager::new(),
        )),
        ip_access: None,
        hostname: "test".to_string(),
        federation_config_path: None,
        local_token: None,
        default_backend_token: None,
        server_id: "test-server-id".to_string(),
        shutdown_notify: tokio_util::sync::CancellationToken::new(),
        tcp_addr: None,
        instance_name: "test".to_string(),
        http_socket_path: std::path::PathBuf::from("/tmp/test.http.sock"),
            recordings: wsh::recording::RecordingRegistry::new(),
    }
}

/// Build a router from the given state.
fn make_app(state: AppState) -> axum::Router {
    api::router(state, RouterConfig::default())
}

/// Collect response body as JSON.
async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

// ── GET /server/token ────────────────────────────────────────────

#[tokio::test]
async fn test_server_token_allowed_without_transport() {
    let state = make_state();
    let app = make_app(state);

    let req = Request::builder()
        .uri("/server/token")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "GET /server/token without transport extension should return 200"
    );
}

#[tokio::test]
async fn test_server_token_allowed_via_uds() {
    let state = make_state();
    let app = make_app(state);

    let req = Request::builder()
        .uri("/server/token")
        .extension(wsh::api::transport::Transport::Uds {
            uid: 1000,
            gid: 1000,
            pid: Some(42),
        })
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "GET /server/token with UDS transport should return 200"
    );
}

#[tokio::test]
async fn test_server_token_rejected_via_tcp() {
    let state = make_state();
    let app = make_app(state);

    let req = Request::builder()
        .uri("/server/token")
        .extension(wsh::api::transport::Transport::Tcp {
            addr: "127.0.0.1:1234".parse().unwrap(),
        })
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "GET /server/token with TCP transport should return 403"
    );
}

#[tokio::test]
async fn test_server_token_returns_configured_token() {
    let mut state = make_state();
    state.local_token = Some("test-secret-token".to_string());
    let app = make_app(state);

    let req = Request::builder()
        .uri("/server/token")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(
        json["token"], "test-secret-token",
        "GET /server/token should return the configured token"
    );
}

// ── POST /server/shutdown ────────────────────────────────────────

#[tokio::test]
async fn test_server_shutdown_allowed_via_uds() {
    let state = make_state();
    let shutdown_notify = state.shutdown_notify.clone();
    let app = make_app(state);

    assert!(
        !shutdown_notify.is_cancelled(),
        "shutdown_notify should not be cancelled before request"
    );

    let req = Request::builder()
        .method("POST")
        .uri("/server/shutdown")
        .extension(wsh::api::transport::Transport::Uds {
            uid: 1000,
            gid: 1000,
            pid: Some(42),
        })
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "POST /server/shutdown with UDS transport should return 200"
    );

    assert!(
        shutdown_notify.is_cancelled(),
        "shutdown_notify should be cancelled after successful shutdown request"
    );
}

#[tokio::test]
async fn test_server_shutdown_rejected_via_tcp() {
    let state = make_state();
    let shutdown_notify = state.shutdown_notify.clone();
    let app = make_app(state);

    let req = Request::builder()
        .method("POST")
        .uri("/server/shutdown")
        .extension(wsh::api::transport::Transport::Tcp {
            addr: "127.0.0.1:1234".parse().unwrap(),
        })
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "POST /server/shutdown with TCP transport should return 403"
    );

    assert!(
        !shutdown_notify.is_cancelled(),
        "shutdown_notify should NOT be cancelled after rejected shutdown request"
    );
}

// ── POST /server/reload-config ───────────────────────────────────

#[tokio::test]
async fn test_server_reload_config_rejected_via_tcp() {
    let state = make_state();
    let app = make_app(state);

    let req = Request::builder()
        .method("POST")
        .uri("/server/reload-config")
        .extension(wsh::api::transport::Transport::Tcp {
            addr: "127.0.0.1:1234".parse().unwrap(),
        })
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "POST /server/reload-config with TCP transport should return 403"
    );
}

#[tokio::test]
async fn test_server_reload_config_no_config_path() {
    // federation_config_path is None, so the endpoint should return 400
    let state = make_state();
    let app = make_app(state);

    let req = Request::builder()
        .method("POST")
        .uri("/server/reload-config")
        .extension(wsh::api::transport::Transport::Uds {
            uid: 1000,
            gid: 1000,
            pid: Some(42),
        })
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "POST /server/reload-config without config path should return 400"
    );

    let json = body_json(resp).await;
    let msg = json["error"]["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("no federation config file"),
        "error message should mention missing config file, got: {}",
        msg
    );
}
