use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use crate::overlay::{Overlay, OverlaySpan, OverlayStore};
use crate::parser::{
    events::{Event, EventType, Subscribe},
    state::{Format, Query, QueryResponse},
    Parser,
};
use crate::shutdown::ShutdownCoordinator;

#[derive(Clone)]
pub struct AppState {
    pub input_tx: mpsc::Sender<Bytes>,
    pub output_rx: broadcast::Sender<Bytes>,
    pub shutdown: ShutdownCoordinator,
    pub parser: Parser,
    pub overlays: OverlayStore,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn input(State(state): State<AppState>, body: Bytes) -> StatusCode {
    match state.input_tx.send(body).await {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(e) => {
            tracing::error!("Failed to send input to PTY: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

async fn ws_raw(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_ws_raw(socket, state))
}

async fn handle_ws_raw(socket: WebSocket, state: AppState) {
    // Register this connection for graceful shutdown tracking
    let (_guard, mut shutdown_rx) = state.shutdown.register();

    let (mut ws_tx, mut ws_rx) = socket.split();

    let mut output_rx = state.output_rx.subscribe();
    let input_tx = state.input_tx.clone();

    // Main loop: handle PTY output, WebSocket input, and shutdown signal
    loop {
        tokio::select! {
            // PTY output -> WebSocket
            result = output_rx.recv() => {
                match result {
                    Ok(data) => {
                        if ws_tx.send(Message::Binary(data.to_vec())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }

            // WebSocket input -> PTY
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        if input_tx.send(Bytes::from(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        if input_tx.send(Bytes::from(text)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => continue, // Ping/Pong handled automatically
                    Some(Err(_)) => break,
                }
            }

            // Shutdown signal
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::debug!("WebSocket received shutdown signal, closing");
                    let close_frame = CloseFrame {
                        code: axum::extract::ws::close_code::NORMAL,
                        reason: "server shutting down".into(),
                    };
                    let _ = ws_tx.send(Message::Close(Some(close_frame))).await;
                    let _ = ws_tx.flush().await;
                    break;
                }
            }
        }
    }

    // _guard is dropped here, decrementing active connection count
}

async fn ws_json(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_ws_json(socket, state))
}

async fn handle_ws_json(socket: WebSocket, state: AppState) {
    let (_guard, mut shutdown_rx) = state.shutdown.register();
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Send connected message
    let connected_msg = serde_json::json!({ "connected": true });
    if ws_tx
        .send(Message::Text(connected_msg.to_string()))
        .await
        .is_err()
    {
        return;
    }

    // Wait for subscribe message
    let subscribe: Subscribe = loop {
        tokio::select! {
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<Subscribe>(&text) {
                            Ok(sub) => break sub,
                            Err(e) => {
                                let err = serde_json::json!({
                                    "error": format!("invalid subscribe message: {}", e),
                                    "code": "invalid_subscribe"
                                });
                                let _ = ws_tx.send(Message::Text(err.to_string())).await;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => return,
                    _ => continue,
                }
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::debug!("WebSocket received shutdown signal during subscribe");
                    let close_frame = CloseFrame {
                        code: axum::extract::ws::close_code::NORMAL,
                        reason: "server shutting down".into(),
                    };
                    let _ = ws_tx.send(Message::Close(Some(close_frame))).await;
                    let _ = ws_tx.flush().await;
                    return;
                }
            }
        }
    };

    // Confirm subscription
    let subscribed_msg = serde_json::json!({
        "subscribed": subscribe.events.iter().map(|e| format!("{:?}", e).to_lowercase()).collect::<Vec<_>>()
    });
    if ws_tx
        .send(Message::Text(subscribed_msg.to_string()))
        .await
        .is_err()
    {
        return;
    }

    // Send initial Sync event with current screen state
    if let Ok(QueryResponse::Screen(screen)) = state
        .parser
        .query(Query::Screen {
            format: subscribe.format,
        })
        .await
    {
        let scrollback_lines = screen.total_lines;
        let sync_event = Event::Sync {
            seq: 0,
            screen,
            scrollback_lines,
        };
        if let Ok(json) = serde_json::to_string(&sync_event) {
            if ws_tx.send(Message::Text(json)).await.is_err() {
                return;
            }
        }
    }

    // Subscribe to parser events
    let mut events = Box::pin(state.parser.subscribe());
    let subscribed_types = subscribe.events;

    // Main event loop
    loop {
        tokio::select! {
            event = events.next() => {
                match event {
                    Some(event) => {
                        // Filter based on subscription
                        let should_send = match &event {
                            crate::parser::events::Event::Line { .. } => {
                                subscribed_types.contains(&EventType::Lines)
                            }
                            crate::parser::events::Event::Cursor { .. } => {
                                subscribed_types.contains(&EventType::Cursor)
                            }
                            crate::parser::events::Event::Mode { .. } => {
                                subscribed_types.contains(&EventType::Mode)
                            }
                            crate::parser::events::Event::Diff { .. } => {
                                subscribed_types.contains(&EventType::Diffs)
                            }
                            crate::parser::events::Event::Reset { .. }
                            | crate::parser::events::Event::Sync { .. } => true,
                        };

                        if should_send {
                            if let Ok(json) = serde_json::to_string(&event) {
                                if ws_tx.send(Message::Text(json)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    None => break,
                }
            }

            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // Handle resubscribe (simplified: just acknowledge)
                        if let Ok(_sub) = serde_json::from_str::<Subscribe>(&text) {
                            // In a full implementation, we'd update the filter
                            let ack = serde_json::json!({ "subscribed": true });
                            let _ = ws_tx.send(Message::Text(ack.to_string())).await;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => continue,
                }
            }

            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::debug!("WebSocket handler received shutdown signal");
                    let close_frame = CloseFrame {
                        code: axum::extract::ws::close_code::NORMAL,
                        reason: "server shutting down".into(),
                    };
                    let _ = ws_tx.send(Message::Close(Some(close_frame))).await;
                    let _ = ws_tx.flush().await;
                    break;
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct ScreenQuery {
    #[serde(default)]
    format: Format,
}

async fn screen(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<ScreenQuery>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let response = state
        .parser
        .query(Query::Screen { format: params.format })
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
        })?;

    Ok(Json(response))
}

#[derive(Deserialize)]
struct ScrollbackQuery {
    #[serde(default)]
    format: Format,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    100
}

async fn scrollback(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<ScrollbackQuery>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let response = state
        .parser
        .query(Query::Scrollback {
            format: params.format,
            offset: params.offset,
            limit: params.limit,
        })
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
        })?;

    Ok(Json(response))
}

// Overlay request/response types
#[derive(Deserialize)]
struct CreateOverlayRequest {
    x: u16,
    y: u16,
    z: Option<i32>,
    spans: Vec<OverlaySpan>,
}

#[derive(Serialize)]
struct CreateOverlayResponse {
    id: String,
}

#[derive(Deserialize)]
struct UpdateOverlayRequest {
    spans: Vec<OverlaySpan>,
}

#[derive(Deserialize)]
struct PatchOverlayRequest {
    x: Option<u16>,
    y: Option<u16>,
    z: Option<i32>,
}

// Overlay handlers
async fn overlay_create(
    State(state): State<AppState>,
    Json(req): Json<CreateOverlayRequest>,
) -> (StatusCode, Json<CreateOverlayResponse>) {
    let id = state.overlays.create(req.x, req.y, req.z, req.spans);
    (StatusCode::CREATED, Json(CreateOverlayResponse { id }))
}

async fn overlay_list(State(state): State<AppState>) -> Json<Vec<Overlay>> {
    Json(state.overlays.list())
}

async fn overlay_get(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<Overlay>, StatusCode> {
    state.overlays.get(&id).map(Json).ok_or(StatusCode::NOT_FOUND)
}

async fn overlay_update(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<UpdateOverlayRequest>,
) -> StatusCode {
    if state.overlays.update(&id, req.spans) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn overlay_patch(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<PatchOverlayRequest>,
) -> StatusCode {
    if state.overlays.move_to(&id, req.x, req.y, req.z) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn overlay_delete(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> StatusCode {
    if state.overlays.delete(&id) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn overlay_clear(State(state): State<AppState>) -> StatusCode {
    state.overlays.clear();
    StatusCode::NO_CONTENT
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/input", post(input))
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
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::Broker;
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
        };
        (state, input_rx)
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let (state, _input_rx) = create_test_state();
        let app = router(state);

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
        let app = router(state);

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
        let app = router(state);

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
        let app = router(state);

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
        let app = router(state);

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

        let app = router(state);

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

        let app = router(state);

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
}
