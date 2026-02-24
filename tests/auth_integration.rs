//! Integration tests for auth middleware enforcement through the full router.
//!
//! These tests verify end-to-end auth behavior:
//! - Protected routes require a token when one is configured
//! - Health endpoint is exempt from auth
//! - Bearer header and query param both grant access
//! - Wrong token returns 403
//! - No auth enforcement when token is None

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
async fn test_query_param_token_grants_access() {
    let (state, _, _, _ptx) = common::create_test_state();
    let app = router(state, RouterConfig { token: Some("test-token".to_string()), ..Default::default() });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions/test/screen?token=test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
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
