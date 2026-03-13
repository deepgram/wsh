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
        visual_update_tx: tokio::sync::broadcast::channel::<wsh::session::VisualUpdate>(16).0,
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
            server_id: "test-server-id".to_string(),
            shutdown_notify: tokio_util::sync::CancellationToken::new(),
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
        visual_update_tx: tokio::sync::broadcast::channel::<wsh::session::VisualUpdate>(16).0,
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
            server_id: "test-server-id".to_string(),
            shutdown_notify: tokio_util::sync::CancellationToken::new(),
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
        visual_update_tx: tokio::sync::broadcast::channel::<wsh::session::VisualUpdate>(16).0,
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
            server_id: "test-server-id".to_string(),
            shutdown_notify: tokio_util::sync::CancellationToken::new(),
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

#[tokio::test]
async fn test_ws_subscribe_overlay_events() {
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

    // Subscribe to overlay events
    tx.send(Message::Text(
        serde_json::json!({
            "method": "subscribe",
            "params": {"events": ["overlay"]}
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();

    // Read subscribe response
    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["method"], "subscribe");
    assert!(resp["result"]["events"].is_array());

    // After subscribe, we expect: sync event, initial overlay_sync, initial panel_sync
    // Collect the next messages to find each one
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut found_sync = false;
    let mut found_initial_overlay_sync = false;
    let mut found_initial_panel_sync = false;

    while tokio::time::Instant::now() < deadline
        && !(found_sync && found_initial_overlay_sync && found_initial_panel_sync)
    {
        if let Ok(Some(Ok(Message::Text(text)))) =
            tokio::time::timeout(Duration::from_millis(500), rx.next()).await
        {
            let json: serde_json::Value = serde_json::from_str(&text).unwrap();
            if json.get("event") == Some(&serde_json::json!("sync")) {
                found_sync = true;
            } else if json.get("type") == Some(&serde_json::json!("overlay_sync")) {
                // Initial overlay_sync should have empty overlays
                assert!(json["overlays"].is_array());
                assert_eq!(json["overlays"].as_array().unwrap().len(), 0);
                found_initial_overlay_sync = true;
            } else if json.get("type") == Some(&serde_json::json!("panel_sync")) {
                // Initial panel_sync should have empty panels
                assert!(json["panels"].is_array());
                assert_eq!(json["panels"].as_array().unwrap().len(), 0);
                found_initial_panel_sync = true;
            }
        }
    }
    assert!(
        found_initial_overlay_sync,
        "should receive initial overlay_sync with empty overlays"
    );
    assert!(
        found_initial_panel_sync,
        "should receive initial panel_sync with empty panels"
    );

    // Create an overlay via HTTP POST
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("http://{}/sessions/test/overlay", addr))
        .json(&serde_json::json!({
            "x": 0, "y": 0, "width": 10, "height": 1,
            "spans": [{"text": "hello"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Read WebSocket messages until we find an overlay_sync with the new overlay
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut found_overlay_event = false;
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(Message::Text(text)))) =
            tokio::time::timeout(Duration::from_millis(500), rx.next()).await
        {
            let json: serde_json::Value = serde_json::from_str(&text).unwrap();
            if json.get("type") == Some(&serde_json::json!("overlay_sync")) {
                let overlays = json["overlays"].as_array().unwrap();
                assert_eq!(overlays.len(), 1);
                // Verify the overlay contains "hello" text in its spans
                let spans = overlays[0]["spans"].as_array().unwrap();
                assert!(
                    spans.iter().any(|s| s["text"] == "hello"),
                    "overlay spans should contain 'hello', got: {:?}",
                    spans
                );
                found_overlay_event = true;
                break;
            }
        }
    }
    assert!(
        found_overlay_event,
        "should receive overlay_sync event after creating overlay via HTTP"
    );
}

#[tokio::test]
async fn test_ws_subscribe_panel_events() {
    let (state, _rx, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    let (ws, _) = connect_async(format!("ws://{}/sessions/test/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    // Read "connected" message
    let _ = recv_json(&mut rx).await;

    // Subscribe to overlay events (which includes panel events)
    tx.send(Message::Text(
        serde_json::json!({
            "method": "subscribe",
            "params": {"events": ["overlay"]}
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();

    // Read subscribe response
    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["method"], "subscribe");

    // Drain initial sync events (sync, overlay_sync, panel_sync)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut initial_done = 0;
    while tokio::time::Instant::now() < deadline && initial_done < 3 {
        if let Ok(Some(Ok(Message::Text(text)))) =
            tokio::time::timeout(Duration::from_millis(500), rx.next()).await
        {
            let json: serde_json::Value = serde_json::from_str(&text).unwrap();
            if json.get("event") == Some(&serde_json::json!("sync"))
                || json.get("type") == Some(&serde_json::json!("overlay_sync"))
                || json.get("type") == Some(&serde_json::json!("panel_sync"))
            {
                initial_done += 1;
            }
        }
    }
    assert_eq!(initial_done, 3, "should receive all three initial sync events");

    // Create a panel via HTTP POST
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("http://{}/sessions/test/panel", addr))
        .json(&serde_json::json!({
            "position": "bottom",
            "height": 1,
            "spans": [{"text": "status"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Read WebSocket messages until we find a panel_sync with the new panel
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut found_panel_event = false;
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(Message::Text(text)))) =
            tokio::time::timeout(Duration::from_millis(500), rx.next()).await
        {
            let json: serde_json::Value = serde_json::from_str(&text).unwrap();
            if json.get("type") == Some(&serde_json::json!("panel_sync")) {
                let panels = json["panels"].as_array().unwrap();
                if !panels.is_empty() {
                    assert_eq!(panels.len(), 1);
                    // Verify the panel contains "status" text in its spans
                    let spans = panels[0]["spans"].as_array().unwrap();
                    assert!(
                        spans.iter().any(|s| s["text"] == "status"),
                        "panel spans should contain 'status', got: {:?}",
                        spans
                    );
                    found_panel_event = true;
                    break;
                }
            }
        }
    }
    assert!(
        found_panel_event,
        "should receive panel_sync event after creating panel via HTTP"
    );
}
