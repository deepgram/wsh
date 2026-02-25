pub mod auth;
pub mod error;
mod handlers;
pub mod origin;
pub mod ticket;
mod web;
pub mod ws_methods;

use axum::{
    extract::DefaultBodyLimit,
    http::{header, HeaderName, HeaderValue, Method},
    response::Redirect,
    routing::{get, post},
    Router,
};
use tower_http::cors::CorsLayer;
use tower_http::set_header::SetResponseHeaderLayer;

use std::net::SocketAddr;
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
        self.persistent.load(Ordering::Acquire)
    }

    pub fn set_persistent(&self, value: bool) {
        self.persistent.store(value, Ordering::Release);
    }
}

/// Maximum concurrent server-level WebSocket connections.
///
/// Per-session WS endpoints already have a limit (MAX_CLIENTS_PER_SESSION = 64),
/// but the server-level `/ws/json` endpoint was previously unbounded. This cap
/// prevents resource exhaustion from a buggy or malicious client opening
/// thousands of connections.
const MAX_SERVER_WS_CONNECTIONS: usize = 256;

/// Maximum concurrent MCP sessions allowed via the Streamable HTTP transport.
const MAX_MCP_SESSIONS: usize = 256;

#[derive(Clone)]
pub struct AppState {
    pub sessions: SessionRegistry,
    pub shutdown: ShutdownCoordinator,
    pub server_config: Arc<ServerConfig>,
    /// Counter for server-level WebSocket connections.
    pub server_ws_count: Arc<std::sync::atomic::AtomicUsize>,
    /// Counter for active MCP sessions (Streamable HTTP transport).
    pub mcp_session_count: Arc<std::sync::atomic::AtomicUsize>,
    /// Short-lived ticket store for WebSocket authentication.
    pub ticket_store: Arc<ticket::TicketStore>,
}

pub(crate) fn get_session(
    sessions: &SessionRegistry,
    name: &str,
) -> Result<crate::session::Session, error::ApiError> {
    sessions
        .get(name)
        .ok_or_else(|| error::ApiError::SessionNotFound(name.to_string()))
}

/// Configuration for the HTTP/WS router.
///
/// Controls authentication, CORS, rate limiting, and origin checks.
/// Use `RouterConfig::default()` in tests for a minimal no-auth setup.
pub struct RouterConfig {
    pub token: Option<String>,
    pub bind: SocketAddr,
    pub cors_origins: Vec<String>,
    pub rate_limit: Option<u32>,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            token: None,
            bind: "127.0.0.1:8080".parse().unwrap(),
            cors_origins: vec![],
            rate_limit: None,
        }
    }
}

pub fn router(state: AppState, config: RouterConfig) -> Router {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpService, StreamableHttpServerConfig,
        session::local::LocalSessionManager,
    };

    let mcp_state = state.clone();
    let mcp_counter = state.mcp_session_count.clone();
    let mcp_service = StreamableHttpService::new(
        move || {
            let current = mcp_counter.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            if current >= MAX_MCP_SESSIONS {
                mcp_counter.fetch_sub(1, std::sync::atomic::Ordering::Release);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "maximum MCP sessions reached",
                ));
            }
            Ok(crate::mcp::WshMcpServer::new(mcp_state.clone())
                .with_session_counter(mcp_counter.clone()))
        },
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
        .route("/idle", get(idle))
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
            "/overlay/{id}",
            get(overlay_get)
                .put(overlay_update)
                .patch(overlay_patch)
                .delete(overlay_delete),
        )
        .route("/overlay/{id}/spans", post(overlay_update_spans))
        .route("/overlay/{id}/write", post(overlay_region_write))
        .route(
            "/panel",
            get(panel_list)
                .post(panel_create)
                .delete(panel_clear),
        )
        .route(
            "/panel/{id}",
            get(panel_get)
                .put(panel_update)
                .patch(panel_patch)
                .delete(panel_delete),
        )
        .route("/panel/{id}/spans", post(panel_update_spans))
        .route("/panel/{id}/write", post(panel_region_write))
        .route("/screen_mode", get(screen_mode_get))
        .route("/screen_mode/enter_alt", post(enter_alt_screen))
        .route("/screen_mode/exit_alt", post(exit_alt_screen));

    let session_mgmt_routes = Router::new()
        .route(
            "/sessions",
            get(session_list).post(session_create),
        )
        .route(
            "/sessions/{name}",
            get(session_get)
                .patch(session_update)
                .delete(session_kill),
        )
        .route("/sessions/{name}/detach", post(session_detach))
        .route("/idle", get(idle_any))
        .route("/server/persist", get(server_persist_get).put(server_persist_set))
        .route("/ws/json", get(ws_json_server));

    let ticket_store = state.ticket_store.clone();
    let protected = Router::new()
        .merge(session_mgmt_routes)
        .nest("/sessions/{name}", session_routes)
        .route("/auth/ws-ticket", post(ws_ticket))
        .route("/openapi.yaml", get(openapi_spec))
        .route("/docs", get(docs_index))
        .nest_service("/mcp", mcp_service)
        .with_state(state);

    // Apply rate limiting to the protected routes if configured.
    let protected = if let Some(rps) = config.rate_limit {
        use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder, key_extractor::PeerIpKeyExtractor};
        let governor_conf = Arc::new(
            GovernorConfigBuilder::default()
                .per_second(u64::from(rps))
                .burst_size(rps)
                .key_extractor(PeerIpKeyExtractor)
                .finish()
                .unwrap()
        );
        protected.layer(GovernorLayer::new(governor_conf))
    } else {
        protected
    };

    let protected = match config.token {
        Some(token) => {
            let ts = Some(ticket_store);
            protected.layer(axum::middleware::from_fn(move |req, next| {
                let t = token.clone();
                let ts = ts.clone();
                async move { auth::require_auth(t, ts, req, next).await }
            }))
        }
        None => {
            // When running without auth (localhost), protect against CSWSH attacks
            // by validating the Origin header on WebSocket upgrade requests.
            let port = config.bind.port();
            let mut allowed_origins = vec![
                format!("http://127.0.0.1:{}", port),
                format!("http://localhost:{}", port),
                format!("http://[::1]:{}", port),
            ];
            allowed_origins.extend(config.cors_origins.iter().cloned());
            protected.layer(axum::middleware::from_fn(move |req, next| {
                let origins = allowed_origins.clone();
                origin::check_ws_origin(origins, req, next)
            }))
        }
    };

    let ui = Router::new().fallback(web::web_asset);

    let router = Router::new()
        .route("/", get(|| async { Redirect::temporary("/ui") }))
        .route("/health", get(health))
        .merge(protected)
        .nest("/ui", ui)
        .layer(DefaultBodyLimit::max(1024 * 1024)) // 1 MB
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static(
                "default-src 'self'; script-src 'self'; style-src 'self'; \
                 connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'"
            ),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("geolocation=(), microphone=(), camera=()"),
        ));

    // Conditionally apply CORS if origins are configured.
    if config.cors_origins.is_empty() {
        router
    } else {
        let origins: Vec<HeaderValue> = config.cors_origins.iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        router.layer(
            CorsLayer::new()
                .allow_origin(origins)
                .allow_methods([Method::GET, Method::POST, Method::PUT,
                               Method::PATCH, Method::DELETE, Method::OPTIONS])
                .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        )
    }
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
        let (_parser_tx, parser_rx) = mpsc::channel(256);
        let parser = Parser::spawn(parser_rx, 80, 24, 1000);
        let session = crate::session::Session {
            name: "test".to_string(),
            pid: None,
            command: "test".to_string(),
            client_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            tags: std::sync::Arc::new(parking_lot::RwLock::new(std::collections::HashSet::new())),
            child_exited: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            input_tx,
            output_rx: broker.sender(),
            shutdown: ShutdownCoordinator::new(),
            parser,
            overlays: OverlayStore::new(),
            panels: crate::panel::PanelStore::new(),
            pty: std::sync::Arc::new(parking_lot::Mutex::new(
                crate::pty::Pty::spawn(24, 80, crate::pty::SpawnCommand::default())
                    .expect("failed to spawn PTY for test"),
            )),
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
            server_ws_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            mcp_session_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            ticket_store: Arc::new(ticket::TicketStore::new()),
        };
        (state, input_rx, "test".to_string())
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let (state, _input_rx, _name) = create_test_state();
        let app = router(state, RouterConfig::default());

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
        let app = router(state, RouterConfig::default());

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
        let app = router(state, RouterConfig::default());

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
        let app = router(state, RouterConfig::default());

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

        // Non-API, non-UI routes return 404
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // Web UI is served under /ui (SPA fallback)
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/ui")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Root redirects to /ui
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            response.headers().get("location").unwrap().to_str().unwrap(),
            "/ui",
        );
    }

    #[tokio::test]
    async fn test_overlay_create() {
        let (state, _input_rx, _name) = create_test_state();
        let app = router(state, RouterConfig::default());

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

        let app = router(state, RouterConfig::default());

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

        let app = router(state, RouterConfig::default());

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
        let app = router(state, RouterConfig::default());

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
        let app = router(state, RouterConfig::default());

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
        let app = router(state, RouterConfig::default());

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
        let app = router(state, RouterConfig::default());

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
    async fn test_docs_and_openapi_require_auth() {
        let (state, _input_rx, _name) = create_test_state();
        let app = router(state, RouterConfig { token: Some("secret-token".to_string()), ..Default::default() });

        // /openapi.yaml should require auth
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
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // /docs should require auth
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
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // /openapi.yaml with valid auth should return 200
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/openapi.yaml")
                    .header("authorization", "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // /docs with valid auth should return 200
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/docs")
                    .header("authorization", "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // /sessions/test/screen should require auth
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/sessions/test/screen")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // /mcp should require auth
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_ws_ticket_requires_auth() {
        let (state, _input_rx, _name) = create_test_state();
        let app = router(state, RouterConfig { token: Some("secret-token".to_string()), ..Default::default() });

        // Without auth — should 401
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/ws-ticket")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // With auth — should return ticket
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/ws-ticket")
                    .header("authorization", "Bearer secret-token")
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
        assert!(json["ticket"].is_string());
        assert_eq!(json["ticket"].as_str().unwrap().len(), 32);
    }

    #[tokio::test]
    async fn test_query_token_no_longer_accepted() {
        let (state, _input_rx, _name) = create_test_state();
        let app = router(state, RouterConfig { token: Some("secret-token".to_string()), ..Default::default() });

        // ?token= should NOT work anymore
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/sessions/test/screen?token=secret-token")
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
            server_ws_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            mcp_session_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            ticket_store: Arc::new(ticket::TicketStore::new()),
        }
    }

    #[tokio::test]
    async fn test_session_create_returns_201_with_name() {
        let state = create_empty_state();
        let app = router(state, RouterConfig::default());

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
        let app = router(state, RouterConfig::default());

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
        let app = router(state.clone(), RouterConfig::default());

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
        let app = router(state, RouterConfig::default());

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
        let app = router(state, RouterConfig::default());

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
        let app = router(state, RouterConfig::default());

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
        let app = router(state, RouterConfig::default());

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
        let app = router(state, RouterConfig::default());

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
        let app = router(state, RouterConfig::default());

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
        let app = router(state.clone(), RouterConfig::default());

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
        let app = router(state.clone(), RouterConfig::default());

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
        let app = router(state.clone(), RouterConfig::default());

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
                server_ws_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                mcp_session_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                ticket_store: Arc::new(ticket::TicketStore::new()),
            },
            RouterConfig::default(),
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
                server_ws_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                mcp_session_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                ticket_store: Arc::new(ticket::TicketStore::new()),
            },
            RouterConfig::default(),
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
                server_ws_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                mcp_session_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                ticket_store: Arc::new(ticket::TicketStore::new()),
            },
            RouterConfig::default(),
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

        let app = router(state.clone(), RouterConfig::default());

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
        let app = router(state, RouterConfig::default());

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
        let app = router(state, RouterConfig::default());

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

        let app = router(state, RouterConfig::default());

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

        let app = router(state, RouterConfig::default());

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
        let app = router(state, RouterConfig::default());

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

        let app = router(state.clone(), RouterConfig::default());

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

        let app = router(state.clone(), RouterConfig::default());

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

    // ── Tag HTTP API tests ──────────────────────────────────────────

    #[tokio::test]
    async fn test_session_create_with_tags() {
        let state = create_empty_state();
        let app = router(state, RouterConfig::default());

        let body = serde_json::json!({"name": "tagged", "tags": ["build", "ci"]});
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
        assert_eq!(json["name"], "tagged");
        let tags: Vec<String> = serde_json::from_value(json["tags"].clone()).unwrap();
        assert_eq!(tags, vec!["build", "ci"]);
    }

    #[tokio::test]
    async fn test_session_create_with_invalid_tag_returns_400() {
        let state = create_empty_state();
        let app = router(state, RouterConfig::default());

        let body = serde_json::json!({"name": "bad-tags", "tags": ["valid", "has space"]});
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

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "invalid_tag");
    }

    #[tokio::test]
    async fn test_session_list_with_tag_filter() {
        let state = create_empty_state();
        let app = router(state.clone(), RouterConfig::default());

        // Create sessions with different tags
        let body = serde_json::json!({"name": "alpha", "tags": ["build"]});
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

        let body = serde_json::json!({"name": "beta", "tags": ["test"]});
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

        let body = serde_json::json!({"name": "gamma", "tags": ["build", "test"]});
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

        // Filter by "build" tag — should return alpha and gamma
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/sessions?tag=build")
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
        let mut names: Vec<String> = json.iter().map(|v| v["name"].as_str().unwrap().to_string()).collect();
        names.sort();
        assert_eq!(names, vec!["alpha", "gamma"]);

        // Filter by "test" tag — should return beta and gamma
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/sessions?tag=test")
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
        let mut names: Vec<String> = json.iter().map(|v| v["name"].as_str().unwrap().to_string()).collect();
        names.sort();
        assert_eq!(names, vec!["beta", "gamma"]);
    }

    #[tokio::test]
    async fn test_session_list_without_filter_returns_all() {
        let state = create_empty_state();
        let app = router(state.clone(), RouterConfig::default());

        // Create sessions with tags
        let body = serde_json::json!({"name": "a", "tags": ["x"]});
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

        let body = serde_json::json!({"name": "b"});
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

        // List without filter — should return all
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
    }

    #[tokio::test]
    async fn test_session_patch_add_tags() {
        let state = create_empty_state();
        let app = router(state, RouterConfig::default());

        // Create a session
        let body = serde_json::json!({"name": "taggable"});
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

        // Add tags via PATCH
        let patch_body = serde_json::json!({"add_tags": ["build", "ci"]});
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/sessions/taggable")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&patch_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let tags: Vec<String> = serde_json::from_value(json["tags"].clone()).unwrap();
        assert_eq!(tags, vec!["build", "ci"]);
    }

    #[tokio::test]
    async fn test_session_patch_remove_tags() {
        let state = create_empty_state();
        let app = router(state, RouterConfig::default());

        // Create a session with tags
        let body = serde_json::json!({"name": "removable", "tags": ["a", "b", "c"]});
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

        // Remove some tags via PATCH
        let patch_body = serde_json::json!({"remove_tags": ["a", "c"]});
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/sessions/removable")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&patch_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let tags: Vec<String> = serde_json::from_value(json["tags"].clone()).unwrap();
        assert_eq!(tags, vec!["b"]);
    }

    #[tokio::test]
    async fn test_session_get_includes_tags() {
        let state = create_empty_state();
        let app = router(state, RouterConfig::default());

        // Create a session with tags
        let body = serde_json::json!({"name": "info-test", "tags": ["deploy"]});
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

        // GET session info
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/sessions/info-test")
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
        assert_eq!(json["name"], "info-test");
        let tags: Vec<String> = serde_json::from_value(json["tags"].clone()).unwrap();
        assert_eq!(tags, vec!["deploy"]);
    }

    #[tokio::test]
    async fn test_session_patch_rename_and_add_tags() {
        let state = create_empty_state();
        let app = router(state, RouterConfig::default());

        // Create a session
        let body = serde_json::json!({"name": "original"});
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

        // Rename and add tags in a single PATCH
        let patch_body = serde_json::json!({"name": "renamed", "add_tags": ["new-tag"]});
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/sessions/original")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&patch_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["name"], "renamed");
        let tags: Vec<String> = serde_json::from_value(json["tags"].clone()).unwrap();
        assert_eq!(tags, vec!["new-tag"]);

        // Old name should be gone
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/sessions/original")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_session_patch_invalid_tag_returns_400() {
        let state = create_empty_state();
        let app = router(state, RouterConfig::default());

        // Create a session
        let body = serde_json::json!({"name": "patch-bad"});
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

        // Try adding an invalid tag
        let patch_body = serde_json::json!({"add_tags": ["valid", ""]});
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/sessions/patch-bad")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&patch_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "invalid_tag");
    }

    #[tokio::test]
    async fn test_security_headers_on_health() {
        let (state, _input_rx, _name) = create_test_state();
        let app = router(state, RouterConfig::default());
        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.headers().get("x-frame-options").unwrap(), "DENY");
        assert_eq!(
            response.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
        assert_eq!(
            response.headers().get("referrer-policy").unwrap(),
            "no-referrer"
        );
        assert!(
            response.headers().get("content-security-policy").unwrap()
                .to_str().unwrap().contains("default-src 'self'")
        );
        assert!(
            response.headers().get("permissions-policy").unwrap()
                .to_str().unwrap().contains("geolocation=()")
        );
    }

    #[tokio::test]
    async fn test_session_create_no_tags_has_empty_array() {
        let state = create_empty_state();
        let app = router(state, RouterConfig::default());

        let body = serde_json::json!({"name": "no-tags"});
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
        let tags = json["tags"].as_array().unwrap();
        assert!(tags.is_empty());
    }
}
