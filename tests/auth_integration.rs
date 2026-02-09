//! Integration tests for auth middleware enforcement through the full router.
//!
//! These tests verify end-to-end auth behavior:
//! - Protected routes require a token when one is configured
//! - Health endpoint is exempt from auth
//! - Bearer header and query param both grant access
//! - Wrong token returns 403
//! - No auth enforcement when token is None

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use bytes::Bytes;
use tokio::sync::mpsc;
use tower::ServiceExt;
use wsh::api::{router, AppState};
use wsh::broker::Broker;
use wsh::input::{InputBroadcaster, InputMode};
use wsh::overlay::OverlayStore;
use wsh::parser::Parser;
use wsh::shutdown::ShutdownCoordinator;

/// Creates a test state for integration tests.
fn create_test_state() -> AppState {
    let (input_tx, _) = mpsc::channel::<Bytes>(64);
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    AppState {
        input_tx,
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
    }
}

#[tokio::test]
async fn test_auth_required_on_protected_routes() {
    let state = create_test_state();
    let app = router(state, Some("test-token".to_string()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/screen")
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
    let state = create_test_state();
    let app = router(state, Some("test-token".to_string()));

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
    let state = create_test_state();
    let app = router(state, Some("test-token".to_string()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/screen")
                .header("authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_query_param_token_grants_access() {
    let state = create_test_state();
    let app = router(state, Some("test-token".to_string()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/screen?token=test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_wrong_token_returns_403() {
    let state = create_test_state();
    let app = router(state, Some("test-token".to_string()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/screen")
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
    let state = create_test_state();
    let app = router(state, None);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/screen")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
