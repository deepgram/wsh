//! Integration tests for panel API endpoints.
//!
//! Tests verify the full CRUD flow for panels, visibility behavior when
//! panels compete for space, and span-only updates vs layout-changing updates.

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

/// Creates a test state with a specified terminal size.
fn create_test_state_with_size(rows: u16, cols: u16) -> AppState {
    let (input_tx, _) = mpsc::channel::<Bytes>(64);
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, cols as usize, rows as usize, 1000);
    AppState {
        input_tx,
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: std::sync::Arc::new(
            wsh::pty::Pty::spawn(rows, cols, wsh::pty::SpawnCommand::default())
                .expect("failed to spawn PTY for test"),
        ),
        terminal_size: wsh::terminal::TerminalSize::new(rows, cols),
    }
}

fn create_test_state() -> AppState {
    create_test_state_with_size(24, 80)
}

async fn json_body(response: axum::http::Response<Body>) -> serde_json::Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn test_panel_crud_flow() {
    let state = create_test_state();
    let app = router(state, None);

    // Step 1: Create panel
    let create_body = serde_json::json!({
        "position": "top",
        "height": 2,
        "spans": [
            {"text": "Status: ", "bold": true},
            {"text": "OK", "fg": "green"}
        ]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/panel")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let json = json_body(response).await;
    let panel_id = json["id"].as_str().expect("id should be a string").to_string();
    assert!(!panel_id.is_empty());

    // Step 2: Get panel
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/panel/{}", panel_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = json_body(response).await;
    assert_eq!(json["id"], panel_id);
    assert_eq!(json["position"], "top");
    assert_eq!(json["height"], 2);
    assert_eq!(json["spans"][0]["text"], "Status: ");
    assert_eq!(json["spans"][0]["bold"], true);
    assert_eq!(json["spans"][1]["text"], "OK");
    assert_eq!(json["visible"], true);

    // Step 3: Update panel spans with PATCH
    let patch_body = serde_json::json!({
        "spans": [{"text": "Error", "fg": "red", "bold": true}]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/panel/{}", panel_id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&patch_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify spans changed but position/height unchanged
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/panel/{}", panel_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let json = json_body(response).await;
    assert_eq!(json["spans"][0]["text"], "Error");
    assert_eq!(json["position"], "top");
    assert_eq!(json["height"], 2);

    // Step 4: Delete panel
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/panel/{}", panel_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Step 5: Verify deleted panel returns 404
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/panel/{}", panel_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_panel_list_and_clear() {
    let state = create_test_state();
    let app = router(state, None);

    // Create two panels
    for pos in ["top", "bottom"] {
        let body = serde_json::json!({
            "position": pos,
            "height": 1,
            "spans": [{"text": format!("{} panel", pos)}]
        });

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/panel")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    // List panels
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/panel")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = json_body(response).await;
    let panels = json.as_array().unwrap();
    assert_eq!(panels.len(), 2);

    // Clear all panels
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/panel")
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
                .uri("/panel")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let json = json_body(response).await;
    assert_eq!(json.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_panel_put_replaces_all_fields() {
    let state = create_test_state();
    let app = router(state, None);

    // Create panel
    let body = serde_json::json!({
        "position": "top",
        "height": 1,
        "spans": [{"text": "Original"}]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/panel")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let json = json_body(response).await;
    let panel_id = json["id"].as_str().unwrap().to_string();

    // Get panel to find z value
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/panel/{}", panel_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let json = json_body(response).await;
    let z = json["z"].as_i64().unwrap();

    // PUT to replace
    let put_body = serde_json::json!({
        "position": "bottom",
        "height": 3,
        "z": z,
        "spans": [{"text": "Replaced", "fg": "red"}]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/panel/{}", panel_id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&put_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify all fields replaced
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/panel/{}", panel_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let json = json_body(response).await;
    assert_eq!(json["position"], "bottom");
    assert_eq!(json["height"], 3);
    assert_eq!(json["spans"][0]["text"], "Replaced");
}

#[tokio::test]
async fn test_panel_not_found() {
    let state = create_test_state();
    let app = router(state, None);

    // GET non-existent
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/panel/nonexistent-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = json_body(response).await;
    assert_eq!(json["error"]["code"], "panel_not_found");

    // DELETE non-existent
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/panel/nonexistent-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // PATCH non-existent
    let patch_body = serde_json::json!({"height": 2});
    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/panel/nonexistent-id")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&patch_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_panel_visibility_when_space_exhausted() {
    // Use a small terminal (10 rows) to easily test visibility
    let state = create_test_state_with_size(10, 80);
    let app = router(state, None);

    // Create a large panel that takes 8 rows (z=10)
    let body = serde_json::json!({
        "position": "top",
        "height": 8,
        "z": 10,
        "spans": [{"text": "Large panel"}]
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/panel")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let json = json_body(response).await;
    let large_id = json["id"].as_str().unwrap().to_string();

    // Create another panel that would exceed available space (z=5, lower priority)
    let body = serde_json::json!({
        "position": "bottom",
        "height": 3,
        "z": 5,
        "spans": [{"text": "Small panel"}]
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/panel")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let json = json_body(response).await;
    let small_id = json["id"].as_str().unwrap().to_string();

    // Get both panels and check visibility
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/panel/{}", large_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let json = json_body(response).await;
    assert_eq!(json["visible"], true, "high-z panel should be visible");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/panel/{}", small_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let json = json_body(response).await;
    assert_eq!(json["visible"], false, "low-z panel should be hidden when no space");

    // Delete the large panel -- small panel should become visible
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/panel/{}", large_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/panel/{}", small_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let json = json_body(response).await;
    assert_eq!(json["visible"], true, "panel should become visible after space freed");
}

#[tokio::test]
async fn test_multiple_panels_cumulative_height() {
    let state = create_test_state();
    let app = router(state, None);

    // Create two panels: top=2, bottom=3 (total 5 of 24 rows used)
    let body = serde_json::json!({
        "position": "top",
        "height": 2,
        "spans": [{"text": "Top"}]
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/panel")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let body = serde_json::json!({
        "position": "bottom",
        "height": 3,
        "spans": [{"text": "Bottom"}]
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/panel")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    // List panels -- both should be visible
    let response = app
        .oneshot(
            Request::builder()
                .uri("/panel")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let json = json_body(response).await;
    let panels = json.as_array().unwrap();
    assert_eq!(panels.len(), 2);
    for panel in panels {
        assert_eq!(panel["visible"], true);
    }
}
