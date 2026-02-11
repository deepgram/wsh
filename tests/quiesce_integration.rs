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
    input::{InputBroadcaster, InputMode},
    overlay::OverlayStore,
    parser::Parser,
    session::{Session, SessionRegistry},
    shutdown::ShutdownCoordinator,
};

fn create_test_state() -> (api::AppState, mpsc::Receiver<Bytes>, ActivityTracker) {
    let (input_tx, input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    let activity = ActivityTracker::new();
    let session = Session {
        name: "test".to_string(),
        input_tx,
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: Arc::new(
            wsh::pty::Pty::spawn(24, 80, wsh::pty::SpawnCommand::default())
                .expect("failed to spawn PTY for test"),
        ),
        terminal_size: wsh::terminal::TerminalSize::new(24, 80),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        activity: activity.clone(),
        is_local: false,
    };
    let registry = SessionRegistry::new();
    registry.insert(Some("test".into()), session).unwrap();
    let state = api::AppState {
        sessions: registry,
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(api::ServerConfig::new(false)),
    };
    (state, input_rx, activity)
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
    let (state, _rx, _activity) = create_test_state();
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
    let (state, _rx, activity) = create_test_state();
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
    let (state, _rx, _activity) = create_test_state();
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
    let (state, _rx, activity) = create_test_state();
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
    let (state, _rx, activity) = create_test_state();
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
    let (state, _rx, _activity) = create_test_state();
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
    let (state, _rx, activity) = create_test_state();
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
    let (state, _rx, activity) = create_test_state();
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
