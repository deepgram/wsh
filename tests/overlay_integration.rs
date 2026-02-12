//! Integration tests for overlay API endpoints.
//!
//! These tests verify the full CRUD flow for overlays:
//! - Create overlay with POST /overlay
//! - Get overlay with GET /overlay/:id
//! - Update overlay with PUT /overlay/:id
//! - Delete overlay with DELETE /overlay/:id
//! - Verify deleted overlay returns 404

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;
use wsh::api::router;

#[tokio::test]
async fn test_overlay_crud_flow() {
    let (state, _, _) = common::create_test_state();
    let app = router(state, None);

    // Step 1: Create overlay with styled span (yellow, bold)
    let create_body = serde_json::json!({
        "x": 10,
        "y": 5,
        "width": 80,
        "height": 1,
        "spans": [
            {
                "text": "Hello World",
                "fg": "yellow",
                "bold": true
            }
        ]
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
    let overlay_id = json["id"].as_str().expect("id should be a string").to_string();
    assert!(!overlay_id.is_empty(), "overlay id should not be empty");

    // Step 2: Get overlay with GET /overlay/:id
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/sessions/test/overlay/{}", overlay_id))
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
    assert_eq!(json["id"], overlay_id);
    assert_eq!(json["x"], 10);
    assert_eq!(json["y"], 5);
    assert_eq!(json["spans"][0]["text"], "Hello World");
    assert_eq!(json["spans"][0]["fg"], "yellow");
    assert_eq!(json["spans"][0]["bold"], true);

    // Step 3: Update overlay with PUT /overlay/:id
    let update_body = serde_json::json!({
        "spans": [
            {
                "text": "Updated Text",
                "fg": "cyan",
                "italic": true
            }
        ]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/sessions/test/overlay/{}", overlay_id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&update_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify update by getting the overlay again
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/sessions/test/overlay/{}", overlay_id))
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
    assert_eq!(json["spans"][0]["text"], "Updated Text");
    assert_eq!(json["spans"][0]["fg"], "cyan");
    assert_eq!(json["spans"][0]["italic"], true);
    // Position should remain unchanged
    assert_eq!(json["x"], 10);
    assert_eq!(json["y"], 5);

    // Step 4: Delete overlay with DELETE /overlay/:id
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

    // Step 5: Verify deleted overlay returns 404
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/sessions/test/overlay/{}", overlay_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_overlay_list_and_clear() {
    let (state, _, _) = common::create_test_state();
    let app = router(state, None);

    // Create two overlays
    let create_body1 = serde_json::json!({
        "x": 0,
        "y": 0,
        "width": 80,
        "height": 1,
        "spans": [{ "text": "Overlay 1" }]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/overlay")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&create_body1).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let create_body2 = serde_json::json!({
        "x": 10,
        "y": 10,
        "width": 80,
        "height": 1,
        "spans": [{ "text": "Overlay 2" }]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/overlay")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&create_body2).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    // List overlays
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/sessions/test/overlay")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    assert_eq!(json.len(), 2);

    // Clear all overlays
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/sessions/test/overlay")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify list is empty
    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions/test/overlay")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    assert_eq!(json.len(), 0);
}

#[tokio::test]
async fn test_overlay_patch_position() {
    let (state, _, _) = common::create_test_state();
    let app = router(state, None);

    // Create overlay
    let create_body = serde_json::json!({
        "x": 5,
        "y": 10,
        "z": 1,
        "width": 80,
        "height": 1,
        "spans": [{ "text": "Test" }]
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

    // Patch position
    let patch_body = serde_json::json!({
        "x": 20,
        "y": 30,
        "z": 5
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/sessions/test/overlay/{}", overlay_id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&patch_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify position changed
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/sessions/test/overlay/{}", overlay_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["x"], 20);
    assert_eq!(json["y"], 30);
    assert_eq!(json["z"], 5);
    // Text should be unchanged
    assert_eq!(json["spans"][0]["text"], "Test");
}

#[tokio::test]
async fn test_overlay_not_found() {
    let (state, _, _) = common::create_test_state();
    let app = router(state, None);

    // Try to get non-existent overlay
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/sessions/test/overlay/nonexistent-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Try to update non-existent overlay
    let update_body = serde_json::json!({
        "spans": [{ "text": "Test" }]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/sessions/test/overlay/nonexistent-id")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&update_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Try to delete non-existent overlay
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/sessions/test/overlay/nonexistent-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
