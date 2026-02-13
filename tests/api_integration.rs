//! Integration tests for API endpoints.
//!
//! These tests verify that the HTTP API works correctly through the full router:
//! - Health endpoint returns expected response
//! - POST /input sends data through to the channel (simulating PTY input)
//! - WebSocket /ws/raw receives PTY output broadcasts
//! - WebSocket can send input that reaches the PTY channel

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tower::ServiceExt;
use wsh::api::{router, AppState};
use wsh::broker::Broker;
use wsh::input::{FocusTracker, InputBroadcaster, InputMode};
use wsh::overlay::OverlayStore;
use wsh::parser::Parser;
use wsh::session::{Session, SessionRegistry};
use wsh::shutdown::ShutdownCoordinator;

/// Creates a test application with channels for input/output.
/// Returns the router, input receiver, and output sender for test verification.
fn create_test_app() -> (axum::Router, mpsc::Receiver<Bytes>, broadcast::Sender<Bytes>) {
    let (input_tx, input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    let session = Session {
        name: "test".to_string(),
        pid: None,
        command: "test".to_string(),
        client_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        input_tx,
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: std::sync::Arc::new(wsh::pty::Pty::spawn(24, 80, wsh::pty::SpawnCommand::default()).expect("failed to spawn PTY for test")),
        terminal_size: wsh::terminal::TerminalSize::new(24, 80),
        activity: wsh::activity::ActivityTracker::new(),
        focus: FocusTracker::new(),
        detach_signal: tokio::sync::broadcast::channel::<()>(1).0,
        visual_update_tx: tokio::sync::broadcast::channel::<wsh::protocol::VisualUpdate>(16).0,
        screen_mode: std::sync::Arc::new(parking_lot::RwLock::new(wsh::overlay::ScreenMode::Normal)),
    };
    let registry = SessionRegistry::new();
    registry.insert(Some("test".into()), session).unwrap();
    let state = AppState {
        sessions: registry,
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(wsh::api::ServerConfig::new(false)),
    };
    (router(state, None), input_rx, broker.sender())
}

/// Starts the server on a random available port and returns the address.
async fn start_test_server(app: axum::Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Give the server a moment to start
    tokio::time::sleep(Duration::from_millis(10)).await;

    addr
}

#[tokio::test]
async fn test_full_api_health_check() {
    let (app, _input_rx, _output_tx) = create_test_app();

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
async fn test_api_input_to_pty() {
    let (app, mut input_rx, _output_tx) = create_test_app();

    let test_input = b"hello from API test";

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/test/input")
                .body(Body::from(test_input.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify the input was forwarded to the channel
    let received = tokio::time::timeout(Duration::from_secs(1), input_rx.recv())
        .await
        .expect("timed out waiting for input")
        .expect("channel closed unexpectedly");

    assert_eq!(received.as_ref(), test_input);
}

#[tokio::test]
async fn test_api_input_multiple_requests() {
    // Test that multiple sequential inputs are all forwarded correctly
    let (input_tx, mut input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    let session = Session {
        name: "test".to_string(),
        pid: None,
        command: "test".to_string(),
        client_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        input_tx,
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: std::sync::Arc::new(wsh::pty::Pty::spawn(24, 80, wsh::pty::SpawnCommand::default()).expect("failed to spawn PTY for test")),
        terminal_size: wsh::terminal::TerminalSize::new(24, 80),
        activity: wsh::activity::ActivityTracker::new(),
        focus: FocusTracker::new(),
        detach_signal: tokio::sync::broadcast::channel::<()>(1).0,
        visual_update_tx: tokio::sync::broadcast::channel::<wsh::protocol::VisualUpdate>(16).0,
        screen_mode: std::sync::Arc::new(parking_lot::RwLock::new(wsh::overlay::ScreenMode::Normal)),
    };
    let registry = SessionRegistry::new();
    registry.insert(Some("test".into()), session).unwrap();
    let state = AppState { sessions: registry, shutdown: ShutdownCoordinator::new(), server_config: std::sync::Arc::new(wsh::api::ServerConfig::new(false)) };
    let app = router(state, None);

    let inputs = vec!["first input", "second input", "third input"];

    // Clone app for each request since oneshot consumes it
    for (i, input) in inputs.iter().enumerate() {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/test/input")
                    .body(Body::from(*input))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::NO_CONTENT,
            "Request {} failed",
            i
        );
    }

    // Verify all inputs were received in order
    for expected in inputs {
        let received = tokio::time::timeout(Duration::from_secs(1), input_rx.recv())
            .await
            .expect("timed out waiting for input")
            .expect("channel closed unexpectedly");

        assert_eq!(
            String::from_utf8_lossy(&received),
            expected,
            "Input mismatch"
        );
    }
}

#[tokio::test]
async fn test_websocket_upgrade_response() {
    // Test that /ws/raw endpoint exists and responds appropriately to non-upgrade requests
    let (app, _input_rx, _output_tx) = create_test_app();

    // A regular GET without upgrade headers should not return 404
    let response = app
        .oneshot(Request::builder().uri("/sessions/test/ws/raw").body(Body::empty()).unwrap())
        .await
        .unwrap();

    // WebSocket endpoints typically return an error status (not 404) when accessed
    // without proper upgrade headers
    assert_ne!(
        response.status(),
        StatusCode::NOT_FOUND,
        "WebSocket endpoint should exist"
    );
}

#[tokio::test]
async fn test_websocket_receives_pty_output() {
    let (input_tx, _input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let output_tx = broker.sender();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    let session = Session {
        name: "test".to_string(),
        pid: None,
        command: "test".to_string(),
        client_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        input_tx,
        output_rx: output_tx.clone(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: std::sync::Arc::new(wsh::pty::Pty::spawn(24, 80, wsh::pty::SpawnCommand::default()).expect("failed to spawn PTY for test")),
        terminal_size: wsh::terminal::TerminalSize::new(24, 80),
        activity: wsh::activity::ActivityTracker::new(),
        focus: FocusTracker::new(),
        detach_signal: tokio::sync::broadcast::channel::<()>(1).0,
        visual_update_tx: tokio::sync::broadcast::channel::<wsh::protocol::VisualUpdate>(16).0,
        screen_mode: std::sync::Arc::new(parking_lot::RwLock::new(wsh::overlay::ScreenMode::Normal)),
    };
    let registry = SessionRegistry::new();
    registry.insert(Some("test".into()), session).unwrap();
    let state = AppState { sessions: registry, shutdown: ShutdownCoordinator::new(), server_config: std::sync::Arc::new(wsh::api::ServerConfig::new(false)) };
    let app = router(state, None);

    let addr = start_test_server(app).await;
    let ws_url = format!("ws://{}/sessions/test/ws/raw", addr);

    // Connect WebSocket client
    let (mut ws_stream, _response) = connect_async(&ws_url)
        .await
        .expect("Failed to connect WebSocket");

    // Give the connection a moment to establish
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Simulate PTY output by publishing to the broadcast channel
    let test_output = Bytes::from("PTY output test data");
    output_tx
        .send(test_output.clone())
        .expect("Failed to send to broadcast channel");

    // Receive the message on the WebSocket
    let received = tokio::time::timeout(Duration::from_secs(2), ws_stream.next())
        .await
        .expect("timed out waiting for WebSocket message")
        .expect("WebSocket stream ended")
        .expect("WebSocket error");

    match received {
        Message::Binary(data) => {
            assert_eq!(data, test_output.to_vec(), "Received data mismatch");
        }
        other => panic!("Expected binary message, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_websocket_sends_input_to_pty() {
    let (input_tx, mut input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    let session = Session {
        name: "test".to_string(),
        pid: None,
        command: "test".to_string(),
        client_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        input_tx,
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: std::sync::Arc::new(wsh::pty::Pty::spawn(24, 80, wsh::pty::SpawnCommand::default()).expect("failed to spawn PTY for test")),
        terminal_size: wsh::terminal::TerminalSize::new(24, 80),
        activity: wsh::activity::ActivityTracker::new(),
        focus: FocusTracker::new(),
        detach_signal: tokio::sync::broadcast::channel::<()>(1).0,
        visual_update_tx: tokio::sync::broadcast::channel::<wsh::protocol::VisualUpdate>(16).0,
        screen_mode: std::sync::Arc::new(parking_lot::RwLock::new(wsh::overlay::ScreenMode::Normal)),
    };
    let registry = SessionRegistry::new();
    registry.insert(Some("test".into()), session).unwrap();
    let state = AppState { sessions: registry, shutdown: ShutdownCoordinator::new(), server_config: std::sync::Arc::new(wsh::api::ServerConfig::new(false)) };
    let app = router(state, None);

    let addr = start_test_server(app).await;
    let ws_url = format!("ws://{}/sessions/test/ws/raw", addr);

    // Connect WebSocket client
    let (mut ws_stream, _response) = connect_async(&ws_url)
        .await
        .expect("Failed to connect WebSocket");

    // Give the connection a moment to establish
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send input via WebSocket
    let test_input = b"WebSocket input test";
    ws_stream
        .send(Message::Binary(test_input.to_vec()))
        .await
        .expect("Failed to send WebSocket message");

    // Verify the input was forwarded to the channel
    let received = tokio::time::timeout(Duration::from_secs(2), input_rx.recv())
        .await
        .expect("timed out waiting for input on channel")
        .expect("channel closed unexpectedly");

    assert_eq!(received.as_ref(), test_input);
}

#[tokio::test]
async fn test_websocket_text_input_to_pty() {
    // Test that text messages are also handled
    let (input_tx, mut input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    let session = Session {
        name: "test".to_string(),
        pid: None,
        command: "test".to_string(),
        client_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        input_tx,
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: std::sync::Arc::new(wsh::pty::Pty::spawn(24, 80, wsh::pty::SpawnCommand::default()).expect("failed to spawn PTY for test")),
        terminal_size: wsh::terminal::TerminalSize::new(24, 80),
        activity: wsh::activity::ActivityTracker::new(),
        focus: FocusTracker::new(),
        detach_signal: tokio::sync::broadcast::channel::<()>(1).0,
        visual_update_tx: tokio::sync::broadcast::channel::<wsh::protocol::VisualUpdate>(16).0,
        screen_mode: std::sync::Arc::new(parking_lot::RwLock::new(wsh::overlay::ScreenMode::Normal)),
    };
    let registry = SessionRegistry::new();
    registry.insert(Some("test".into()), session).unwrap();
    let state = AppState { sessions: registry, shutdown: ShutdownCoordinator::new(), server_config: std::sync::Arc::new(wsh::api::ServerConfig::new(false)) };
    let app = router(state, None);

    let addr = start_test_server(app).await;
    let ws_url = format!("ws://{}/sessions/test/ws/raw", addr);

    let (mut ws_stream, _response) = connect_async(&ws_url)
        .await
        .expect("Failed to connect WebSocket");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send text input via WebSocket
    let test_text = "text message input";
    ws_stream
        .send(Message::Text(test_text.to_string()))
        .await
        .expect("Failed to send WebSocket text message");

    // Verify the input was forwarded to the channel
    let received = tokio::time::timeout(Duration::from_secs(2), input_rx.recv())
        .await
        .expect("timed out waiting for input on channel")
        .expect("channel closed unexpectedly");

    assert_eq!(String::from_utf8_lossy(&received), test_text);
}

#[tokio::test]
async fn test_websocket_bidirectional_communication() {
    // Test that WebSocket can both send and receive simultaneously
    let (input_tx, mut input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let output_tx = broker.sender();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    let session = Session {
        name: "test".to_string(),
        pid: None,
        command: "test".to_string(),
        client_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        input_tx,
        output_rx: output_tx.clone(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: std::sync::Arc::new(wsh::pty::Pty::spawn(24, 80, wsh::pty::SpawnCommand::default()).expect("failed to spawn PTY for test")),
        terminal_size: wsh::terminal::TerminalSize::new(24, 80),
        activity: wsh::activity::ActivityTracker::new(),
        focus: FocusTracker::new(),
        detach_signal: tokio::sync::broadcast::channel::<()>(1).0,
        visual_update_tx: tokio::sync::broadcast::channel::<wsh::protocol::VisualUpdate>(16).0,
        screen_mode: std::sync::Arc::new(parking_lot::RwLock::new(wsh::overlay::ScreenMode::Normal)),
    };
    let registry = SessionRegistry::new();
    registry.insert(Some("test".into()), session).unwrap();
    let state = AppState { sessions: registry, shutdown: ShutdownCoordinator::new(), server_config: std::sync::Arc::new(wsh::api::ServerConfig::new(false)) };
    let app = router(state, None);

    let addr = start_test_server(app).await;
    let ws_url = format!("ws://{}/sessions/test/ws/raw", addr);

    let (mut ws_stream, _response) = connect_async(&ws_url)
        .await
        .expect("Failed to connect WebSocket");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send input via WebSocket
    let test_input = b"bidirectional input";
    ws_stream
        .send(Message::Binary(test_input.to_vec()))
        .await
        .expect("Failed to send WebSocket message");

    // Simulate PTY output
    let test_output = Bytes::from("bidirectional output");
    output_tx
        .send(test_output.clone())
        .expect("Failed to send broadcast");

    // Verify input was received on the channel
    let received_input = tokio::time::timeout(Duration::from_secs(2), input_rx.recv())
        .await
        .expect("timed out waiting for input")
        .expect("channel closed");
    assert_eq!(received_input.as_ref(), test_input);

    // Verify output was received on WebSocket
    let received_output = tokio::time::timeout(Duration::from_secs(2), ws_stream.next())
        .await
        .expect("timed out waiting for WebSocket message")
        .expect("WebSocket stream ended")
        .expect("WebSocket error");

    match received_output {
        Message::Binary(data) => {
            assert_eq!(data, test_output.to_vec());
        }
        other => panic!("Expected binary message, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_websocket_multiple_outputs() {
    // Test that multiple PTY outputs are all received by WebSocket
    let (input_tx, _input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let output_tx = broker.sender();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    let session = Session {
        name: "test".to_string(),
        pid: None,
        command: "test".to_string(),
        client_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        input_tx,
        output_rx: output_tx.clone(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: std::sync::Arc::new(wsh::pty::Pty::spawn(24, 80, wsh::pty::SpawnCommand::default()).expect("failed to spawn PTY for test")),
        terminal_size: wsh::terminal::TerminalSize::new(24, 80),
        activity: wsh::activity::ActivityTracker::new(),
        focus: FocusTracker::new(),
        detach_signal: tokio::sync::broadcast::channel::<()>(1).0,
        visual_update_tx: tokio::sync::broadcast::channel::<wsh::protocol::VisualUpdate>(16).0,
        screen_mode: std::sync::Arc::new(parking_lot::RwLock::new(wsh::overlay::ScreenMode::Normal)),
    };
    let registry = SessionRegistry::new();
    registry.insert(Some("test".into()), session).unwrap();
    let state = AppState { sessions: registry, shutdown: ShutdownCoordinator::new(), server_config: std::sync::Arc::new(wsh::api::ServerConfig::new(false)) };
    let app = router(state, None);

    let addr = start_test_server(app).await;
    let ws_url = format!("ws://{}/sessions/test/ws/raw", addr);

    let (mut ws_stream, _response) = connect_async(&ws_url)
        .await
        .expect("Failed to connect WebSocket");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send multiple outputs
    let outputs = vec![
        Bytes::from("first output"),
        Bytes::from("second output"),
        Bytes::from("third output"),
    ];

    for output in &outputs {
        output_tx.send(output.clone()).expect("Failed to send");
    }

    // Receive all outputs
    for expected in outputs {
        let received = tokio::time::timeout(Duration::from_secs(2), ws_stream.next())
            .await
            .expect("timed out waiting for WebSocket message")
            .expect("WebSocket stream ended")
            .expect("WebSocket error");

        match received {
            Message::Binary(data) => {
                assert_eq!(data, expected.to_vec());
            }
            other => panic!("Expected binary message, got: {:?}", other),
        }
    }
}

#[tokio::test]
async fn test_nonexistent_route_returns_404() {
    let (app, _input_rx, _output_tx) = create_test_app();

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
async fn test_input_wrong_method_returns_error() {
    let (app, _input_rx, _output_tx) = create_test_app();

    // GET on /input should fail (only POST is allowed)
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/sessions/test/input")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn test_health_wrong_method_returns_error() {
    let (app, _input_rx, _output_tx) = create_test_app();

    // POST on /health should fail (only GET is allowed)
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn test_websocket_line_event_includes_total_lines() {
    // Setup similar to other WebSocket tests
    let (input_tx, _input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let output_tx = broker.sender();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    let session = Session {
        name: "test".to_string(),
        pid: None,
        command: "test".to_string(),
        client_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        input_tx,
        output_rx: output_tx.clone(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: std::sync::Arc::new(wsh::pty::Pty::spawn(24, 80, wsh::pty::SpawnCommand::default()).expect("failed to spawn PTY for test")),
        terminal_size: wsh::terminal::TerminalSize::new(24, 80),
        activity: wsh::activity::ActivityTracker::new(),
        focus: FocusTracker::new(),
        detach_signal: tokio::sync::broadcast::channel::<()>(1).0,
        visual_update_tx: tokio::sync::broadcast::channel::<wsh::protocol::VisualUpdate>(16).0,
        screen_mode: std::sync::Arc::new(parking_lot::RwLock::new(wsh::overlay::ScreenMode::Normal)),
    };
    let registry = SessionRegistry::new();
    registry.insert(Some("test".into()), session).unwrap();
    let state = AppState { sessions: registry, shutdown: ShutdownCoordinator::new(), server_config: std::sync::Arc::new(wsh::api::ServerConfig::new(false)) };
    let app = router(state, None);

    let addr = start_test_server(app).await;
    let ws_url = format!("ws://{}/sessions/test/ws/json", addr);

    // Connect WebSocket client
    let (mut ws_stream, _response) = connect_async(&ws_url)
        .await
        .expect("Failed to connect WebSocket");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Read "connected" message
    let _ = ws_stream.next().await;

    // Subscribe to lines (using new unified protocol)
    let subscribe_msg = serde_json::json!({"method": "subscribe", "params": {"events": ["lines"]}});
    ws_stream
        .send(Message::Text(subscribe_msg.to_string()))
        .await
        .unwrap();

    // Read subscribe response
    let _ = ws_stream.next().await;
    // Read sync event
    let _ = ws_stream.next().await;

    // Publish text to trigger line events
    output_tx
        .send(bytes::Bytes::from("Hello test\r\n"))
        .unwrap();

    // Look for a line event with total_lines field
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut found_total_lines = false;

    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(msg))) =
            tokio::time::timeout(Duration::from_millis(200), ws_stream.next()).await
        {
            if let Message::Text(text) = msg {
                let json: serde_json::Value = serde_json::from_str(&text).unwrap();
                if json.get("event") == Some(&serde_json::json!("line")) {
                    assert!(
                        json.get("total_lines").is_some(),
                        "line event should have total_lines"
                    );
                    assert!(json.get("index").is_some(), "line event should have index");
                    found_total_lines = true;
                    break;
                }
            }
        }
    }

    assert!(
        found_total_lines,
        "should have received a line event with total_lines"
    );
}

/// Test scrollback endpoint returns correct data
#[tokio::test]
async fn test_scrollback_endpoint() {
    let (input_tx, _input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let output_tx = broker.sender();
    let parser = Parser::spawn(&broker, 80, 5, 1000); // 5-row screen to get scrollback quickly
    let session = Session {
        name: "test".to_string(),
        pid: None,
        command: "test".to_string(),
        client_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        input_tx,
        output_rx: output_tx.clone(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: std::sync::Arc::new(wsh::pty::Pty::spawn(5, 80, wsh::pty::SpawnCommand::default()).expect("failed to spawn PTY for test")),
        terminal_size: wsh::terminal::TerminalSize::new(5, 80),
        activity: wsh::activity::ActivityTracker::new(),
        focus: FocusTracker::new(),
        detach_signal: tokio::sync::broadcast::channel::<()>(1).0,
        visual_update_tx: tokio::sync::broadcast::channel::<wsh::protocol::VisualUpdate>(16).0,
        screen_mode: std::sync::Arc::new(parking_lot::RwLock::new(wsh::overlay::ScreenMode::Normal)),
    };
    let registry = SessionRegistry::new();
    registry.insert(Some("test".into()), session).unwrap();
    let state = AppState { sessions: registry, shutdown: ShutdownCoordinator::new(), server_config: std::sync::Arc::new(wsh::api::ServerConfig::new(false)) };
    let app = router(state, None);

    // Send enough lines to create scrollback (more than 5 rows)
    for i in 0..20 {
        output_tx
            .send(bytes::Bytes::from(format!("Line {}\r\n", i)))
            .expect("Failed to send");
    }

    // Wait for parser to process
    tokio::time::sleep(Duration::from_millis(100)).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions/test/scrollback?format=plain")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Should have scrollback lines
    let total_lines = json["total_lines"].as_u64().unwrap_or(0);
    let lines = json["lines"].as_array().map(|a| a.len()).unwrap_or(0);

    // With 20 lines sent and 5 rows visible, we expect ~15 lines of scrollback
    assert!(total_lines > 0, "Expected total_lines > 0, got {}", total_lines);
    assert!(lines > 0, "Expected lines.len > 0, got {}", lines);
}

/// Test that scrollback initially contains the blank screen
#[tokio::test]
async fn test_scrollback_initial_state() {
    let (input_tx, _input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    let session = Session {
        name: "test".to_string(),
        pid: None,
        command: "test".to_string(),
        client_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        input_tx,
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: std::sync::Arc::new(wsh::pty::Pty::spawn(24, 80, wsh::pty::SpawnCommand::default()).expect("failed to spawn PTY for test")),
        terminal_size: wsh::terminal::TerminalSize::new(24, 80),
        activity: wsh::activity::ActivityTracker::new(),
        focus: FocusTracker::new(),
        detach_signal: tokio::sync::broadcast::channel::<()>(1).0,
        visual_update_tx: tokio::sync::broadcast::channel::<wsh::protocol::VisualUpdate>(16).0,
        screen_mode: std::sync::Arc::new(parking_lot::RwLock::new(wsh::overlay::ScreenMode::Normal)),
    };
    let registry = SessionRegistry::new();
    registry.insert(Some("test".into()), session).unwrap();
    let state = AppState { sessions: registry, shutdown: ShutdownCoordinator::new(), server_config: std::sync::Arc::new(wsh::api::ServerConfig::new(false)) };
    let app = router(state, None);

    // Query immediately without any output
    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions/test/scrollback?format=plain")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Initially, scrollback contains the blank screen (24 rows)
    // Under the new semantics, scrollback includes current screen content
    let total_lines = json["total_lines"].as_u64().unwrap_or(0);
    assert_eq!(total_lines, 24, "Expected initial screen lines (24 rows), got {}", total_lines);
}
