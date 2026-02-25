//! Integration tests for auth middleware enforcement through the full router.
//!
//! These tests verify end-to-end auth behavior:
//! - Protected routes require a token when one is configured
//! - Health endpoint is exempt from auth
//! - Bearer header grants access
//! - Query param ?token= is rejected (removed in favour of ticket exchange)
//! - Wrong token returns 403
//! - No auth enforcement when token is None
//! - Ticket exchange via POST /auth/ws-ticket

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;
use wsh::api::{router, RouterConfig};

#[tokio::test]
async fn test_auth_required_on_protected_routes() {
    let (state, _, _, _ptx) = common::create_test_state();
    let app = router(state, RouterConfig { token: Some("test-token".to_string()), ..Default::default() });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions/test/screen")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "auth_required");
}

#[tokio::test]
async fn test_health_exempt_from_auth() {
    let (state, _, _, _ptx) = common::create_test_state();
    let app = router(state, RouterConfig { token: Some("test-token".to_string()), ..Default::default() });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_bearer_token_grants_access() {
    let (state, _, _, _ptx) = common::create_test_state();
    let app = router(state, RouterConfig { token: Some("test-token".to_string()), ..Default::default() });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions/test/screen")
                .header("authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_query_param_token_rejected() {
    let (state, _, _, _ptx) = common::create_test_state();
    let app = router(state, RouterConfig { token: Some("test-token".to_string()), ..Default::default() });

    // ?token= query param auth was removed — should be rejected
    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions/test/screen?token=test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_ws_ticket_exchange() {
    let (state, _, _, _ptx) = common::create_test_state();
    let app = router(state, RouterConfig { token: Some("test-token".to_string()), ..Default::default() });

    // Acquire a ticket via Bearer auth
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/ws-ticket")
                .header("authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let ticket = json["ticket"].as_str().unwrap();
    assert_eq!(ticket.len(), 32);

    // Use the ticket on a WS upgrade request.
    // In the tower::ServiceExt::oneshot() test harness the full HTTP/1.1
    // upgrade dance isn't supported, so we won't get a 101.  But the auth
    // middleware runs first — if it rejects, we'd see 401/403.  Getting
    // any other status proves the ticket was accepted.
    let response = app
        .oneshot(
            Request::builder()
                .uri(&format!("/sessions/test/ws/json?ticket={}", ticket))
                .header("upgrade", "websocket")
                .header("connection", "Upgrade")
                .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                .header("sec-websocket-version", "13")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Auth must have passed (not 401 or 403)
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
    assert_ne!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_wrong_token_returns_403() {
    let (state, _, _, _ptx) = common::create_test_state();
    let app = router(state, RouterConfig { token: Some("test-token".to_string()), ..Default::default() });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions/test/screen")
                .header("authorization", "Bearer wrong-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "auth_invalid");
}

#[tokio::test]
async fn test_no_auth_when_token_is_none() {
    let (state, _, _, _ptx) = common::create_test_state();
    let app = router(state, RouterConfig::default());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions/test/screen")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
