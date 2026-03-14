//! Integration tests for the MCP WebSocket transport endpoint at `/mcp/ws`.
//!
//! These tests verify that:
//! - The `/mcp/ws` endpoint accepts WebSocket connections and handles MCP initialize
//! - Full MCP request/response cycle works (initialize → tools/list → tool response)
//! - MCP session counter is cleaned up on WebSocket disconnect
//! - Abrupt disconnection (no WS close frame) still cleans up counters
//! - Multiple concurrent WS connections track active_count correctly
//! - ConnectionGuard (active_count) is held while WS is connected and released on close
//! - The existing Streamable HTTP `/mcp` endpoint still works (backward compat)

use std::net::SocketAddr;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use wsh::api::{router, AppState, RouterConfig, ServerConfig};
use wsh::session::SessionRegistry;
use wsh::shutdown::ShutdownCoordinator;

/// Creates a test AppState (shared between tests).
fn create_test_state() -> AppState {
    let registry = SessionRegistry::new();
    AppState {
        sessions: registry,
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(ServerConfig::new(false)),
        server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        ticket_store: std::sync::Arc::new(wsh::api::ticket::TicketStore::new()),
        backends: wsh::federation::registry::BackendRegistry::new(),
        federation: std::sync::Arc::new(tokio::sync::Mutex::new(
            wsh::federation::manager::FederationManager::new(),
        )),
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
    }
}

async fn start_test_server(state: AppState) -> (SocketAddr, AppState) {
    let app = router(state.clone(), RouterConfig::default());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    (addr, state)
}

/// Connect a tungstenite WS client to the given address and path.
async fn connect_ws(
    addr: SocketAddr,
    path: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
{
    let url = format!("ws://{addr}{path}");
    let (ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    ws
}

const INITIALIZE_REQUEST: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}"#;

// ── Test 1: WS MCP initialize returns valid response ────────────

#[tokio::test]
async fn test_mcp_ws_initialize() {
    let state = create_test_state();
    let (addr, _state) = start_test_server(state).await;

    let mut ws = connect_ws(addr, "/mcp/ws").await;

    // Send initialize request
    ws.send(Message::Text(INITIALIZE_REQUEST.into()))
        .await
        .unwrap();

    // Read response
    let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("should receive response within 5s")
        .expect("stream should not end")
        .expect("should not be an error");

    let text = match msg {
        Message::Text(t) => t.to_string(),
        other => panic!("expected text message, got: {:?}", other),
    };

    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 1);

    let result = &json["result"];
    assert!(result.is_object(), "Expected result object");
    assert_eq!(result["protocolVersion"], "2024-11-05");
    assert_eq!(result["serverInfo"]["name"], "wsh");
    assert!(
        result["capabilities"]["tools"].is_object(),
        "Expected tools capability"
    );

    // Clean close
    let _ = ws.close(None).await;
}

// ── Test 2: MCP session counter cleaned up on WS disconnect ─────

#[tokio::test]
async fn test_mcp_ws_session_counter_cleanup() {
    let state = create_test_state();
    let mcp_counter = state.mcp_session_count.clone();
    let (addr, _state) = start_test_server(state).await;

    assert_eq!(
        mcp_counter.load(std::sync::atomic::Ordering::Acquire),
        0,
        "mcp_session_count should start at 0"
    );

    // Connect and initialize
    let mut ws = connect_ws(addr, "/mcp/ws").await;
    ws.send(Message::Text(INITIALIZE_REQUEST.into()))
        .await
        .unwrap();

    // Read initialize response
    let _ = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // MCP session counter should be incremented
    // (Note: the counter is incremented synchronously during upgrade)
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        mcp_counter.load(std::sync::atomic::Ordering::Acquire) >= 1,
        "mcp_session_count should be >= 1 while connected"
    );

    // Close the WebSocket
    let _ = ws.close(None).await;
    drop(ws);

    // Give the server time to clean up
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(
        mcp_counter.load(std::sync::atomic::Ordering::Acquire),
        0,
        "mcp_session_count should return to 0 after disconnect"
    );
}

// ── Test 3: active_count (ConnectionGuard) incremented while connected ──

#[tokio::test]
async fn test_mcp_ws_active_count_invariant() {
    let state = create_test_state();
    let shutdown = state.shutdown.clone();
    let (addr, _state) = start_test_server(state).await;

    assert_eq!(shutdown.active_count(), 0, "active_count should start at 0");

    // Connect WS
    let mut ws = connect_ws(addr, "/mcp/ws").await;
    ws.send(Message::Text(INITIALIZE_REQUEST.into()))
        .await
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // active_count should be >= 1 (ConnectionGuard held)
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        shutdown.active_count() >= 1,
        "active_count should be >= 1 while /mcp/ws is connected"
    );

    // Close the WebSocket
    let _ = ws.close(None).await;
    drop(ws);

    // Give the server time to clean up
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(
        shutdown.active_count(),
        0,
        "active_count should return to 0 after /mcp/ws disconnect"
    );
}

// ── Test 4: HTTP MCP endpoint still works (backward compat) ─────

#[tokio::test]
async fn test_http_mcp_still_works() {
    let state = create_test_state();
    let (addr, _state) = start_test_server(state).await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(INITIALIZE_REQUEST)
        .send()
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        200,
        "HTTP MCP endpoint should still return 200"
    );

    let body = response.text().await.unwrap();
    assert!(
        body.contains("2024-11-05"),
        "HTTP MCP response should contain protocol version"
    );
}

// ── Test 5: HTTP MCP session does NOT affect active_count ────────

#[tokio::test]
async fn test_http_mcp_does_not_affect_active_count() {
    let state = create_test_state();
    let shutdown = state.shutdown.clone();
    let mcp_counter = state.mcp_session_count.clone();
    let (addr, _state) = start_test_server(state).await;
    let client = reqwest::Client::new();

    assert_eq!(shutdown.active_count(), 0);

    // Send MCP initialize over HTTP
    let response = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(INITIALIZE_REQUEST)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    // active_count should still be 0 (HTTP MCP has no ConnectionGuard)
    assert_eq!(
        shutdown.active_count(),
        0,
        "HTTP MCP should NOT affect active_count"
    );

    // mcp_session_count should be incremented (diagnostic only)
    assert!(
        mcp_counter.load(std::sync::atomic::Ordering::Acquire) >= 1,
        "HTTP MCP should increment mcp_session_count (diagnostic)"
    );
}

// ── Test 6: Full MCP cycle: initialize → tools/list → verify tools ──

const INITIALIZED_NOTIFICATION: &str =
    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;

fn tools_list_request(id: u64) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"method":"tools/list","params":{{}}}}"#,
        id
    )
}

#[tokio::test]
async fn test_mcp_ws_full_tool_cycle() {
    let state = create_test_state();
    let (addr, _state) = start_test_server(state).await;

    let mut ws = connect_ws(addr, "/mcp/ws").await;

    // Step 1: Initialize
    ws.send(Message::Text(INITIALIZE_REQUEST.into()))
        .await
        .unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("init response timeout")
        .expect("stream ended")
        .expect("ws error");
    let init_json: serde_json::Value = match msg {
        Message::Text(t) => serde_json::from_str(&t).unwrap(),
        other => panic!("expected text, got: {:?}", other),
    };
    assert_eq!(init_json["id"], 1);
    assert!(init_json["result"]["capabilities"]["tools"].is_object());

    // Step 2: Send initialized notification (required by MCP protocol)
    ws.send(Message::Text(INITIALIZED_NOTIFICATION.into()))
        .await
        .unwrap();

    // Step 3: Send tools/list request
    ws.send(Message::Text(tools_list_request(2).into()))
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("tools/list response timeout")
        .expect("stream ended")
        .expect("ws error");

    let tools_json: serde_json::Value = match msg {
        Message::Text(t) => serde_json::from_str(&t).unwrap(),
        other => panic!("expected text, got: {:?}", other),
    };
    assert_eq!(tools_json["jsonrpc"], "2.0");
    assert_eq!(tools_json["id"], 2);

    let tools = tools_json["result"]["tools"]
        .as_array()
        .expect("expected tools array");
    assert!(!tools.is_empty(), "should have at least one tool");

    // Verify well-known tools are present
    let tool_names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        tool_names.contains(&"wsh_create_session"),
        "expected wsh_create_session tool, found: {:?}",
        tool_names
    );
    assert!(
        tool_names.contains(&"wsh_send_input"),
        "expected wsh_send_input tool, found: {:?}",
        tool_names
    );
    assert!(
        tool_names.contains(&"wsh_get_screen"),
        "expected wsh_get_screen tool, found: {:?}",
        tool_names
    );

    // Clean close
    let _ = ws.close(None).await;
}

// ── Test 7: Abrupt disconnect (no close frame) cleans up counters ──

#[tokio::test]
async fn test_mcp_ws_abrupt_disconnect_cleanup() {
    let state = create_test_state();
    let shutdown = state.shutdown.clone();
    let mcp_counter = state.mcp_session_count.clone();
    let (addr, _state) = start_test_server(state).await;

    assert_eq!(shutdown.active_count(), 0);
    assert_eq!(mcp_counter.load(std::sync::atomic::Ordering::Acquire), 0);

    // Connect and initialize
    let mut ws = connect_ws(addr, "/mcp/ws").await;
    ws.send(Message::Text(INITIALIZE_REQUEST.into()))
        .await
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // Verify counters are up
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(shutdown.active_count() >= 1, "active_count should be >= 1");
    assert!(
        mcp_counter.load(std::sync::atomic::Ordering::Acquire) >= 1,
        "mcp_session_count should be >= 1"
    );

    // Abruptly drop the WS without sending a close frame — simulates SIGKILL
    // of the `wsh mcp` bridge process.
    drop(ws);

    // Server should detect the broken connection and clean up.
    // Poll with timeout rather than fixed sleep — the server detects the
    // dropped TCP connection promptly (no ping/pong timeout needed for
    // in-process tests since the TCP connection is closed immediately).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let ac = shutdown.active_count();
        let mc = mcp_counter.load(std::sync::atomic::Ordering::Acquire);
        if ac == 0 && mc == 0 {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "counters did not return to 0 after abrupt disconnect: \
                 active_count={}, mcp_session_count={}",
                ac, mc
            );
        }
    }
}

// ── Test 8: Multiple concurrent WS connections track counters correctly ──

#[tokio::test]
async fn test_mcp_ws_multiple_concurrent_connections() {
    let state = create_test_state();
    let shutdown = state.shutdown.clone();
    let mcp_counter = state.mcp_session_count.clone();
    let (addr, _state) = start_test_server(state).await;

    assert_eq!(shutdown.active_count(), 0);

    // Helper: connect, initialize, and return the WS stream
    async fn connect_and_init(
        addr: SocketAddr,
    ) -> tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    > {
        let url = format!("ws://{addr}/mcp/ws");
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        ws.send(Message::Text(INITIALIZE_REQUEST.into()))
            .await
            .unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        ws
    }

    // Open 3 concurrent connections
    let ws1 = connect_and_init(addr).await;
    let ws2 = connect_and_init(addr).await;
    let ws3 = connect_and_init(addr).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        shutdown.active_count(),
        3,
        "active_count should be 3 with 3 WS connections"
    );
    assert_eq!(
        mcp_counter.load(std::sync::atomic::Ordering::Acquire),
        3,
        "mcp_session_count should be 3 with 3 WS connections"
    );

    // Drop first connection
    drop(ws1);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if shutdown.active_count() == 2 {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "active_count did not drop to 2, got: {}",
                shutdown.active_count()
            );
        }
    }
    assert_eq!(
        mcp_counter.load(std::sync::atomic::Ordering::Acquire),
        2,
        "mcp_session_count should be 2 after first disconnect"
    );

    // Drop second connection
    drop(ws2);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if shutdown.active_count() == 1 {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "active_count did not drop to 1, got: {}",
                shutdown.active_count()
            );
        }
    }
    assert_eq!(
        mcp_counter.load(std::sync::atomic::Ordering::Acquire),
        1,
        "mcp_session_count should be 1 after second disconnect"
    );

    // Drop last connection
    drop(ws3);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let ac = shutdown.active_count();
        let mc = mcp_counter.load(std::sync::atomic::Ordering::Acquire);
        if ac == 0 && mc == 0 {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "counters did not return to 0 after last disconnect: \
                 active_count={}, mcp_session_count={}",
                ac, mc
            );
        }
    }
}
