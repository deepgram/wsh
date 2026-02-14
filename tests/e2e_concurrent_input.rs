//! Test for concurrent input from multiple sources.
//!
//! This test verifies that input from stdin, HTTP API, and WebSocket
//! all correctly reach the PTY when sent concurrently.

use bytes::Bytes;
use futures::SinkExt;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use wsh::{api, broker::Broker, input::{FocusTracker, InputBroadcaster, InputMode}, overlay::OverlayStore, parser::Parser, pty::{Pty, SpawnCommand}, session::{Session, SessionRegistry}, shutdown::ShutdownCoordinator};

async fn start_server(app: axum::Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

/// Test that input from multiple sources all reach the PTY correctly.
#[tokio::test(flavor = "multi_thread")]
async fn test_concurrent_input_from_multiple_sources() {
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
        pid: None,
        command: "test".to_string(),
        client_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
    };
    let app = api::router(state, None);
    let addr = start_server(app).await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut rx = broker.subscribe();

    // Simulate "stdin" input
    let stdin_tx = input_tx.clone();

    // WebSocket connection
    let ws_url = format!("ws://{}/sessions/test/ws/raw", addr);
    let (mut ws_stream, _) = connect_async(&ws_url).await.expect("WS connect failed");

    // Markers for each input source
    let stdin_marker = "STDIN_MARKER_111";
    let http_marker = "HTTP_MARKER_222";
    let ws_marker = "WS_MARKER_333";

    // Send commands from all three sources concurrently
    let stdin_fut = {
        let stdin_tx = stdin_tx.clone();
        async move {
            let cmd = format!("echo {}\n", stdin_marker);
            stdin_tx.send(Bytes::from(cmd)).await.expect("stdin send failed");
        }
    };

    let http_fut = {
        async move {
            let cmd = format!("echo {}\n", http_marker);
            let stream = tokio::net::TcpStream::connect(addr).await.expect("connect failed");
            let io = hyper_util::rt::TokioIo::new(stream);
            let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await.expect("handshake failed");
            tokio::spawn(async move { let _ = conn.await; });
            let request = hyper::Request::builder()
                .method("POST")
                .uri("/sessions/test/input")
                .body(http_body_util::Full::new(Bytes::from(cmd)))
                .expect("build request");
            sender.send_request(request).await.expect("send request");
        }
    };

    let ws_fut = async {
        let cmd = format!("echo {}\n", ws_marker);
        ws_stream.send(Message::Text(cmd)).await.expect("ws send failed");
    };

    tokio::join!(stdin_fut, http_fut, ws_fut);

    // Collect output
    let mut collected = Vec::new();
    let mut found_stdin = false;
    let mut found_http = false;
    let mut found_ws = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    while tokio::time::Instant::now() < deadline && (!found_stdin || !found_http || !found_ws) {
        tokio::select! {
            result = rx.recv() => {
                if let Ok(data) = result {
                    collected.extend_from_slice(&data);
                    let output = String::from_utf8_lossy(&collected);
                    if !found_stdin && output.contains(stdin_marker) {
                        found_stdin = true;
                    }
                    if !found_http && output.contains(http_marker) {
                        found_http = true;
                    }
                    if !found_ws && output.contains(ws_marker) {
                        found_ws = true;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
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

    assert!(found_stdin, "stdin marker not found in output:\n{}", output);
    assert!(found_http, "HTTP marker not found in output:\n{}", output);
    assert!(found_ws, "WebSocket marker not found in output:\n{}", output);
}

/// Test rapid sequential HTTP requests all reach the PTY.
#[tokio::test(flavor = "multi_thread")]
async fn test_rapid_http_requests() {
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
                Ok(n) => broker_clone.publish(Bytes::copy_from_slice(&buf[..n])),
                Err(e) => {
                    if e.raw_os_error() != Some(5) {
                        eprintln!("Read error: {:?}", e);
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
        pid: None,
        command: "test".to_string(),
        client_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
    };
    let app = api::router(state, None);
    let addr = start_server(app).await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut rx = broker.subscribe();

    // Send 10 rapid HTTP requests
    let markers: Vec<_> = (0..10).map(|i| format!("RAPID_TEST_{}", i)).collect();

    for marker in &markers {
        let cmd = format!("echo {}\n", marker);
        let stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
        let io = hyper_util::rt::TokioIo::new(stream);
        let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await.expect("handshake");
        tokio::spawn(async move { let _ = conn.await; });
        let request = hyper::Request::builder()
            .method("POST")
            .uri("/sessions/test/input")
            .body(http_body_util::Full::new(Bytes::from(cmd)))
            .expect("request");
        let response = sender.send_request(request).await.expect("send");
        assert_eq!(response.status(), 204, "Expected 204 for marker {}", marker);
    }

    // Collect output
    let mut collected = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);

    while tokio::time::Instant::now() < deadline {
        let output = String::from_utf8_lossy(&collected);
        if markers.iter().all(|m| output.contains(m)) {
            break;
        }

        tokio::select! {
            result = rx.recv() => {
                if let Ok(data) = result {
                    collected.extend_from_slice(&data);
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }

    let output = String::from_utf8_lossy(&collected);

    let _ = input_tx.send(Bytes::from("exit\n")).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    stop_flag.store(true, Ordering::Relaxed);
    drop(input_tx);
    tokio::time::sleep(Duration::from_millis(100)).await;

    for marker in &markers {
        assert!(output.contains(marker), "Marker '{}' not found in output", marker);
    }
}
