//! Integration tests for event coalescing under high-throughput output.
mod common;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// Helper: start an axum server on a random port, return the address.
async fn start_server(app: axum::Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// Helper: receive a JSON message from a WS stream with timeout.
async fn recv_json(
    rx: &mut futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) -> serde_json::Value {
    let timeout = Duration::from_secs(5);
    match tokio::time::timeout(timeout, rx.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected text message, got {:?}", other),
    }
}

/// Create a test state with a large parser channel to allow burst sending
/// without the test task blocking on the bounded parser_tx channel.
fn create_burst_test_state() -> (wsh::api::AppState, mpsc::Receiver<Bytes>, tokio::sync::broadcast::Sender<Bytes>, mpsc::Sender<Bytes>) {
    // Use a large parser channel (8192) so we can send thousands of lines
    // without the test task blocking — the default 256 is too small.
    let ts = common::create_test_session("test");
    let output_tx = ts.broker.sender();

    // Create a new parser with a large channel instead of the default 256
    let (large_parser_tx, large_parser_rx) = mpsc::channel(8192);
    let parser = wsh::parser::Parser::spawn(large_parser_rx, 80, 24, 1000);

    // Build session with the large-channel parser
    let session = wsh::session::Session {
        parser,
        ..ts.session
    };

    let registry = wsh::session::SessionRegistry::new();
    registry.insert(Some("test".into()), session).unwrap();
    let state = wsh::api::AppState {
        sessions: registry,
        shutdown: wsh::shutdown::ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(wsh::api::ServerConfig::new(false)),
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
        tcp_addr: None,
        instance_name: "test".to_string(),
        http_socket_path: std::path::PathBuf::from("/tmp/test.http.sock"),
    };
    (state, ts.input_rx, output_tx, large_parser_tx)
}

/// Blast events through the parser and verify that the server-level WS
/// coalesces into Sync events rather than producing lagged notifications.
#[tokio::test]
async fn test_server_ws_coalescing_under_burst() {
    let (state, _input_rx, _output_tx, parser_tx) = create_burst_test_state();
    let app = wsh::api::router(state, wsh::api::RouterConfig::default());
    let addr = start_server(app).await;

    // Connect to the server-level WS
    let (ws, _) = connect_async(format!("ws://{}/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    let _ = recv_json(&mut rx).await; // connected

    // Subscribe with a short interval to trigger coalescing quickly
    tx.send(Message::Text(
        serde_json::json!({
            "method": "subscribe",
            "session": "test",
            "params": {
                "events": ["lines", "diffs"],
                "format": "plain",
                "interval_ms": 50
            }
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();

    // Consume subscribe response + initial sync
    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["method"], "subscribe");
    let sync = recv_json(&mut rx).await;
    assert_eq!(sync["event"], "sync");

    // Blast 5000 lines through the parser — way more than the mpsc can handle.
    // The large parser channel (8192) prevents the test task from blocking.
    for i in 0..5000 {
        let line = format!("line {}\r\n", i);
        let _ = parser_tx.send(Bytes::from(line)).await;
    }

    // Collect events for 2 seconds
    let mut sync_count = 0u32;
    let mut lagged_count = 0u32;
    let mut event_count = 0u32;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), rx.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let json: serde_json::Value = serde_json::from_str(&text).unwrap();
                event_count += 1;
                if json.get("event") == Some(&serde_json::json!("sync")) {
                    sync_count += 1;
                }
                if json.get("type") == Some(&serde_json::json!("lagged")) {
                    lagged_count += 1;
                }
            }
            _ => break,
        }
    }

    // Coalescing should produce Sync snapshots. The non-blocking drain
    // prevents broadcast overflow, so lagged_count should be zero.
    assert!(
        sync_count >= 1,
        "expected at least 1 coalesced sync event, got {sync_count} (total events: {event_count})"
    );
    assert!(
        lagged_count == 0,
        "expected no lagged notifications with coalescing, got {lagged_count}"
    );
}

/// Blast events through the parser and verify that the per-session WS
/// coalesces into Sync events rather than producing lagged notifications.
#[tokio::test]
async fn test_per_session_ws_coalescing_under_burst() {
    let (state, _input_rx, _output_tx, parser_tx) = create_burst_test_state();
    let app = wsh::api::router(state, wsh::api::RouterConfig::default());
    let addr = start_server(app).await;

    // Connect to the per-session WS
    let (ws, _) = connect_async(format!("ws://{}/sessions/test/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    let _ = recv_json(&mut rx).await; // connected

    // Subscribe with short interval
    tx.send(Message::Text(
        serde_json::json!({
            "method": "subscribe",
            "params": {
                "events": ["lines", "diffs"],
                "format": "plain",
                "interval_ms": 50
            }
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["method"], "subscribe");
    let sync = recv_json(&mut rx).await;
    assert_eq!(sync["event"], "sync");

    // Blast 5000 lines
    for i in 0..5000 {
        let line = format!("line {}\r\n", i);
        let _ = parser_tx.send(Bytes::from(line)).await;
    }

    // Collect events for 2 seconds
    let mut sync_count = 0u32;
    let mut lagged_count = 0u32;
    let mut event_count = 0u32;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), rx.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let json: serde_json::Value = serde_json::from_str(&text).unwrap();
                event_count += 1;
                if json.get("event") == Some(&serde_json::json!("sync")) {
                    sync_count += 1;
                }
                if json.get("type") == Some(&serde_json::json!("lagged")) {
                    lagged_count += 1;
                }
            }
            _ => break,
        }
    }

    assert!(
        sync_count >= 1,
        "expected at least 1 coalesced sync event, got {sync_count} (total events: {event_count})"
    );
    assert!(
        lagged_count == 0,
        "expected no lagged notifications with coalescing, got {lagged_count}"
    );
}
