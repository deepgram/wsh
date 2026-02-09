use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::input::Mode;
use crate::overlay::{Overlay, OverlaySpan};
use crate::parser::{
    events::{Event, EventType, Subscribe},
    state::{Format, Query, QueryResponse},
};

use super::error::ApiError;
use super::AppState;

#[derive(Serialize)]
pub(super) struct HealthResponse {
    status: &'static str,
}

pub(super) async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub(super) async fn input(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    state.input_tx.send(body).await.map_err(|e| {
        tracing::error!("Failed to send input to PTY: {}", e);
        ApiError::InputSendFailed
    })?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn ws_raw(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
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

pub(super) async fn ws_json(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
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

    // Subscribe to input events if requested
    let mut input_rx = if subscribed_types.contains(&EventType::Input) {
        Some(state.input_broadcaster.subscribe())
    } else {
        None
    };

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

            // Handle input events if subscribed
            input_event = async {
                match &mut input_rx {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match input_event {
                    Ok(event) => {
                        if let Ok(json) = serde_json::to_string(&event) {
                            if ws_tx.send(Message::Text(json)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Broadcaster closed, remove subscription
                        input_rx = None;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Missed some events, continue
                    }
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
pub(super) struct ScreenQuery {
    #[serde(default)]
    format: Format,
}

pub(super) async fn screen(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<ScreenQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let response = state
        .parser
        .query(Query::Screen { format: params.format })
        .await
        .map_err(|_| ApiError::ParserUnavailable)?;

    Ok(Json(response))
}

#[derive(Deserialize)]
pub(super) struct ScrollbackQuery {
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

pub(super) async fn scrollback(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<ScrollbackQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let response = state
        .parser
        .query(Query::Scrollback {
            format: params.format,
            offset: params.offset,
            limit: params.limit,
        })
        .await
        .map_err(|_| ApiError::ParserUnavailable)?;

    Ok(Json(response))
}

// Overlay request/response types
#[derive(Deserialize)]
pub(super) struct CreateOverlayRequest {
    x: u16,
    y: u16,
    z: Option<i32>,
    spans: Vec<OverlaySpan>,
}

#[derive(Serialize)]
pub(super) struct CreateOverlayResponse {
    id: String,
}

#[derive(Deserialize)]
pub(super) struct UpdateOverlayRequest {
    spans: Vec<OverlaySpan>,
}

#[derive(Deserialize)]
pub(super) struct PatchOverlayRequest {
    x: Option<u16>,
    y: Option<u16>,
    z: Option<i32>,
}

// Overlay handlers
pub(super) async fn overlay_create(
    State(state): State<AppState>,
    Json(req): Json<CreateOverlayRequest>,
) -> (StatusCode, Json<CreateOverlayResponse>) {
    let id = state.overlays.create(req.x, req.y, req.z, req.spans);
    (StatusCode::CREATED, Json(CreateOverlayResponse { id }))
}

pub(super) async fn overlay_list(State(state): State<AppState>) -> Json<Vec<Overlay>> {
    Json(state.overlays.list())
}

pub(super) async fn overlay_get(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<Overlay>, ApiError> {
    state
        .overlays
        .get(&id)
        .map(Json)
        .ok_or_else(|| ApiError::OverlayNotFound(id))
}

pub(super) async fn overlay_update(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<UpdateOverlayRequest>,
) -> Result<StatusCode, ApiError> {
    if state.overlays.update(&id, req.spans) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::OverlayNotFound(id))
    }
}

pub(super) async fn overlay_patch(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<PatchOverlayRequest>,
) -> Result<StatusCode, ApiError> {
    if state.overlays.move_to(&id, req.x, req.y, req.z) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::OverlayNotFound(id))
    }
}

pub(super) async fn overlay_delete(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<StatusCode, ApiError> {
    if state.overlays.delete(&id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::OverlayNotFound(id))
    }
}

pub(super) async fn overlay_clear(State(state): State<AppState>) -> StatusCode {
    state.overlays.clear();
    StatusCode::NO_CONTENT
}

// Input mode response type
#[derive(Serialize)]
pub(super) struct InputModeResponse {
    mode: Mode,
}

// Input mode handlers
pub(super) async fn input_mode_get(State(state): State<AppState>) -> Json<InputModeResponse> {
    Json(InputModeResponse {
        mode: state.input_mode.get(),
    })
}

pub(super) async fn input_capture(State(state): State<AppState>) -> StatusCode {
    state.input_mode.capture();
    StatusCode::NO_CONTENT
}

pub(super) async fn input_release(State(state): State<AppState>) -> StatusCode {
    state.input_mode.release();
    StatusCode::NO_CONTENT
}
