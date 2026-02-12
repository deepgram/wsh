//! Integration tests for alternate screen mode API endpoints.
//!
//! These tests verify the screen mode lifecycle:
//! - Enter/exit alternate screen mode
//! - Conflict errors when already in alt or normal mode
//! - Alt-mode elements destroyed on exit
//! - Screen mode filters overlay and panel lists

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;
use wsh::api::router;

async fn json_body(response: axum::http::Response<Body>) -> serde_json::Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn test_alt_screen_enter_exit_flow() {
    let (state, _, _) = common::create_test_state();
    let app = router(state, None);

    // Verify default mode is normal
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/sessions/test/screen_mode")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = json_body(response).await;
    assert_eq!(json["mode"], "normal");

    // Enter alt screen
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/screen_mode/enter_alt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify mode is alt
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/sessions/test/screen_mode")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = json_body(response).await;
    assert_eq!(json["mode"], "alt");

    // Exit alt screen
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/screen_mode/exit_alt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify mode is back to normal
    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions/test/screen_mode")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = json_body(response).await;
    assert_eq!(json["mode"], "normal");
}

#[tokio::test]
async fn test_alt_screen_enter_when_already_alt_returns_409() {
    let (state, _, _) = common::create_test_state();
    let app = router(state, None);

    // Enter alt screen
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/screen_mode/enter_alt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Try to enter alt again -- should return 409
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/screen_mode/enter_alt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);

    let json = json_body(response).await;
    assert_eq!(json["error"]["code"], "already_in_alt_screen");
}

#[tokio::test]
async fn test_alt_screen_exit_when_normal_returns_409() {
    let (state, _, _) = common::create_test_state();
    let app = router(state, None);

    // Try to exit alt when already in normal mode -- should return 409
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/screen_mode/exit_alt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);

    let json = json_body(response).await;
    assert_eq!(json["error"]["code"], "not_in_alt_screen");
}

#[tokio::test]
async fn test_alt_screen_elements_destroyed_on_exit() {
    let (state, _, _) = common::create_test_state();
    let app = router(state, None);

    // Enter alt screen
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/screen_mode/enter_alt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Create an overlay in alt mode
    let overlay_body = serde_json::json!({
        "x": 0,
        "y": 0,
        "width": 40,
        "height": 1,
        "spans": [{ "text": "Alt overlay" }]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/overlay")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&overlay_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let json = json_body(response).await;
    let alt_overlay_id = json["id"].as_str().unwrap().to_string();

    // Create a panel in alt mode
    let panel_body = serde_json::json!({
        "position": "top",
        "height": 1,
        "spans": [{ "text": "Alt panel" }]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/panel")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&panel_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let json = json_body(response).await;
    let alt_panel_id = json["id"].as_str().unwrap().to_string();

    // Verify they appear in alt-mode lists
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
    let json = json_body(response).await;
    assert_eq!(json.as_array().unwrap().len(), 1);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/sessions/test/panel")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let json = json_body(response).await;
    assert_eq!(json.as_array().unwrap().len(), 1);

    // Exit alt screen -- alt elements should be destroyed
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/screen_mode/exit_alt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify overlay is gone (GET by ID returns 404)
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/sessions/test/overlay/{}", alt_overlay_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Verify panel is gone (GET by ID returns 404)
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/sessions/test/panel/{}", alt_panel_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Verify lists are empty in normal mode
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
    let json = json_body(response).await;
    assert_eq!(json.as_array().unwrap().len(), 0);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions/test/panel")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let json = json_body(response).await;
    assert_eq!(json.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_alt_screen_mode_filters_list() {
    let (state, _, _) = common::create_test_state();
    let app = router(state, None);

    // Create an overlay in normal mode
    let overlay_body = serde_json::json!({
        "x": 0,
        "y": 0,
        "width": 40,
        "height": 1,
        "spans": [{ "text": "Normal overlay" }]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/overlay")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&overlay_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    // Verify 1 overlay in normal mode
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
    let json = json_body(response).await;
    assert_eq!(json.as_array().unwrap().len(), 1);
    assert_eq!(json[0]["spans"][0]["text"], "Normal overlay");

    // Enter alt screen
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/screen_mode/enter_alt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify overlay list is empty in alt mode (normal overlay is filtered out)
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
    let json = json_body(response).await;
    assert_eq!(json.as_array().unwrap().len(), 0, "normal overlay should not appear in alt mode");

    // Create an alt-mode overlay
    let alt_overlay_body = serde_json::json!({
        "x": 10,
        "y": 10,
        "width": 20,
        "height": 1,
        "spans": [{ "text": "Alt overlay" }]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/overlay")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&alt_overlay_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    // Verify alt overlay appears in alt mode
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
    let json = json_body(response).await;
    assert_eq!(json.as_array().unwrap().len(), 1);
    assert_eq!(json[0]["spans"][0]["text"], "Alt overlay");

    // Exit alt screen (destroys alt overlays)
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/screen_mode/exit_alt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify only the normal overlay remains
    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions/test/overlay")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let json = json_body(response).await;
    assert_eq!(json.as_array().unwrap().len(), 1);
    assert_eq!(json[0]["spans"][0]["text"], "Normal overlay");
}
