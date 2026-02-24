//! Integration tests for panel API endpoints.
//!
//! Tests verify the full CRUD flow for panels, visibility behavior when
//! panels compete for space, and span-only updates vs layout-changing updates.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;
use wsh::api::{router, RouterConfig};

fn create_test_state() -> wsh::api::AppState {
    let (state, _, _, _ptx) = common::create_test_state();
    state
}

fn create_test_state_with_size(rows: u16, cols: u16) -> wsh::api::AppState {
    let (state, _, _, _ptx) = common::create_test_state_with_size(rows, cols);
    state
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
    let app = router(state, RouterConfig::default());

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
                .uri("/sessions/test/panel")
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
                .uri(format!("/sessions/test/panel/{}", panel_id))
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
                .uri(format!("/sessions/test/panel/{}", panel_id))
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
                .uri(format!("/sessions/test/panel/{}", panel_id))
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
                .uri(format!("/sessions/test/panel/{}", panel_id))
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
                .uri(format!("/sessions/test/panel/{}", panel_id))
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
    let app = router(state, RouterConfig::default());

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
                    .uri("/sessions/test/panel")
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
                .uri("/sessions/test/panel")
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
                .uri("/sessions/test/panel")
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
async fn test_panel_put_replaces_all_fields() {
    let state = create_test_state();
    let app = router(state, RouterConfig::default());

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
                .uri("/sessions/test/panel")
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
                .uri(format!("/sessions/test/panel/{}", panel_id))
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
                .uri(format!("/sessions/test/panel/{}", panel_id))
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
                .uri(format!("/sessions/test/panel/{}", panel_id))
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
    let app = router(state, RouterConfig::default());

    // GET non-existent
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/sessions/test/panel/nonexistent-id")
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
                .uri("/sessions/test/panel/nonexistent-id")
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
                .uri("/sessions/test/panel/nonexistent-id")
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
    let app = router(state, RouterConfig::default());

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
                .uri("/sessions/test/panel")
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
                .uri("/sessions/test/panel")
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
                .uri(format!("/sessions/test/panel/{}", large_id))
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
                .uri(format!("/sessions/test/panel/{}", small_id))
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
                .uri(format!("/sessions/test/panel/{}", large_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/sessions/test/panel/{}", small_id))
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
    let app = router(state, RouterConfig::default());

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
                .uri("/sessions/test/panel")
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
                .uri("/sessions/test/panel")
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
                .uri("/sessions/test/panel")
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

#[tokio::test]
async fn test_panel_create_with_background() {
    let state = create_test_state();
    let app = router(state, RouterConfig::default());

    // Create panel with a background
    let create_body = serde_json::json!({
        "position": "top",
        "height": 2,
        "background": { "bg": "magenta" },
        "spans": [
            { "text": "Status bar" }
        ]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/panel")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let json = json_body(response).await;
    let panel_id = json["id"].as_str().unwrap().to_string();

    // Verify background in GET response
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/sessions/test/panel/{}", panel_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = json_body(response).await;
    assert_eq!(json["background"]["bg"], "magenta");
    assert_eq!(json["height"], 2);
    assert_eq!(json["spans"][0]["text"], "Status bar");
}

#[tokio::test]
async fn test_panel_named_span_update() {
    let state = create_test_state();
    let app = router(state, RouterConfig::default());

    // Create panel with named spans
    let create_body = serde_json::json!({
        "position": "bottom",
        "height": 1,
        "spans": [
            { "id": "label", "text": "CPU: ", "bold": true },
            { "id": "value", "text": "12%", "fg": "green" }
        ]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/panel")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let json = json_body(response).await;
    let panel_id = json["id"].as_str().unwrap().to_string();

    // Update only the "value" span via POST /panel/:id/spans
    let update_body = serde_json::json!({
        "spans": [
            { "id": "value", "text": "89%", "fg": "red", "bold": true }
        ]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/test/panel/{}/spans", panel_id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&update_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify the "value" span was updated and "label" is unchanged
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/sessions/test/panel/{}", panel_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = json_body(response).await;

    // Label span should be unchanged
    assert_eq!(json["spans"][0]["id"], "label");
    assert_eq!(json["spans"][0]["text"], "CPU: ");
    assert_eq!(json["spans"][0]["bold"], true);

    // Value span should be updated
    assert_eq!(json["spans"][1]["id"], "value");
    assert_eq!(json["spans"][1]["text"], "89%");
    assert_eq!(json["spans"][1]["fg"], "red");
    assert_eq!(json["spans"][1]["bold"], true);
}

#[tokio::test]
async fn test_panel_region_write() {
    let state = create_test_state();
    let app = router(state, RouterConfig::default());

    // Create panel with enough height for region writes
    let create_body = serde_json::json!({
        "position": "bottom",
        "height": 5,
        "spans": []
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/panel")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let json = json_body(response).await;
    let panel_id = json["id"].as_str().unwrap().to_string();

    // POST region writes
    let write_body = serde_json::json!({
        "writes": [
            { "row": 0, "col": 0, "text": "Row 0", "fg": "yellow" },
            { "row": 3, "col": 10, "text": "Row 3", "bold": true }
        ]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/test/panel/{}/write", panel_id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&write_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify region writes via GET
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/sessions/test/panel/{}", panel_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = json_body(response).await;

    let writes = json["region_writes"].as_array().unwrap();
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[0]["row"], 0);
    assert_eq!(writes[0]["col"], 0);
    assert_eq!(writes[0]["text"], "Row 0");
    assert_eq!(writes[0]["fg"], "yellow");
    assert_eq!(writes[1]["row"], 3);
    assert_eq!(writes[1]["col"], 10);
    assert_eq!(writes[1]["text"], "Row 3");
    assert_eq!(writes[1]["bold"], true);
}
