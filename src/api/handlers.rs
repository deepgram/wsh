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

use crate::input::Mode;
use crate::overlay::{BackgroundStyle, Overlay, OverlaySpan, RegionWrite};
use crate::panel::{self, Panel, Position};
use crate::parser::{
    events::EventType,
    state::{Format, Query},
};
use crate::pty::SpawnCommand;
use crate::session::{RegistryError, Session};

use super::error::ApiError;
use super::{get_session, AppState};

/// WebSocket send timeout. If a send takes longer than this, the client is
/// considered dead and the connection is closed. Kept short (5s) to minimize
/// the time a slow/stalled client can freeze the handler's select! loop
/// (blocking ping/pong, idle detection, shutdown, and client messages).
const WS_SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Timeout for parser query calls from HTTP handlers. Prevents a stalled
/// parser from hanging an agent's HTTP request indefinitely.
const PARSER_QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Maximum WebSocket message size (1 MB). Matches the HTTP DefaultBodyLimit to
/// prevent a single WS text frame from allocating unbounded memory during
/// deserialization. The default tungstenite limit is 64 MB which is far too
/// generous for terminal I/O payloads.
const MAX_WS_MESSAGE_SIZE: usize = 1024 * 1024;

/// Maximum allowed value for timeout_ms and max_wait_ms parameters.
/// Prevents clients from holding connections open indefinitely.
const MAX_WAIT_CEILING_MS: u64 = 300_000; // 5 minutes

/// Pending await_idle state: (request_id, method_name, format, future resolving to generation or None on timeout)
type PendingIdle = (
    Option<serde_json::Value>,
    String,
    crate::parser::state::Format,
    std::pin::Pin<Box<dyn std::future::Future<Output = Option<u64>> + Send>>,
);

/// Activity state change sent from the background idle/running monitor task.
enum ActivityStateChange {
    Running { generation: u64 },
    Idle { generation: u64 },
}

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
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        session.input_tx.send(body),
    )
    .await
    .map_err(|_| ApiError::InputSendFailed)?
    .map_err(|e| {
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
    let client_guard = session.connect().ok_or_else(|| {
        ApiError::ResourceLimitReached("too many clients connected to session".into())
    })?;
    Ok(ws.max_message_size(MAX_WS_MESSAGE_SIZE)
        .on_upgrade(|socket| handle_ws_raw(socket, session, state.shutdown, client_guard)))
}

async fn handle_ws_raw(
    socket: WebSocket,
    session: Session,
    shutdown: crate::shutdown::ShutdownCoordinator,
    _client_guard: crate::session::ClientGuard,
) {
    // Register this connection for graceful shutdown tracking.
    // Check borrow immediately after register to handle the case where
    // this connection was upgraded after shutdown was already signaled
    // (the watch::changed() future only fires on *changes*, so a handler
    // that starts after the signal would never see the change).
    let (_guard, mut shutdown_rx) = shutdown.register();
    if *shutdown_rx.borrow_and_update() {
        return;
    }

    let (mut ws_tx, mut ws_rx) = socket.split();

    let mut output_rx = session.output_rx.subscribe();
    let input_tx = session.input_tx.clone();

    // Ping/pong keepalive
    let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(30));
    ping_interval.reset(); // don't fire immediately
    let mut last_pong = tokio::time::Instant::now();
    let mut ping_sent = false;
    const PONG_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

    // Main loop: handle PTY output, WebSocket input, and shutdown signal
    loop {
        tokio::select! {
            // PTY output -> WebSocket
            result = output_rx.recv() => {
                match result {
                    Ok(data) => {
                        match tokio::time::timeout(WS_SEND_TIMEOUT, ws_tx.send(Message::Binary(data.to_vec()))).await {
                            Ok(Ok(())) => {}
                            Ok(Err(_)) => break,
                            Err(_) => {
                                tracing::debug!("ws_raw send timed out, closing");
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "ws_raw client lagged, sending screen sync");
                        // ── Lag recovery: full screen sync ───────────────────
                        //
                        // Matches the socket server and ws_json strategies
                        // (see design decision comment in server.rs). Query
                        // the parser for current screen state, render as raw
                        // ANSI bytes, and send as a Binary frame. The client
                        // stays connected with a correct terminal view.
                        // ─────────────────────────────────────────────────────
                        use crate::parser::ansi::line_to_ansi;
                        use crate::parser::state::{Format, Query, QueryResponse};
                        if let Ok(Ok(QueryResponse::Screen(screen))) = tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            session.parser.query(Query::Screen { format: Format::Styled }),
                        ).await {
                            let mut buf = String::new();
                            buf.push_str("\x1b[H\x1b[2J");
                            for (i, line) in screen.lines.iter().enumerate() {
                                buf.push_str(&line_to_ansi(line));
                                if i + 1 < screen.lines.len() {
                                    buf.push_str("\r\n");
                                }
                            }
                            buf.push_str(&format!(
                                "\x1b[{};{}H",
                                screen.cursor.row + 1,
                                screen.cursor.col + 1,
                            ));
                            match tokio::time::timeout(
                                WS_SEND_TIMEOUT,
                                ws_tx.send(Message::Binary(buf.into_bytes())),
                            ).await {
                                Ok(Ok(())) => {}
                                _ => break,
                            }
                        }
                    }
                }
            }

            // WebSocket input -> PTY
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            input_tx.send(Bytes::from(data)),
                        ).await {
                            Ok(Ok(())) => {}
                            Ok(Err(_)) => break,
                            Err(_) => {
                                tracing::warn!("ws_raw input send timed out, closing");
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            input_tx.send(Bytes::from(text)),
                        ).await {
                            Ok(Ok(())) => {}
                            Ok(Err(_)) => break,
                            Err(_) => {
                                tracing::warn!("ws_raw input send timed out, closing");
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {
                        last_pong = tokio::time::Instant::now();
                        ping_sent = false;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => continue,
                    Some(Err(_)) => break,
                }
            }

            // Ping keepalive
            _ = ping_interval.tick() => {
                if ping_sent && last_pong.elapsed() > PONG_TIMEOUT {
                    tracing::debug!("ws_raw client unresponsive (no pong), closing");
                    break;
                }
                match tokio::time::timeout(WS_SEND_TIMEOUT, ws_tx.send(Message::Ping(vec![]))).await {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) | Err(_) => break,
                }
                ping_sent = true;
            }

            // Session was killed/removed
            _ = session.cancelled.cancelled() => {
                tracing::debug!("session was killed, closing WebSocket");
                break;
            }

            // Shutdown signal
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::debug!("WebSocket received shutdown signal, closing");
                    break;
                }
            }
        }
    }

    // Send close frame with timeout (Phase 2c)
    let close_frame = CloseFrame {
        code: axum::extract::ws::close_code::NORMAL,
        reason: "session ended".into(),
    };
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        ws_tx.send(Message::Close(Some(close_frame))),
    ).await;

    // _guard is dropped here, decrementing active connection count
}

pub(super) async fn ws_json(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let client_guard = session.connect().ok_or_else(|| {
        ApiError::ResourceLimitReached("too many clients connected to session".into())
    })?;
    Ok(ws.max_message_size(MAX_WS_MESSAGE_SIZE)
        .on_upgrade(|socket| handle_ws_json(socket, session, state.shutdown, client_guard)))
}

async fn handle_ws_json(
    socket: WebSocket,
    session: Session,
    shutdown: crate::shutdown::ShutdownCoordinator,
    _client_guard: crate::session::ClientGuard,
) {
    let (_guard, mut shutdown_rx) = shutdown.register();
    if *shutdown_rx.borrow_and_update() {
        return;
    }
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

    let mut pending_idle: Option<PendingIdle> = None;

    // Activity subscription: background task signals Running/Idle transitions
    let mut activity_sub_rx: Option<tokio::sync::mpsc::Receiver<ActivityStateChange>> = None;
    let mut activity_sub_handle: Option<tokio::task::JoinHandle<()>> = None;
    let mut activity_sub_format = crate::parser::state::Format::default();

    // Ping/pong keepalive
    let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(30));
    ping_interval.reset();
    let mut last_pong = tokio::time::Instant::now();
    let mut ping_sent = false;
    const PONG_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

    /// Send a WebSocket message with a timeout. Returns false if the send
    /// failed or timed out (caller should break out of the loop).
    macro_rules! ws_send {
        ($tx:expr, $msg:expr) => {
            match tokio::time::timeout(WS_SEND_TIMEOUT, $tx.send($msg)).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => break,
                Err(_) => {
                    tracing::debug!("ws_json send timed out, closing");
                    break;
                }
            }
        };
    }

    // Main event loop
    loop {
        tokio::select! {
            sub_event = events.next() => {
                match sub_event {
                    Some(crate::parser::SubscriptionEvent::Event(event)) if !subscribed_types.is_empty() => {
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
                            crate::parser::events::Event::Idle { .. }
                            | crate::parser::events::Event::Running { .. } => {
                                subscribed_types.contains(&EventType::Activity)
                            }
                        };

                        if should_send {
                            if let Ok(json) = serde_json::to_string(&event) {
                                ws_send!(ws_tx, Message::Text(json));
                            }
                        }
                    }
                    Some(crate::parser::SubscriptionEvent::Lagged(n)) => {
                        tracing::warn!(skipped = n, "parser event subscriber lagged");
                        let lag_msg = serde_json::json!({"type": "lagged", "skipped": n});
                        if let Ok(json) = serde_json::to_string(&lag_msg) {
                            ws_send!(ws_tx, Message::Text(json));
                        }
                        // After lag, push a full sync so the client can recover.
                        // Without this, the client has an incomplete view of state.
                        if let Ok(Ok(crate::parser::state::QueryResponse::Screen(screen))) = tokio::time::timeout(
                            std::time::Duration::from_secs(10),
                            session.parser.query(crate::parser::state::Query::Screen {
                                format: crate::parser::state::Format::default(),
                            }),
                        ).await {
                            let scrollback_lines = screen.total_lines;
                            let sync_event = crate::parser::events::Event::Sync {
                                seq: 0,
                                screen,
                                scrollback_lines,
                            };
                            if let Ok(json) = serde_json::to_string(&sync_event) {
                                ws_send!(ws_tx, Message::Text(json));
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
                            ws_send!(ws_tx, Message::Text(json));
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        input_rx = None;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "input event subscriber lagged");
                        let lag_msg = serde_json::json!({"type": "input_lagged", "skipped": n});
                        if let Ok(json) = serde_json::to_string(&lag_msg) {
                            ws_send!(ws_tx, Message::Text(json));
                        }
                    }
                }
            }

            // Pending await_idle resolves
            result = async {
                match &mut pending_idle {
                    Some((_, _, _, fut)) => fut.as_mut().await,
                    None => std::future::pending().await,
                }
            } => {
                let (req_id, method_name, format, _) = pending_idle.take().unwrap();
                if let Some(generation) = result {
                    // Idle — query screen and return (with timeout to avoid blocking the loop)
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        session.parser.query(crate::parser::state::Query::Screen { format }),
                    ).await {
                        Ok(Ok(crate::parser::state::QueryResponse::Screen(screen))) => {
                            let scrollback_lines = screen.total_lines;
                            let resp = super::ws_methods::WsResponse::success(
                                req_id,
                                &method_name,
                                serde_json::json!({
                                    "screen": screen,
                                    "scrollback_lines": scrollback_lines,
                                    "generation": generation,
                                }),
                            );
                            if let Ok(json) = serde_json::to_string(&resp) {
                                ws_send!(ws_tx, Message::Text(json));
                            }
                        }
                        _ => {
                            let resp = super::ws_methods::WsResponse::error(
                                req_id,
                                &method_name,
                                "parser_error",
                                "Terminal is idle but screen query failed.",
                            );
                            if let Ok(json) = serde_json::to_string(&resp) {
                                ws_send!(ws_tx, Message::Text(json));
                            }
                        }
                    }
                } else {
                    // Timeout
                    let resp = super::ws_methods::WsResponse::error(
                        req_id,
                        &method_name,
                        "idle_timeout",
                        "Terminal did not become idle within the deadline.",
                    );
                    if let Ok(json) = serde_json::to_string(&resp) {
                        ws_send!(ws_tx, Message::Text(json));
                    }
                }
            }

            // Activity subscription fires (Running/Idle transitions)
            signal = async {
                match &mut activity_sub_rx {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match signal {
                    Some(ActivityStateChange::Idle { generation }) => {
                        // Emit Idle event with screen snapshot
                        if let Ok(Ok(crate::parser::state::QueryResponse::Screen(screen))) = tokio::time::timeout(
                            std::time::Duration::from_secs(10),
                            session.parser.query(crate::parser::state::Query::Screen { format: activity_sub_format }),
                        ).await {
                            let scrollback_lines = screen.total_lines;
                            let idle_event = crate::parser::events::Event::Idle {
                                seq: 0,
                                generation,
                                screen,
                                scrollback_lines,
                            };
                            if let Ok(json) = serde_json::to_string(&idle_event) {
                                ws_send!(ws_tx, Message::Text(json));
                            }
                        }
                    }
                    Some(ActivityStateChange::Running { generation }) => {
                        let running_event = crate::parser::events::Event::Running {
                            seq: 0,
                            generation,
                        };
                        if let Ok(json) = serde_json::to_string(&running_event) {
                            ws_send!(ws_tx, Message::Text(json));
                        }
                    }
                    None => {
                        // Channel closed, clear subscription
                        activity_sub_rx = None;
                    }
                }
            }

            // Ping keepalive
            _ = ping_interval.tick() => {
                if ping_sent && last_pong.elapsed() > PONG_TIMEOUT {
                    tracing::debug!("ws_json client unresponsive (no pong), closing");
                    break;
                }
                ws_send!(ws_tx, Message::Ping(vec![]));
                ping_sent = true;
            }

            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Pong(_))) => {
                        last_pong = tokio::time::Instant::now();
                        ping_sent = false;
                    }
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
                                    ws_send!(ws_tx, Message::Text(json));
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

                                    // Set up activity subscription if requested
                                    if let Some(handle) = activity_sub_handle.take() {
                                        handle.abort();
                                    }
                                    activity_sub_rx = None;

                                    if params.idle_timeout_ms > 0 {
                                        let timeout = std::time::Duration::from_millis(params.idle_timeout_ms);
                                        let activity = session.activity.clone();
                                        let (tx, rx) = tokio::sync::mpsc::channel(4);
                                        activity_sub_rx = Some(rx);
                                        activity_sub_format = sub_format;

                                        activity_sub_handle = Some(tokio::spawn(async move {
                                            let mut watch_rx = activity.subscribe();

                                            // If the session is currently active at subscribe
                                            // time, first wait for it to go idle. The initial
                                            // activity state code already sent Running to the
                                            // client, so we just need to detect the idle
                                            // transition. Without this, the main loop starts at
                                            // changed().await which never fires if no new
                                            // touches arrive after subscribe.
                                            //
                                            // We inline the idle wait using watch_rx rather than
                                            // calling wait_for_idle (which creates a separate
                                            // receiver) to ensure the same receiver tracks all
                                            // activity throughout the task's lifetime.
                                            if activity.last_activity_ms() < timeout.as_millis() as u64 {
                                                loop {
                                                    let last = *watch_rx.borrow_and_update();
                                                    let elapsed = last.elapsed();
                                                    if elapsed >= timeout {
                                                        break;
                                                    }
                                                    let remaining = timeout - elapsed;
                                                    tokio::select! {
                                                        _ = tokio::time::sleep(remaining) => {
                                                            let last = *watch_rx.borrow_and_update();
                                                            if last.elapsed() >= timeout {
                                                                break;
                                                            }
                                                        }
                                                        res = watch_rx.changed() => {
                                                            if res.is_err() {
                                                                return;
                                                            }
                                                        }
                                                    }
                                                }
                                                let gen = activity.generation();
                                                if tx.send(ActivityStateChange::Idle { generation: gen }).await.is_err() {
                                                    return;
                                                }
                                            }

                                            loop {
                                                // Wait for activity → emit Running
                                                if watch_rx.changed().await.is_err() {
                                                    break;
                                                }
                                                let gen = activity.generation();
                                                if tx.send(ActivityStateChange::Running { generation: gen }).await.is_err() {
                                                    break;
                                                }
                                                // Wait for idle → emit Idle
                                                let gen = activity.wait_for_idle(timeout, None).await;
                                                if tx.send(ActivityStateChange::Idle { generation: gen }).await.is_err() {
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
                                        ws_send!(ws_tx, Message::Text(json));
                                    }

                                    // Send sync event (with timeout to avoid blocking the loop)
                                    if let Ok(Ok(crate::parser::state::QueryResponse::Screen(screen))) = tokio::time::timeout(
                                        std::time::Duration::from_secs(10),
                                        session.parser.query(crate::parser::state::Query::Screen { format: sub_format }),
                                    ).await {
                                        let scrollback_lines = screen.total_lines;
                                        let sync_event = crate::parser::events::Event::Sync {
                                            seq: 0,
                                            screen,
                                            scrollback_lines,
                                        };
                                        if let Ok(json) = serde_json::to_string(&sync_event) {
                                            ws_send!(ws_tx, Message::Text(json));
                                        }
                                    }

                                    // Send initial activity state if activity subscription is active
                                    if params.idle_timeout_ms > 0 && subscribed_types.contains(&EventType::Activity) {
                                        let generation = session.activity.generation();
                                        let is_idle = session.activity.last_activity_ms() >= params.idle_timeout_ms;
                                        if is_idle {
                                            if let Ok(Ok(crate::parser::state::QueryResponse::Screen(screen))) = tokio::time::timeout(
                                                std::time::Duration::from_secs(10),
                                                session.parser.query(crate::parser::state::Query::Screen { format: sub_format }),
                                            ).await {
                                                let scrollback_lines = screen.total_lines;
                                                let idle_event = crate::parser::events::Event::Idle {
                                                    seq: 0,
                                                    generation,
                                                    screen,
                                                    scrollback_lines,
                                                };
                                                if let Ok(json) = serde_json::to_string(&idle_event) {
                                                    ws_send!(ws_tx, Message::Text(json));
                                                }
                                            }
                                        } else {
                                            let running_event = crate::parser::events::Event::Running {
                                                seq: 0,
                                                generation,
                                            };
                                            if let Ok(json) = serde_json::to_string(&running_event) {
                                                ws_send!(ws_tx, Message::Text(json));
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
                                        ws_send!(ws_tx, Message::Text(json));
                                    }
                                }
                            }
                        } else if req.method == "await_idle" || req.method == "await_quiesce" {
                            // Handle await_idle specially (async wait)
                            let params_value = req.params.clone().unwrap_or(serde_json::Value::Object(Default::default()));
                            match serde_json::from_value::<super::ws_methods::AwaitIdleParams>(params_value) {
                                Ok(params) => {
                                    let timeout = std::time::Duration::from_millis(params.timeout_ms.min(MAX_WAIT_CEILING_MS));
                                    let format = params.format;
                                    let activity = session.activity.clone();
                                    let last_generation = params.last_generation;
                                    let fresh = params.fresh;

                                    let deadline = std::time::Duration::from_millis(params.max_wait_ms.min(MAX_WAIT_CEILING_MS));
                                    let fut: std::pin::Pin<Box<dyn std::future::Future<Output = Option<u64>> + Send>> =
                                        Box::pin(async move {
                                            let inner = if fresh {
                                                futures::future::Either::Left(activity.wait_for_fresh_idle(timeout))
                                            } else {
                                                futures::future::Either::Right(activity.wait_for_idle(timeout, last_generation))
                                            };
                                            tokio::time::timeout(deadline, inner)
                                                .await
                                                .ok()
                                        });

                                    // If there's already a pending idle, cancel it with an error
                                    // so the client doesn't hang waiting for a response.
                                    if let Some((old_id, old_method, _, _)) = pending_idle.take() {
                                        let resp = super::ws_methods::WsResponse::error(
                                            old_id,
                                            &old_method,
                                            "idle_superseded",
                                            "A new await_idle request superseded this one.",
                                        );
                                        if let Ok(json) = serde_json::to_string(&resp) {
                                            ws_send!(ws_tx, Message::Text(json));
                                        }
                                    }
                                    pending_idle = Some((req.id.clone(), req.method.clone(), format, fut));
                                }
                                Err(e) => {
                                    let resp = super::ws_methods::WsResponse::error(
                                        req.id.clone(),
                                        &req.method,
                                        "invalid_request",
                                        &format!("Invalid await_idle params: {}.", e),
                                    );
                                    if let Ok(json) = serde_json::to_string(&resp) {
                                        ws_send!(ws_tx, Message::Text(json));
                                    }
                                }
                            }
                        } else {
                            // Dispatch all other methods
                            let resp = super::ws_methods::dispatch(&req, &session).await;

                            if let Ok(json) = serde_json::to_string(&resp) {
                                ws_send!(ws_tx, Message::Text(json));
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => continue,
                }
            }

            // Session was killed/removed
            _ = session.cancelled.cancelled() => {
                tracing::debug!("session was killed, closing WebSocket");
                break;
            }

            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::debug!("WebSocket handler received shutdown signal");
                    break;
                }
            }
        }
    }

    // Send close frame on any exit path (with timeout to avoid blocking on dead connections)
    let close_frame = CloseFrame {
        code: axum::extract::ws::close_code::NORMAL,
        reason: "session ended".into(),
    };
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        ws_tx.send(Message::Close(Some(close_frame))),
    ).await;

    // Clean up activity subscription task
    if let Some(handle) = activity_sub_handle {
        handle.abort();
    }
}

// ---------------------------------------------------------------------------
// Server-level multiplexed WebSocket
// ---------------------------------------------------------------------------

/// RAII guard that decrements the server-level WebSocket connection counter on
/// drop. Unlike the manual `fetch_sub` pattern, this ensures the counter is
/// correctly decremented even if the `on_upgrade` future is dropped without
/// executing (e.g. client disconnects before the upgrade completes).
struct ServerWsGuard(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl Drop for ServerWsGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::Release);
    }
}

pub(super) async fn ws_json_server(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    // Enforce server-level WS connection limit with a race-free CAS loop.
    loop {
        let current = state.server_ws_count.load(std::sync::atomic::Ordering::Acquire);
        if current >= super::MAX_SERVER_WS_CONNECTIONS {
            return Err(ApiError::ResourceLimitReached(
                "too many server-level WebSocket connections".into(),
            ));
        }
        if state
            .server_ws_count
            .compare_exchange(
                current,
                current + 1,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
        {
            break;
        }
    }
    let guard = ServerWsGuard(state.server_ws_count.clone());
    Ok(ws.max_message_size(MAX_WS_MESSAGE_SIZE)
        .on_upgrade(|socket| async move {
            handle_ws_json_server(socket, state).await;
            drop(guard); // explicitly drop after handler completes
        }))
}

/// A tagged session event forwarded through the internal mpsc channel.
struct TaggedSessionEvent {
    session: String,
    event: crate::parser::SubscriptionEvent,
}

/// Tracks a per-session subscription's forwarding task.
struct SubHandle {
    subscribed_types: Vec<EventType>,
    task: tokio::task::JoinHandle<()>,
    /// Optional background task that monitors activity and produces
    /// synthetic Idle/Running events via the shared mpsc channel.
    activity_task: Option<tokio::task::JoinHandle<()>>,
    _client_guard: Option<crate::session::ClientGuard>,
    /// Shared name that the forwarding task reads. Updated by
    /// `format_registry_event` on rename so the task tags events
    /// with the session's current name.
    shared_name: std::sync::Arc<parking_lot::Mutex<String>>,
    /// The idle_timeout_ms from the subscribe params (needed to send
    /// initial activity state after the sync event).
    idle_timeout_ms: u64,
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
        crate::session::SessionEvent::Renamed { old_name, new_name } => {
            if let Some(handle) = sub_handles.remove(old_name.as_str()) {
                // Update the shared name so the forwarding task tags
                // subsequent events with the new name.
                *handle.shared_name.lock() = new_name.clone();
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
                if let Some(at) = handle.activity_task {
                    at.abort();
                }
            }
            serde_json::json!({
                "event": "session_destroyed",
                "params": { "name": name }
            })
        }
        crate::session::SessionEvent::TagsChanged { name, added, removed } => {
            serde_json::json!({
                "event": "session_tags_changed",
                "params": { "name": name, "added": added, "removed": removed }
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
        crate::parser::events::Event::Idle { .. }
        | crate::parser::events::Event::Running { .. } => {
            handle.subscribed_types.contains(&EventType::Activity)
        }
    }
}

async fn handle_ws_json_server(socket: WebSocket, state: AppState) {
    let (_guard, mut shutdown_rx) = state.shutdown.register();
    if *shutdown_rx.borrow_and_update() {
        return;
    }
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

    // Ping/pong keepalive
    let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(30));
    ping_interval.reset();
    let mut last_pong = tokio::time::Instant::now();
    let mut ping_sent = false;
    const PONG_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

    // Timeout-wrapped send macro (same as handle_ws_json)
    macro_rules! ws_send {
        ($tx:expr, $msg:expr) => {
            match tokio::time::timeout(WS_SEND_TIMEOUT, $tx.send($msg)).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => break,
                Err(_) => {
                    tracing::debug!("server ws_json send timed out, closing");
                    break;
                }
            }
        };
    }

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
                                    ws_send!(ws_tx, Message::Text(json));
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
                                ws_send!(ws_tx, Message::Text(json));
                            }

                            // Send sync event + initial activity state after successful subscribe
                            if subscribe_ok {
                                if let Some(session_name) = &subscribe_session {
                                    if let Some(session) = state.sessions.get(session_name) {
                                        let format = {
                                            let params_value = req.params.clone().unwrap_or(serde_json::Value::Object(Default::default()));
                                            serde_json::from_value::<super::ws_methods::SubscribeParams>(params_value)
                                                .map(|p| p.format)
                                                .unwrap_or_default()
                                        };
                                        if let Ok(Ok(crate::parser::state::QueryResponse::Screen(screen))) = tokio::time::timeout(
                                            std::time::Duration::from_secs(10),
                                            session.parser.query(crate::parser::state::Query::Screen { format }),
                                        ).await {
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
                                                ws_send!(ws_tx, Message::Text(json));
                                            }
                                        }

                                        // Send initial activity state if activity subscription is active
                                        if let Some(handle) = sub_handles.get(session_name) {
                                            if handle.idle_timeout_ms > 0
                                                && handle.subscribed_types.contains(&EventType::Activity)
                                            {
                                                let generation = session.activity.generation();
                                                let is_idle = session.activity.last_activity_ms()
                                                    >= handle.idle_timeout_ms;
                                                let initial_event = if is_idle {
                                                    // Query screen for idle event
                                                    if let Ok(Ok(
                                                        crate::parser::state::QueryResponse::Screen(screen),
                                                    )) = tokio::time::timeout(
                                                        std::time::Duration::from_secs(10),
                                                        session.parser.query(
                                                            crate::parser::state::Query::Screen { format },
                                                        ),
                                                    )
                                                    .await
                                                    {
                                                        let scrollback_lines = screen.total_lines;
                                                        Some(crate::parser::events::Event::Idle {
                                                            seq: 0,
                                                            generation,
                                                            screen,
                                                            scrollback_lines,
                                                        })
                                                    } else {
                                                        None
                                                    }
                                                } else {
                                                    Some(crate::parser::events::Event::Running {
                                                        seq: 0,
                                                        generation,
                                                    })
                                                };
                                                if let Some(event) = initial_event {
                                                    if let Ok(event_value) = serde_json::to_value(&event) {
                                                        let tagged = if let serde_json::Value::Object(mut map) = event_value {
                                                            map.insert("session".to_string(), serde_json::json!(session_name));
                                                            serde_json::Value::Object(map)
                                                        } else {
                                                            event_value
                                                        };
                                                        if let Ok(json) = serde_json::to_string(&tagged) {
                                                            ws_send!(ws_tx, Message::Text(json));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {
                        last_pong = tokio::time::Instant::now();
                        ping_sent = false;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => continue,
                }
            }

            // Ping keepalive
            _ = ping_interval.tick() => {
                if ping_sent && last_pong.elapsed() > PONG_TIMEOUT {
                    tracing::debug!("server ws_json client unresponsive (no pong), closing");
                    break;
                }
                ws_send!(ws_tx, Message::Ping(vec![]));
                ping_sent = true;
            }

            // Registry lifecycle events
            result = registry_rx.recv() => {
                match result {
                    Ok(event) => {
                        let event_json = format_registry_event(&event, &mut sub_handles);
                        if let Ok(json) = serde_json::to_string(&event_json) {
                            ws_send!(ws_tx, Message::Text(json));
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "server WS client lagged on registry events");
                        continue;
                    }
                }
            }

            // Per-session parser events forwarded from subscription tasks
            Some(tagged) = sub_rx.recv() => {
                match tagged.event {
                    crate::parser::SubscriptionEvent::Event(ref event) => {
                        if let Some(handle) = sub_handles.get(&tagged.session) {
                            if should_forward_session_event(event, handle) {
                                if let Ok(event_value) = serde_json::to_value(event) {
                                    let tagged_json = if let serde_json::Value::Object(mut map) = event_value {
                                        map.insert("session".to_string(), serde_json::json!(tagged.session));
                                        serde_json::Value::Object(map)
                                    } else {
                                        event_value
                                    };
                                    if let Ok(json) = serde_json::to_string(&tagged_json) {
                                        ws_send!(ws_tx, Message::Text(json));
                                    }
                                }
                            }
                        }
                    }
                    crate::parser::SubscriptionEvent::Lagged(n) => {
                        tracing::warn!(session = %tagged.session, skipped = n, "parser event subscriber lagged");
                        let lag_msg = serde_json::json!({
                            "type": "lagged",
                            "session": tagged.session,
                            "skipped": n,
                        });
                        if let Ok(json) = serde_json::to_string(&lag_msg) {
                            ws_send!(ws_tx, Message::Text(json));
                        }
                        // After lag, push a full sync so the client can recover,
                        // matching the per-session ws_json behavior.
                        if let Some(session) = state.sessions.get(&tagged.session) {
                            if let Ok(Ok(crate::parser::state::QueryResponse::Screen(screen))) = tokio::time::timeout(
                                std::time::Duration::from_secs(10),
                                session.parser.query(crate::parser::state::Query::Screen {
                                    format: crate::parser::state::Format::default(),
                                }),
                            ).await {
                                let scrollback_lines = screen.total_lines;
                                let sync_event = crate::parser::events::Event::Sync {
                                    seq: 0,
                                    screen,
                                    scrollback_lines,
                                };
                                if let Ok(event_value) = serde_json::to_value(&sync_event) {
                                    let tagged_json = if let serde_json::Value::Object(mut map) = event_value {
                                        map.insert("session".to_string(), serde_json::json!(tagged.session));
                                        serde_json::Value::Object(map)
                                    } else {
                                        event_value
                                    };
                                    if let Ok(json) = serde_json::to_string(&tagged_json) {
                                        ws_send!(ws_tx, Message::Text(json));
                                    }
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
                    break;
                }
            }
        }
    }

    // Send close frame on any exit path (with timeout to avoid blocking on dead connections)
    let close_frame = CloseFrame {
        code: axum::extract::ws::close_code::NORMAL,
        reason: "session ended".into(),
    };
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        ws_tx.send(Message::Close(Some(close_frame))),
    ).await;

    // Clean up all subscription tasks
    for (_, handle) in sub_handles {
        handle.task.abort();
        if let Some(at) = handle.activity_task {
            at.abort();
        }
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
                #[serde(default)]
                tags: Vec<String>,
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
                    tags: vec![],
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

            let rows = params.rows.unwrap_or(24).max(1);
            let cols = params.cols.unwrap_or(80).max(1);

            // Advisory pre-check — see name_available() doc for TOCTOU rationale.
            // The authoritative check is insert_and_get() below.
            if let Err(e) = state.sessions.name_available(&params.name) {
                return Some(match e {
                    RegistryError::NameExists(n) => super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "session_name_conflict",
                        &format!("Session name already exists: {}.", n),
                    ),
                    RegistryError::NotFound(n) => super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "session_not_found",
                        &format!("Session not found: {}.", n),
                    ),
                    RegistryError::MaxSessionsReached => super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "max_sessions_reached",
                        "Maximum number of sessions reached.",
                    ),
                    RegistryError::InvalidTag(msg) => super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "invalid_tag",
                        &format!("Invalid tag: {}.", msg),
                    ),
                });
            }

            // Save tags before spawn_blocking consumes params
            let initial_tags = params.tags;

            // Validate tags early
            if !initial_tags.is_empty() {
                for tag in &initial_tags {
                    if let Err(e) = crate::session::validate_tag(tag) {
                        return Some(super::ws_methods::WsResponse::error(
                            id,
                            method,
                            "invalid_tag",
                            &format!("Invalid tag: {}.", e),
                        ));
                    }
                }
            }

            let param_name = params.name;
            let cwd = params.cwd;
            let env = params.env;
            let spawn_result = tokio::task::spawn_blocking(move || {
                Session::spawn_with_options("".to_string(), command, rows, cols, cwd, env)
            }).await;
            let (session, child_exit_rx) = match spawn_result {
                Ok(Ok(result)) => result,
                Ok(Err(e)) => {
                    return Some(super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "session_create_failed",
                        &format!("Failed to create session: {}.", e),
                    ));
                }
                Err(e) => {
                    return Some(super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "session_create_failed",
                        &format!("Spawn task failed: {}.", e),
                    ));
                }
            };

            // Set initial tags before registry insertion
            if !initial_tags.is_empty() {
                *session.tags.write() = initial_tags.into_iter().collect();
            }

            match state.sessions.insert_and_get(param_name, session.clone()) {
                Ok((assigned_name, _session)) => {
                    // Monitor child exit so the session is auto-removed.
                    state.sessions.monitor_child_exit(assigned_name.clone(), session.client_count.clone(), session.child_exited.clone(), child_exit_rx);
                    let mut tags: Vec<String> = session.tags.read().iter().cloned().collect();
                    tags.sort();
                    return Some(super::ws_methods::WsResponse::success(
                        id,
                        method,
                        serde_json::json!({ "name": assigned_name, "tags": tags }),
                    ));
                }
                Err(e) => {
                    session.shutdown();
                    return Some(match e {
                        RegistryError::NameExists(n) => super::ws_methods::WsResponse::error(
                            id,
                            method,
                            "session_name_conflict",
                            &format!("Session name already exists: {}.", n),
                        ),
                        RegistryError::NotFound(n) => super::ws_methods::WsResponse::error(
                            id,
                            method,
                            "session_not_found",
                            &format!("Session not found: {}.", n),
                        ),
                        RegistryError::MaxSessionsReached => super::ws_methods::WsResponse::error(
                            id,
                            method,
                            "max_sessions_reached",
                            "Maximum number of sessions reached.",
                        ),
                        RegistryError::InvalidTag(msg) => super::ws_methods::WsResponse::error(
                            id,
                            method,
                            "invalid_tag",
                            &format!("Invalid tag: {}.", msg),
                        ),
                    });
                }
            }
        }

        "list_sessions" => {
            #[derive(Deserialize)]
            struct ListParams {
                #[serde(default)]
                tag: Vec<String>,
            }
            let params: ListParams = match &req.params {
                Some(v) => serde_json::from_value(v.clone()).unwrap_or(ListParams { tag: vec![] }),
                None => ListParams { tag: vec![] },
            };

            let names = if params.tag.is_empty() {
                state.sessions.list()
            } else {
                state.sessions.sessions_by_tags(&params.tag)
            };
            let sessions: Vec<serde_json::Value> = names
                .into_iter()
                .filter_map(|name| {
                    let session = state.sessions.get(&name)?;
                    let (rows, cols) = session.terminal_size.get();
                    let mut tags: Vec<String> = session.tags.read().iter().cloned().collect();
                    tags.sort();
                    Some(serde_json::json!({
                        "name": name,
                        "pid": session.pid,
                        "command": session.command,
                        "rows": rows,
                        "cols": cols,
                        "clients": session.clients(),
                        "tags": tags,
                    }))
                })
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
                Some(session) => {
                    session.force_kill();
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
                Ok(session) => {
                    // Update subscription key and shared name if it exists
                    if let Some(handle) = sub_handles.remove(&params.name) {
                        *handle.shared_name.lock() = params.new_name.clone();
                        sub_handles.insert(params.new_name.clone(), handle);
                    }
                    let mut tags: Vec<String> = session.tags.read().iter().cloned().collect();
                    tags.sort();
                    return Some(super::ws_methods::WsResponse::success(
                        id,
                        method,
                        serde_json::json!({ "name": params.new_name, "tags": tags }),
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
                Err(RegistryError::MaxSessionsReached) => {
                    return Some(super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "max_sessions_reached",
                        "Maximum number of sessions reached.",
                    ));
                }
                Err(RegistryError::InvalidTag(msg)) => {
                    return Some(super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "invalid_tag",
                        &format!("Invalid tag: {}.", msg),
                    ));
                }
            }
        }

        "update_tags" => {
            #[derive(Deserialize)]
            struct UpdateTagsParams {
                session: String,
                #[serde(default)]
                add: Vec<String>,
                #[serde(default)]
                remove: Vec<String>,
            }
            let params: UpdateTagsParams = match &req.params {
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
                        "Missing params.",
                    ));
                }
            };

            if !params.add.is_empty() {
                if let Err(e) = state.sessions.add_tags(&params.session, &params.add) {
                    return Some(super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "invalid_request",
                        &format!("Failed to add tags: {}.", e),
                    ));
                }
            }

            if !params.remove.is_empty() {
                if let Err(e) = state.sessions.remove_tags(&params.session, &params.remove) {
                    return Some(super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "invalid_request",
                        &format!("Failed to remove tags: {}.", e),
                    ));
                }
            }

            let session = match state.sessions.get(&params.session) {
                Some(s) => s,
                None => {
                    return Some(super::ws_methods::WsResponse::error(
                        id,
                        method,
                        "session_not_found",
                        &format!("Session not found: {}.", params.session),
                    ));
                }
            };
            let (rows, cols) = session.terminal_size.get();
            let mut tags: Vec<String> = session.tags.read().iter().cloned().collect();
            tags.sort();
            return Some(super::ws_methods::WsResponse::success(
                id,
                method,
                serde_json::json!({
                    "name": session.name,
                    "pid": session.pid,
                    "command": session.command,
                    "rows": rows,
                    "cols": cols,
                    "clients": session.clients(),
                    "tags": tags,
                }),
            ));
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
                    if let Some(at) = old.activity_task {
                        at.abort();
                    }
                }

                // Spawn a task that reads from the parser event stream and
                // forwards into the shared mpsc channel. The session's
                // cancellation token ensures this exits promptly when the
                // session is killed, rather than waiting for all Parser
                // clones to drop.
                //
                // The task reads the session's current name from shared_name
                // (an Arc<Mutex>) rather than capturing a name clone. This
                // ensures events are tagged with the correct name even if the
                // session is renamed by another client while the subscription
                // is active. format_registry_event updates shared_name on
                // rename events.
                let mut events = Box::pin(session.parser.subscribe());
                let tx = sub_tx.clone();
                let shared_name = std::sync::Arc::new(parking_lot::Mutex::new(session_name.clone()));
                let task_name = shared_name.clone();
                let cancelled = session.cancelled.clone();
                let task = tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            event = events.next() => {
                                match event {
                                    Some(e) => {
                                        let current_name = task_name.lock().clone();
                                        if tx
                                            .send(TaggedSessionEvent {
                                                session: current_name,
                                                event: e,
                                            })
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    None => break,
                                }
                            }
                            _ = cancelled.cancelled() => break,
                        }
                    }
                });

                // Spawn activity monitoring task if idle_timeout_ms > 0
                let activity_task = if params.idle_timeout_ms > 0 {
                    let timeout = std::time::Duration::from_millis(params.idle_timeout_ms);
                    let activity = session.activity.clone();
                    let activity_tx = sub_tx.clone();
                    let activity_name = shared_name.clone();
                    let activity_parser = session.parser.clone();
                    let activity_format = params.format;
                    Some(tokio::spawn(async move {
                        let mut watch_rx = activity.subscribe();

                        // If the session is currently active at subscribe
                        // time, first wait for it to go idle using the same
                        // watch_rx. This ensures we don't miss activity that
                        // arrives during the wait.
                        if activity.last_activity_ms() < timeout.as_millis() as u64 {
                            loop {
                                let last = *watch_rx.borrow_and_update();
                                let elapsed = last.elapsed();
                                if elapsed >= timeout {
                                    break;
                                }
                                let remaining = timeout - elapsed;
                                tokio::select! {
                                    _ = tokio::time::sleep(remaining) => {
                                        let last = *watch_rx.borrow_and_update();
                                        if last.elapsed() >= timeout {
                                            break;
                                        }
                                    }
                                    res = watch_rx.changed() => {
                                        if res.is_err() {
                                            return;
                                        }
                                    }
                                }
                            }
                            let gen = activity.generation();
                            let idle_event = if let Ok(Ok(
                                crate::parser::state::QueryResponse::Screen(screen),
                            )) = tokio::time::timeout(
                                std::time::Duration::from_secs(10),
                                activity_parser.query(
                                    crate::parser::state::Query::Screen { format: activity_format },
                                ),
                            )
                            .await
                            {
                                let scrollback_lines = screen.total_lines;
                                crate::parser::events::Event::Idle {
                                    seq: 0,
                                    generation: gen,
                                    screen,
                                    scrollback_lines,
                                }
                            } else {
                                crate::parser::events::Event::Running {
                                    seq: 0,
                                    generation: gen,
                                }
                            };
                            let current_name = activity_name.lock().clone();
                            if activity_tx
                                .send(TaggedSessionEvent {
                                    session: current_name,
                                    event: crate::parser::SubscriptionEvent::Event(idle_event),
                                })
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }

                        loop {
                            // Wait for activity → emit Running
                            if watch_rx.changed().await.is_err() {
                                break;
                            }
                            let gen = activity.generation();
                            let current_name = activity_name.lock().clone();
                            if activity_tx
                                .send(TaggedSessionEvent {
                                    session: current_name,
                                    event: crate::parser::SubscriptionEvent::Event(
                                        crate::parser::events::Event::Running {
                                            seq: 0,
                                            generation: gen,
                                        },
                                    ),
                                })
                                .await
                                .is_err()
                            {
                                break;
                            }
                            // Wait for idle → emit Idle with screen snapshot
                            let gen = activity.wait_for_idle(timeout, None).await;
                            let idle_event = if let Ok(Ok(
                                crate::parser::state::QueryResponse::Screen(screen),
                            )) = tokio::time::timeout(
                                std::time::Duration::from_secs(10),
                                activity_parser.query(
                                    crate::parser::state::Query::Screen { format: activity_format },
                                ),
                            )
                            .await
                            {
                                let scrollback_lines = screen.total_lines;
                                crate::parser::events::Event::Idle {
                                    seq: 0,
                                    generation: gen,
                                    screen,
                                    scrollback_lines,
                                }
                            } else {
                                // Fallback: emit Running generation if screen
                                // query fails (shouldn't happen in practice)
                                continue;
                            };
                            let current_name = activity_name.lock().clone();
                            if activity_tx
                                .send(TaggedSessionEvent {
                                    session: current_name,
                                    event: crate::parser::SubscriptionEvent::Event(idle_event),
                                })
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }))
                } else {
                    None
                };

                sub_handles.insert(
                    session_name.clone(),
                    SubHandle {
                        subscribed_types: subscribed_types.clone(),
                        task,
                        activity_task,
                        _client_guard: session.connect(),
                        shared_name,
                        idle_timeout_ms: params.idle_timeout_ms,
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

// Idle query parameters
#[derive(Deserialize)]
pub(super) struct IdleQuery {
    timeout_ms: u64,
    #[serde(default)]
    format: Format,
    #[serde(default = "default_max_wait")]
    max_wait_ms: u64,
    /// Generation from a previous idle response. If provided and matches
    /// the current generation, the server waits for new activity before
    /// checking idle state — preventing a busy-loop storm.
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

pub(super) async fn idle(
    State(state): State<AppState>,
    Path(name): Path<String>,
    axum::extract::Query(params): axum::extract::Query<IdleQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let timeout = std::time::Duration::from_millis(params.timeout_ms.min(MAX_WAIT_CEILING_MS));
    let deadline = std::time::Duration::from_millis(params.max_wait_ms.min(MAX_WAIT_CEILING_MS));

    let activity = &session.activity;
    let idle_fut = if params.fresh {
        futures::future::Either::Left(activity.wait_for_fresh_idle(timeout))
    } else {
        futures::future::Either::Right(
            activity.wait_for_idle(timeout, params.last_generation),
        )
    };

    match tokio::time::timeout(deadline, idle_fut).await {
        Ok(generation) => {
            // Idle — query screen state
            let response = tokio::time::timeout(
                PARSER_QUERY_TIMEOUT,
                session.parser.query(Query::Screen { format: params.format }),
            )
            .await
            .map_err(|_| ApiError::ParserTimeout)?
            .map_err(|_| ApiError::ParserUnavailable)?;

            match response {
                crate::parser::state::QueryResponse::Screen(screen) => {
                    let scrollback_lines = screen.total_lines;
                    let last_activity_ms = session.activity.last_activity_ms();
                    Ok(Json(serde_json::json!({
                        "screen": screen,
                        "scrollback_lines": scrollback_lines,
                        "generation": generation,
                        "last_activity_ms": last_activity_ms,
                    })))
                }
                _ => Err(ApiError::ParserUnavailable),
            }
        }
        Err(_) => {
            // Deadline exceeded
            Err(ApiError::IdleTimeout)
        }
    }
}

// Server-level idle query parameters (any session)
#[derive(Deserialize)]
pub(super) struct IdleAnyQuery {
    timeout_ms: u64,
    #[serde(default)]
    format: Format,
    #[serde(default = "default_max_wait")]
    max_wait_ms: u64,
    /// Generation from a previous idle response, paired with `last_session`.
    /// When both are provided, the named session waits for new activity before
    /// checking idle state (preventing busy-loop storms). Other sessions are
    /// checked immediately.
    last_generation: Option<u64>,
    /// The session name from a previous idle response.
    /// Used together with `last_generation`.
    last_session: Option<String>,
    /// When true, always observe at least `timeout_ms` of real silence before
    /// responding, even if a session is already idle.
    #[serde(default)]
    fresh: bool,
    /// Comma-separated list of tags (e.g. `?tag=build,test`).
    /// When provided, only sessions matching all tags are considered.
    #[serde(default)]
    tag: Option<String>,
}

pub(super) async fn idle_any(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<IdleAnyQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let tags: Vec<String> = params
        .tag
        .as_deref()
        .map(|t| t.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let names = if tags.is_empty() {
        state.sessions.list()
    } else {
        state.sessions.sessions_by_tags(&tags)
    };
    if names.is_empty() {
        return Err(ApiError::NoSessions);
    }

    let timeout = std::time::Duration::from_millis(params.timeout_ms.min(MAX_WAIT_CEILING_MS));
    let deadline = std::time::Duration::from_millis(params.max_wait_ms.min(MAX_WAIT_CEILING_MS));

    // Build an idle future for each session, racing them all.
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
                activity.wait_for_fresh_idle(timeout).await
            } else {
                activity.wait_for_idle(timeout, last_seen).await
            };
            (session_name, generation)
        };
        futs.push(fut);
    }

    // Race all futures under the overall deadline.
    let race = async {
        // select_all requires pinned futures
        let pinned: Vec<_> = futs.into_iter().map(Box::pin).collect();
        let (result, _index, _remaining) = futures::future::select_all(pinned).await;
        result
    };

    match tokio::time::timeout(deadline, race).await {
        Ok((session_name, generation)) => {
            let session = get_session(&state.sessions, &session_name)?;
            let response = tokio::time::timeout(
                PARSER_QUERY_TIMEOUT,
                session.parser.query(Query::Screen { format: params.format }),
            )
            .await
            .map_err(|_| ApiError::ParserTimeout)?
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
        Err(_) => Err(ApiError::IdleTimeout),
    }
}

#[derive(Deserialize)]
pub(super) struct ScreenQuery {
    #[serde(default)]
    format: Format,
}

#[derive(Serialize)]
struct EnrichedScreen {
    #[serde(flatten)]
    screen: crate::parser::state::QueryResponse,
    last_activity_ms: u64,
}

pub(super) async fn screen(
    State(state): State<AppState>,
    Path(name): Path<String>,
    axum::extract::Query(params): axum::extract::Query<ScreenQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let response = tokio::time::timeout(
        PARSER_QUERY_TIMEOUT,
        session.parser.query(Query::Screen { format: params.format }),
    )
    .await
    .map_err(|_| ApiError::ParserTimeout)?
    .map_err(|_| ApiError::ParserUnavailable)?;

    let last_activity_ms = session.activity.last_activity_ms();
    Ok(Json(EnrichedScreen {
        screen: response,
        last_activity_ms,
    }))
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
    let limit = params.limit.min(10_000);
    let response = tokio::time::timeout(
        PARSER_QUERY_TIMEOUT,
        session.parser.query(Query::Scrollback {
            format: params.format,
            offset: params.offset,
            limit,
        }),
    )
    .await
    .map_err(|_| ApiError::ParserTimeout)?
    .map_err(|_| ApiError::ParserUnavailable)?;

    Ok(Json(response))
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
    #[serde(default)]
    background: Option<BackgroundStyle>,
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
    let id = session.overlays.create(req.x, req.y, req.z, req.width, req.height, req.background, req.spans, req.focusable, current_mode)
        .map_err(|e| ApiError::ResourceLimitReached(e.to_string()))?;
    let _ = session.visual_update_tx.send(crate::protocol::VisualUpdate::OverlaysChanged);
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
    if session.overlays.update(&id, req.spans).map_err(|e| ApiError::InvalidOverlay(e.into()))? {
        let _ = session.visual_update_tx.send(crate::protocol::VisualUpdate::OverlaysChanged);
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
    if session.overlays.move_to(&id, req.x, req.y, req.z, req.width, req.height, req.background) {
        let _ = session.visual_update_tx.send(crate::protocol::VisualUpdate::OverlaysChanged);
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
    if session.overlays.delete(&id) {
        session.focus.clear_if_focused(&id);
        let _ = session.visual_update_tx.send(crate::protocol::VisualUpdate::OverlaysChanged);
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
    session.overlays.clear();
    session.focus.unfocus();
    let _ = session.visual_update_tx.send(crate::protocol::VisualUpdate::OverlaysChanged);
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn overlay_update_spans(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
    Json(req): Json<UpdateSpansRequest>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    if session.overlays.update_spans(&id, &req.spans).map_err(|e| ApiError::InvalidOverlay(e.into()))? {
        let _ = session.visual_update_tx.send(crate::protocol::VisualUpdate::OverlaysChanged);
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
    if session.overlays.region_write(&id, req.writes).map_err(|e| ApiError::InvalidOverlay(e.into()))? {
        let _ = session.visual_update_tx.send(crate::protocol::VisualUpdate::OverlaysChanged);
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
        .create(req.position, req.height, req.z, req.background, req.spans, req.focusable, current_mode)
        .map_err(|e| ApiError::ResourceLimitReached(e.to_string()))?;
    panel::reconfigure_layout(&session.panels, &session.terminal_size, &session.pty, &session.parser)
        .await;
    let _ = session.visual_update_tx.send(crate::protocol::VisualUpdate::PanelsChanged);
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
        .map_err(|e| ApiError::InvalidOverlay(e.into()))?
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

    let _ = session.visual_update_tx.send(crate::protocol::VisualUpdate::PanelsChanged);
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
        .map_err(|e| ApiError::InvalidOverlay(e.into()))?
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

    let _ = session.visual_update_tx.send(crate::protocol::VisualUpdate::PanelsChanged);
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
    let _ = session.visual_update_tx.send(crate::protocol::VisualUpdate::PanelsChanged);
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
    let _ = session.visual_update_tx.send(crate::protocol::VisualUpdate::PanelsChanged);
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn panel_update_spans(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
    Json(req): Json<UpdateSpansRequest>,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    if session.panels.update_spans(&id, &req.spans).map_err(|e| ApiError::InvalidOverlay(e.into()))? {
        panel::flush_panel_content(&session.panels, &id, &session.terminal_size);
        let _ = session.visual_update_tx.send(crate::protocol::VisualUpdate::PanelsChanged);
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
    if session.panels.region_write(&id, req.writes).map_err(|e| ApiError::InvalidOverlay(e.into()))? {
        panel::flush_panel_content(&session.panels, &id, &session.terminal_size);
        let _ = session.visual_update_tx.send(crate::protocol::VisualUpdate::PanelsChanged);
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
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Serialize)]
pub(super) struct SessionInfo {
    pub name: String,
    pub pid: Option<u32>,
    pub command: String,
    pub rows: u16,
    pub cols: u16,
    pub clients: usize,
    pub tags: Vec<String>,
    pub last_activity_ms: u64,
}

fn build_session_info(session: &crate::session::Session) -> SessionInfo {
    let (rows, cols) = session.terminal_size.get();
    let mut tags: Vec<String> = session.tags.read().iter().cloned().collect();
    tags.sort();
    SessionInfo {
        name: session.name.clone(),
        pid: session.pid,
        command: session.command.clone(),
        rows,
        cols,
        clients: session.clients(),
        tags,
        last_activity_ms: session.activity.last_activity_ms(),
    }
}

#[derive(Deserialize)]
pub(super) struct UpdateSessionRequest {
    /// New name for the session (optional, for rename)
    pub name: Option<String>,
    /// Tags to add (optional)
    #[serde(default)]
    pub add_tags: Vec<String>,
    /// Tags to remove (optional)
    #[serde(default)]
    pub remove_tags: Vec<String>,
}

#[derive(Deserialize)]
pub(super) struct ListSessionsQuery {
    /// Comma-separated list of tags (e.g. `?tag=build,test`).
    #[serde(default)]
    pub tag: Option<String>,
}

// ── Session management handlers ──────────────────────────────────

pub(super) async fn session_list(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<ListSessionsQuery>,
) -> Json<Vec<SessionInfo>> {
    let tags: Vec<String> = params
        .tag
        .map(|t| t.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let names = if tags.is_empty() {
        state.sessions.list()
    } else {
        state.sessions.sessions_by_tags(&tags)
    };
    let infos = names
        .into_iter()
        .filter_map(|name| {
            let session = state.sessions.get(&name)?;
            Some(build_session_info(&session))
        })
        .collect();
    Json(infos)
}

pub(super) async fn session_create(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<SessionInfo>), ApiError> {
    let req_name = req.name;
    let req_tags = req.tags;
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

    let rows = req.rows.unwrap_or(24).max(1);
    let cols = req.cols.unwrap_or(80).max(1);

    // Advisory pre-check — see name_available() doc for TOCTOU rationale.
    // The authoritative check is insert_and_get() below.
    state.sessions.name_available(&req_name).map_err(|e| match e {
        RegistryError::NameExists(n) => ApiError::SessionNameConflict(n),
        RegistryError::NotFound(n) => ApiError::SessionNotFound(n),
        RegistryError::MaxSessionsReached => ApiError::MaxSessionsReached,
        RegistryError::InvalidTag(msg) => ApiError::InvalidTag(msg),
    })?;

    // Use a placeholder name for spawn; registry.insert will assign the real name.
    //
    // spawn_with_options calls fork()/exec() which is a blocking syscall.
    // Under load, fork() on a large-RSS process can take hundreds of ms,
    // so we run it on the blocking thread pool to avoid stalling the
    // async executor.
    let cwd = req.cwd;
    let env = req.env;
    let (session, child_exit_rx) = tokio::task::spawn_blocking(move || {
        Session::spawn_with_options("".to_string(), command, rows, cols, cwd, env)
    })
    .await
    .map_err(|e| ApiError::SessionCreateFailed(e.to_string()))?
    .map_err(|e| ApiError::SessionCreateFailed(e.to_string()))?;

    // Validate and set initial tags before inserting into registry,
    // so that insert_and_get() properly indexes them.
    if !req_tags.is_empty() {
        for tag in &req_tags {
            crate::session::validate_tag(tag).map_err(ApiError::InvalidTag)?;
        }
        *session.tags.write() = req_tags.into_iter().collect();
    }

    let (assigned_name, session) = match state.sessions.insert_and_get(req_name, session.clone()) {
        Ok(result) => result,
        Err(e) => {
            session.shutdown();
            return Err(match e {
                RegistryError::NameExists(n) => ApiError::SessionNameConflict(n),
                RegistryError::NotFound(n) => ApiError::SessionNotFound(n),
                RegistryError::MaxSessionsReached => ApiError::MaxSessionsReached,
                RegistryError::InvalidTag(msg) => ApiError::InvalidTag(msg),
            });
        }
    };

    // Monitor child exit so the session is auto-removed when the process dies.
    state.sessions.monitor_child_exit(assigned_name.clone(), session.client_count.clone(), session.child_exited.clone(), child_exit_rx);

    Ok((
        StatusCode::CREATED,
        Json(build_session_info(&session)),
    ))
}

pub(super) async fn session_get(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<SessionInfo>, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    Ok(Json(build_session_info(&session)))
}

pub(super) async fn session_update(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<UpdateSessionRequest>,
) -> Result<Json<SessionInfo>, ApiError> {
    // Handle rename if requested
    let current_name = if let Some(new_name) = req.name {
        state.sessions.rename(&name, &new_name).map_err(|e| match e {
            RegistryError::NameExists(n) => ApiError::SessionNameConflict(n),
            RegistryError::NotFound(n) => ApiError::SessionNotFound(n),
            RegistryError::MaxSessionsReached => ApiError::MaxSessionsReached,
            RegistryError::InvalidTag(e) => ApiError::InvalidTag(e),
        })?;
        new_name
    } else {
        name.clone()
    };

    // Handle tag additions
    if !req.add_tags.is_empty() {
        state.sessions.add_tags(&current_name, &req.add_tags).map_err(|e| match e {
            RegistryError::NotFound(n) => ApiError::SessionNotFound(n),
            RegistryError::InvalidTag(e) => ApiError::InvalidTag(e),
            _ => ApiError::SessionNotFound(current_name.clone()),
        })?;
    }

    // Handle tag removals
    if !req.remove_tags.is_empty() {
        state.sessions.remove_tags(&current_name, &req.remove_tags).map_err(|e| match e {
            RegistryError::NotFound(n) => ApiError::SessionNotFound(n),
            _ => ApiError::SessionNotFound(current_name.clone()),
        })?;
    }

    let session = get_session(&state.sessions, &current_name)?;
    Ok(Json(build_session_info(&session)))
}

pub(super) async fn session_kill(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let session = state
        .sessions
        .remove(&name)
        .ok_or(ApiError::SessionNotFound(name))?;
    session.force_kill();
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn session_detach(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let session = state
        .sessions
        .get(&name)
        .ok_or(ApiError::SessionNotFound(name))?;
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
    let _ = session.visual_update_tx.send(crate::protocol::VisualUpdate::OverlaysChanged);
    let _ = session.visual_update_tx.send(crate::protocol::VisualUpdate::PanelsChanged);
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn server_persist_get(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let persistent = state.server_config.is_persistent();
    (StatusCode::OK, Json(serde_json::json!({"persistent": persistent})))
}

pub(super) async fn server_persist_set(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Some(persistent) = body.get("persistent").and_then(|v| v.as_bool()) {
        state.server_config.set_persistent(persistent);
        (StatusCode::OK, Json(serde_json::json!({"persistent": persistent})))
    } else {
        (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "missing or invalid 'persistent' boolean field"})))
    }
}
