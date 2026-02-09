pub mod auth;
pub mod error;
mod handlers;

use axum::{
    routing::{get, post},
    Router,
};
use bytes::Bytes;
use tokio::sync::{broadcast, mpsc};

use crate::input::{InputBroadcaster, InputMode};
use crate::overlay::OverlayStore;
use crate::parser::Parser;
use crate::shutdown::ShutdownCoordinator;

use handlers::*;

#[derive(Clone)]
pub struct AppState {
    pub input_tx: mpsc::Sender<Bytes>,
    pub output_rx: broadcast::Sender<Bytes>,
    pub shutdown: ShutdownCoordinator,
    pub parser: Parser,
    pub overlays: OverlayStore,
    pub input_mode: InputMode,
    pub input_broadcaster: InputBroadcaster,
}

pub fn router(state: AppState, token: Option<String>) -> Router {
    let protected = Router::new()
        .route("/input", post(input))
        .route("/input/mode", get(input_mode_get))
        .route("/input/capture", post(input_capture))
        .route("/input/release", post(input_release))
        .route("/ws/raw", get(ws_raw))
        .route("/ws/json", get(ws_json))
        .route("/screen", get(screen))
        .route("/scrollback", get(scrollback))
        .route(
            "/overlay",
            get(overlay_list)
                .post(overlay_create)
                .delete(overlay_clear),
        )
        .route(
            "/overlay/:id",
            get(overlay_get)
                .put(overlay_update)
                .patch(overlay_patch)
                .delete(overlay_delete),
        )
        .with_state(state);

    let protected = match token {
        Some(token) => protected.layer(axum::middleware::from_fn(move |req, next| {
            let t = token.clone();
            async move { auth::require_auth(t, req, next).await }
        })),
        None => protected,
    };

    Router::new()
        .route("/health", get(health))
        .merge(protected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::Broker;
    use crate::input::InputMode;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt; // for oneshot()

    /// Creates a test state and returns both the state and the input receiver.
    /// The receiver must be kept alive for the duration of the test to prevent
    /// send failures.
    fn create_test_state() -> (AppState, mpsc::Receiver<Bytes>) {
        let (input_tx, input_rx) = mpsc::channel(64);
        let broker = Broker::new();
        let parser = Parser::spawn(&broker, 80, 24, 1000);
        let state = AppState {
            input_tx,
            output_rx: broker.sender(),
            shutdown: ShutdownCoordinator::new(),
            parser,
            overlays: OverlayStore::new(),
            input_mode: InputMode::new(),
            input_broadcaster: crate::input::InputBroadcaster::new(),
        };
        (state, input_rx)
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let (state, _input_rx) = create_test_state();
        let app = router(state, None);

        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
    }

    #[tokio::test]
    async fn test_input_endpoint_success() {
        let (state, _input_rx) = create_test_state();
        let app = router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/input")
                    .body(Body::from("test input"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_input_endpoint_forwards_to_channel() {
        let (state, mut input_rx) = create_test_state();
        let app = router(state, None);

        let test_data = b"hello world";
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/input")
                    .body(Body::from(test_data.to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // Verify the data was forwarded to the channel
        let received = input_rx.recv().await.expect("should receive data");
        assert_eq!(received.as_ref(), test_data);
    }

    #[tokio::test]
    async fn test_router_has_correct_routes() {
        let (state, _input_rx) = create_test_state();
        let app = router(state, None);

        // Test /health exists (GET)
        let response = app
            .clone()
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Test /input exists (POST)
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/input")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // Test /ws/raw exists (GET) - will return upgrade required since we're not using WebSocket
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/ws/raw")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // WebSocket upgrade endpoints typically return a non-404 status when accessed without upgrade
        // This confirms the route exists
        assert_ne!(response.status(), StatusCode::NOT_FOUND);

        // Test non-existent route returns 404
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_overlay_create() {
        let (state, _input_rx) = create_test_state();
        let app = router(state, None);

        let body = serde_json::json!({
            "x": 10,
            "y": 5,
            "spans": [
                { "text": "Hello" }
            ]
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/overlay")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["id"].is_string());
        assert!(!json["id"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_overlay_list() {
        let (state, _input_rx) = create_test_state();

        // Pre-populate with an overlay
        state.overlays.create(
            1,
            2,
            None,
            vec![crate::overlay::OverlaySpan {
                text: "Test".to_string(),
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            }],
        );

        let app = router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/overlay")
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
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["x"], 1);
        assert_eq!(json[0]["y"], 2);
    }

    #[tokio::test]
    async fn test_overlay_delete() {
        let (state, _input_rx) = create_test_state();

        // Create an overlay
        let id = state.overlays.create(0, 0, None, vec![]);

        let app = router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/overlay/{}", id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_input_mode_default() {
        let (state, _input_rx) = create_test_state();
        let app = router(state, None);

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
        assert_eq!(json["mode"], "passthrough");
    }

    #[tokio::test]
    async fn test_input_capture_and_release() {
        let (state, _input_rx) = create_test_state();
        let app = router(state, None);

        // Switch to capture mode
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

        // Verify mode is now capture
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
        assert_eq!(json["mode"], "capture");

        // Switch back to passthrough mode
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

        // Verify mode is back to passthrough
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
        assert_eq!(json["mode"], "passthrough");
    }
}
