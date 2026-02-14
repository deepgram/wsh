pub mod auth;
pub mod error;
mod handlers;
pub mod ws_methods;

use axum::{
    routing::{get, post},
    Router,
};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::session::SessionRegistry;
use crate::shutdown::ShutdownCoordinator;

use handlers::*;

/// Configuration controlling server lifecycle behavior.
///
/// In ephemeral mode (default, `persistent = false`) the server shuts down
/// when its last session exits or is destroyed. In persistent mode the server
/// stays alive indefinitely, waiting for new sessions to be created.
pub struct ServerConfig {
    persistent: AtomicBool,
}

impl ServerConfig {
    pub fn new(persistent: bool) -> Self {
        Self {
            persistent: AtomicBool::new(persistent),
        }
    }

    pub fn is_persistent(&self) -> bool {
        self.persistent.load(Ordering::Relaxed)
    }

    pub fn set_persistent(&self, value: bool) {
        self.persistent.store(value, Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub struct AppState {
    pub sessions: SessionRegistry,
    pub shutdown: ShutdownCoordinator,
    pub server_config: Arc<ServerConfig>,
}

pub(crate) fn get_session(
    sessions: &SessionRegistry,
    name: &str,
) -> Result<crate::session::Session, error::ApiError> {
    sessions
        .get(name)
        .ok_or_else(|| error::ApiError::SessionNotFound(name.to_string()))
}

pub fn router(state: AppState, token: Option<String>) -> Router {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpService, StreamableHttpServerConfig,
        session::local::LocalSessionManager,
    };

    let mcp_state = state.clone();
    let mcp_service = StreamableHttpService::new(
        move || Ok(crate::mcp::WshMcpServer::new(mcp_state.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let session_routes = Router::new()
        .route("/input", post(input))
        .route("/input/mode", get(input_mode_get))
        .route("/input/capture", post(input_capture))
        .route("/input/release", post(input_release))
        .route("/input/focus", get(input_focus_get).post(input_focus))
        .route("/input/unfocus", post(input_unfocus))
        .route("/quiesce", get(quiesce))
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
        .route("/overlay/:id/spans", post(overlay_update_spans))
        .route("/overlay/:id/write", post(overlay_region_write))
        .route(
            "/panel",
            get(panel_list)
                .post(panel_create)
                .delete(panel_clear),
        )
        .route(
            "/panel/:id",
            get(panel_get)
                .put(panel_update)
                .patch(panel_patch)
                .delete(panel_delete),
        )
        .route("/panel/:id/spans", post(panel_update_spans))
        .route("/panel/:id/write", post(panel_region_write))
        .route("/screen_mode", get(screen_mode_get))
        .route("/screen_mode/enter_alt", post(enter_alt_screen))
        .route("/screen_mode/exit_alt", post(exit_alt_screen));

    let session_mgmt_routes = Router::new()
        .route(
            "/sessions",
            get(session_list).post(session_create),
        )
        .route(
            "/sessions/:name",
            get(session_get)
                .patch(session_rename)
                .delete(session_kill),
        )
        .route("/sessions/:name/detach", post(session_detach))
        .route("/quiesce", get(quiesce_any))
        .route("/server/persist", get(server_persist_get).put(server_persist_set))
        .route("/ws/json", get(ws_json_server));

    let protected = Router::new()
        .merge(session_mgmt_routes)
        .nest("/sessions/:name", session_routes)
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
        .route("/openapi.yaml", get(openapi_spec))
        .route("/docs", get(docs_index))
        .nest_service("/mcp", mcp_service)
        .merge(protected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::ActivityTracker;
    use crate::broker::Broker;
    use crate::input::InputMode;
    use crate::overlay::OverlayStore;
    use crate::parser::Parser;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use bytes::Bytes;
    use tokio::sync::mpsc;
    use tower::ServiceExt; // for oneshot()

    /// Creates a test state and returns both the state, the input receiver,
    /// and the session name (for URL construction).
    fn create_test_state() -> (AppState, mpsc::Receiver<Bytes>, String) {
        let (input_tx, input_rx) = mpsc::channel(64);
        let broker = Broker::new();
        let parser = Parser::spawn(&broker, 80, 24, 1000);
        let session = crate::session::Session {
            name: "test".to_string(),
            pid: None,
            command: "test".to_string(),
            client_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            input_tx,
            output_rx: broker.sender(),
            shutdown: ShutdownCoordinator::new(),
            parser,
            overlays: OverlayStore::new(),
            panels: crate::panel::PanelStore::new(),
            pty: std::sync::Arc::new(
                crate::pty::Pty::spawn(24, 80, crate::pty::SpawnCommand::default())
                    .expect("failed to spawn PTY for test"),
            ),
            terminal_size: crate::terminal::TerminalSize::new(24, 80),
            input_mode: InputMode::new(),
            input_broadcaster: crate::input::InputBroadcaster::new(),
            activity: ActivityTracker::new(),
            focus: crate::input::FocusTracker::new(),
            detach_signal: tokio::sync::broadcast::channel::<()>(1).0,
            visual_update_tx: tokio::sync::broadcast::channel::<crate::protocol::VisualUpdate>(16).0,
            screen_mode: std::sync::Arc::new(parking_lot::RwLock::new(crate::overlay::ScreenMode::Normal)),
            cancelled: tokio_util::sync::CancellationToken::new(),
        };
        let registry = crate::session::SessionRegistry::new();
        registry.insert(Some("test".into()), session).unwrap();
        let state = AppState {
            sessions: registry,
            shutdown: ShutdownCoordinator::new(),
            server_config: Arc::new(ServerConfig::new(false)),
        };
        (state, input_rx, "test".to_string())
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let (state, _input_rx, _name) = create_test_state();
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
        let (state, _input_rx, _name) = create_test_state();
        let app = router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/test/input")
                    .body(Body::from("test input"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_input_endpoint_forwards_to_channel() {
        let (state, mut input_rx, _name) = create_test_state();
        let app = router(state, None);

        let test_data = b"hello world";
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/test/input")
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
        let (state, _input_rx, _name) = create_test_state();
        let app = router(state, None);

        // Test /health exists (GET)
        let response = app
            .clone()
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Test /sessions/test/input exists (POST)
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/test/input")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // Test /sessions/test/ws/raw exists (GET) - will return upgrade required since we're not using WebSocket
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/sessions/test/ws/raw")
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
        let (state, _input_rx, _name) = create_test_state();
        let app = router(state, None);

        let body = serde_json::json!({
            "x": 10,
            "y": 5,
            "width": 80,
            "height": 1,
            "spans": [
                { "text": "Hello" }
            ]
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/test/overlay")
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
        let (state, _input_rx, _name) = create_test_state();

        // Pre-populate with an overlay
        {
            let session = state.sessions.get("test").unwrap();
            session.overlays.create(
                1,
                2,
                None,
                80,
                1,
                None,
                vec![crate::overlay::OverlaySpan {
                    text: "Test".to_string(),
                    id: None,
                    fg: None,
                    bg: None,
                    bold: false,
                    italic: false,
                    underline: false,
                }],
                false,
                crate::overlay::ScreenMode::Normal,
            ).unwrap();
        }

        let app = router(state, None);

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
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["x"], 1);
        assert_eq!(json[0]["y"], 2);
    }

    #[tokio::test]
    async fn test_overlay_delete() {
        let (state, _input_rx, _name) = create_test_state();

        // Create an overlay
        let id = {
            let session = state.sessions.get("test").unwrap();
            session.overlays.create(0, 0, None, 80, 1, None, vec![], false, crate::overlay::ScreenMode::Normal).unwrap()
        };

        let app = router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/sessions/test/overlay/{}", id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_input_mode_default() {
        let (state, _input_rx, _name) = create_test_state();
        let app = router(state, None);

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
        assert_eq!(json["mode"], "passthrough");
    }

    #[tokio::test]
    async fn test_input_capture_and_release() {
        let (state, _input_rx, _name) = create_test_state();
        let app = router(state, None);

        // Switch to capture mode
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

        // Verify mode is now capture
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
        assert_eq!(json["mode"], "capture");

        // Switch back to passthrough mode
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

        // Verify mode is back to passthrough
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
        assert_eq!(json["mode"], "passthrough");
    }

    #[tokio::test]
    async fn test_openapi_spec_endpoint() {
        let (state, _input_rx, _name) = create_test_state();
        let app = router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/openapi.yaml")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let ct = response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.contains("text/yaml"));

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("openapi:"));
        assert!(text.contains("/health"));
    }

    #[tokio::test]
    async fn test_docs_endpoint() {
        let (state, _input_rx, _name) = create_test_state();
        let app = router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/docs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let ct = response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.contains("text/markdown"));

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("wsh API"));
        assert!(text.contains("/health"));
    }

    #[tokio::test]
    async fn test_docs_and_openapi_exempt_from_auth() {
        let (state, _input_rx, _name) = create_test_state();
        let app = router(state, Some("secret-token".to_string()));

        // /openapi.yaml should work without auth
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/openapi.yaml")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // /docs should work without auth
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/docs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // /sessions/test/screen should require auth
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
    }

    // ── Session management tests ─────────────────────────────────────

    /// Helper: creates a minimal AppState with an empty registry (no pre-seeded sessions).
    fn create_empty_state() -> AppState {
        AppState {
            sessions: crate::session::SessionRegistry::new(),
            shutdown: ShutdownCoordinator::new(),
            server_config: Arc::new(ServerConfig::new(false)),
        }
    }

    #[tokio::test]
    async fn test_session_create_returns_201_with_name() {
        let state = create_empty_state();
        let app = router(state, None);

        let body = serde_json::json!({});
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
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
        assert!(json["name"].is_string());
        // Auto-generated name should be "0" for the first session
        assert_eq!(json["name"], "0");
    }

    #[tokio::test]
    async fn test_session_create_with_custom_name() {
        let state = create_empty_state();
        let app = router(state, None);

        let body = serde_json::json!({"name": "my-session"});
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
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
        assert_eq!(json["name"], "my-session");
    }

    #[tokio::test]
    async fn test_session_list() {
        let state = create_empty_state();
        let app = router(state.clone(), None);

        // Create two sessions first
        let body = serde_json::json!({"name": "alpha"});
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = serde_json::json!({"name": "beta"});
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        // List sessions
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/sessions")
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

        let mut names: Vec<String> = json
            .iter()
            .map(|v| v["name"].as_str().unwrap().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[tokio::test]
    async fn test_session_get_returns_info() {
        let state = create_empty_state();
        let app = router(state, None);

        // Create a session
        let body = serde_json::json!({"name": "my-session"});
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Get session info
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/sessions/my-session")
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
        assert_eq!(json["name"], "my-session");
    }

    #[tokio::test]
    async fn test_session_get_nonexistent_returns_404() {
        let state = create_empty_state();
        let app = router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/sessions/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_session_rename() {
        let state = create_empty_state();
        let app = router(state, None);

        // Create a session
        let body = serde_json::json!({"name": "old-name"});
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Rename it
        let rename_body = serde_json::json!({"name": "new-name"});
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/sessions/old-name")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&rename_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["name"], "new-name");

        // Verify old name is gone
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/sessions/old-name")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // Verify new name exists
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/sessions/new-name")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_session_kill_returns_204() {
        let state = create_empty_state();
        let app = router(state, None);

        // Create a session
        let body = serde_json::json!({"name": "doomed"});
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Kill it
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/sessions/doomed")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // Verify it's gone
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/sessions/doomed")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_session_kill_nonexistent_returns_404() {
        let state = create_empty_state();
        let app = router(state, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/sessions/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_session_create_duplicate_name_returns_409() {
        let state = create_empty_state();
        let app = router(state, None);

        // Create a session
        let body = serde_json::json!({"name": "unique"});
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // Try to create another with the same name
        let body = serde_json::json!({"name": "unique"});
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn test_server_persist_get_returns_current_state() {
        let state = create_empty_state();
        assert!(!state.server_config.is_persistent());
        let app = router(state.clone(), None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/server/persist")
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
        assert_eq!(json["persistent"], false);
        // State unchanged
        assert!(!state.server_config.is_persistent());
    }

    #[tokio::test]
    async fn test_server_persist_put_sets_persistent_on() {
        let state = create_empty_state();
        assert!(!state.server_config.is_persistent());
        let app = router(state.clone(), None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/server/persist")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"persistent": true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["persistent"], true);
        assert!(state.server_config.is_persistent());
    }

    #[tokio::test]
    async fn test_server_persist_put_sets_persistent_off() {
        let state = create_empty_state();
        state.server_config.set_persistent(true);
        let app = router(state.clone(), None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/server/persist")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"persistent": false}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["persistent"], false);
        assert!(!state.server_config.is_persistent());
    }

    // ── ServerConfig unit tests ──────────────────────────────────────

    #[test]
    fn test_server_config_defaults_to_ephemeral() {
        let config = ServerConfig::new(false);
        assert!(!config.is_persistent());
    }

    #[test]
    fn test_server_config_can_be_created_persistent() {
        let config = ServerConfig::new(true);
        assert!(config.is_persistent());
    }

    #[test]
    fn test_server_config_set_persistent_toggles() {
        let config = ServerConfig::new(false);
        assert!(!config.is_persistent());

        config.set_persistent(true);
        assert!(config.is_persistent());

        config.set_persistent(false);
        assert!(!config.is_persistent());
    }

    // ── Ephemeral shutdown logic tests ───────────────────────────────

    /// Simulates the ephemeral shutdown monitor: watches for session events
    /// and returns `true` when the last session is removed in ephemeral mode.
    async fn run_ephemeral_monitor(
        config: Arc<ServerConfig>,
        sessions: crate::session::SessionRegistry,
    ) -> bool {
        let mut events = sessions.subscribe_events();
        loop {
            match events.recv().await {
                Ok(event) => {
                    let is_removal = matches!(
                        event,
                        crate::session::SessionEvent::Destroyed { .. }
                    );
                    if is_removal && !config.is_persistent() && sessions.is_empty() {
                        return true;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    }

    #[tokio::test]
    async fn test_ephemeral_shutdown_triggers_when_last_session_removed() {
        let config = Arc::new(ServerConfig::new(false)); // ephemeral
        let registry = crate::session::SessionRegistry::new();

        // Start the monitor and yield so it subscribes to events before
        // we create any sessions.  (In production the monitor starts at
        // server boot, well before any sessions exist.)
        let monitor = tokio::spawn(run_ephemeral_monitor(config.clone(), registry.clone()));
        tokio::task::yield_now().await;

        // Create a session via the registry
        let app = router(
            AppState {
                sessions: registry.clone(),
                shutdown: ShutdownCoordinator::new(),
                server_config: config.clone(),
            },
            None,
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"ephemeral-test"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // Remove the session
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/sessions/ephemeral-test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // The monitor should fire within a reasonable time
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            monitor,
        )
        .await;
        assert!(result.is_ok(), "ephemeral monitor should complete");
        assert!(result.unwrap().unwrap(), "ephemeral monitor should return true");
    }

    #[tokio::test]
    async fn test_persistent_server_stays_alive_when_last_session_removed() {
        let config = Arc::new(ServerConfig::new(true)); // persistent
        let registry = crate::session::SessionRegistry::new();

        // Start the monitor and yield so it subscribes before events fire
        let monitor = tokio::spawn(run_ephemeral_monitor(config.clone(), registry.clone()));
        tokio::task::yield_now().await;

        // Create and remove a session
        let app = router(
            AppState {
                sessions: registry.clone(),
                shutdown: ShutdownCoordinator::new(),
                server_config: config.clone(),
            },
            None,
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"persistent-test"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/sessions/persistent-test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // The monitor should NOT fire (persistent mode)
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            monitor,
        )
        .await;
        assert!(result.is_err(), "persistent server should not trigger shutdown");
    }

    #[tokio::test]
    async fn test_ephemeral_does_not_trigger_while_sessions_remain() {
        let config = Arc::new(ServerConfig::new(false)); // ephemeral
        let registry = crate::session::SessionRegistry::new();

        // Start the monitor and yield so it subscribes before events fire
        let monitor = tokio::spawn(run_ephemeral_monitor(config.clone(), registry.clone()));
        tokio::task::yield_now().await;

        let app = router(
            AppState {
                sessions: registry.clone(),
                shutdown: ShutdownCoordinator::new(),
                server_config: config.clone(),
            },
            None,
        );

        // Create two sessions
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"sess-a"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"sess-b"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // Remove only one session
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/sessions/sess-a")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // Monitor should NOT fire (one session remains)
        let result: Result<bool, _> = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            async { monitor.await.unwrap() },
        )
        .await;
        assert!(result.is_err(), "should not shutdown while sessions remain");
    }

    #[tokio::test]
    async fn test_upgrade_from_ephemeral_to_persistent_via_http() {
        let state = create_empty_state();
        assert!(!state.server_config.is_persistent());

        let app = router(state.clone(), None);

        // Upgrade to persistent
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/server/persist")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"persistent": true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(state.server_config.is_persistent());

        // Create and remove a session -- should not trigger shutdown
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"upgraded-test"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let monitor = tokio::spawn(run_ephemeral_monitor(
            state.server_config.clone(),
            state.sessions.clone(),
        ));

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/sessions/upgraded-test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // Should not trigger because server is now persistent
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            monitor,
        )
        .await;
        assert!(result.is_err(), "upgraded-to-persistent server should not shutdown");
    }

    // ── Screen mode HTTP tests ──────────────────────────────────────

    #[tokio::test]
    async fn test_screen_mode_get_default_normal() {
        let (state, _input_rx, _name) = create_test_state();
        let app = router(state, None);

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

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["mode"], "normal");
    }

    #[tokio::test]
    async fn test_enter_alt_screen() {
        let (state, _input_rx, _name) = create_test_state();
        let app = router(state, None);

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

        // Verify mode is now alt
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

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["mode"], "alt");
    }

    #[tokio::test]
    async fn test_enter_alt_screen_already_alt_returns_409() {
        let (state, _input_rx, _name) = create_test_state();

        // Pre-set to alt mode
        {
            let session = state.sessions.get("test").unwrap();
            *session.screen_mode.write() = crate::overlay::ScreenMode::Alt;
        }

        let app = router(state, None);

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

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "already_in_alt_screen");
    }

    #[tokio::test]
    async fn test_exit_alt_screen() {
        let (state, _input_rx, _name) = create_test_state();

        // Pre-set to alt mode
        {
            let session = state.sessions.get("test").unwrap();
            *session.screen_mode.write() = crate::overlay::ScreenMode::Alt;
        }

        let app = router(state, None);

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

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["mode"], "normal");
    }

    #[tokio::test]
    async fn test_exit_alt_screen_already_normal_returns_409() {
        let (state, _input_rx, _name) = create_test_state();
        let app = router(state, None);

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

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "not_in_alt_screen");
    }

    #[tokio::test]
    async fn test_screen_mode_filters_overlay_list() {
        let (state, _input_rx, _name) = create_test_state();

        // Create an overlay in normal mode
        {
            let session = state.sessions.get("test").unwrap();
            session.overlays.create(
                0, 0, None, 80, 1, None,
                vec![crate::overlay::OverlaySpan {
                    text: "Normal overlay".to_string(),
                    id: None, fg: None, bg: None, bold: false, italic: false, underline: false,
                }],
                false,
                crate::overlay::ScreenMode::Normal,
            ).unwrap();
        }

        // Create an overlay in alt mode
        {
            let session = state.sessions.get("test").unwrap();
            session.overlays.create(
                0, 0, None, 80, 1, None,
                vec![crate::overlay::OverlaySpan {
                    text: "Alt overlay".to_string(),
                    id: None, fg: None, bg: None, bold: false, italic: false, underline: false,
                }],
                false,
                crate::overlay::ScreenMode::Alt,
            ).unwrap();
        }

        let app = router(state.clone(), None);

        // In normal mode, should see 1 overlay
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
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["spans"][0]["text"], "Normal overlay");

        // Switch to alt mode
        {
            let session = state.sessions.get("test").unwrap();
            *session.screen_mode.write() = crate::overlay::ScreenMode::Alt;
        }

        // In alt mode, should see 1 overlay (the alt one)
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
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["spans"][0]["text"], "Alt overlay");
    }

    #[tokio::test]
    async fn test_screen_mode_filters_panel_list() {
        let (state, _input_rx, _name) = create_test_state();

        // Create a panel in normal mode
        {
            let session = state.sessions.get("test").unwrap();
            session.panels.create(
                crate::panel::Position::Top,
                1,
                None,
                None,
                vec![crate::overlay::OverlaySpan {
                    text: "Normal panel".to_string(),
                    id: None, fg: None, bg: None, bold: false, italic: false, underline: false,
                }],
                false,
                crate::overlay::ScreenMode::Normal,
            ).unwrap();
        }

        // Create a panel in alt mode
        {
            let session = state.sessions.get("test").unwrap();
            session.panels.create(
                crate::panel::Position::Bottom,
                1,
                None,
                None,
                vec![crate::overlay::OverlaySpan {
                    text: "Alt panel".to_string(),
                    id: None, fg: None, bg: None, bold: false, italic: false, underline: false,
                }],
                false,
                crate::overlay::ScreenMode::Alt,
            ).unwrap();
        }

        let app = router(state.clone(), None);

        // In normal mode, should see 1 panel
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
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["spans"][0]["text"], "Normal panel");

        // Switch to alt mode
        {
            let session = state.sessions.get("test").unwrap();
            *session.screen_mode.write() = crate::overlay::ScreenMode::Alt;
        }

        // In alt mode, should see 1 panel (the alt one)
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/sessions/test/panel")
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
        assert_eq!(json[0]["spans"][0]["text"], "Alt panel");
    }
}
