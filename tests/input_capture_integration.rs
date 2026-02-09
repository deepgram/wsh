//! Integration tests for input capture API endpoints.
//!
//! These tests verify the input mode switching flow:
//! - Verify default mode is passthrough (GET /input/mode)
//! - Capture with POST /input/capture
//! - Verify mode is capture
//! - Release with POST /input/release
//! - Verify mode is passthrough

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
async fn test_input_capture_flow() {
    let state = create_test_state();
    let app = router(state, None);

    // Step 1: Verify default mode is passthrough
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/input/mode")
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
    assert_eq!(json["mode"], "passthrough", "default mode should be passthrough");

    // Step 2: Capture with POST /input/capture
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/input/capture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Step 3: Verify mode is capture
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/input/mode")
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
    assert_eq!(json["mode"], "capture", "mode should be capture after /input/capture");

    // Step 4: Release with POST /input/release
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/input/release")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Step 5: Verify mode is passthrough
    let response = app
        .oneshot(
            Request::builder()
                .uri("/input/mode")
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
    assert_eq!(json["mode"], "passthrough", "mode should be passthrough after /input/release");
}

#[tokio::test]
async fn test_input_capture_idempotent() {
    let state = create_test_state();
    let app = router(state, None);

    // Capture multiple times should be idempotent
    for _ in 0..3 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/input/capture")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    // Should still be in capture mode
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/input/mode")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["mode"], "capture");

    // Release multiple times should be idempotent
    for _ in 0..3 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/input/release")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    // Should be in passthrough mode
    let response = app
        .oneshot(
            Request::builder()
                .uri("/input/mode")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["mode"], "passthrough");
}

#[tokio::test]
async fn test_input_mode_wrong_method() {
    let state = create_test_state();
    let app = router(state, None);

    // POST on /input/mode should fail (only GET is allowed)
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/input/mode")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

    // GET on /input/capture should fail (only POST is allowed)
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/input/capture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

    // GET on /input/release should fail (only POST is allowed)
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/input/release")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn test_input_mode_state_shared_across_requests() {
    // Test that state is properly shared across multiple requests
    let state = create_test_state();
    let app = router(state, None);

    // Capture mode
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/input/capture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Make multiple GET requests to verify state persists
    for _ in 0..5 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/input/mode")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["mode"], "capture", "mode should persist across requests");
    }
}
