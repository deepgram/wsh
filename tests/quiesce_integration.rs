//! Integration tests for the quiescence sync feature.
//!
//! Tests cover:
//! - HTTP GET /quiesce endpoint
//! - WebSocket await_quiesce method
//! - Subscription quiesce_ms parameter
//! - Activity tracking from input sources

use bytes::Bytes;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use wsh::{
    activity::ActivityTracker,
    api,
    broker::Broker,
    input::{FocusTracker, InputBroadcaster, InputMode},
    overlay::OverlayStore,
    parser::Parser,
    session::{Session, SessionRegistry},
    shutdown::ShutdownCoordinator,
};

fn create_test_state() -> (api::AppState, mpsc::Receiver<Bytes>, ActivityTracker, mpsc::Sender<Bytes>) {
    let (input_tx, input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let (parser_tx, parser_rx) = mpsc::channel(256);
    let parser = Parser::spawn(parser_rx, 80, 24, 1000);
    let activity = ActivityTracker::new();
    let session = Session {
        name: "test".to_string(),
        pid: None,
        command: "test".to_string(),
        client_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        tags: Arc::new(parking_lot::RwLock::new(std::collections::HashSet::new())),
        child_exited: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        input_tx,
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: Arc::new(parking_lot::Mutex::new(
            wsh::pty::Pty::spawn(24, 80, wsh::pty::SpawnCommand::default())
                .expect("failed to spawn PTY for test"),
        )),
        terminal_size: wsh::terminal::TerminalSize::new(24, 80),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        activity: activity.clone(),
        focus: FocusTracker::new(),
        detach_signal: tokio::sync::broadcast::channel::<()>(1).0,
        visual_update_tx: tokio::sync::broadcast::channel::<wsh::protocol::VisualUpdate>(16).0,
        screen_mode: std::sync::Arc::new(parking_lot::RwLock::new(wsh::overlay::ScreenMode::Normal)),
        cancelled: tokio_util::sync::CancellationToken::new(),
    };
    let registry = SessionRegistry::new();
    registry.insert(Some("test".into()), session).unwrap();
    let state = api::AppState {
        sessions: registry,
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(api::ServerConfig::new(false)),
            server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    (state, input_rx, activity, parser_tx)
}

async fn start_server(app: axum::Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

async fn http_get(addr: SocketAddr, uri: &str) -> (u16, serde_json::Value) {
    let stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = hyper::Request::builder()
        .method("GET")
        .uri(uri)
        .body(http_body_util::Full::new(Bytes::new()))
        .unwrap();

    let resp = sender.send_request(req).await.expect("request");
    let status = resp.status().as_u16();
    let body = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::json!(null));
    (status, json)
}

// ---------------------------------------------------------------------------
// HTTP /quiesce tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_http_quiesce_returns_screen_state_after_quiet() {
    let (state, _rx, _activity, _parser_tx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    // Terminal should already be quiet (no activity for >100ms given setup time)
    let (status, json) = http_get(addr, "/sessions/test/quiesce?timeout_ms=100&format=plain").await;

    assert_eq!(status, 200);
    assert!(json.get("screen").is_some(), "response should have screen field");
    assert!(
        json.get("scrollback_lines").is_some(),
        "response should have scrollback_lines field"
    );
    let screen = &json["screen"];
    assert!(screen.get("cols").is_some());
    assert!(screen.get("rows").is_some());
    assert!(screen.get("lines").is_some());
    assert!(screen.get("cursor").is_some());
}

#[tokio::test]
async fn test_http_quiesce_returns_408_when_deadline_exceeded() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    // Keep touching to prevent quiescence
    let a = activity.clone();
    let touch_handle = tokio::spawn(async move {
        loop {
            a.touch();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    let (status, json) =
        http_get(addr, "/sessions/test/quiesce?timeout_ms=500&max_wait_ms=200&format=plain").await;

    touch_handle.abort();

    assert_eq!(status, 408);
    assert_eq!(json["error"]["code"], "quiesce_timeout");
}

#[tokio::test]
async fn test_http_quiesce_returns_immediately_when_already_quiescent() {
    let (state, _rx, _activity, _parser_tx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    // Wait for well past the timeout
    tokio::time::sleep(Duration::from_millis(200)).await;

    let start = std::time::Instant::now();
    let (status, _json) = http_get(addr, "/sessions/test/quiesce?timeout_ms=100&format=plain").await;
    let elapsed = start.elapsed();

    assert_eq!(status, 200);
    // Should return quickly (well under the timeout)
    assert!(
        elapsed < Duration::from_millis(500),
        "Expected fast return, took {:?}",
        elapsed
    );
}

// ---------------------------------------------------------------------------
// HTTP /quiesce activity tracking tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_http_quiesce_waits_for_activity_to_stop() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    // Generate activity for 200ms then stop
    let a = activity.clone();
    tokio::spawn(async move {
        for _ in 0..10 {
            a.touch();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    let start = std::time::Instant::now();
    let (status, _json) =
        http_get(addr, "/sessions/test/quiesce?timeout_ms=150&max_wait_ms=5000&format=plain").await;
    let elapsed = start.elapsed();

    assert_eq!(status, 200);
    // Should wait at least 200ms (activity) + 150ms (timeout)
    assert!(
        elapsed >= Duration::from_millis(300),
        "Expected wait >= 300ms, took {:?}",
        elapsed
    );
}

// ---------------------------------------------------------------------------
// HTTP input resets activity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_http_input_resets_quiescence_timer() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    // Touch to mark current activity — the quiesce request will need to wait
    activity.touch();

    // Start a quiesce request with a 200ms timeout
    let addr_clone = addr;
    let quiesce_task = tokio::spawn(async move {
        let start = std::time::Instant::now();
        let (status, _json) =
            http_get(addr_clone, "/sessions/test/quiesce?timeout_ms=200&max_wait_ms=5000&format=plain").await;
        (status, start.elapsed())
    });

    // After 100ms, send HTTP input (which resets the timer via activity.touch())
    tokio::time::sleep(Duration::from_millis(100)).await;
    let stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = hyper::Request::builder()
        .method("POST")
        .uri("/sessions/test/input")
        .body(http_body_util::Full::new(Bytes::from("x")))
        .unwrap();
    let resp = sender.send_request(req).await.expect("request");
    assert_eq!(resp.status().as_u16(), 204);

    let (status, elapsed) = quiesce_task.await.unwrap();
    assert_eq!(status, 200);
    // Should take at least 100ms (wait before input) + 200ms (timeout after input reset)
    assert!(
        elapsed >= Duration::from_millis(250),
        "Expected >= 250ms, got {:?}",
        elapsed
    );
}

// ---------------------------------------------------------------------------
// WebSocket await_quiesce tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ws_await_quiesce_returns_sync_result() {
    let (state, _rx, _activity, _parser_tx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    // Wait for terminal to be quiet
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Connect WebSocket
    let (mut ws, _resp) =
        tokio_tungstenite::connect_async(format!("ws://{}/sessions/test/ws/json", addr))
            .await
            .expect("WS connect");

    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    // Read connected message
    let msg = ws.next().await.unwrap().unwrap();
    let connected: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(connected["connected"], true);

    // Send await_quiesce
    let req = serde_json::json!({
        "id": 42,
        "method": "await_quiesce",
        "params": {"timeout_ms": 100, "format": "plain"}
    });
    ws.send(Message::Text(req.to_string())).await.unwrap();

    // Read response
    let msg = ws.next().await.unwrap().unwrap();
    let resp: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();

    assert_eq!(resp["id"], 42);
    assert_eq!(resp["method"], "await_quiesce");
    assert!(resp.get("result").is_some(), "expected result, got: {:?}", resp);
    assert!(resp["result"].get("screen").is_some());
    assert!(resp["result"].get("scrollback_lines").is_some());
}

#[tokio::test]
async fn test_ws_await_quiesce_timeout_error() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    // Keep touching
    let a = activity.clone();
    let touch_handle = tokio::spawn(async move {
        loop {
            a.touch();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    let (mut ws, _resp) =
        tokio_tungstenite::connect_async(format!("ws://{}/sessions/test/ws/json", addr))
            .await
            .expect("WS connect");

    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    // Read connected message
    let _ = ws.next().await.unwrap().unwrap();

    // Send await_quiesce with short deadline
    let req = serde_json::json!({
        "id": 1,
        "method": "await_quiesce",
        "params": {"timeout_ms": 500, "max_wait_ms": 200}
    });
    ws.send(Message::Text(req.to_string())).await.unwrap();

    // Read response (should be error)
    let msg = ws.next().await.unwrap().unwrap();
    let resp: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();

    touch_handle.abort();

    assert_eq!(resp["id"], 1);
    assert_eq!(resp["method"], "await_quiesce");
    assert_eq!(resp["error"]["code"], "quiesce_timeout");
}

// ---------------------------------------------------------------------------
// WebSocket quiesce_ms subscription tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ws_quiesce_ms_emits_sync_after_quiet() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (mut ws, _resp) =
        tokio_tungstenite::connect_async(format!("ws://{}/sessions/test/ws/json", addr))
            .await
            .expect("WS connect");

    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    // Read connected message
    let _ = ws.next().await.unwrap().unwrap();

    // Subscribe with quiesce_ms
    let req = serde_json::json!({
        "id": 1,
        "method": "subscribe",
        "params": {"events": ["lines"], "quiesce_ms": 200, "format": "plain"}
    });
    ws.send(Message::Text(req.to_string())).await.unwrap();

    // Read subscribe response
    let msg = ws.next().await.unwrap().unwrap();
    let resp: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(resp["method"], "subscribe");

    // Read initial sync event
    let msg = ws.next().await.unwrap().unwrap();
    let event: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(event["event"], "sync");

    // Now trigger activity then let it settle
    activity.touch();
    tokio::time::sleep(Duration::from_millis(50)).await;
    activity.touch();

    // Wait for quiescence sync event (should arrive ~200ms after last touch)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut got_quiesce_sync = false;

    while tokio::time::Instant::now() < deadline {
        tokio::select! {
            msg = ws.next() => {
                match msg {
                    Some(Ok(msg)) => {
                        if let Ok(text) = msg.to_text() {
                            if let Ok(event) = serde_json::from_str::<serde_json::Value>(text) {
                                if event.get("event").and_then(|e| e.as_str()) == Some("sync") {
                                    got_quiesce_sync = true;
                                    // Verify it has screen data
                                    assert!(event.get("screen").is_some());
                                    assert!(event.get("scrollback_lines").is_some());
                                    break;
                                }
                            }
                        }
                    }
                    _ => break,
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }

    assert!(got_quiesce_sync, "Expected a quiescence sync event");
}

// ---------------------------------------------------------------------------
// HTTP /quiesce generation counter tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_http_quiesce_returns_generation() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    activity.touch();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let (status, json) = http_get(addr, "/sessions/test/quiesce?timeout_ms=100&format=plain").await;

    assert_eq!(status, 200);
    assert!(
        json.get("generation").is_some(),
        "response should have generation field, got: {:?}",
        json
    );
    assert_eq!(json["generation"], 1);
}

#[tokio::test]
async fn test_http_quiesce_last_generation_prevents_storm() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    // Touch once, let it settle
    activity.touch();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // First call: should return immediately with generation=1
    let start = std::time::Instant::now();
    let (status, json) = http_get(addr, "/sessions/test/quiesce?timeout_ms=100&format=plain").await;
    assert_eq!(status, 200);
    assert_eq!(json["generation"], 1);
    assert!(start.elapsed() < Duration::from_millis(500));

    // Second call with last_generation=1: should block until new activity
    let a = activity.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        a.touch(); // generation becomes 2
    });

    let start = std::time::Instant::now();
    let (status, json) = http_get(
        addr,
        "/sessions/test/quiesce?timeout_ms=100&last_generation=1&format=plain",
    )
    .await;
    let elapsed = start.elapsed();

    assert_eq!(status, 200);
    assert_eq!(json["generation"], 2);
    // Should have waited ~200ms for new activity + ~100ms for quiescence
    assert!(
        elapsed >= Duration::from_millis(250),
        "Expected >= 250ms, got {:?}",
        elapsed
    );
}

#[tokio::test]
async fn test_http_quiesce_last_generation_stale_returns_normally() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    // Touch twice, let it settle
    activity.touch(); // gen 1
    activity.touch(); // gen 2
    tokio::time::sleep(Duration::from_millis(200)).await;

    // last_generation=1 but current is 2: should NOT block on new activity
    let start = std::time::Instant::now();
    let (status, json) = http_get(
        addr,
        "/sessions/test/quiesce?timeout_ms=100&last_generation=1&format=plain",
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(json["generation"], 2);
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "Expected fast return for stale generation"
    );
}

#[tokio::test]
async fn test_http_quiesce_last_generation_timeout() {
    let (state, _rx, _activity, _parser_tx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    // Terminal is idle at generation=0, no new activity will come
    let (status, json) = http_get(
        addr,
        "/sessions/test/quiesce?timeout_ms=100&last_generation=0&max_wait_ms=300&format=plain",
    )
    .await;

    // Should timeout because no new activity arrives
    assert_eq!(status, 408);
    assert_eq!(json["error"]["code"], "quiesce_timeout");
}

// ---------------------------------------------------------------------------
// HTTP /quiesce fresh mode tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_http_quiesce_fresh_always_waits() {
    let (state, _rx, _activity, _parser_tx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    // Wait for terminal to be idle well past the timeout
    tokio::time::sleep(Duration::from_millis(300)).await;

    let start = std::time::Instant::now();
    let (status, json) = http_get(
        addr,
        "/sessions/test/quiesce?timeout_ms=200&fresh=true&format=plain",
    )
    .await;
    let elapsed = start.elapsed();

    assert_eq!(status, 200);
    assert!(json.get("generation").is_some());
    // Should have waited at least 200ms even though already quiescent
    assert!(
        elapsed >= Duration::from_millis(150),
        "Expected >= 150ms for fresh mode, got {:?}",
        elapsed
    );
}

#[tokio::test]
async fn test_http_quiesce_fresh_resets_on_activity() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Touch during the fresh wait to reset the timer
    let a = activity.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        a.touch();
    });

    let start = std::time::Instant::now();
    let (status, _json) = http_get(
        addr,
        "/sessions/test/quiesce?timeout_ms=200&fresh=true&format=plain",
    )
    .await;
    let elapsed = start.elapsed();

    assert_eq!(status, 200);
    // Should wait ~100ms (activity) + ~200ms (fresh timeout after activity)
    assert!(
        elapsed >= Duration::from_millis(250),
        "Expected >= 250ms for fresh mode with activity, got {:?}",
        elapsed
    );
}

// ---------------------------------------------------------------------------
// WebSocket await_quiesce generation counter tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ws_await_quiesce_returns_generation() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    activity.touch();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let (mut ws, _resp) =
        tokio_tungstenite::connect_async(format!("ws://{}/sessions/test/ws/json", addr))
            .await
            .expect("WS connect");

    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    // Read connected message
    let _ = ws.next().await.unwrap().unwrap();

    let req = serde_json::json!({
        "id": 1,
        "method": "await_quiesce",
        "params": {"timeout_ms": 100, "format": "plain"}
    });
    ws.send(Message::Text(req.to_string())).await.unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let resp: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();

    assert_eq!(resp["id"], 1);
    assert!(resp["result"].get("generation").is_some(), "expected generation in result: {:?}", resp);
    assert_eq!(resp["result"]["generation"], 1);
}

#[tokio::test]
async fn test_ws_await_quiesce_last_generation_blocks() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    activity.touch(); // generation = 1
    tokio::time::sleep(Duration::from_millis(200)).await;

    let (mut ws, _resp) =
        tokio_tungstenite::connect_async(format!("ws://{}/sessions/test/ws/json", addr))
            .await
            .expect("WS connect");

    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    // Read connected message
    let _ = ws.next().await.unwrap().unwrap();

    // Trigger new activity after 200ms
    let a = activity.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        a.touch(); // generation = 2
    });

    // Send await_quiesce with last_generation=1 — should block until generation changes
    let req = serde_json::json!({
        "id": 2,
        "method": "await_quiesce",
        "params": {"timeout_ms": 100, "last_generation": 1, "format": "plain"}
    });
    let start = std::time::Instant::now();
    ws.send(Message::Text(req.to_string())).await.unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let resp: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    let elapsed = start.elapsed();

    assert_eq!(resp["id"], 2);
    assert_eq!(resp["result"]["generation"], 2);
    assert!(
        elapsed >= Duration::from_millis(250),
        "Expected >= 250ms (wait for activity + quiescence), got {:?}",
        elapsed
    );
}

// ---------------------------------------------------------------------------
// Server-level GET /quiesce (any session) tests
// ---------------------------------------------------------------------------

/// Creates a test state with two sessions ("alpha" and "beta") and returns
/// their respective activity trackers.
fn create_multi_session_state() -> (api::AppState, ActivityTracker, ActivityTracker, mpsc::Sender<Bytes>, mpsc::Sender<Bytes>) {
    let registry = SessionRegistry::new();

    let make_session = |name: &str| -> (Session, ActivityTracker, mpsc::Sender<Bytes>) {
        let (input_tx, _input_rx) = mpsc::channel(64);
        let broker = Broker::new();
        let (parser_tx, parser_rx) = mpsc::channel(256);
        let parser = Parser::spawn(parser_rx, 80, 24, 1000);
        let activity = ActivityTracker::new();
        let session = Session {
            name: name.to_string(),
            pid: None,
            command: "test".to_string(),
            client_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            tags: Arc::new(parking_lot::RwLock::new(std::collections::HashSet::new())),
            child_exited: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            input_tx,
            output_rx: broker.sender(),
            shutdown: ShutdownCoordinator::new(),
            parser,
            overlays: OverlayStore::new(),
            panels: wsh::panel::PanelStore::new(),
            pty: Arc::new(parking_lot::Mutex::new(
                wsh::pty::Pty::spawn(24, 80, wsh::pty::SpawnCommand::default())
                    .expect("failed to spawn PTY for test"),
            )),
            terminal_size: wsh::terminal::TerminalSize::new(24, 80),
            input_mode: InputMode::new(),
            input_broadcaster: InputBroadcaster::new(),
            activity: activity.clone(),
            focus: FocusTracker::new(),
            detach_signal: tokio::sync::broadcast::channel::<()>(1).0,
            visual_update_tx: tokio::sync::broadcast::channel::<wsh::protocol::VisualUpdate>(16).0,
            screen_mode: std::sync::Arc::new(parking_lot::RwLock::new(wsh::overlay::ScreenMode::Normal)),
            cancelled: tokio_util::sync::CancellationToken::new(),
        };
        (session, activity, parser_tx)
    };

    let (session_a, activity_a, parser_tx_a) = make_session("alpha");
    let (session_b, activity_b, parser_tx_b) = make_session("beta");
    registry.insert(Some("alpha".into()), session_a).unwrap();
    registry.insert(Some("beta".into()), session_b).unwrap();

    let state = api::AppState {
        sessions: registry,
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(api::ServerConfig::new(false)),
            server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    (state, activity_a, activity_b, parser_tx_a, parser_tx_b)
}

#[tokio::test]
async fn test_http_quiesce_any_returns_first_quiescent_session() {
    let (state, _activity_a, _activity_b, _ptx_a, _ptx_b) = create_multi_session_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    // Both sessions are idle, so one should be returned
    let (status, json) = http_get(addr, "/quiesce?timeout_ms=100&format=plain").await;

    assert_eq!(status, 200);
    let session = json["session"].as_str().expect("response should have session field");
    assert!(
        session == "alpha" || session == "beta",
        "session should be alpha or beta, got: {}",
        session
    );
    assert!(json.get("screen").is_some());
    assert!(json.get("scrollback_lines").is_some());
    assert!(json.get("generation").is_some());
}

#[tokio::test]
async fn test_http_quiesce_any_returns_408_when_all_busy() {
    let (state, activity_a, activity_b, _ptx_a, _ptx_b) = create_multi_session_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    // Keep both sessions busy
    let a = activity_a.clone();
    let b = activity_b.clone();
    let touch_handle = tokio::spawn(async move {
        loop {
            a.touch();
            b.touch();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    let (status, json) = http_get(addr, "/quiesce?timeout_ms=500&max_wait_ms=200&format=plain").await;
    touch_handle.abort();

    assert_eq!(status, 408);
    assert_eq!(json["error"]["code"], "quiesce_timeout");
}

#[tokio::test]
async fn test_http_quiesce_any_picks_quiet_session_while_other_busy() {
    let (state, activity_a, _activity_b, _ptx_a, _ptx_b) = create_multi_session_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    // Keep alpha busy, leave beta idle
    let a = activity_a.clone();
    let touch_handle = tokio::spawn(async move {
        loop {
            a.touch();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    // Wait a bit so beta becomes clearly quiescent
    tokio::time::sleep(Duration::from_millis(200)).await;

    let (status, json) = http_get(addr, "/quiesce?timeout_ms=100&format=plain").await;
    touch_handle.abort();

    assert_eq!(status, 200);
    assert_eq!(json["session"], "beta", "should pick the idle session");
}

#[tokio::test]
async fn test_http_quiesce_any_last_generation_skips_stale_session() {
    let (state, activity_a, activity_b, _ptx_a, _ptx_b) = create_multi_session_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    // Touch alpha once, leave beta untouched
    activity_a.touch(); // alpha generation = 1
    tokio::time::sleep(Duration::from_millis(200)).await;

    // First call: should return one of them (both idle)
    let (status, json) = http_get(addr, "/quiesce?timeout_ms=100&format=plain").await;
    assert_eq!(status, 200);
    let first_session = json["session"].as_str().unwrap().to_string();
    let first_gen = json["generation"].as_u64().unwrap();

    // Now, if we pass last_session + last_generation matching the returned session,
    // it should NOT immediately return that session again.
    // Trigger new activity on the OTHER session so it becomes the winner.
    let other_session = if first_session == "alpha" { "beta" } else { "alpha" };
    let other_activity = if first_session == "alpha" {
        &activity_b
    } else {
        &activity_a
    };
    other_activity.touch();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let (status, json) = http_get(
        addr,
        &format!(
            "/quiesce?timeout_ms=100&last_session={}&last_generation={}&max_wait_ms=3000&format=plain",
            first_session, first_gen
        ),
    )
    .await;
    assert_eq!(status, 200);
    // The other session should win because the first session is blocked by last_generation
    assert_eq!(
        json["session"], other_session,
        "should pick the other session when first is blocked by last_generation"
    );
}

#[tokio::test]
async fn test_http_quiesce_any_fresh_always_waits() {
    let (state, _activity_a, _activity_b, _ptx_a, _ptx_b) = create_multi_session_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    // Wait well past the timeout
    tokio::time::sleep(Duration::from_millis(300)).await;

    let start = std::time::Instant::now();
    let (status, json) = http_get(addr, "/quiesce?timeout_ms=200&fresh=true&format=plain").await;
    let elapsed = start.elapsed();

    assert_eq!(status, 200);
    assert!(json.get("session").is_some());
    assert!(json.get("generation").is_some());
    // Should wait at least 200ms even though all sessions already quiescent
    assert!(
        elapsed >= Duration::from_millis(150),
        "Expected >= 150ms for fresh mode, got {:?}",
        elapsed
    );
}

#[tokio::test]
async fn test_http_quiesce_any_no_sessions_returns_404() {
    let state = api::AppState {
        sessions: SessionRegistry::new(),
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(api::ServerConfig::new(false)),
            server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (status, json) = http_get(addr, "/quiesce?timeout_ms=100&format=plain").await;

    assert_eq!(status, 404);
    assert_eq!(json["error"]["code"], "no_sessions");
}
