//! Integration tests for WebSocket JSON request/response protocol.

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use wsh::{
    api,
    broker::Broker,
    input::{FocusTracker, InputBroadcaster, InputMode},
    overlay::OverlayStore,
    parser::Parser,
    session::{Session, SessionRegistry},
    shutdown::ShutdownCoordinator,
};

fn create_test_state() -> (api::AppState, mpsc::Receiver<Bytes>, mpsc::Sender<Bytes>) {
    let (input_tx, input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let (parser_tx, parser_rx) = mpsc::channel(256);
    let parser = Parser::spawn(parser_rx, 80, 24, 1000);
    let session = Session {
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
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: std::sync::Arc::new(parking_lot::Mutex::new(wsh::pty::Pty::spawn(24, 80, wsh::pty::SpawnCommand::default()).expect("failed to spawn PTY for test"))),
        terminal_size: wsh::terminal::TerminalSize::new(24, 80),
        activity: wsh::activity::ActivityTracker::new(),
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
    };
    (state, input_rx, parser_tx)
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

/// Helper: receive next text message, parse as JSON.
async fn recv_json(
    ws: &mut futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) -> serde_json::Value {
    let deadline = Duration::from_secs(2);
    let msg = tokio::time::timeout(deadline, ws.next())
        .await
        .expect("timeout waiting for message")
        .expect("stream ended")
        .expect("ws error");
    match msg {
        Message::Text(text) => serde_json::from_str(&text).expect("invalid JSON"),
        other => panic!("expected text message, got {:?}", other),
    }
}

#[tokio::test]
async fn test_ws_method_get_input_mode() {
    let (state, _rx, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    let (ws, _) = connect_async(format!("ws://{}/sessions/test/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    // Read "connected" message
    let msg = recv_json(&mut rx).await;
    assert_eq!(msg["connected"], true);

    // Send method call (no subscribe needed first!)
    tx.send(Message::Text(
        serde_json::json!({"id": 1, "method": "get_input_mode"}).to_string().into(),
    ))
    .await
    .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["method"], "get_input_mode");
    assert_eq!(resp["result"]["mode"], "passthrough");
}

#[tokio::test]
async fn test_ws_method_get_screen() {
    let (state, _rx, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    let (ws, _) = connect_async(format!("ws://{}/sessions/test/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    let _ = recv_json(&mut rx).await; // connected

    tx.send(Message::Text(
        serde_json::json!({"method": "get_screen", "params": {"format": "plain"}}).to_string().into(),
    ))
    .await
    .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["method"], "get_screen");
    assert!(resp["result"]["cols"].is_number());
    assert!(resp["result"]["rows"].is_number());
}

#[tokio::test]
async fn test_ws_method_send_input() {
    let (state, mut input_rx, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    let (ws, _) = connect_async(format!("ws://{}/sessions/test/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    let _ = recv_json(&mut rx).await; // connected

    tx.send(Message::Text(
        serde_json::json!({"method": "send_input", "params": {"data": "hello"}}).to_string().into(),
    ))
    .await
    .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["method"], "send_input");
    assert!(resp["result"].is_object());

    // Verify input reached the channel
    let received = tokio::time::timeout(Duration::from_secs(1), input_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.as_ref(), b"hello");
}

#[tokio::test]
async fn test_ws_subscribe_then_events() {
    let (input_tx, _input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let (_parser_tx, parser_rx) = mpsc::channel(256);
    let parser = Parser::spawn(parser_rx, 80, 24, 1000);
    let session = Session {
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
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: std::sync::Arc::new(parking_lot::Mutex::new(wsh::pty::Pty::spawn(24, 80, wsh::pty::SpawnCommand::default()).expect("failed to spawn PTY for test"))),
        terminal_size: wsh::terminal::TerminalSize::new(24, 80),
        activity: wsh::activity::ActivityTracker::new(),
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
    };
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    let (ws, _) = connect_async(format!("ws://{}/sessions/test/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    let _ = recv_json(&mut rx).await; // connected

    // Subscribe
    tx.send(Message::Text(
        serde_json::json!({
            "method": "subscribe",
            "params": {"events": ["lines"], "format": "plain"}
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();

    // Should get subscribe response
    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["method"], "subscribe");
    assert!(resp["result"]["events"].is_array());

    // Should get sync event
    let sync = recv_json(&mut rx).await;
    assert_eq!(sync["event"], "sync");

    // Send to parser channel and broadcast to reach both parser and subscribers
    _parser_tx.send(Bytes::from("Hello\r\n")).await.unwrap();
    broker.publish(Bytes::from("Hello\r\n"));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut found_line = false;
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(Message::Text(text)))) =
            tokio::time::timeout(Duration::from_millis(200), rx.next()).await
        {
            let json: serde_json::Value = serde_json::from_str(&text).unwrap();
            if json.get("event") == Some(&serde_json::json!("line")) {
                found_line = true;
                break;
            }
        }
    }
    assert!(found_line, "should receive line events after subscribing");
}

#[tokio::test]
async fn test_ws_unknown_method() {
    let (state, _rx, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    let (ws, _) = connect_async(format!("ws://{}/sessions/test/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    let _ = recv_json(&mut rx).await; // connected

    tx.send(Message::Text(
        serde_json::json!({"method": "nonexistent"}).to_string().into(),
    ))
    .await
    .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["method"], "nonexistent");
    assert_eq!(resp["error"]["code"], "unknown_method");
}

#[tokio::test]
async fn test_ws_malformed_request() {
    let (state, _rx, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    let (ws, _) = connect_async(format!("ws://{}/sessions/test/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    let _ = recv_json(&mut rx).await; // connected

    // Send JSON without method field
    tx.send(Message::Text(r#"{"id": 1}"#.to_string().into()))
        .await
        .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["error"]["code"], "invalid_request");
    // No method or id since parsing failed
}

#[tokio::test]
async fn test_ws_methods_interleaved_with_events() {
    let (input_tx, _input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let (_parser_tx, parser_rx) = mpsc::channel(256);
    let parser = Parser::spawn(parser_rx, 80, 24, 1000);
    let session = Session {
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
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: std::sync::Arc::new(parking_lot::Mutex::new(wsh::pty::Pty::spawn(24, 80, wsh::pty::SpawnCommand::default()).expect("failed to spawn PTY for test"))),
        terminal_size: wsh::terminal::TerminalSize::new(24, 80),
        activity: wsh::activity::ActivityTracker::new(),
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
    };
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    let (ws, _) = connect_async(format!("ws://{}/sessions/test/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    let _ = recv_json(&mut rx).await; // connected

    // Subscribe first
    tx.send(Message::Text(
        serde_json::json!({
            "method": "subscribe",
            "params": {"events": ["lines"], "format": "plain"}
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();

    let _ = recv_json(&mut rx).await; // subscribe response
    let _ = recv_json(&mut rx).await; // sync event

    // Now send a method call WHILE events could be flowing
    // Send to parser channel and broadcast to reach both parser and subscribers
    _parser_tx.send(Bytes::from("data\r\n")).await.unwrap();
    broker.publish(Bytes::from("data\r\n"));
    tokio::time::sleep(Duration::from_millis(50)).await;

    tx.send(Message::Text(
        serde_json::json!({"id": 42, "method": "get_input_mode"}).to_string().into(),
    ))
    .await
    .unwrap();

    // Collect messages until we see our response
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut found_response = false;
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(Message::Text(text)))) =
            tokio::time::timeout(Duration::from_millis(200), rx.next()).await
        {
            let json: serde_json::Value = serde_json::from_str(&text).unwrap();
            if json.get("method") == Some(&serde_json::json!("get_input_mode")) {
                assert_eq!(json["id"], 42);
                assert_eq!(json["result"]["mode"], "passthrough");
                found_response = true;
                break;
            }
            // Other messages (line events) are fine, skip them
        }
    }
    assert!(
        found_response,
        "should receive method response even while events are streaming"
    );
}
