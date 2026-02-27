//! Integration tests for the idle detection feature.
//!
//! Tests cover:
//! - HTTP GET /idle endpoint
//! - WebSocket await_idle method
//! - Subscription idle_timeout_ms parameter
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
            mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            ticket_store: std::sync::Arc::new(wsh::api::ticket::TicketStore::new()),
            backends: wsh::federation::registry::BackendRegistry::new(),
            federation: std::sync::Arc::new(tokio::sync::Mutex::new(wsh::federation::manager::FederationManager::new())),
            ip_access: None,
            hostname: "test".to_string(),
            federation_config_path: None,
            local_token: None,
            default_backend_token: None,
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
// HTTP /idle tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_http_idle_returns_screen_state_after_quiet() {
    let (state, _rx, _activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Terminal should already be idle (no activity for >100ms given setup time)
    let (status, json) = http_get(addr, "/sessions/test/idle?timeout_ms=100&format=plain").await;

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
async fn test_http_idle_returns_408_when_deadline_exceeded() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Keep touching to prevent idle
    let a = activity.clone();
    let touch_handle = tokio::spawn(async move {
        loop {
            a.touch();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    let (status, json) =
        http_get(addr, "/sessions/test/idle?timeout_ms=500&max_wait_ms=200&format=plain").await;

    touch_handle.abort();

    assert_eq!(status, 408);
    assert_eq!(json["error"]["code"], "idle_timeout");
}

#[tokio::test]
async fn test_http_idle_returns_immediately_when_already_idle() {
    let (state, _rx, _activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Wait for well past the timeout
    tokio::time::sleep(Duration::from_millis(200)).await;

    let start = std::time::Instant::now();
    let (status, _json) = http_get(addr, "/sessions/test/idle?timeout_ms=100&format=plain").await;
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
// HTTP /idle activity tracking tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_http_idle_waits_for_activity_to_stop() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
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
        http_get(addr, "/sessions/test/idle?timeout_ms=150&max_wait_ms=5000&format=plain").await;
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
async fn test_http_input_resets_idle_timer() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Touch to mark current activity -- the idle request will need to wait
    activity.touch();

    // Start an idle request with a 200ms timeout
    let addr_clone = addr;
    let idle_task = tokio::spawn(async move {
        let start = std::time::Instant::now();
        let (status, _json) =
            http_get(addr_clone, "/sessions/test/idle?timeout_ms=200&max_wait_ms=5000&format=plain").await;
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

    let (status, elapsed) = idle_task.await.unwrap();
    assert_eq!(status, 200);
    // Should take at least 100ms (wait before input) + 200ms (timeout after input reset)
    assert!(
        elapsed >= Duration::from_millis(250),
        "Expected >= 250ms, got {:?}",
        elapsed
    );
}

// ---------------------------------------------------------------------------
// WebSocket await_idle tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ws_await_idle_returns_sync_result() {
    let (state, _rx, _activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Wait for terminal to be idle
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

    // Send await_idle
    let req = serde_json::json!({
        "id": 42,
        "method": "await_idle",
        "params": {"timeout_ms": 100, "format": "plain"}
    });
    ws.send(Message::Text(req.to_string().into())).await.unwrap();

    // Read response
    let msg = ws.next().await.unwrap().unwrap();
    let resp: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();

    assert_eq!(resp["id"], 42);
    assert_eq!(resp["method"], "await_idle");
    assert!(resp.get("result").is_some(), "expected result, got: {:?}", resp);
    assert!(resp["result"].get("screen").is_some());
    assert!(resp["result"].get("scrollback_lines").is_some());
}

#[tokio::test]
async fn test_ws_await_idle_timeout_error() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
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

    // Send await_idle with short deadline
    let req = serde_json::json!({
        "id": 1,
        "method": "await_idle",
        "params": {"timeout_ms": 500, "max_wait_ms": 200}
    });
    ws.send(Message::Text(req.to_string().into())).await.unwrap();

    // Read response (should be error)
    let msg = ws.next().await.unwrap().unwrap();
    let resp: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();

    touch_handle.abort();

    assert_eq!(resp["id"], 1);
    assert_eq!(resp["method"], "await_idle");
    assert_eq!(resp["error"]["code"], "idle_timeout");
}

// ---------------------------------------------------------------------------
// WebSocket idle_timeout_ms subscription tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ws_idle_timeout_emits_running_then_idle() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    let (mut ws, _resp) =
        tokio_tungstenite::connect_async(format!("ws://{}/sessions/test/ws/json", addr))
            .await
            .expect("WS connect");

    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    // Read connected message
    let _ = ws.next().await.unwrap().unwrap();

    // Subscribe with idle_timeout_ms and activity events
    let req = serde_json::json!({
        "id": 1,
        "method": "subscribe",
        "params": {"events": ["lines", "activity"], "idle_timeout_ms": 200, "format": "plain"}
    });
    ws.send(Message::Text(req.to_string().into())).await.unwrap();

    // Read subscribe response
    let msg = ws.next().await.unwrap().unwrap();
    let resp: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(resp["method"], "subscribe");

    // Read initial sync event
    let msg = ws.next().await.unwrap().unwrap();
    let event: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(event["event"], "sync");

    // Read initial activity state event
    let msg = ws.next().await.unwrap().unwrap();
    let event: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    let initial_event = event["event"].as_str().unwrap();
    // Initial state could be either idle or running depending on timing
    assert!(
        initial_event == "idle" || initial_event == "running",
        "Expected initial idle or running event, got: {}",
        initial_event
    );

    // Wait for the monitoring task to settle into the main loop by draining
    // any pending events. If the initial state was "running", the monitoring
    // task will emit an idle event once the session goes quiet.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        tokio::select! {
            msg = ws.next() => {
                match msg {
                    Some(Ok(msg)) => {
                        if let Ok(text) = msg.to_text() {
                            if let Ok(ev) = serde_json::from_str::<serde_json::Value>(text) {
                                if ev.get("event").and_then(|e| e.as_str()) == Some("idle") {
                                    break;
                                }
                            }
                        }
                    }
                    _ => break,
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }

    // Now trigger activity after the monitoring task is in its main loop
    activity.touch();
    tokio::time::sleep(Duration::from_millis(50)).await;
    activity.touch();

    // Wait for running and idle events
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut got_running = false;
    let mut got_idle = false;

    while tokio::time::Instant::now() < deadline && !got_idle {
        tokio::select! {
            msg = ws.next() => {
                match msg {
                    Some(Ok(msg)) => {
                        if let Ok(text) = msg.to_text() {
                            if let Ok(event) = serde_json::from_str::<serde_json::Value>(text) {
                                match event.get("event").and_then(|e| e.as_str()) {
                                    Some("running") => {
                                        got_running = true;
                                        assert!(event.get("generation").is_some());
                                    }
                                    Some("idle") => {
                                        got_idle = true;
                                        // Verify it has screen data and generation
                                        assert!(event.get("screen").is_some());
                                        assert!(event.get("scrollback_lines").is_some());
                                        assert!(event.get("generation").is_some());
                                    }
                                    _ => {} // ignore other events
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

    assert!(got_running, "Expected a running event after activity");
    assert!(got_idle, "Expected an idle event after quiet period");
}

#[tokio::test]
async fn test_ws_quiesce_ms_alias_still_works() {
    // Verify backward compatibility: quiesce_ms alias is accepted
    let (state, _rx, _activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    let (mut ws, _resp) =
        tokio_tungstenite::connect_async(format!("ws://{}/sessions/test/ws/json", addr))
            .await
            .expect("WS connect");

    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    // Read connected message
    let _ = ws.next().await.unwrap().unwrap();

    // Subscribe with old quiesce_ms field name
    let req = serde_json::json!({
        "id": 1,
        "method": "subscribe",
        "params": {"events": ["activity"], "quiesce_ms": 200, "format": "plain"}
    });
    ws.send(Message::Text(req.to_string().into())).await.unwrap();

    // Read subscribe response — should succeed
    let msg = ws.next().await.unwrap().unwrap();
    let resp: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(resp["method"], "subscribe");
    assert!(resp["error"].is_null(), "subscribe should succeed with quiesce_ms alias");
}

// ---------------------------------------------------------------------------
// HTTP /idle generation counter tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_http_idle_returns_generation() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    activity.touch();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let (status, json) = http_get(addr, "/sessions/test/idle?timeout_ms=100&format=plain").await;

    assert_eq!(status, 200);
    assert!(
        json.get("generation").is_some(),
        "response should have generation field, got: {:?}",
        json
    );
    assert_eq!(json["generation"], 1);
}

#[tokio::test]
async fn test_http_idle_last_generation_prevents_storm() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Touch once, let it settle
    activity.touch();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // First call: should return immediately with generation=1
    let start = std::time::Instant::now();
    let (status, json) = http_get(addr, "/sessions/test/idle?timeout_ms=100&format=plain").await;
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
        "/sessions/test/idle?timeout_ms=100&last_generation=1&format=plain",
    )
    .await;
    let elapsed = start.elapsed();

    assert_eq!(status, 200);
    assert_eq!(json["generation"], 2);
    // Should have waited ~200ms for new activity + ~100ms for idle
    assert!(
        elapsed >= Duration::from_millis(250),
        "Expected >= 250ms, got {:?}",
        elapsed
    );
}

#[tokio::test]
async fn test_http_idle_last_generation_stale_returns_normally() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Touch twice, let it settle
    activity.touch(); // gen 1
    activity.touch(); // gen 2
    tokio::time::sleep(Duration::from_millis(200)).await;

    // last_generation=1 but current is 2: should NOT block on new activity
    let start = std::time::Instant::now();
    let (status, json) = http_get(
        addr,
        "/sessions/test/idle?timeout_ms=100&last_generation=1&format=plain",
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
async fn test_http_idle_last_generation_timeout() {
    let (state, _rx, _activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Terminal is idle at generation=0, no new activity will come
    let (status, json) = http_get(
        addr,
        "/sessions/test/idle?timeout_ms=100&last_generation=0&max_wait_ms=300&format=plain",
    )
    .await;

    // Should timeout because no new activity arrives
    assert_eq!(status, 408);
    assert_eq!(json["error"]["code"], "idle_timeout");
}

// ---------------------------------------------------------------------------
// HTTP /idle fresh mode tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_http_idle_fresh_always_waits() {
    let (state, _rx, _activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Wait for terminal to be idle well past the timeout
    tokio::time::sleep(Duration::from_millis(300)).await;

    let start = std::time::Instant::now();
    let (status, json) = http_get(
        addr,
        "/sessions/test/idle?timeout_ms=200&fresh=true&format=plain",
    )
    .await;
    let elapsed = start.elapsed();

    assert_eq!(status, 200);
    assert!(json.get("generation").is_some());
    // Should have waited at least 200ms even though already idle
    assert!(
        elapsed >= Duration::from_millis(150),
        "Expected >= 150ms for fresh mode, got {:?}",
        elapsed
    );
}

#[tokio::test]
async fn test_http_idle_fresh_resets_on_activity() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
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
        "/sessions/test/idle?timeout_ms=200&fresh=true&format=plain",
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
// WebSocket await_idle generation counter tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ws_await_idle_returns_generation() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
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
        "method": "await_idle",
        "params": {"timeout_ms": 100, "format": "plain"}
    });
    ws.send(Message::Text(req.to_string().into())).await.unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let resp: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();

    assert_eq!(resp["id"], 1);
    assert!(resp["result"].get("generation").is_some(), "expected generation in result: {:?}", resp);
    assert_eq!(resp["result"]["generation"], 1);
}

#[tokio::test]
async fn test_ws_await_idle_last_generation_blocks() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
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

    // Send await_idle with last_generation=1 -- should block until generation changes
    let req = serde_json::json!({
        "id": 2,
        "method": "await_idle",
        "params": {"timeout_ms": 100, "last_generation": 1, "format": "plain"}
    });
    let start = std::time::Instant::now();
    ws.send(Message::Text(req.to_string().into())).await.unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let resp: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    let elapsed = start.elapsed();

    assert_eq!(resp["id"], 2);
    assert_eq!(resp["result"]["generation"], 2);
    assert!(
        elapsed >= Duration::from_millis(250),
        "Expected >= 250ms (wait for activity + idle), got {:?}",
        elapsed
    );
}

// ---------------------------------------------------------------------------
// Server-level GET /idle (any session) tests
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
            mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            ticket_store: std::sync::Arc::new(wsh::api::ticket::TicketStore::new()),
            backends: wsh::federation::registry::BackendRegistry::new(),
            federation: std::sync::Arc::new(tokio::sync::Mutex::new(wsh::federation::manager::FederationManager::new())),
            ip_access: None,
            hostname: "test".to_string(),
            federation_config_path: None,
            local_token: None,
            default_backend_token: None,
    };
    (state, activity_a, activity_b, parser_tx_a, parser_tx_b)
}

#[tokio::test]
async fn test_http_idle_any_returns_first_idle_session() {
    let (state, _activity_a, _activity_b, _ptx_a, _ptx_b) = create_multi_session_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Both sessions are idle, so one should be returned
    let (status, json) = http_get(addr, "/idle?timeout_ms=100&format=plain").await;

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
async fn test_http_idle_any_returns_408_when_all_busy() {
    let (state, activity_a, activity_b, _ptx_a, _ptx_b) = create_multi_session_state();
    let app = api::router(state, api::RouterConfig::default());
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

    let (status, json) = http_get(addr, "/idle?timeout_ms=500&max_wait_ms=200&format=plain").await;
    touch_handle.abort();

    assert_eq!(status, 408);
    assert_eq!(json["error"]["code"], "idle_timeout");
}

#[tokio::test]
async fn test_http_idle_any_picks_idle_session_while_other_busy() {
    let (state, activity_a, _activity_b, _ptx_a, _ptx_b) = create_multi_session_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Keep alpha busy, leave beta idle
    let a = activity_a.clone();
    let touch_handle = tokio::spawn(async move {
        loop {
            a.touch();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    // Wait a bit so beta becomes clearly idle
    tokio::time::sleep(Duration::from_millis(200)).await;

    let (status, json) = http_get(addr, "/idle?timeout_ms=100&format=plain").await;
    touch_handle.abort();

    assert_eq!(status, 200);
    assert_eq!(json["session"], "beta", "should pick the idle session");
}

#[tokio::test]
async fn test_http_idle_any_last_generation_skips_stale_session() {
    let (state, activity_a, activity_b, _ptx_a, _ptx_b) = create_multi_session_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Touch alpha once, leave beta untouched
    activity_a.touch(); // alpha generation = 1
    tokio::time::sleep(Duration::from_millis(200)).await;

    // First call: should return one of them (both idle)
    let (status, json) = http_get(addr, "/idle?timeout_ms=100&format=plain").await;
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
            "/idle?timeout_ms=100&last_session={}&last_generation={}&max_wait_ms=3000&format=plain",
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
async fn test_http_idle_any_fresh_always_waits() {
    let (state, _activity_a, _activity_b, _ptx_a, _ptx_b) = create_multi_session_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Wait well past the timeout
    tokio::time::sleep(Duration::from_millis(300)).await;

    let start = std::time::Instant::now();
    let (status, json) = http_get(addr, "/idle?timeout_ms=200&fresh=true&format=plain").await;
    let elapsed = start.elapsed();

    assert_eq!(status, 200);
    assert!(json.get("session").is_some());
    assert!(json.get("generation").is_some());
    // Should wait at least 200ms even though all sessions already idle
    assert!(
        elapsed >= Duration::from_millis(150),
        "Expected >= 150ms for fresh mode, got {:?}",
        elapsed
    );
}

#[tokio::test]
async fn test_http_idle_any_no_sessions_returns_404() {
    let state = api::AppState {
        sessions: SessionRegistry::new(),
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(api::ServerConfig::new(false)),
            server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            ticket_store: std::sync::Arc::new(wsh::api::ticket::TicketStore::new()),
            backends: wsh::federation::registry::BackendRegistry::new(),
            federation: std::sync::Arc::new(tokio::sync::Mutex::new(wsh::federation::manager::FederationManager::new())),
            ip_access: None,
            hostname: "test".to_string(),
            federation_config_path: None,
            local_token: None,
            default_backend_token: None,
    };
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    let (status, json) = http_get(addr, "/idle?timeout_ms=100&format=plain").await;

    assert_eq!(status, 404);
    assert_eq!(json["error"]["code"], "no_sessions");
}

// ---------------------------------------------------------------------------
// HTTP /sessions and /sessions/:name/screen include last_activity_ms
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_http_session_info_includes_last_activity_ms() {
    let (state, _rx, _activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    let (status, json) = http_get(addr, "/sessions").await;
    assert_eq!(status, 200);

    let sessions = json.as_array().expect("response should be an array");
    assert!(!sessions.is_empty(), "expected at least one session");

    for session in sessions {
        assert!(
            session.get("last_activity_ms").is_some(),
            "session info should have last_activity_ms field, got: {:?}",
            session
        );
        assert!(
            session["last_activity_ms"].is_number(),
            "last_activity_ms should be a number, got: {:?}",
            session["last_activity_ms"]
        );
    }
}

#[tokio::test]
async fn test_ws_subscribe_activity_initial_idle() {
    // A session that has been idle longer than idle_timeout_ms should emit
    // an immediate Idle event (with screen data) right after subscribe.
    let (state, _rx, _activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Let the session sit idle for longer than the timeout we'll subscribe with.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let (mut ws, _resp) =
        tokio_tungstenite::connect_async(format!("ws://{}/sessions/test/ws/json", addr))
            .await
            .expect("WS connect");

    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    // Read connected message
    let _ = ws.next().await.unwrap().unwrap();

    // Subscribe with idle_timeout_ms shorter than how long the session has been idle
    let req = serde_json::json!({
        "id": 1,
        "method": "subscribe",
        "params": {"events": ["activity"], "idle_timeout_ms": 200, "format": "plain"}
    });
    ws.send(Message::Text(req.to_string().into())).await.unwrap();

    // Read subscribe response
    let msg = ws.next().await.unwrap().unwrap();
    let resp: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(resp["method"], "subscribe");

    // Read sync event
    let msg = ws.next().await.unwrap().unwrap();
    let event: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(event["event"], "sync");

    // The very next event should be an initial idle event
    let msg = ws.next().await.unwrap().unwrap();
    let event: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(event["event"], "idle", "Expected initial idle event, got: {}", event);
    assert!(event.get("generation").is_some());
    assert!(event.get("screen").is_some());
    assert!(event.get("scrollback_lines").is_some());
}

#[tokio::test]
async fn test_ws_subscribe_activity_initial_running() {
    // A session with very recent activity should emit an immediate Running
    // event right after subscribe.
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    let (mut ws, _resp) =
        tokio_tungstenite::connect_async(format!("ws://{}/sessions/test/ws/json", addr))
            .await
            .expect("WS connect");

    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    // Touch activity right before subscribing so last_activity_ms < idle_timeout_ms
    activity.touch();

    // Read connected message
    let _ = ws.next().await.unwrap().unwrap();

    // Subscribe with a long idle_timeout_ms so the session is definitely "running"
    let req = serde_json::json!({
        "id": 1,
        "method": "subscribe",
        "params": {"events": ["activity"], "idle_timeout_ms": 5000, "format": "plain"}
    });
    ws.send(Message::Text(req.to_string().into())).await.unwrap();

    // Read subscribe response
    let msg = ws.next().await.unwrap().unwrap();
    let resp: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(resp["method"], "subscribe");

    // Read sync event
    let msg = ws.next().await.unwrap().unwrap();
    let event: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(event["event"], "sync");

    // The very next event should be an initial running event
    let msg = ws.next().await.unwrap().unwrap();
    let event: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(event["event"], "running", "Expected initial running event, got: {}", event);
    assert!(event.get("generation").is_some());
    // Running events should NOT have screen data
    assert!(event.get("screen").is_none());
}

#[tokio::test]
async fn test_ws_subscribe_activity_multiple_cycles() {
    // Verify that two activity→idle cycles produce two Running+Idle pairs.
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    let (mut ws, _resp) =
        tokio_tungstenite::connect_async(format!("ws://{}/sessions/test/ws/json", addr))
            .await
            .expect("WS connect");

    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    // Read connected message
    let _ = ws.next().await.unwrap().unwrap();

    // Subscribe with a short idle timeout
    let req = serde_json::json!({
        "id": 1,
        "method": "subscribe",
        "params": {"events": ["activity"], "idle_timeout_ms": 150, "format": "plain"}
    });
    ws.send(Message::Text(req.to_string().into())).await.unwrap();

    // Read subscribe response
    let _ = ws.next().await.unwrap().unwrap();
    // Read sync event
    let _ = ws.next().await.unwrap().unwrap();
    // Read initial activity state event
    let _ = ws.next().await.unwrap().unwrap();

    // Wait for the monitoring task to settle into its main loop. If the
    // initial state was "running", the monitoring task will emit an idle
    // event once the session goes quiet. We drain events until we see idle.
    {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            tokio::select! {
                msg = ws.next() => {
                    match msg {
                        Some(Ok(msg)) => {
                            if let Ok(text) = msg.to_text() {
                                if let Ok(ev) = serde_json::from_str::<serde_json::Value>(text) {
                                    if ev.get("event").and_then(|e| e.as_str()) == Some("idle") {
                                        break;
                                    }
                                }
                            }
                        }
                        _ => break,
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        }
    }

    // Collect activity events until we see an idle event (one cycle)
    async fn collect_until_idle(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> Vec<String> {
        use futures::StreamExt;
        let mut events = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            tokio::select! {
                msg = ws.next() => {
                    match msg {
                        Some(Ok(msg)) => {
                            if let Ok(text) = msg.to_text() {
                                if let Ok(event) = serde_json::from_str::<serde_json::Value>(text) {
                                    if let Some(name) = event.get("event").and_then(|e| e.as_str()) {
                                        if name == "running" || name == "idle" {
                                            events.push(name.to_string());
                                            if name == "idle" {
                                                return events;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        _ => break,
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        }
        events
    }

    // Cycle 1: trigger activity, wait for Running + Idle
    activity.touch();
    tokio::time::sleep(Duration::from_millis(30)).await;
    activity.touch(); // second touch to ensure activity is detected
    let cycle1 = collect_until_idle(&mut ws).await;
    assert!(
        cycle1.contains(&"running".to_string()),
        "Cycle 1: expected Running event, got: {:?}", cycle1
    );
    assert!(
        cycle1.last() == Some(&"idle".to_string()),
        "Cycle 1: expected Idle event at end, got: {:?}", cycle1
    );

    // Cycle 2: trigger activity again, wait for Running + Idle
    activity.touch();
    tokio::time::sleep(Duration::from_millis(30)).await;
    activity.touch();
    let cycle2 = collect_until_idle(&mut ws).await;
    assert!(
        cycle2.contains(&"running".to_string()),
        "Cycle 2: expected Running event, got: {:?}", cycle2
    );
    assert!(
        cycle2.last() == Some(&"idle".to_string()),
        "Cycle 2: expected Idle event at end, got: {:?}", cycle2
    );
}

#[tokio::test]
async fn test_ws_subscribe_activity_idle_to_running_transition() {
    // After the session becomes idle, new activity should emit a Running
    // event, demonstrating the idle→running transition.
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    let (mut ws, _resp) =
        tokio_tungstenite::connect_async(format!("ws://{}/sessions/test/ws/json", addr))
            .await
            .expect("WS connect");

    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    // Read connected message
    let _ = ws.next().await.unwrap().unwrap();

    // Subscribe with a short idle timeout
    let req = serde_json::json!({
        "id": 1,
        "method": "subscribe",
        "params": {"events": ["activity"], "idle_timeout_ms": 150, "format": "plain"}
    });
    ws.send(Message::Text(req.to_string().into())).await.unwrap();

    // Read subscribe response, sync event, and initial activity state
    let _ = ws.next().await.unwrap().unwrap();
    let _ = ws.next().await.unwrap().unwrap();
    let _ = ws.next().await.unwrap().unwrap();

    // Step 1: Trigger activity and wait until we see Idle (Running first, then Idle)
    activity.touch();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut reached_idle = false;
    while tokio::time::Instant::now() < deadline && !reached_idle {
        tokio::select! {
            msg = ws.next() => {
                match msg {
                    Some(Ok(msg)) => {
                        if let Ok(text) = msg.to_text() {
                            if let Ok(event) = serde_json::from_str::<serde_json::Value>(text) {
                                if event.get("event").and_then(|e| e.as_str()) == Some("idle") {
                                    reached_idle = true;
                                }
                            }
                        }
                    }
                    _ => break,
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }
    assert!(reached_idle, "Session should have become idle");

    // Step 2: Now trigger new activity — the server should emit Running
    activity.touch();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut got_running_after_idle = false;
    while tokio::time::Instant::now() < deadline && !got_running_after_idle {
        tokio::select! {
            msg = ws.next() => {
                match msg {
                    Some(Ok(msg)) => {
                        if let Ok(text) = msg.to_text() {
                            if let Ok(event) = serde_json::from_str::<serde_json::Value>(text) {
                                if event.get("event").and_then(|e| e.as_str()) == Some("running") {
                                    got_running_after_idle = true;
                                    assert!(event.get("generation").is_some());
                                }
                            }
                        }
                    }
                    _ => break,
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }
    assert!(
        got_running_after_idle,
        "Expected Running event after new activity following idle state"
    );
}

#[tokio::test]
async fn test_http_screen_includes_last_activity_ms() {
    let (state, _rx, _activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    let (status, json) = http_get(addr, "/sessions/test/screen?format=plain").await;
    assert_eq!(status, 200);

    assert!(
        json.get("last_activity_ms").is_some(),
        "screen response should have last_activity_ms field, got: {:?}",
        json
    );
    assert!(
        json["last_activity_ms"].is_number(),
        "last_activity_ms should be a number, got: {:?}",
        json["last_activity_ms"]
    );
}
