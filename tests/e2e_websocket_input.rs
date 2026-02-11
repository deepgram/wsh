//! End-to-end test for WebSocket input -> PTY data flow.
//!
//! This test verifies that input sent via WebSocket correctly reaches the PTY
//! and produces output that is broadcast back.

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use wsh::{api, broker::Broker, input::{InputBroadcaster, InputMode}, overlay::OverlayStore, parser::Parser, pty::{Pty, SpawnCommand}, session::{Session, SessionRegistry}, shutdown::ShutdownCoordinator};

async fn start_server(app: axum::Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

/// Full E2E test: WebSocket input -> PTY -> output broadcast back via WebSocket
#[tokio::test(flavor = "multi_thread")]
async fn test_websocket_input_reaches_pty_and_output_returns() {
    let pty = Arc::new(Pty::spawn(24, 80, SpawnCommand::default()).expect("Failed to spawn PTY"));
    let mut pty_reader = pty.take_reader().expect("Failed to get reader");
    let mut pty_writer = pty.take_writer().expect("Failed to get writer");

    let broker = Broker::new();
    let broker_clone = broker.clone();

    let (input_tx, mut input_rx) = mpsc::channel::<Bytes>(64);

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_reader = stop_flag.clone();

    // PTY reader
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 4096];
        while !stop_flag_reader.load(Ordering::Relaxed) {
            match pty_reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    broker_clone.publish(Bytes::copy_from_slice(&buf[..n]));
                }
                Err(e) => {
                    if e.raw_os_error() != Some(5) {
                        eprintln!("[PTY Reader] Error: {:?}", e);
                    }
                    break;
                }
            }
        }
    });

    // PTY writer
    tokio::task::spawn_blocking(move || {
        while let Some(data) = input_rx.blocking_recv() {
            let _ = pty_writer.write_all(&data);
            let _ = pty_writer.flush();
        }
    });

    let parser = Parser::spawn(&broker, 80, 24, 1000);
    let session = Session {
        name: "test".to_string(),
        input_tx: input_tx.clone(),
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: pty.clone(),
        terminal_size: wsh::terminal::TerminalSize::new(24, 80),
        activity: wsh::activity::ActivityTracker::new(),
        is_local: false,
    };
    let registry = SessionRegistry::new();
    registry.insert(Some("test".into()), session).unwrap();
    let state = api::AppState {
        sessions: registry,
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(api::ServerConfig::new(false)),
    };
    let app = api::router(state, None);
    let addr = start_server(app).await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Connect WebSocket
    let ws_url = format!("ws://{}/sessions/test/ws/raw", addr);
    let (mut ws_stream, _response) = connect_async(&ws_url)
        .await
        .expect("Failed to connect WebSocket");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send command via WebSocket
    let marker = "WS_E2E_TEST_77777";
    let cmd = format!("echo {}\n", marker);

    ws_stream
        .send(Message::Binary(cmd.as_bytes().to_vec()))
        .await
        .expect("Failed to send WebSocket message");

    // Wait for output to come back
    let mut collected = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    loop {
        if tokio::time::Instant::now() >= deadline {
            break;
        }

        tokio::select! {
            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        collected.extend_from_slice(&data);
                        if String::from_utf8_lossy(&collected).contains(marker) {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        collected.extend_from_slice(text.as_bytes());
                        if String::from_utf8_lossy(&collected).contains(marker) {
                            break;
                        }
                    }
                    Some(Err(_)) | None => break,
                    _ => {}
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }

    let output = String::from_utf8_lossy(&collected);

    // Cleanup
    let _ = input_tx.send(Bytes::from("exit\n")).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    stop_flag.store(true, Ordering::Relaxed);
    drop(input_tx);
    let _ = ws_stream.close(None).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(
        output.contains(marker),
        "Expected WebSocket output to contain '{}', but got:\n{}",
        marker,
        output
    );
}

/// Test sending Text message (like websocat does by default)
#[tokio::test(flavor = "multi_thread")]
async fn test_websocket_text_input_reaches_pty() {
    let pty = Arc::new(Pty::spawn(24, 80, SpawnCommand::default()).expect("Failed to spawn PTY"));
    let mut pty_reader = pty.take_reader().expect("Failed to get reader");
    let mut pty_writer = pty.take_writer().expect("Failed to get writer");

    let broker = Broker::new();
    let broker_clone = broker.clone();

    let (input_tx, mut input_rx) = mpsc::channel::<Bytes>(64);

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_reader = stop_flag.clone();

    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 4096];
        while !stop_flag_reader.load(Ordering::Relaxed) {
            match pty_reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    broker_clone.publish(Bytes::copy_from_slice(&buf[..n]));
                }
                Err(e) => {
                    if e.raw_os_error() != Some(5) {
                        eprintln!("[PTY Reader] Error: {:?}", e);
                    }
                    break;
                }
            }
        }
    });

    tokio::task::spawn_blocking(move || {
        while let Some(data) = input_rx.blocking_recv() {
            let _ = pty_writer.write_all(&data);
            let _ = pty_writer.flush();
        }
    });

    let parser = Parser::spawn(&broker, 80, 24, 1000);
    let session = Session {
        name: "test".to_string(),
        input_tx: input_tx.clone(),
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: pty.clone(),
        terminal_size: wsh::terminal::TerminalSize::new(24, 80),
        activity: wsh::activity::ActivityTracker::new(),
        is_local: false,
    };
    let registry = SessionRegistry::new();
    registry.insert(Some("test".into()), session).unwrap();
    let state = api::AppState {
        sessions: registry,
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(api::ServerConfig::new(false)),
    };
    let app = api::router(state, None);
    let addr = start_server(app).await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    let ws_url = format!("ws://{}/sessions/test/ws/raw", addr);
    let (mut ws_stream, _) = connect_async(&ws_url).await.expect("WebSocket connect failed");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send as TEXT (this is what websocat does by default)
    let marker = "WS_TEXT_TEST_88888";
    let cmd = format!("echo {}\n", marker);

    ws_stream
        .send(Message::Text(cmd))
        .await
        .expect("Failed to send WebSocket text");

    // Wait for output
    let mut collected = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    loop {
        if tokio::time::Instant::now() >= deadline {
            break;
        }

        tokio::select! {
            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        collected.extend_from_slice(&data);
                        if String::from_utf8_lossy(&collected).contains(marker) {
                            break;
                        }
                    }
                    Some(Err(_)) | None => break,
                    _ => {}
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }

    let output = String::from_utf8_lossy(&collected);

    let _ = input_tx.send(Bytes::from("exit\n")).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    stop_flag.store(true, Ordering::Relaxed);
    drop(input_tx);
    let _ = ws_stream.close(None).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(
        output.contains(marker),
        "Expected output to contain '{}', but got:\n{}",
        marker,
        output
    );
}
