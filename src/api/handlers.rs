use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::IntoResponse,
    Json,
};

static OPENAPI_SPEC: &str = include_str!("../../docs/api/openapi.yaml");
static DOCS_INDEX: &str = include_str!("../../docs/api/README.md");
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use std::io::Write;

use crate::input::Mode;
use crate::overlay::{self, Overlay, OverlaySpan};
use crate::parser::{
    events::EventType,
    state::{Format, Query},
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

    // Mutable subscription state (initially no subscription)
    let mut subscribed_types: Vec<crate::parser::events::EventType> = Vec::new();

    // Subscribe to parser events (stream is always active, filtering is local)
    let mut events = Box::pin(state.parser.subscribe());

    // Input subscription (lazily created when EventType::Input is subscribed)
    let mut input_rx: Option<tokio::sync::broadcast::Receiver<crate::input::InputEvent>> = None;

    // Main event loop
    loop {
        tokio::select! {
            event = events.next() => {
                match event {
                    Some(event) if !subscribed_types.is_empty() => {
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
                    _ => {} // No subscription active, discard
                }
            }

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
                        input_rx = None;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                }
            }

            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // Parse as WsRequest
                        let req = match serde_json::from_str::<super::ws_methods::WsRequest>(&text) {
                            Ok(req) => req,
                            Err(_e) => {
                                let err = super::ws_methods::WsResponse::protocol_error(
                                    "invalid_request",
                                    "Invalid JSON or missing 'method' field.",
                                );
                                if let Ok(json) = serde_json::to_string(&err) {
                                    let _ = ws_tx.send(Message::Text(json)).await;
                                }
                                continue;
                            }
                        };

                        // Handle subscribe specially (needs to update local state)
                        if req.method == "subscribe" {
                            let params_value = req.params.clone().unwrap_or(serde_json::Value::Object(Default::default()));
                            match serde_json::from_value::<super::ws_methods::SubscribeParams>(params_value) {
                                Ok(params) => {
                                    subscribed_types = params.events.clone();
                                    let sub_format = params.format;

                                    // Set up input subscription if needed
                                    if subscribed_types.contains(&EventType::Input) {
                                        if input_rx.is_none() {
                                            input_rx = Some(state.input_broadcaster.subscribe());
                                        }
                                    } else {
                                        input_rx = None;
                                    }

                                    // Send response
                                    let event_names: Vec<String> = subscribed_types.iter()
                                        .map(|e| format!("{:?}", e).to_lowercase())
                                        .collect();
                                    let resp = super::ws_methods::WsResponse::success(
                                        req.id.clone(),
                                        "subscribe",
                                        serde_json::json!({"events": event_names}),
                                    );
                                    if let Ok(json) = serde_json::to_string(&resp) {
                                        if ws_tx.send(Message::Text(json)).await.is_err() {
                                            break;
                                        }
                                    }

                                    // Send sync event
                                    if let Ok(crate::parser::state::QueryResponse::Screen(screen)) = state
                                        .parser
                                        .query(crate::parser::state::Query::Screen { format: sub_format })
                                        .await
                                    {
                                        let scrollback_lines = screen.total_lines;
                                        let sync_event = crate::parser::events::Event::Sync {
                                            seq: 0,
                                            screen,
                                            scrollback_lines,
                                        };
                                        if let Ok(json) = serde_json::to_string(&sync_event) {
                                            if ws_tx.send(Message::Text(json)).await.is_err() {
                                                break;
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    let resp = super::ws_methods::WsResponse::error(
                                        req.id.clone(),
                                        "subscribe",
                                        "invalid_request",
                                        &format!("Invalid subscribe params: {}.", e),
                                    );
                                    if let Ok(json) = serde_json::to_string(&resp) {
                                        let _ = ws_tx.send(Message::Text(json)).await;
                                    }
                                }
                            }
                        } else {
                            // Dispatch all other methods
                            let resp = super::ws_methods::dispatch(&req, &state).await;
                            if let Ok(json) = serde_json::to_string(&resp) {
                                if ws_tx.send(Message::Text(json)).await.is_err() {
                                    break;
                                }
                            }
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

/// Write erase+render sequences for overlays to stdout immediately.
///
/// Erases `to_erase` overlays, then renders `to_render` overlays, all wrapped
/// in synchronized output to avoid tearing.
pub(super) fn flush_overlays_to_stdout(to_erase: &[Overlay], to_render: &[Overlay]) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(overlay::begin_sync().as_bytes());
    if !to_erase.is_empty() {
        let erase = overlay::erase_all_overlays(to_erase);
        let _ = lock.write_all(erase.as_bytes());
    }
    if !to_render.is_empty() {
        let render = overlay::render_all_overlays(to_render);
        let _ = lock.write_all(render.as_bytes());
    }
    let _ = lock.write_all(overlay::end_sync().as_bytes());
    let _ = lock.flush();
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
    let all = state.overlays.list();
    flush_overlays_to_stdout(&[], &all);
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
        .ok_or(ApiError::OverlayNotFound(id))
}

pub(super) async fn overlay_update(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<UpdateOverlayRequest>,
) -> Result<StatusCode, ApiError> {
    let old = state
        .overlays
        .get(&id)
        .ok_or_else(|| ApiError::OverlayNotFound(id.clone()))?;
    if state.overlays.update(&id, req.spans) {
        let all = state.overlays.list();
        flush_overlays_to_stdout(&[old], &all);
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
    let old = state
        .overlays
        .get(&id)
        .ok_or_else(|| ApiError::OverlayNotFound(id.clone()))?;
    if state.overlays.move_to(&id, req.x, req.y, req.z) {
        let all = state.overlays.list();
        flush_overlays_to_stdout(&[old], &all);
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::OverlayNotFound(id))
    }
}

pub(super) async fn overlay_delete(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<StatusCode, ApiError> {
    let old = state
        .overlays
        .get(&id)
        .ok_or_else(|| ApiError::OverlayNotFound(id.clone()))?;
    if state.overlays.delete(&id) {
        let remaining = state.overlays.list();
        flush_overlays_to_stdout(&[old], &remaining);
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::OverlayNotFound(id))
    }
}

pub(super) async fn overlay_clear(State(state): State<AppState>) -> StatusCode {
    let old_list = state.overlays.list();
    state.overlays.clear();
    flush_overlays_to_stdout(&old_list, &[]);
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

pub(super) async fn openapi_spec() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "text/yaml; charset=utf-8")],
        OPENAPI_SPEC,
    )
}

pub(super) async fn docs_index() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "text/markdown; charset=utf-8")],
        DOCS_INDEX,
    )
}
