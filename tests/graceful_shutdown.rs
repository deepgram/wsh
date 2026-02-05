//! Integration test for graceful WebSocket shutdown.
//!
//! This test verifies that when wsh exits, WebSocket clients receive a proper
//! close frame rather than experiencing an I/O error from a dropped connection.

use futures::{SinkExt, StreamExt};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Write;
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

#[tokio::test]
async fn test_websocket_receives_close_frame_on_shutdown() {
    // Find an available port
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    // 1. Spawn wsh inside a PTY (it requires a TTY for raw mode)
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("Failed to open PTY");

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_wsh"));
    cmd.arg("--bind");
    cmd.arg(format!("127.0.0.1:{}", port));

    let mut child = pair.slave.spawn_command(cmd).expect("Failed to spawn wsh");
    let mut writer = pair.master.take_writer().expect("Failed to get PTY writer");

    // 2. Wait for server to be ready
    wait_for_ready(port)
        .await
        .expect("wsh should become ready");

    // 3. Connect WebSocket client
    let ws_url = format!("ws://127.0.0.1:{}/ws/json", port);
    let (ws_stream, _response) = timeout(WS_CONNECT_TIMEOUT, connect_async(&ws_url))
        .await
        .expect("WebSocket connect should not timeout")
        .expect("WebSocket connect should succeed");

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // 4. Read the "connected" message
    let msg = timeout(Duration::from_secs(1), ws_rx.next())
        .await
        .expect("should receive connected message in time")
        .expect("stream should have message")
        .expect("message should be valid");
    assert!(matches!(msg, Message::Text(_)), "expected text message");

    // 5. Send subscribe message
    let subscribe = serde_json::json!({"events": ["lines"]});
    ws_tx
        .send(Message::Text(subscribe.to_string()))
        .await
        .expect("should send subscribe");

    // 6. Read the "subscribed" confirmation
    let msg = timeout(Duration::from_secs(1), ws_rx.next())
        .await
        .expect("should receive subscribed message in time")
        .expect("stream should have message")
        .expect("message should be valid");
    assert!(matches!(msg, Message::Text(_)), "expected text message");

    // 7. Send "exit" to the shell inside wsh, triggering wsh shutdown
    writer.write_all(b"exit\n").expect("write exit command");
    drop(writer); // Close the PTY writer

    // 8. Read from WebSocket - we should get a Close frame, not an error
    let result = timeout(SHUTDOWN_TIMEOUT, ws_rx.next()).await;

    match result {
        Ok(Some(Ok(Message::Close(frame)))) => {
            // SUCCESS: We received a proper close frame
            println!("Received close frame: {:?}", frame);
            if let Some(f) = frame {
                assert_eq!(
                    f.code,
                    tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
                    "expected normal close code"
                );
            }
        }
        Ok(Some(Ok(other))) => {
            // We got some other message - might be line events, keep reading
            println!("Got message while waiting for close: {:?}", other);

            // Keep reading until we get Close or error
            loop {
                let inner_result = timeout(SHUTDOWN_TIMEOUT, ws_rx.next()).await;
                match inner_result {
                    Ok(Some(Ok(Message::Close(frame)))) => {
                        println!("Received close frame: {:?}", frame);
                        if let Some(f) = frame {
                            assert_eq!(
                                f.code,
                                tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
                                "expected normal close code"
                            );
                        }
                        break;
                    }
                    Ok(Some(Ok(msg))) => {
                        println!("Got another message: {:?}", msg);
                        continue;
                    }
                    Ok(Some(Err(e))) => {
                        panic!("BUG: WebSocket error instead of close frame: {:?}", e);
                    }
                    Ok(None) => {
                        panic!("BUG: WebSocket stream ended without close frame");
                    }
                    Err(_) => {
                        panic!("BUG: Timeout waiting for close frame");
                    }
                }
            }
        }
        Ok(Some(Err(e))) => {
            panic!("BUG: WebSocket error instead of close frame: {:?}", e);
        }
        Ok(None) => {
            panic!("BUG: WebSocket stream ended without close frame");
        }
        Err(_) => {
            panic!("BUG: Timeout waiting for WebSocket close frame - wsh may have hung");
        }
    }

    // 9. Wait for wsh to exit (with timeout)
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

/// Helper to wait for a close frame, consuming any other messages first.
async fn expect_close_frame(ws_rx: &mut futures::stream::SplitStream<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>>) {
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
                panic!("BUG: WebSocket stream ended without close frame");
            }
            Err(_) => {
                panic!("BUG: Timeout waiting for close frame");
            }
        }
    }
}

/// Test that unsubscribed clients also receive a close frame on shutdown.
#[tokio::test]
async fn test_unsubscribed_websocket_receives_close_frame_on_shutdown() {
    // Find an available port
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    // 1. Spawn wsh inside a PTY
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("Failed to open PTY");

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_wsh"));
    cmd.arg("--bind");
    cmd.arg(format!("127.0.0.1:{}", port));

    let mut child = pair.slave.spawn_command(cmd).expect("Failed to spawn wsh");
    let mut writer = pair.master.take_writer().expect("Failed to get PTY writer");

    // 2. Wait for server to be ready
    wait_for_ready(port)
        .await
        .expect("wsh should become ready");

    // 3. Connect WebSocket client
    let ws_url = format!("ws://127.0.0.1:{}/ws/json", port);
    let (ws_stream, _response) = timeout(WS_CONNECT_TIMEOUT, connect_async(&ws_url))
        .await
        .expect("WebSocket connect should not timeout")
        .expect("WebSocket connect should succeed");

    let (_ws_tx, mut ws_rx) = ws_stream.split();

    // 4. Read the "connected" message
    let msg = timeout(Duration::from_secs(1), ws_rx.next())
        .await
        .expect("should receive connected message in time")
        .expect("stream should have message")
        .expect("message should be valid");
    assert!(matches!(msg, Message::Text(_)), "expected text message");

    // 5. DO NOT subscribe - trigger shutdown immediately
    writer.write_all(b"exit\n").expect("write exit command");
    drop(writer);

    // 6. Should still receive a close frame
    expect_close_frame(&mut ws_rx).await;

    // 7. Wait for wsh to exit
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
