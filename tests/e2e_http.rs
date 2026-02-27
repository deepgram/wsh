//! End-to-end test for HTTP API -> PTY data flow.
//!
//! This test starts an actual HTTP server and verifies that POST /input
//! correctly forwards data to a real PTY and we see the output.

use bytes::Bytes;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use wsh::{api, broker::Broker, input::{FocusTracker, InputBroadcaster, InputMode}, overlay::OverlayStore, parser::Parser, pty::{Pty, SpawnCommand}, session::{Session, SessionRegistry}, shutdown::ShutdownCoordinator};

/// Starts an HTTP server and returns its address
async fn start_server(app: axum::Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

/// Full E2E test: HTTP POST /input -> PTY -> broker -> verification
#[tokio::test(flavor = "multi_thread")]
async fn test_http_post_input_reaches_pty_and_produces_output() {
    // === Setup PTY ===
    let pty = Arc::new(parking_lot::Mutex::new(Pty::spawn(24, 80, SpawnCommand::default()).expect("Failed to spawn PTY")));
    let mut pty_reader = pty.lock().take_reader().expect("Failed to get reader");
    let mut pty_writer = pty.lock().take_writer().expect("Failed to get writer");

    let broker = Broker::new();
    let broker_clone = broker.clone();

    let (input_tx, mut input_rx) = mpsc::channel::<Bytes>(64);

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_reader = stop_flag.clone();

    // PTY reader task
    let pty_reader_handle = tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 4096];
        while !stop_flag_reader.load(Ordering::Relaxed) {
            match pty_reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    broker_clone.publish(Bytes::copy_from_slice(&buf[..n]));
                }
                Err(e) => {
                    // EIO (5) is expected when PTY closes
                    if e.raw_os_error() != Some(5) {
                        eprintln!("PTY read error: {:?}", e);
                    }
                    break;
                }
            }
        }
    });

    // PTY writer task
    let pty_writer_handle = tokio::task::spawn_blocking(move || {
        while let Some(data) = input_rx.blocking_recv() {
            if pty_writer.write_all(&data).is_err() {
                break;
            }
            let _ = pty_writer.flush();
        }
    });

    // === Setup HTTP Server ===
    let (_parser_tx, parser_rx) = tokio::sync::mpsc::channel(256);
    let parser = Parser::spawn(parser_rx, 80, 24, 1000);
    let session = Session {
        name: "test".to_string(),
        pid: None,
        command: "test".to_string(),
        client_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        tags: std::sync::Arc::new(parking_lot::RwLock::new(std::collections::HashSet::new())),
        child_exited: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
    };
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Give PTY time to start shell
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut rx = broker.subscribe();

    // === Send HTTP POST /input ===
    let marker = "E2E_HTTP_TEST_55555";
    let cmd = format!("echo {}\n", marker);

    let stream = tokio::net::TcpStream::connect(addr).await.expect("Failed to connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await.expect("Handshake failed");

    tokio::spawn(async move {
        let _ = conn.await;
    });

    let request = hyper::Request::builder()
        .method("POST")
        .uri("/sessions/test/input")
        .body(http_body_util::Full::new(Bytes::from(cmd)))
        .expect("Failed to build request");

    let response = sender.send_request(request).await.expect("Request failed");
    assert_eq!(response.status(), 204, "Expected 204 No Content");

    // === Collect output from broker ===
    let mut collected = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);

    loop {
        if tokio::time::Instant::now() >= deadline {
            break;
        }

        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(data) => {
                        collected.extend_from_slice(&data);
                        if String::from_utf8_lossy(&collected).contains(marker) {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }

    let output = String::from_utf8_lossy(&collected);

    // Cleanup: send exit command to close shell, then drop channel
    let _ = input_tx.send(Bytes::from("exit\n")).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    stop_flag.store(true, Ordering::Relaxed);
    drop(input_tx);

    // Abort blocking tasks if they don't finish quickly
    tokio::select! {
        _ = pty_writer_handle => {}
        _ = tokio::time::sleep(Duration::from_millis(500)) => {}
    }
    tokio::select! {
        _ = pty_reader_handle => {}
        _ = tokio::time::sleep(Duration::from_millis(500)) => {}
    }

    assert!(
        output.contains(marker),
        "Expected output to contain '{}', but got:\n{}",
        marker,
        output
    );
}

/// Test that scrollback endpoint returns data when there's scrollback
#[tokio::test(flavor = "multi_thread")]
async fn test_scrollback_endpoint_with_real_pty() {
    // === Setup PTY ===
    let pty = Arc::new(parking_lot::Mutex::new(Pty::spawn(5, 80, SpawnCommand::default()).expect("Failed to spawn PTY"))); // Small screen: 5 rows
    let mut pty_reader = pty.lock().take_reader().expect("Failed to get reader");
    let mut pty_writer = pty.lock().take_writer().expect("Failed to get writer");

    let broker = Broker::new();
    let broker_clone = broker.clone();

    let (input_tx, mut input_rx) = mpsc::channel::<Bytes>(64);

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_reader = stop_flag.clone();

    // PTY reader task
    let pty_reader_handle = tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 4096];
        while !stop_flag_reader.load(Ordering::Relaxed) {
            match pty_reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    broker_clone.publish(Bytes::copy_from_slice(&buf[..n]));
                }
                Err(e) => {
                    if e.raw_os_error() != Some(5) {
                        eprintln!("PTY read error: {:?}", e);
                    }
                    break;
                }
            }
        }
    });

    // PTY writer task
    let pty_writer_handle = tokio::task::spawn_blocking(move || {
        while let Some(data) = input_rx.blocking_recv() {
            if pty_writer.write_all(&data).is_err() {
                break;
            }
            let _ = pty_writer.flush();
        }
    });

    // === Setup HTTP Server ===
    let (_parser_tx2, parser_rx) = tokio::sync::mpsc::channel(256);
    let parser = Parser::spawn(parser_rx, 80, 5, 1000); // 80 cols, 5 rows
    let session = Session {
        name: "test".to_string(),
        pid: None,
        command: "test".to_string(),
        client_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        tags: std::sync::Arc::new(parking_lot::RwLock::new(std::collections::HashSet::new())),
        child_exited: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        input_tx: input_tx.clone(),
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: pty.clone(),
        terminal_size: wsh::terminal::TerminalSize::new(5, 80),
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
            backends: wsh::federation::registry::BackendRegistry::new(),
            federation: std::sync::Arc::new(tokio::sync::Mutex::new(wsh::federation::manager::FederationManager::new())),
            ip_access: None,
            hostname: "test".to_string(),
            federation_config_path: None,
            local_token: None,
            default_backend_token: None,
            server_id: "test-server-id".to_string(),
    };
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Give PTY time to start shell
    tokio::time::sleep(Duration::from_millis(300)).await;

    // === Send many echo commands to generate scrollback ===
    for i in 0..20 {
        let cmd = format!("echo 'Line {}'\n", i);
        let stream = tokio::net::TcpStream::connect(addr).await.expect("Failed to connect");
        let io = hyper_util::rt::TokioIo::new(stream);
        let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await.expect("Handshake failed");

        tokio::spawn(async move {
            let _ = conn.await;
        });

        let request = hyper::Request::builder()
            .method("POST")
            .uri("/sessions/test/input")
            .body(http_body_util::Full::new(Bytes::from(cmd)))
            .expect("Failed to build request");

        let response = sender.send_request(request).await.expect("Request failed");
        assert_eq!(response.status(), 204);
    }

    // Wait for all output to be processed
    tokio::time::sleep(Duration::from_millis(500)).await;

    // === Query scrollback endpoint ===
    let stream = tokio::net::TcpStream::connect(addr).await.expect("Failed to connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await.expect("Handshake failed");

    tokio::spawn(async move {
        let _ = conn.await;
    });

    let request = hyper::Request::builder()
        .method("GET")
        .uri("/sessions/test/scrollback?format=plain")
        .body(http_body_util::Full::new(Bytes::new()))
        .expect("Failed to build request");

    let response = sender.send_request(request).await.expect("Request failed");
    assert_eq!(response.status(), 200);

    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    eprintln!("Scrollback response: {}", serde_json::to_string_pretty(&json).unwrap());

    // Check that we got scrollback
    let total_lines = json["total_lines"].as_u64().unwrap_or(0);
    let empty_vec = vec![];
    let lines = json["lines"].as_array().unwrap_or(&empty_vec);

    eprintln!("total_lines: {}, lines.len(): {}", total_lines, lines.len());

    // Cleanup
    let _ = input_tx.send(Bytes::from("exit\n")).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    stop_flag.store(true, Ordering::Relaxed);
    drop(input_tx);

    tokio::select! {
        _ = pty_writer_handle => {}
        _ = tokio::time::sleep(Duration::from_millis(500)) => {}
    }
    tokio::select! {
        _ = pty_reader_handle => {}
        _ = tokio::time::sleep(Duration::from_millis(500)) => {}
    }

    assert!(
        total_lines > 0,
        "Expected scrollback total_lines > 0, got: {}. Full response: {}",
        total_lines,
        serde_json::to_string_pretty(&json).unwrap()
    );
    assert!(
        !lines.is_empty(),
        "Expected scrollback lines not empty, got {} lines",
        lines.len()
    );
}
