//! Regression tests for the send-then-idle race condition.
//!
//! These tests verify that the generation counter returned by input
//! endpoints enables correct idle sequencing -- callers can pass it to
//! /idle?last_generation=N to avoid the race where idle returns
//! immediately because the PTY hasn't echoed the input yet.

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
    input::{FocusTracker, InputBroadcaster, InputMode},
    overlay::OverlayStore,
    parser::Parser,
    session::{Session, SessionRegistry},
    shutdown::ShutdownCoordinator,
};

fn create_test_state() -> (api::AppState, mpsc::Receiver<Bytes>, ActivityTracker, mpsc::Sender<Bytes>) {
    let (input_tx, input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let (parser_tx, parser_rx) = mpsc::channel(256);
    let parser = Parser::spawn(parser_rx, 80, 24, 1000);
    let activity = ActivityTracker::new();
    let session = Session {
        name: "test".to_string(),
        pid: None,
        command: "test".to_string(),
        client_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        tags: Arc::new(parking_lot::RwLock::new(std::collections::HashSet::new())),
        child_exited: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        input_tx,
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        panels: wsh::panel::PanelStore::new(),
        pty: Arc::new(parking_lot::Mutex::new(
            wsh::pty::Pty::spawn(24, 80, wsh::pty::SpawnCommand::default())
                .expect("failed to spawn PTY for test"),
        )),
        terminal_size: wsh::terminal::TerminalSize::new(24, 80),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        activity: activity.clone(),
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
        tcp_addr: None,
        instance_name: "test".to_string(),
        http_socket_path: std::path::PathBuf::from("/tmp/test.http.sock"),
            recordings: wsh::recording::RecordingRegistry::new(),
    };
    (state, input_rx, activity, parser_tx)
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

async fn http_post(addr: SocketAddr, uri: &str, body_str: &str) -> (u16, serde_json::Value) {
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
        .method("POST")
        .uri(uri)
        .body(http_body_util::Full::new(Bytes::from(body_str.to_string())))
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
// Race condition regression tests
// ---------------------------------------------------------------------------

/// Verify that POST /input returns a generation counter and that
/// GET /idle?last_generation=N waits for new activity instead of
/// returning immediately.
#[tokio::test]
async fn test_input_generation_prevents_stale_idle() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Let initial activity settle
    tokio::time::sleep(Duration::from_millis(100)).await;

    // POST /input -- should return generation
    let (status, body) = http_post(addr, "/sessions/test/input", "x").await;
    assert_eq!(status, 200, "POST /input should return 200");
    let gen = body["generation"].as_u64().expect("should have generation");

    // The handler calls generation() before touch(), so gen is the pre-touch
    // value. After touch(), the actual generation is gen+1. When we call
    // /idle?last_generation=<gen>, wait_for_idle sees current (gen+1) > last_seen
    // (gen), so it skips the "wait for new activity" gate and enters the idle
    // loop directly.

    // Spawn a task that touches activity after 50ms to simulate PTY output
    let activity_clone = activity.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        activity_clone.touch();
    });

    // GET /idle with the generation -- should wait for the activity + timeout
    let start = std::time::Instant::now();
    let (idle_status, idle_body) = http_get(
        addr,
        &format!(
            "/sessions/test/idle?timeout_ms=100&max_wait_ms=5000&last_generation={}&last_session=test&format=plain",
            gen
        ),
    )
    .await;
    let elapsed = start.elapsed();

    assert_eq!(idle_status, 200, "idle should succeed");
    assert!(idle_body["generation"].is_number(), "idle should return generation");
    // Should have waited: ~50ms for activity + ~100ms for idle timeout = ~150ms
    assert!(
        elapsed >= Duration::from_millis(100),
        "idle should have waited, got {:?}",
        elapsed
    );
}

/// Verify that /idle WITHOUT last_generation returns immediately when
/// the terminal is already idle.
#[tokio::test]
async fn test_idle_without_generation_returns_immediately_when_idle() {
    let (state, _rx, _activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Wait for terminal to become idle
    tokio::time::sleep(Duration::from_millis(200)).await;

    let start = std::time::Instant::now();
    let (status, body) = http_get(
        addr,
        "/sessions/test/idle?timeout_ms=100&max_wait_ms=5000&format=plain",
    )
    .await;
    let elapsed = start.elapsed();

    assert_eq!(status, 200);
    assert!(body["generation"].is_number());
    // Should return almost immediately since terminal is already idle
    assert!(
        elapsed < Duration::from_millis(50),
        "should have returned quickly, got {:?}",
        elapsed
    );
}

/// Verify the full send-then-idle pattern: POST /input, extract
/// generation, then GET /idle with that generation. The idle endpoint
/// should not return stale data from before the input was processed.
#[tokio::test]
async fn test_send_then_idle_pattern_end_to_end() {
    let (state, _rx, _activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Let initial state settle
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Step 1: Confirm terminal is idle
    let (status, body) = http_get(
        addr,
        "/sessions/test/idle?timeout_ms=100&format=plain",
    )
    .await;
    assert_eq!(status, 200);
    let initial_gen = body["generation"].as_u64().expect("should have generation");

    // Step 2: Send input
    let (status, body) = http_post(addr, "/sessions/test/input", "echo hello\n").await;
    assert_eq!(status, 200);
    let input_gen = body["generation"].as_u64().expect("should have generation");

    // The input handler captures generation before touch, so input_gen
    // should equal initial_gen (the value before the touch that input does).
    assert_eq!(
        input_gen, initial_gen,
        "input should return the pre-touch generation"
    );

    // Step 3: Use the generation from input to call idle. Since the
    // handler already did touch(), the current generation is input_gen+1.
    // The idle endpoint sees current > last_seen and enters the idle loop.
    // The PTY shell will eventually echo our input and go idle.
    let start = std::time::Instant::now();
    let (status, body) = http_get(
        addr,
        &format!(
            "/sessions/test/idle?timeout_ms=200&max_wait_ms=5000&last_generation={}&format=plain",
            input_gen
        ),
    )
    .await;
    let elapsed = start.elapsed();

    assert_eq!(status, 200, "idle should succeed");
    let idle_gen = body["generation"].as_u64().expect("should have generation");
    // The idle generation should be greater than what input returned
    assert!(
        idle_gen > input_gen,
        "idle generation ({}) should be > input generation ({})",
        idle_gen,
        input_gen
    );
    // The idle endpoint should have waited for the idle timeout at minimum
    assert!(
        elapsed >= Duration::from_millis(150),
        "idle should have waited for quiescence, got {:?}",
        elapsed
    );
}

/// Verify that passing last_generation equal to the current generation
/// causes the idle endpoint to block until new activity arrives. Without
/// the generation counter, the idle endpoint would return immediately
/// because the terminal is already idle -- this is the race condition.
#[tokio::test]
async fn test_last_generation_blocks_when_current_matches() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Touch once, let it settle -- generation becomes 1
    activity.touch();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify we're at generation 1 by calling idle without last_generation
    let (status, body) = http_get(
        addr,
        "/sessions/test/idle?timeout_ms=100&format=plain",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["generation"], 1);

    // Now call idle with last_generation=1 (matching current). This should
    // block because wait_for_idle sees current == last_seen and waits for
    // new activity. We trigger new activity after 200ms.
    let a = activity.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        a.touch(); // generation becomes 2
    });

    let start = std::time::Instant::now();
    let (status, body) = http_get(
        addr,
        "/sessions/test/idle?timeout_ms=100&last_generation=1&max_wait_ms=5000&format=plain",
    )
    .await;
    let elapsed = start.elapsed();

    assert_eq!(status, 200);
    assert_eq!(body["generation"], 2);
    // Should have waited ~200ms for new activity + ~100ms idle timeout
    assert!(
        elapsed >= Duration::from_millis(250),
        "expected >= 250ms (wait for activity + idle timeout), got {:?}",
        elapsed
    );
}

/// Verify that passing a stale last_generation (less than current) does
/// NOT block -- the idle endpoint should proceed to the idle loop
/// immediately since there has been activity since the caller last checked.
#[tokio::test]
async fn test_stale_generation_does_not_block() {
    let (state, _rx, activity, _parser_tx) = create_test_state();
    let app = api::router(state, api::RouterConfig::default());
    let addr = start_server(app).await;

    // Touch twice, let settle -- generation becomes 2
    activity.touch();
    activity.touch();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Call idle with last_generation=1 (stale -- current is 2). Should
    // NOT block on waiting for new activity.
    let start = std::time::Instant::now();
    let (status, body) = http_get(
        addr,
        "/sessions/test/idle?timeout_ms=100&last_generation=1&format=plain",
    )
    .await;
    let elapsed = start.elapsed();

    assert_eq!(status, 200);
    assert_eq!(body["generation"], 2);
    // Should return quickly -- no need to wait for new activity
    assert!(
        elapsed < Duration::from_millis(500),
        "stale generation should not block, got {:?}",
        elapsed
    );
}
