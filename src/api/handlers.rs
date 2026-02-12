use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
        Path, State,
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
use crate::overlay::{self, BackgroundStyle, Overlay, OverlaySpan, RegionWrite};
use crate::panel::{self, Panel, Position};
use crate::parser::{
    events::EventType,
    state::{Format, Query},
};
use crate::pty::SpawnCommand;
use crate::session::{RegistryError, Session};

use super::error::ApiError;
use super::{get_session, AppState};

#[derive(Serialize)]
pub(super) struct HealthResponse {
    status: &'static str,
}

pub(super) async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub(super) async fn input(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    session.input_tx.send(body).await.map_err(|e| {
        tracing::error!("Failed to send input to PTY: {}", e);
        ApiError::InputSendFailed
    })?;
    session.activity.touch();
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn ws_raw(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    Ok(ws.on_upgrade(|socket| handle_ws_raw(socket, session, state.shutdown)))
}

async fn handle_ws_raw(
    socket: WebSocket,
    session: Session,
    shutdown: crate::shutdown::ShutdownCoordinator,
) {
    // Register this connection for graceful shutdown tracking
    let (_guard, mut shutdown_rx) = shutdown.register();

    let (mut ws_tx, mut ws_rx) = socket.split();

    let mut output_rx = session.output_rx.subscribe();
    let input_tx = session.input_tx.clone();

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
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    Ok(ws.on_upgrade(|socket| handle_ws_json(socket, session, state.shutdown)))
}

async fn handle_ws_json(
    socket: WebSocket,
    session: Session,
    shutdown: crate::shutdown::ShutdownCoordinator,
) {
    let (_guard, mut shutdown_rx) = shutdown.register();
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
    let mut events = Box::pin(session.parser.subscribe());

    // Input subscription (lazily created when EventType::Input is subscribed)
    let mut input_rx: Option<tokio::sync::broadcast::Receiver<crate::input::InputEvent>> = None;

    // Pending await_quiesce: (request_id, format, future resolving to Some(generation) or None on timeout)
    let mut pending_quiesce: Option<(
        Option<serde_json::Value>,
        crate::parser::state::Format,
        std::pin::Pin<Box<dyn std::future::Future<Output = Option<u64>> + Send>>,
    )> = None;

    // Quiescence subscription: background task signals through this channel
    let mut quiesce_sub_rx: Option<tokio::sync::mpsc::Receiver<()>> = None;
    let mut quiesce_sub_handle: Option<tokio::task::JoinHandle<()>> = None;
    let mut quiesce_sub_format = crate::parser::state::Format::default();

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

            // Pending await_quiesce resolves
            result = async {
                match &mut pending_quiesce {
                    Some((_, _, fut)) => fut.as_mut().await,
                    None => std::future::pending().await,
                }
            } => {
                let (req_id, format, _) = pending_quiesce.take().unwrap();
                if let Some(generation) = result {
                    // Quiescent — query screen and return
                    if let Ok(crate::parser::state::QueryResponse::Screen(screen)) = session
                        .parser
                        .query(crate::parser::state::Query::Screen { format })
                        .await
                    {
                        let scrollback_lines = screen.total_lines;
                        let resp = super::ws_methods::WsResponse::success(
                            req_id,
                            "await_quiesce",
                            serde_json::json!({
                                "screen": screen,
                                "scrollback_lines": scrollback_lines,
                                "generation": generation,
                            }),
                        );
                        if let Ok(json) = serde_json::to_string(&resp) {
                            if ws_tx.send(Message::Text(json)).await.is_err() {
                                break;
                            }
                        }
                    }
                } else {
                    // Timeout
                    let resp = super::ws_methods::WsResponse::error(
                        req_id,
                        "await_quiesce",
                        "quiesce_timeout",
                        "Terminal did not become quiescent within the deadline.",
                    );
                    if let Ok(json) = serde_json::to_string(&resp) {
                        if ws_tx.send(Message::Text(json)).await.is_err() {
                            break;
                        }
                    }
                }
            }

            // Quiescence subscription fires
            signal = async {
                match &mut quiesce_sub_rx {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match signal {
                    Some(()) => {
                        // Emit a sync event
                        if let Ok(crate::parser::state::QueryResponse::Screen(screen)) = session
                            .parser
                            .query(crate::parser::state::Query::Screen { format: quiesce_sub_format })
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
                    None => {
                        // Channel closed, clear subscription
                        quiesce_sub_rx = None;
                    }
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
                                            input_rx = Some(session.input_broadcaster.subscribe());
                                        }
                                    } else {
                                        input_rx = None;
                                    }

                                    // Set up quiescence subscription if requested
                                    if let Some(handle) = quiesce_sub_handle.take() {
                                        handle.abort();
                                    }
                                    quiesce_sub_rx = None;

                                    if params.quiesce_ms > 0 {
                                        let timeout = std::time::Duration::from_millis(params.quiesce_ms);
                                        let activity = session.activity.clone();
                                        let (tx, rx) = tokio::sync::mpsc::channel(1);
                                        quiesce_sub_rx = Some(rx);
                                        quiesce_sub_format = sub_format;

                                        quiesce_sub_handle = Some(tokio::spawn(async move {
                                            let mut watch_rx = activity.subscribe();
                                            loop {
                                                // Wait for activity
                                                if watch_rx.changed().await.is_err() {
                                                    break;
                                                }
                                                // Wait for quiescence (None: already gated on changed())
                                                activity.wait_for_quiescence(timeout, None).await;
                                                // Signal the main loop
                                                if tx.send(()).await.is_err() {
                                                    break;
                                                }
                                            }
                                        }));
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
                                    if let Ok(crate::parser::state::QueryResponse::Screen(screen)) = session
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
                        } else if req.method == "await_quiesce" {
                            // Handle await_quiesce specially (async wait)
                            let params_value = req.params.clone().unwrap_or(serde_json::Value::Object(Default::default()));
                            match serde_json::from_value::<super::ws_methods::AwaitQuiesceParams>(params_value) {
                                Ok(params) => {
                                    let timeout = std::time::Duration::from_millis(params.timeout_ms);
                                    let format = params.format;
                                    let activity = session.activity.clone();
                                    let last_generation = params.last_generation;
                                    let fresh = params.fresh;

                                    let fut: std::pin::Pin<Box<dyn std::future::Future<Output = Option<u64>> + Send>> = if let Some(max_wait) = params.max_wait_ms {
                                        let deadline = std::time::Duration::from_millis(max_wait);
                                        Box::pin(async move {
                                            let inner = if fresh {
                                                futures::future::Either::Left(activity.wait_for_fresh_quiescence(timeout))
                                            } else {
                                                futures::future::Either::Right(activity.wait_for_quiescence(timeout, last_generation))
                                            };
                                            tokio::time::timeout(deadline, inner)
                                                .await
                                                .ok()
                                        })
                                    } else {
                                        Box::pin(async move {
                                            let gen = if fresh {
                                                activity.wait_for_fresh_quiescence(timeout).await
                                            } else {
                                                activity.wait_for_quiescence(timeout, last_generation).await
                                            };
                                            Some(gen)
                                        })
                                    };

                                    // Replace any pending quiesce
                                    pending_quiesce = Some((req.id.clone(), format, fut));
                                }
                                Err(e) => {
                                    let resp = super::ws_methods::WsResponse::error(
                                        req.id.clone(),
                                        "await_quiesce",
                                        "invalid_request",
                                        &format!("Invalid await_quiesce params: {}.", e),
                                    );
                                    if let Ok(json) = serde_json::to_string(&resp) {
                                        let _ = ws_tx.send(Message::Text(json)).await;
                                    }
                                }
                            }
                        } else {
                            // Dispatch all other methods
                            let resp = super::ws_methods::dispatch(&req, &session).await;
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

    // Clean up quiescence subscription task
    if let Some(handle) = quiesce_sub_handle {
        handle.abort();
    }
}

// ---------------------------------------------------------------------------
// Server-level multiplexed WebSocket
// ---------------------------------------------------------------------------

pub(super) async fn ws_json_server(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_ws_json_server(socket, state))
}

/// A tagged session event forwarded through the internal mpsc channel.
struct TaggedSessionEvent {
    session: String,
    event: crate::parser::events::Event,
}

/// Tracks a per-session subscription's forwarding task.
struct SubHandle {
    subscribed_types: Vec<EventType>,
    task: tokio::task::JoinHandle<()>,
}

/// Convert a registry-level SessionEvent to a JSON value for the WS protocol.
/// Also handles cleanup of subscription handles on rename/destroy.
fn format_registry_event(
    event: &crate::session::SessionEvent,
    sub_handles: &mut std::collections::HashMap<String, SubHandle>,
) -> serde_json::Value {
    match event {
        crate::session::SessionEvent::Created { name } => {
            serde_json::json!({
                "event": "session_created",
                "params": { "name": name }
            })
        }
        crate::session::SessionEvent::Exited { name } => {
            serde_json::json!({
                "event": "session_exited",
                "params": { "name": name }
            })
        }
        crate::session::SessionEvent::Renamed { old_name, new_name } => {
            if let Some(handle) = sub_handles.remove(old_name.as_str()) {
                sub_handles.insert(new_name.clone(), handle);
            }
            serde_json::json!({
                "event": "session_renamed",
                "params": { "old_name": old_name, "new_name": new_name }
            })
        }
        crate::session::SessionEvent::Destroyed { name } => {
            if let Some(handle) = sub_handles.remove(name) {
                handle.task.abort();
            }
            serde_json::json!({
                "event": "session_destroyed",
                "params": { "name": name }
            })
        }
    }
}

/// Check if a per-session tagged event should be forwarded based on subscription filters.
fn should_forward_session_event(
    event: &crate::parser::events::Event,
    handle: &SubHandle,
) -> bool {
    match event {
        crate::parser::events::Event::Line { .. } => {
            handle.subscribed_types.contains(&EventType::Lines)
        }
        crate::parser::events::Event::Cursor { .. } => {
            handle.subscribed_types.contains(&EventType::Cursor)
        }
        crate::parser::events::Event::Mode { .. } => {
            handle.subscribed_types.contains(&EventType::Mode)
        }
        crate::parser::events::Event::Diff { .. } => {
            handle.subscribed_types.contains(&EventType::Diffs)
        }
        crate::parser::events::Event::Reset { .. }
        | crate::parser::events::Event::Sync { .. } => true,
    }
}

async fn handle_ws_json_server(socket: WebSocket, state: AppState) {
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

    // Subscribe to registry-level lifecycle events
    let mut registry_rx = state.sessions.subscribe_events();

    // Per-session subscription forwarding: all events funnel through this channel
    let (sub_tx, mut sub_rx) =
        tokio::sync::mpsc::channel::<TaggedSessionEvent>(256);

    // Track active subscription tasks by session name
    let mut sub_handles: std::collections::HashMap<String, SubHandle> =
        std::collections::HashMap::new();

    // Main event loop
    loop {
        tokio::select! {
            // Incoming client message
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let req = match serde_json::from_str::<super::ws_methods::ServerWsRequest>(&text) {
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

                        let is_subscribe = req.method == "subscribe";
                        let subscribe_session = req.session.clone();

                        let response = handle_server_ws_request(
                            &req,
                            &state,
                            &mut sub_handles,
                            &sub_tx,
                        )
                        .await;

                        if let Some(resp) = response {
                            // Check if this was a successful subscribe
                            let subscribe_ok = is_subscribe && resp.error.is_none();

                            if let Ok(json) = serde_json::to_string(&resp) {
                                if ws_tx.send(Message::Text(json)).await.is_err() {
                                    break;
                                }
                            }

                            // Send sync event after successful subscribe
                            if subscribe_ok {
                                if let Some(session_name) = &subscribe_session {
                                    if let Some(session) = state.sessions.get(session_name) {
                                        let format = {
                                            let params_value = req.params.clone().unwrap_or(serde_json::Value::Object(Default::default()));
                                            serde_json::from_value::<super::ws_methods::SubscribeParams>(params_value)
                                                .map(|p| p.format)
                                                .unwrap_or_default()
                                        };
                                        if let Ok(crate::parser::state::QueryResponse::Screen(screen)) = session
                                            .parser
                                            .query(crate::parser::state::Query::Screen { format })
                                            .await
                                        {
                                            let scrollback_lines = screen.total_lines;
                                            let sync_event = serde_json::json!({
                                                "event": "sync",
                                                "session": session_name,
                                                "params": {
                                                    "seq": 0,
                                                    "screen": screen,
                                                    "scrollback_lines": scrollback_lines,
                                                }
                                            });
                                            if let Ok(json) = serde_json::to_string(&sync_event) {
                                                if ws_tx.send(Message::Text(json)).await.is_err() {
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => continue,
                }
            }

            // Registry lifecycle events
            result = registry_rx.recv() => {
                match result {
                    Ok(event) => {
                        let event_json = format_registry_event(&event, &mut sub_handles);
                        if let Ok(json) = serde_json::to_string(&event_json) {
                            if ws_tx.send(Message::Text(json)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }

            // Per-session parser events forwarded from subscription tasks
            Some(tagged) = sub_rx.recv() => {
                if let Some(handle) = sub_handles.get(&tagged.session) {
                    if should_forward_session_event(&tagged.event, handle) {
                        if let Ok(event_value) = serde_json::to_value(&tagged.event) {
                            let tagged_json = if let serde_json::Value::Object(mut map) = event_value {
                                map.insert("session".to_string(), serde_json::json!(tagged.session));
                                serde_json::Value::Object(map)
                            } else {
                                event_value
                            };
                            if let Ok(json) = serde_json::to_string(&tagged_json) {
                                if ws_tx.send(Message::Text(json)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            // Shutdown signal
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::debug!("Server WebSocket received shutdown signal");
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

    // Clean up all subscription tasks
    for (_, handle) in sub_handles {
        handle.task.abort();
    }
}

/// Handle a single server-level WebSocket request.
///
/// Returns `Some(response)` for methods that produce a response.
async fn handle_server_ws_request(
    req: &super::ws_methods::ServerWsRequest,
    state: &AppState,
    sub_handles: &mut std::collections::HashMap<String, SubHandle>,
    sub_tx: &tokio::sync::mpsc::Sender<TaggedSessionEvent>,
) -> Option<super::ws_methods::WsResponse> {
    let id = req.id.clone();
    let method = req.method.as_str();

    // Server-level session management methods (no session field required)
    match method {
        "create_session" => {
            #[derive(Deserialize)]
            struct CreateParams {
                name: Option<String>,
                command: Option<String>,
                rows: Option<u16>,
                cols: Option<u16>,
                cwd: Option<String>,
                env: Option<std::collections::HashMap<String, String>>,
            }
            let params: CreateParams = match &req.params {
                Some(v) => match serde_json::from_value(v.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        return Some(super::ws_methods::WsResponse::error(
                            id,
                            method,
                            "invalid_request",
                            &format!("Invalid params: {}.", e),
                        ));
                    }
                },
                None => CreateParams {
                    name: None,
                    command: None,
                    rows: None,
                    cols: None,
                    cwd: None,
                    env: None,
                },
            };

            let command = match params.command {
                Some(cmd) => SpawnCommand::Command {
                    command: cmd,
                    interactive: true,
                },
                None => SpawnCommand::Shell {
                    interactive: true,
                    shell: None,
                },
            };

            let rows = params.rows.unwrap_or(24);
            let cols = params.cols.unwrap_or(80);

            let (session, child_exit_rx) =
                match Session::spawn_with_options("".to_string(), command, rows, cols, params.cwd, params.env) {
                    Ok(result) => result,
                    Err(e) => {
                        return Some(super::ws_methods::WsResponse::error(
                            id,
                            method,
                            "session_create_failed",
                            &format!("Failed to create session: {}.", e),
                        ));
                    }
                };

            match state.sessions.insert(params.name, session) {
                Ok(assigned_name) => {
                    // Monitor child exit so we emit SessionEvent::Exited.
                    state.sessions.monitor_child_exit(assigned_name.clone(), child_exit_rx);
                    return Some(super::ws_methods::WsResponse::success(
                        id,
                        method,
                        serde_json::json!({ "name": assigned_name }),
                    ));
                }
                Err(RegistryError::NameExists(n)) => {
                    return Some(super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "session_name_conflict",
                        &format!("Session name already exists: {}.", n),
                    ));
                }
                Err(RegistryError::NotFound(n)) => {
                    return Some(super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "session_not_found",
                        &format!("Session not found: {}.", n),
                    ));
                }
            }
        }

        "list_sessions" => {
            let names = state.sessions.list();
            let sessions: Vec<serde_json::Value> = names
                .into_iter()
                .map(|name| serde_json::json!({ "name": name }))
                .collect();
            return Some(super::ws_methods::WsResponse::success(
                id,
                method,
                serde_json::json!(sessions),
            ));
        }

        "kill_session" => {
            #[derive(Deserialize)]
            struct KillParams {
                name: String,
            }
            let params: KillParams = match &req.params {
                Some(v) => match serde_json::from_value(v.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        return Some(super::ws_methods::WsResponse::error(
                            id,
                            method,
                            "invalid_request",
                            &format!("Invalid params: {}.", e),
                        ));
                    }
                },
                None => {
                    return Some(super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "invalid_request",
                        "Missing 'params' with 'name' field.",
                    ));
                }
            };

            match state.sessions.remove(&params.name) {
                Some(_) => {
                    // Also clean up any subscription for this session
                    if let Some(handle) = sub_handles.remove(&params.name) {
                        handle.task.abort();
                    }
                    return Some(super::ws_methods::WsResponse::success(
                        id,
                        method,
                        serde_json::json!({}),
                    ));
                }
                None => {
                    return Some(super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "session_not_found",
                        &format!("Session not found: {}.", params.name),
                    ));
                }
            }
        }

        "detach_session" => {
            #[derive(Deserialize)]
            struct DetachParams {
                name: String,
            }
            let params: DetachParams = match &req.params {
                Some(v) => match serde_json::from_value(v.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        return Some(super::ws_methods::WsResponse::error(
                            id,
                            method,
                            "invalid_request",
                            &format!("Invalid params: {}.", e),
                        ));
                    }
                },
                None => {
                    return Some(super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "invalid_request",
                        "Missing 'params' with 'name' field.",
                    ));
                }
            };

            match state.sessions.get(&params.name) {
                Some(session) => {
                    session.detach();
                    return Some(super::ws_methods::WsResponse::success(
                        id,
                        method,
                        serde_json::json!({}),
                    ));
                }
                None => {
                    return Some(super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "session_not_found",
                        &format!("Session not found: {}.", params.name),
                    ));
                }
            }
        }

        "rename_session" => {
            #[derive(Deserialize)]
            struct RenameParams {
                name: String,
                new_name: String,
            }
            let params: RenameParams = match &req.params {
                Some(v) => match serde_json::from_value(v.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        return Some(super::ws_methods::WsResponse::error(
                            id,
                            method,
                            "invalid_request",
                            &format!("Invalid params: {}.", e),
                        ));
                    }
                },
                None => {
                    return Some(super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "invalid_request",
                        "Missing 'params' with 'name' and 'new_name' fields.",
                    ));
                }
            };

            match state.sessions.rename(&params.name, &params.new_name) {
                Ok(()) => {
                    // Update subscription key if it exists
                    if let Some(handle) = sub_handles.remove(&params.name) {
                        sub_handles.insert(params.new_name.clone(), handle);
                    }
                    return Some(super::ws_methods::WsResponse::success(
                        id,
                        method,
                        serde_json::json!({ "name": params.new_name }),
                    ));
                }
                Err(RegistryError::NameExists(n)) => {
                    return Some(super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "session_name_conflict",
                        &format!("Session name already exists: {}.", n),
                    ));
                }
                Err(RegistryError::NotFound(n)) => {
                    return Some(super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "session_not_found",
                        &format!("Session not found: {}.", n),
                    ));
                }
            }
        }

        "set_server_mode" => {
            if let Some(params) = &req.params {
                if let Some(persistent) = params.get("persistent").and_then(|v| v.as_bool()) {
                    state.server_config.set_persistent(persistent);
                }
            }
            return Some(super::ws_methods::WsResponse::success(
                id,
                method,
                serde_json::json!({"persistent": state.server_config.is_persistent()}),
            ));
        }

        _ => {
            // Not a server-level method; requires a session field.
        }
    }

    // Per-session methods: require a session field
    let session_name = match &req.session {
        Some(name) => name.clone(),
        None => {
            return Some(super::ws_methods::WsResponse::error(
                id,
                method,
                "session_required",
                "This method requires a 'session' field.",
            ));
        }
    };

    let session = match state.sessions.get(&session_name) {
        Some(s) => s,
        None => {
            return Some(super::ws_methods::WsResponse::error(
                id,
                method,
                "session_not_found",
                &format!("Session not found: {}.", session_name),
            ));
        }
    };

    // Handle subscribe specially (needs to set up forwarding task)
    if method == "subscribe" {
        let params_value = req
            .params
            .clone()
            .unwrap_or(serde_json::Value::Object(Default::default()));
        match serde_json::from_value::<super::ws_methods::SubscribeParams>(params_value) {
            Ok(params) => {
                let subscribed_types = params.events.clone();

                // Abort previous subscription for this session if any
                if let Some(old) = sub_handles.remove(&session_name) {
                    old.task.abort();
                }

                // Spawn a task that reads from the parser event stream and
                // forwards into the shared mpsc channel.
                let mut events = Box::pin(session.parser.subscribe());
                let tx = sub_tx.clone();
                let name = session_name.clone();
                let task = tokio::spawn(async move {
                    while let Some(event) = events.next().await {
                        if tx
                            .send(TaggedSessionEvent {
                                session: name.clone(),
                                event,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                });

                sub_handles.insert(
                    session_name.clone(),
                    SubHandle {
                        subscribed_types: subscribed_types.clone(),
                        task,
                    },
                );

                let event_names: Vec<String> = subscribed_types
                    .iter()
                    .map(|e| format!("{:?}", e).to_lowercase())
                    .collect();
                return Some(super::ws_methods::WsResponse::success(
                    id,
                    method,
                    serde_json::json!({ "events": event_names }),
                ));
            }
            Err(e) => {
                return Some(super::ws_methods::WsResponse::error(
                    id,
                    method,
                    "invalid_request",
                    &format!("Invalid subscribe params: {}.", e),
                ));
            }
        }
    }

    // Convert to a WsRequest and delegate to dispatch()
    let ws_req = super::ws_methods::WsRequest {
        id: req.id.clone(),
        method: req.method.clone(),
        params: req.params.clone(),
    };

    Some(super::ws_methods::dispatch(&ws_req, &session).await)
}

// Quiescence query parameters
#[derive(Deserialize)]
pub(super) struct QuiesceQuery {
    timeout_ms: u64,
    #[serde(default)]
    format: Format,
    #[serde(default = "default_max_wait")]
    max_wait_ms: u64,
    /// Generation from a previous quiescence response. If provided and matches
    /// the current generation, the server waits for new activity before
    /// checking quiescence — preventing a busy-loop storm.
    last_generation: Option<u64>,
    /// When true, always observe at least `timeout_ms` of real silence before
    /// responding, even if the terminal is already idle. Trades latency for
    /// API simplicity (no generation tracking required).
    #[serde(default)]
    fresh: bool,
}

fn default_max_wait() -> u64 {
    30_000
}

pub(super) async fn quiesce(
    State(state): State<AppState>,
    Path(name): Path<String>,
    axum::extract::Query(params): axum::extract::Query<QuiesceQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let timeout = std::time::Duration::from_millis(params.timeout_ms);
    let deadline = std::time::Duration::from_millis(params.max_wait_ms);

    let activity = &session.activity;
    let quiesce_fut = if params.fresh {
        futures::future::Either::Left(activity.wait_for_fresh_quiescence(timeout))
    } else {
        futures::future::Either::Right(
            activity.wait_for_quiescence(timeout, params.last_generation),
        )
    };

    match tokio::time::timeout(deadline, quiesce_fut).await {
        Ok(generation) => {
            // Quiescent — query screen state
            let response = session
                .parser
                .query(Query::Screen { format: params.format })
                .await
                .map_err(|_| ApiError::ParserUnavailable)?;

            match response {
                crate::parser::state::QueryResponse::Screen(screen) => {
                    let scrollback_lines = screen.total_lines;
                    Ok(Json(serde_json::json!({
                        "screen": screen,
                        "scrollback_lines": scrollback_lines,
                        "generation": generation,
                    })))
                }
                _ => Err(ApiError::ParserUnavailable),
            }
        }
        Err(_) => {
            // Deadline exceeded
            Err(ApiError::QuiesceTimeout)
        }
    }
}

// Server-level quiescence query parameters (any session)
#[derive(Deserialize)]
pub(super) struct QuiesceAnyQuery {
    timeout_ms: u64,
    #[serde(default)]
    format: Format,
    #[serde(default = "default_max_wait")]
    max_wait_ms: u64,
    /// Generation from a previous quiescence response, paired with `last_session`.
    /// When both are provided, the named session waits for new activity before
    /// checking quiescence (preventing busy-loop storms). Other sessions are
    /// checked immediately.
    last_generation: Option<u64>,
    /// The session name from a previous quiescence response.
    /// Used together with `last_generation`.
    last_session: Option<String>,
    /// When true, always observe at least `timeout_ms` of real silence before
    /// responding, even if a session is already idle.
    #[serde(default)]
    fresh: bool,
}

pub(super) async fn quiesce_any(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<QuiesceAnyQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let names = state.sessions.list();
    if names.is_empty() {
        return Err(ApiError::NoSessions);
    }

    let timeout = std::time::Duration::from_millis(params.timeout_ms);
    let deadline = std::time::Duration::from_millis(params.max_wait_ms);

    // Build a quiescence future for each session, racing them all.
    let mut futs = Vec::with_capacity(names.len());
    for name in &names {
        let session = match state.sessions.get(name) {
            Some(s) => s,
            None => continue, // session removed between list() and get()
        };
        let activity = session.activity.clone();
        let session_name = name.clone();

        let last_seen = if params.last_session.as_deref() == Some(name.as_str()) {
            params.last_generation
        } else {
            None
        };

        let fut = async move {
            let generation = if params.fresh {
                activity.wait_for_fresh_quiescence(timeout).await
            } else {
                activity.wait_for_quiescence(timeout, last_seen).await
            };
            (session_name, generation)
        };
        futs.push(fut);
    }

    // Race all futures under the overall deadline.
    let race = async {
        // select_all requires pinned futures
        let pinned: Vec<_> = futs.into_iter().map(|f| Box::pin(f)).collect();
        let (result, _index, _remaining) = futures::future::select_all(pinned).await;
        result
    };

    match tokio::time::timeout(deadline, race).await {
        Ok((session_name, generation)) => {
            let session = get_session(&state.sessions, &session_name)?;
            let response = session
                .parser
                .query(Query::Screen { format: params.format })
                .await
                .map_err(|_| ApiError::ParserUnavailable)?;

            match response {
                crate::parser::state::QueryResponse::Screen(screen) => {
                    let scrollback_lines = screen.total_lines;
                    Ok(Json(serde_json::json!({
                        "session": session_name,
                        "screen": screen,
                        "scrollback_lines": scrollback_lines,
                        "generation": generation,
                    })))
                }
                _ => Err(ApiError::ParserUnavailable),
            }
        }
        Err(_) => Err(ApiError::QuiesceTimeout),
    }
}

#[derive(Deserialize)]
pub(super) struct ScreenQuery {
    #[serde(default)]
    format: Format,
}

pub(super) async fn screen(
    State(state): State<AppState>,
    Path(name): Path<String>,
    axum::extract::Query(params): axum::extract::Query<ScreenQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let response = session
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
    Path(name): Path<String>,
    axum::extract::Query(params): axum::extract::Query<ScrollbackQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let response = session
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
    width: u16,
    height: u16,
    #[serde(default)]
    background: Option<BackgroundStyle>,
    spans: Vec<OverlaySpan>,
    #[serde(default)]
    focusable: bool,
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
    width: Option<u16>,
    height: Option<u16>,
}

#[derive(Deserialize)]
pub(super) struct UpdateSpansRequest {
    spans: Vec<OverlaySpan>,
}

#[derive(Deserialize)]
pub(super) struct RegionWriteRequest {
    writes: Vec<RegionWrite>,
}

// Overlay handlers
pub(super) async fn overlay_create(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<CreateOverlayRequest>,
) -> Result<(StatusCode, Json<CreateOverlayResponse>), ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let current_mode = *session.screen_mode.read();
    let id = session.overlays.create(req.x, req.y, req.z, req.width, req.height, req.background, req.spans, req.focusable, current_mode);
    if session.is_local {
        let all = session.overlays.list();
        flush_overlays_to_stdout(&[], &all);
    }
    Ok((StatusCode::CREATED, Json(CreateOverlayResponse { id })))
}

pub(super) async fn overlay_list(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Vec<Overlay>>, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let mode = *session.screen_mode.read();
    Ok(Json(session.overlays.list_by_mode(mode)))
}

pub(super) async fn overlay_get(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
) -> Result<Json<Overlay>, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    session
        .overlays
        .get(&id)
        .map(Json)
        .ok_or(ApiError::OverlayNotFound(id))
}

pub(super) async fn overlay_update(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
    Json(req): Json<UpdateOverlayRequest>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let old = session
        .overlays
        .get(&id)
        .ok_or_else(|| ApiError::OverlayNotFound(id.clone()))?;
    if session.overlays.update(&id, req.spans) {
        if session.is_local {
            let all = session.overlays.list();
            flush_overlays_to_stdout(&[old], &all);
        }
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::OverlayNotFound(id))
    }
}

pub(super) async fn overlay_patch(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
    Json(req): Json<PatchOverlayRequest>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let old = session
        .overlays
        .get(&id)
        .ok_or_else(|| ApiError::OverlayNotFound(id.clone()))?;
    if session.overlays.move_to(&id, req.x, req.y, req.z, req.width, req.height) {
        if session.is_local {
            let all = session.overlays.list();
            flush_overlays_to_stdout(&[old], &all);
        }
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::OverlayNotFound(id))
    }
}

pub(super) async fn overlay_delete(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let old = session
        .overlays
        .get(&id)
        .ok_or_else(|| ApiError::OverlayNotFound(id.clone()))?;
    if session.overlays.delete(&id) {
        session.focus.clear_if_focused(&id);
        if session.is_local {
            let remaining = session.overlays.list();
            flush_overlays_to_stdout(&[old], &remaining);
        }
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::OverlayNotFound(id))
    }
}

pub(super) async fn overlay_clear(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let old_list = session.overlays.list();
    session.overlays.clear();
    session.focus.unfocus();
    if session.is_local {
        flush_overlays_to_stdout(&old_list, &[]);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn overlay_update_spans(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
    Json(req): Json<UpdateSpansRequest>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let old = session
        .overlays
        .get(&id)
        .ok_or_else(|| ApiError::OverlayNotFound(id.clone()))?;
    if session.overlays.update_spans(&id, &req.spans) {
        if session.is_local {
            let all = session.overlays.list();
            flush_overlays_to_stdout(&[old], &all);
        }
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::OverlayNotFound(id))
    }
}

pub(super) async fn overlay_region_write(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
    Json(req): Json<RegionWriteRequest>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let old = session
        .overlays
        .get(&id)
        .ok_or_else(|| ApiError::OverlayNotFound(id.clone()))?;
    if session.overlays.region_write(&id, req.writes) {
        if session.is_local {
            let all = session.overlays.list();
            flush_overlays_to_stdout(&[old], &all);
        }
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::OverlayNotFound(id))
    }
}

// Panel request/response types

#[derive(Deserialize)]
pub(super) struct CreatePanelRequest {
    position: Position,
    height: u16,
    z: Option<i32>,
    #[serde(default)]
    background: Option<BackgroundStyle>,
    #[serde(default)]
    spans: Vec<OverlaySpan>,
    #[serde(default)]
    focusable: bool,
}

#[derive(Serialize)]
pub(super) struct CreatePanelResponse {
    id: String,
}

#[derive(Deserialize)]
pub(super) struct UpdatePanelRequest {
    position: Position,
    height: u16,
    z: i32,
    spans: Vec<OverlaySpan>,
}

#[derive(Deserialize)]
pub(super) struct PatchPanelRequest {
    position: Option<Position>,
    height: Option<u16>,
    z: Option<i32>,
    #[serde(default)]
    background: Option<BackgroundStyle>,
    spans: Option<Vec<OverlaySpan>>,
}

// Panel handlers

pub(super) async fn panel_create(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<CreatePanelRequest>,
) -> Result<(StatusCode, Json<CreatePanelResponse>), ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let current_mode = *session.screen_mode.read();
    let id = session
        .panels
        .create(req.position, req.height, req.z, req.background, req.spans, req.focusable, current_mode);
    panel::reconfigure_layout(&session.panels, &session.terminal_size, &session.pty, &session.parser)
        .await;
    Ok((StatusCode::CREATED, Json(CreatePanelResponse { id })))
}

pub(super) async fn panel_list(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Vec<Panel>>, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let mode = *session.screen_mode.read();
    Ok(Json(session.panels.list_by_mode(mode)))
}

pub(super) async fn panel_get(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
) -> Result<Json<Panel>, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    session
        .panels
        .get(&id)
        .map(Json)
        .ok_or(ApiError::PanelNotFound(id))
}

pub(super) async fn panel_update(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
    Json(req): Json<UpdatePanelRequest>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let old = session
        .panels
        .get(&id)
        .ok_or_else(|| ApiError::PanelNotFound(id.clone()))?;

    // Full replace: update all fields via patch
    if !session
        .panels
        .patch(&id, Some(req.position.clone()), Some(req.height), Some(req.z), None, Some(req.spans))
    {
        return Err(ApiError::PanelNotFound(id));
    }

    // Check if layout-affecting fields changed
    let needs_reconfigure =
        old.position != req.position || old.height != req.height || old.z != req.z;

    if needs_reconfigure {
        panel::reconfigure_layout(&session.panels, &session.terminal_size, &session.pty, &session.parser)
            .await;
    } else {
        panel::flush_panel_content(&session.panels, &id, &session.terminal_size);
    }

    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn panel_patch(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
    Json(req): Json<PatchPanelRequest>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let old = session
        .panels
        .get(&id)
        .ok_or_else(|| ApiError::PanelNotFound(id.clone()))?;

    if !session
        .panels
        .patch(&id, req.position.clone(), req.height, req.z, req.background, req.spans.clone())
    {
        return Err(ApiError::PanelNotFound(id));
    }

    // Check if layout-affecting fields changed
    let needs_reconfigure = req.position.is_some_and(|p| p != old.position)
        || req.height.is_some_and(|h| h != old.height)
        || req.z.is_some_and(|z| z != old.z);

    if needs_reconfigure {
        panel::reconfigure_layout(&session.panels, &session.terminal_size, &session.pty, &session.parser)
            .await;
    } else if req.spans.is_some() {
        panel::flush_panel_content(&session.panels, &id, &session.terminal_size);
    }

    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn panel_delete(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    if !session.panels.delete(&id) {
        return Err(ApiError::PanelNotFound(id));
    }
    session.focus.clear_if_focused(&id);
    panel::reconfigure_layout(&session.panels, &session.terminal_size, &session.pty, &session.parser)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn panel_clear(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    session.panels.clear();
    session.focus.unfocus();
    panel::reconfigure_layout(&session.panels, &session.terminal_size, &session.pty, &session.parser)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn panel_update_spans(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
    Json(req): Json<UpdateSpansRequest>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    if session.panels.update_spans(&id, &req.spans) {
        panel::flush_panel_content(&session.panels, &id, &session.terminal_size);
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::PanelNotFound(id))
    }
}

pub(super) async fn panel_region_write(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
    Json(req): Json<RegionWriteRequest>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    if session.panels.region_write(&id, req.writes) {
        panel::flush_panel_content(&session.panels, &id, &session.terminal_size);
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::PanelNotFound(id))
    }
}

// Input mode response type
#[derive(Serialize)]
pub(super) struct InputModeResponse {
    mode: Mode,
}

// Input mode handlers
pub(super) async fn input_mode_get(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<InputModeResponse>, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    Ok(Json(InputModeResponse {
        mode: session.input_mode.get(),
    }))
}

pub(super) async fn input_capture(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    session.input_mode.capture();
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn input_release(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    session.input_mode.release();
    session.focus.unfocus();
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(super) struct FocusRequest {
    pub id: String,
}

#[derive(Serialize)]
pub(super) struct FocusResponse {
    pub focused: Option<String>,
}

pub(super) async fn input_focus(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<FocusRequest>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;

    // Check if the target is a focusable overlay or panel
    let is_focusable = if let Some(overlay) = session.overlays.get(&req.id) {
        overlay.focusable
    } else if let Some(panel) = session.panels.get(&req.id) {
        panel.focusable
    } else {
        return Err(ApiError::InvalidRequest(format!(
            "no overlay or panel with id '{}'",
            req.id
        )));
    };

    if !is_focusable {
        return Err(ApiError::NotFocusable(req.id));
    }

    session.focus.focus(req.id);
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn input_unfocus(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    session.focus.unfocus();
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn input_focus_get(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<FocusResponse>, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    Ok(Json(FocusResponse {
        focused: session.focus.focused(),
    }))
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

// ── Session management types ─────────────────────────────────────

#[derive(Deserialize)]
pub(super) struct CreateSessionRequest {
    pub name: Option<String>,
    pub command: Option<String>,
    pub rows: Option<u16>,
    pub cols: Option<u16>,
    pub cwd: Option<String>,
    pub env: Option<std::collections::HashMap<String, String>>,
}

#[derive(Serialize)]
pub(super) struct SessionInfo {
    pub name: String,
}

#[derive(Deserialize)]
pub(super) struct RenameSessionRequest {
    pub name: String,
}

// ── Session management handlers ──────────────────────────────────

pub(super) async fn session_list(
    State(state): State<AppState>,
) -> Json<Vec<SessionInfo>> {
    let names = state.sessions.list();
    let infos = names.into_iter().map(|name| SessionInfo { name }).collect();
    Json(infos)
}

pub(super) async fn session_create(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<SessionInfo>), ApiError> {
    let command = match req.command {
        Some(cmd) => SpawnCommand::Command {
            command: cmd,
            interactive: true,
        },
        None => SpawnCommand::Shell {
            interactive: true,
            shell: None,
        },
    };

    let rows = req.rows.unwrap_or(24);
    let cols = req.cols.unwrap_or(80);

    // Use a placeholder name for spawn; registry.insert will assign the real name.
    let (session, child_exit_rx) =
        Session::spawn_with_options("".to_string(), command, rows, cols, req.cwd, req.env)
            .map_err(|e| ApiError::SessionCreateFailed(e.to_string()))?;

    let assigned_name = state
        .sessions
        .insert(req.name, session)
        .map_err(|e| match e {
            RegistryError::NameExists(n) => ApiError::SessionNameConflict(n),
            RegistryError::NotFound(n) => ApiError::SessionNotFound(n),
        })?;

    // Monitor child exit so we emit SessionEvent::Exited when the process dies.
    state.sessions.monitor_child_exit(assigned_name.clone(), child_exit_rx);

    Ok((
        StatusCode::CREATED,
        Json(SessionInfo {
            name: assigned_name,
        }),
    ))
}

pub(super) async fn session_get(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<SessionInfo>, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    Ok(Json(SessionInfo {
        name: session.name,
    }))
}

pub(super) async fn session_rename(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<RenameSessionRequest>,
) -> Result<Json<SessionInfo>, ApiError> {
    state.sessions.rename(&name, &req.name).map_err(|e| match e {
        RegistryError::NameExists(n) => ApiError::SessionNameConflict(n),
        RegistryError::NotFound(n) => ApiError::SessionNotFound(n),
    })?;

    Ok(Json(SessionInfo { name: req.name }))
}

pub(super) async fn session_kill(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .sessions
        .remove(&name)
        .ok_or_else(|| ApiError::SessionNotFound(name))?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn session_detach(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let session = state
        .sessions
        .get(&name)
        .ok_or_else(|| ApiError::SessionNotFound(name))?;
    session.detach();
    Ok(StatusCode::NO_CONTENT)
}

// ── Screen mode handlers ──────────────────────────────────────

#[derive(Serialize)]
pub(super) struct ScreenModeResponse {
    pub mode: crate::overlay::ScreenMode,
}

pub(super) async fn screen_mode_get(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<ScreenModeResponse>, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let mode = *session.screen_mode.read();
    Ok(Json(ScreenModeResponse { mode }))
}

pub(super) async fn enter_alt_screen(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let mut mode = session.screen_mode.write();
    if *mode == crate::overlay::ScreenMode::Alt {
        return Err(ApiError::AlreadyInAltScreen);
    }
    *mode = crate::overlay::ScreenMode::Alt;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn exit_alt_screen(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    {
        let mut mode = session.screen_mode.write();
        if *mode == crate::overlay::ScreenMode::Normal {
            return Err(ApiError::NotInAltScreen);
        }
        *mode = crate::overlay::ScreenMode::Normal;
    }
    // Delete all alt-mode overlays and panels
    session.overlays.delete_by_mode(crate::overlay::ScreenMode::Alt);
    session.panels.delete_by_mode(crate::overlay::ScreenMode::Alt);
    // Reconfigure panel layout to restore normal-mode panels
    panel::reconfigure_layout(
        &session.panels,
        &session.terminal_size,
        &session.pty,
        &session.parser,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn server_persist(
    State(state): State<AppState>,
) -> impl IntoResponse {
    state.server_config.set_persistent(true);
    (StatusCode::OK, Json(serde_json::json!({"persistent": true})))
}
