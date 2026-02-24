//! Integration tests for input capture API endpoints.
//!
//! These tests verify the input mode switching flow:
//! - Verify default mode is passthrough (GET /input/mode)
//! - Capture with POST /input/capture
//! - Verify mode is capture
//! - Release with POST /input/release
//! - Verify mode is passthrough

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;
use wsh::api::{router, RouterConfig};

#[tokio::test]
async fn test_input_capture_flow() {
    let (state, _, _, _ptx) = common::create_test_state();
    let app = router(state, RouterConfig::default());

    // Step 1: Verify default mode is passthrough
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/sessions/test/input/mode")
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
                .uri("/sessions/test/input/capture")
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
                .uri("/sessions/test/input/mode")
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
                .uri("/sessions/test/input/release")
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
                .uri("/sessions/test/input/mode")
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
    let (state, _, _, _ptx) = common::create_test_state();
    let app = router(state, RouterConfig::default());

    // Capture multiple times should be idempotent
    for _ in 0..3 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/test/input/capture")
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
                .uri("/sessions/test/input/mode")
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
                    .uri("/sessions/test/input/release")
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
                .uri("/sessions/test/input/mode")
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
    let (state, _, _, _ptx) = common::create_test_state();
    let app = router(state, RouterConfig::default());

    // POST on /input/mode should fail (only GET is allowed)
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/input/mode")
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
                .uri("/sessions/test/input/capture")
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
                .uri("/sessions/test/input/release")
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
    let (state, _, _, _ptx) = common::create_test_state();
    let app = router(state, RouterConfig::default());

    // Capture mode
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/input/capture")
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
                    .uri("/sessions/test/input/mode")
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

#[tokio::test]
async fn test_focus_and_unfocus_flow() {
    let (state, _, _, _ptx) = common::create_test_state();
    let app = router(state, RouterConfig::default());

    // Create a focusable overlay
    let create_body = serde_json::json!({
        "x": 0,
        "y": 0,
        "width": 40,
        "height": 5,
        "focusable": true,
        "spans": [{ "text": "Focusable overlay" }]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/overlay")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let overlay_id = json["id"].as_str().unwrap().to_string();

    // Verify no focus initially
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/sessions/test/input/focus")
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
    assert!(json["focused"].is_null(), "initially no focus");

    // Focus the overlay
    let focus_body = serde_json::json!({ "id": overlay_id });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/input/focus")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&focus_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify focus is set
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/sessions/test/input/focus")
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
    assert_eq!(json["focused"], overlay_id);

    // Unfocus
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/input/unfocus")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify focus is cleared
    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions/test/input/focus")
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
    assert!(json["focused"].is_null(), "focus should be cleared after unfocus");
}

#[tokio::test]
async fn test_focus_cleared_on_input_release() {
    let (state, _, _, _ptx) = common::create_test_state();
    let app = router(state, RouterConfig::default());

    // Create a focusable overlay
    let create_body = serde_json::json!({
        "x": 0,
        "y": 0,
        "width": 40,
        "height": 5,
        "focusable": true,
        "spans": [{ "text": "Focusable" }]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/overlay")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let overlay_id = json["id"].as_str().unwrap().to_string();

    // Capture input and focus the overlay
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/input/capture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let focus_body = serde_json::json!({ "id": overlay_id });
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/input/focus")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&focus_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Verify focus is set
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/sessions/test/input/focus")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["focused"], overlay_id);

    // Release input -- should clear focus
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/input/release")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify focus is cleared
    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions/test/input/focus")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["focused"].is_null(), "focus should be cleared after input release");
}

#[tokio::test]
async fn test_focus_cleared_on_element_delete() {
    let (state, _, _, _ptx) = common::create_test_state();
    let app = router(state, RouterConfig::default());

    // Create a focusable overlay
    let create_body = serde_json::json!({
        "x": 0,
        "y": 0,
        "width": 40,
        "height": 5,
        "focusable": true,
        "spans": [{ "text": "Will be deleted" }]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/overlay")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let overlay_id = json["id"].as_str().unwrap().to_string();

    // Focus the overlay
    let focus_body = serde_json::json!({ "id": overlay_id });
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/input/focus")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&focus_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Verify focus is set
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/sessions/test/input/focus")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["focused"], overlay_id);

    // Delete the focused overlay
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/sessions/test/overlay/{}", overlay_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify focus is cleared
    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions/test/input/focus")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["focused"].is_null(), "focus should be cleared when focused element is deleted");
}

#[tokio::test]
async fn test_focus_non_focusable_returns_400() {
    let (state, _, _, _ptx) = common::create_test_state();
    let app = router(state, RouterConfig::default());

    // Create a non-focusable overlay (focusable defaults to false)
    let create_body = serde_json::json!({
        "x": 0,
        "y": 0,
        "width": 40,
        "height": 5,
        "spans": [{ "text": "Not focusable" }]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/overlay")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let overlay_id = json["id"].as_str().unwrap().to_string();

    // Attempt to focus the non-focusable overlay
    let focus_body = serde_json::json!({ "id": overlay_id });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/input/focus")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&focus_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "not_focusable");
}
