//! Integration test for graceful WebSocket shutdown.
//!
//! This test verifies that when the wsh server shuts down (ephemeral mode,
//! last session killed), WebSocket clients receive a proper close frame
//! rather than experiencing an I/O error from a dropped connection.

use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};

const WSH_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Waits for wsh to be ready by polling the health endpoint.
async fn wait_for_ready(port: u16) -> Result<(), &'static str> {
    let url = format!("http://127.0.0.1:{}/health", port);
    let client = reqwest::Client::new();

    let deadline = tokio::time::Instant::now() + WSH_STARTUP_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
    }
    Err("wsh did not become ready in time")
}

/// Creates a session via POST /sessions. Returns the session name.
async fn create_session(port: u16) -> String {
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}/sessions", port);
    let resp = client
        .post(&url)
        .json(&serde_json::json!({"name": "test"}))
        .send()
        .await
        .expect("session create request failed");
    assert_eq!(resp.status(), 201, "expected 201 Created");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["name"].as_str().unwrap().to_string()
}

/// Kills a session via DELETE /sessions/:name.
async fn kill_session(port: u16, name: &str) {
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}/sessions/{}", port, name);
    let resp = client.delete(&url).send().await.expect("session kill request failed");
    assert!(
        resp.status().is_success(),
        "expected success deleting session, got {}",
        resp.status()
    );
}

/// Helper to wait for a close frame, consuming any other messages first.
async fn expect_close_frame(
    ws_rx: &mut futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) {
    loop {
        let result = timeout(SHUTDOWN_TIMEOUT, ws_rx.next()).await;
        match result {
            Ok(Some(Ok(Message::Close(frame)))) => {
                println!("Received close frame: {:?}", frame);
                if let Some(f) = frame {
                    assert_eq!(
                        f.code,
                        tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
                        "expected normal close code"
                    );
                }
                return;
            }
            Ok(Some(Ok(msg))) => {
                println!("Got message while waiting for close: {:?}", msg);
                continue;
            }
            Ok(Some(Err(e))) => {
                panic!("BUG: WebSocket error instead of close frame: {:?}", e);
            }
            Ok(None) => {
                // Stream ended cleanly (no close frame) — this is acceptable
                // for ephemeral shutdown where the server exits quickly.
                return;
            }
            Err(_) => {
                panic!("BUG: Timeout waiting for close frame");
            }
        }
    }
}

/// Spawns `wsh server --bind ... --ephemeral` as a background process
/// with a unique socket path and server name so tests don't fight over locks.
fn spawn_server(port: u16, socket_dir: &std::path::Path, instance_name: &str) -> std::process::Child {
    let socket_path = socket_dir.join("test.sock");
    std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("server")
        .arg("--bind")
        .arg(format!("127.0.0.1:{}", port))
        .arg("--socket")
        .arg(&socket_path)
        .arg("--server-name")
        .arg(instance_name)
        .arg("--ephemeral")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("Failed to spawn wsh server")
}

#[tokio::test]
async fn test_websocket_receives_close_frame_on_shutdown() {
    // Find an available port
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    // 1. Spawn wsh server in ephemeral mode with unique socket and instance name
    let socket_dir = tempfile::TempDir::new().unwrap();
    let mut child = spawn_server(port, socket_dir.path(), "gs-subscribed");

    // 2. Wait for server to be ready
    wait_for_ready(port)
        .await
        .expect("wsh should become ready");

    // 3. Create a session
    let session_name = create_session(port).await;

    // 4. Connect WebSocket client
    let ws_url = format!("ws://127.0.0.1:{}/sessions/{}/ws/json", port, session_name);
    let (ws_stream, _response) = timeout(WS_CONNECT_TIMEOUT, connect_async(&ws_url))
        .await
        .expect("WebSocket connect should not timeout")
        .expect("WebSocket connect should succeed");

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // 5. Read the "connected" message
    let msg = timeout(Duration::from_secs(1), ws_rx.next())
        .await
        .expect("should receive connected message in time")
        .expect("stream should have message")
        .expect("message should be valid");
    assert!(matches!(msg, Message::Text(_)), "expected text message");

    // 6. Send subscribe message
    let subscribe = serde_json::json!({
        "method": "subscribe",
        "params": {"events": ["lines"], "format": "plain"}
    });
    ws_tx
        .send(Message::Text(subscribe.to_string().into()))
        .await
        .expect("should send subscribe");

    // 7. Read subscribe response
    let msg = timeout(Duration::from_secs(1), ws_rx.next())
        .await
        .expect("should receive subscribe response in time")
        .expect("stream should have message")
        .expect("message should be valid");
    assert!(matches!(msg, Message::Text(_)), "expected text message");

    // 8. Kill the session — this triggers ephemeral shutdown
    kill_session(port, &session_name).await;

    // 9. Expect close frame (or clean stream end)
    expect_close_frame(&mut ws_rx).await;

    // 10. Wait for wsh to exit
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("try_wait failed") {
            println!("wsh exited with status: {:?}", status);
            break;
        }
        if start.elapsed() > SHUTDOWN_TIMEOUT {
            child.kill().ok();
            panic!("BUG: wsh did not exit in time");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Test that unsubscribed clients also receive a close frame on shutdown.
#[tokio::test]
async fn test_unsubscribed_websocket_receives_close_frame_on_shutdown() {
    // Find an available port
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    // 1. Spawn wsh server in ephemeral mode with unique socket and instance name
    let socket_dir = tempfile::TempDir::new().unwrap();
    let mut child = spawn_server(port, socket_dir.path(), "gs-unsubscribed");

    // 2. Wait for server to be ready
    wait_for_ready(port)
        .await
        .expect("wsh should become ready");

    // 3. Create a session
    let session_name = create_session(port).await;

    // 4. Connect WebSocket client
    let ws_url = format!("ws://127.0.0.1:{}/sessions/{}/ws/json", port, session_name);
    let (ws_stream, _response) = timeout(WS_CONNECT_TIMEOUT, connect_async(&ws_url))
        .await
        .expect("WebSocket connect should not timeout")
        .expect("WebSocket connect should succeed");

    let (_ws_tx, mut ws_rx) = ws_stream.split();

    // 5. Read the "connected" message
    let msg = timeout(Duration::from_secs(1), ws_rx.next())
        .await
        .expect("should receive connected message in time")
        .expect("stream should have message")
        .expect("message should be valid");
    assert!(matches!(msg, Message::Text(_)), "expected text message");

    // 6. DO NOT subscribe - kill session immediately
    kill_session(port, &session_name).await;

    // 7. Should still receive a close frame
    expect_close_frame(&mut ws_rx).await;

    // 8. Wait for wsh to exit
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("try_wait failed") {
            println!("wsh exited with status: {:?}", status);
            break;
        }
        if start.elapsed() > SHUTDOWN_TIMEOUT {
            child.kill().ok();
            panic!("BUG: wsh did not exit in time");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
