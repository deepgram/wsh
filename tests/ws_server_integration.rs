//! Integration tests for the server-level multiplexed WebSocket at `/ws/json`.

use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use wsh::{
    api,
    session::SessionRegistry,
    shutdown::ShutdownCoordinator,
};

fn create_empty_state() -> api::AppState {
    api::AppState {
        sessions: SessionRegistry::new(),
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(api::ServerConfig::new(false)),
            server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    }
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
    let deadline = Duration::from_secs(5);
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

/// Helper: try to receive a JSON message within a timeout, returning None if
/// no message arrived.
async fn try_recv_json(
    ws: &mut futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    timeout: Duration,
) -> Option<serde_json::Value> {
    match tokio::time::timeout(timeout, ws.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => Some(serde_json::from_str(&text).unwrap()),
        _ => None,
    }
}

/// Connect to the server-level WS and consume the connected message.
async fn connect_server_ws(
    addr: SocketAddr,
) -> (
    futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) {
    let (ws, _) = connect_async(format!("ws://{}/ws/json", addr))
        .await
        .unwrap();
    let (tx, mut rx) = ws.split();

    // Consume the connected message
    let msg = recv_json(&mut rx).await;
    assert_eq!(msg["connected"], true);

    (tx, rx)
}

// ── Test: list_sessions ─────────────────────────────────────────

#[tokio::test]
async fn test_server_ws_list_sessions() {
    let state = create_empty_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (mut tx, mut rx) = connect_server_ws(addr).await;

    // List sessions — should be empty
    tx.send(Message::Text(
        serde_json::json!({"id": 1, "method": "list_sessions"}).to_string(),
    ))
    .await
    .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["method"], "list_sessions");
    assert_eq!(resp["result"], serde_json::json!([]));

    // Create a session via WS
    tx.send(Message::Text(
        serde_json::json!({"id": 2, "method": "create_session", "params": {"name": "alpha"}})
            .to_string(),
    ))
    .await
    .unwrap();

    // Drain events until we see the create_session response
    let mut create_resp = None;
    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("method") == Some(&serde_json::json!("create_session")) {
            create_resp = Some(msg);
            break;
        }
    }
    let create_resp = create_resp.expect("should get create_session response");
    assert_eq!(create_resp["result"]["name"], "alpha");

    // List sessions again — should have one
    tx.send(Message::Text(
        serde_json::json!({"id": 3, "method": "list_sessions"}).to_string(),
    ))
    .await
    .unwrap();

    // Drain until list_sessions response
    let mut list_resp = None;
    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("method") == Some(&serde_json::json!("list_sessions")) {
            list_resp = Some(msg);
            break;
        }
    }
    let list_resp = list_resp.expect("should get list_sessions response");
    assert_eq!(list_resp["id"], 3);
    let sessions = list_resp["result"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["name"], "alpha");
}

// ── Test: create_session ────────────────────────────────────────

#[tokio::test]
async fn test_server_ws_create_session() {
    let state = create_empty_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (mut tx, mut rx) = connect_server_ws(addr).await;

    // Create a named session
    tx.send(Message::Text(
        serde_json::json!({
            "id": 1,
            "method": "create_session",
            "params": {"name": "my-session"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    // Collect the create_session response (may receive session_created event first)
    let mut resp = None;
    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("method") == Some(&serde_json::json!("create_session")) {
            resp = Some(msg);
            break;
        }
    }
    let resp = resp.expect("should get create_session response");
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["name"], "my-session");
    assert!(resp.get("error").is_none());
}

// ── Test: create_session auto-name ──────────────────────────────

#[tokio::test]
async fn test_server_ws_create_session_auto_name() {
    let state = create_empty_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (mut tx, mut rx) = connect_server_ws(addr).await;

    // Create without a name
    tx.send(Message::Text(
        serde_json::json!({"id": 1, "method": "create_session"}).to_string(),
    ))
    .await
    .unwrap();

    let mut resp = None;
    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("method") == Some(&serde_json::json!("create_session")) {
            resp = Some(msg);
            break;
        }
    }
    let resp = resp.expect("should get create_session response");
    assert_eq!(resp["result"]["name"], "0");
}

// ── Test: kill_session ──────────────────────────────────────────

#[tokio::test]
async fn test_server_ws_kill_session() {
    let state = create_empty_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (mut tx, mut rx) = connect_server_ws(addr).await;

    // Create a session
    tx.send(Message::Text(
        serde_json::json!({
            "id": 1,
            "method": "create_session",
            "params": {"name": "doomed"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    // Drain until create response
    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("method") == Some(&serde_json::json!("create_session")) {
            break;
        }
    }

    // Kill it
    tx.send(Message::Text(
        serde_json::json!({
            "id": 2,
            "method": "kill_session",
            "params": {"name": "doomed"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    let mut kill_resp = None;
    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("method") == Some(&serde_json::json!("kill_session")) {
            kill_resp = Some(msg);
            break;
        }
    }
    let kill_resp = kill_resp.expect("should get kill_session response");
    assert_eq!(kill_resp["id"], 2);
    assert!(kill_resp["result"].is_object());
    assert!(kill_resp.get("error").is_none());

    // Verify it's gone by listing
    tx.send(Message::Text(
        serde_json::json!({"id": 3, "method": "list_sessions"}).to_string(),
    ))
    .await
    .unwrap();

    let mut list_resp = None;
    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("method") == Some(&serde_json::json!("list_sessions")) {
            list_resp = Some(msg);
            break;
        }
    }
    let list_resp = list_resp.expect("should get list_sessions response");
    assert_eq!(list_resp["result"], serde_json::json!([]));
}

// ── Test: rename_session ────────────────────────────────────────

#[tokio::test]
async fn test_server_ws_rename_session() {
    let state = create_empty_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (mut tx, mut rx) = connect_server_ws(addr).await;

    // Create
    tx.send(Message::Text(
        serde_json::json!({
            "id": 1,
            "method": "create_session",
            "params": {"name": "old-name"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("method") == Some(&serde_json::json!("create_session")) {
            break;
        }
    }

    // Rename
    tx.send(Message::Text(
        serde_json::json!({
            "id": 2,
            "method": "rename_session",
            "params": {"name": "old-name", "new_name": "new-name"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    let mut rename_resp = None;
    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("method") == Some(&serde_json::json!("rename_session")) {
            rename_resp = Some(msg);
            break;
        }
    }
    let rename_resp = rename_resp.expect("should get rename_session response");
    assert_eq!(rename_resp["id"], 2);
    assert_eq!(rename_resp["result"]["name"], "new-name");
    assert!(rename_resp.get("error").is_none());

    // Verify by listing
    tx.send(Message::Text(
        serde_json::json!({"id": 3, "method": "list_sessions"}).to_string(),
    ))
    .await
    .unwrap();

    let mut list_resp = None;
    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("method") == Some(&serde_json::json!("list_sessions")) {
            list_resp = Some(msg);
            break;
        }
    }
    let list_resp = list_resp.expect("should get list_sessions response");
    let sessions = list_resp["result"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["name"], "new-name");
}

// ── Test: per-session dispatch ──────────────────────────────────

#[tokio::test]
async fn test_server_ws_per_session_dispatch() {
    let state = create_empty_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (mut tx, mut rx) = connect_server_ws(addr).await;

    // Create a session
    tx.send(Message::Text(
        serde_json::json!({
            "id": 1,
            "method": "create_session",
            "params": {"name": "worker"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("method") == Some(&serde_json::json!("create_session")) {
            break;
        }
    }

    // Send get_screen with session field
    tx.send(Message::Text(
        serde_json::json!({
            "id": 2,
            "method": "get_screen",
            "session": "worker",
            "params": {"format": "plain"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    let mut screen_resp = None;
    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("method") == Some(&serde_json::json!("get_screen")) {
            screen_resp = Some(msg);
            break;
        }
    }
    let screen_resp = screen_resp.expect("should get get_screen response");
    assert_eq!(screen_resp["id"], 2);
    assert!(screen_resp["result"]["cols"].is_number());
    assert!(screen_resp["result"]["rows"].is_number());
}

// ── Test: session events ────────────────────────────────────────

#[tokio::test]
async fn test_server_ws_session_events() {
    let state = create_empty_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (mut tx, mut rx) = connect_server_ws(addr).await;

    // Create a session — should trigger session_created event
    tx.send(Message::Text(
        serde_json::json!({
            "id": 1,
            "method": "create_session",
            "params": {"name": "evt-test"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    // Look for session_created event
    let mut found_created = false;
    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("event") == Some(&serde_json::json!("session_created")) {
            assert_eq!(msg["params"]["name"], "evt-test");
            found_created = true;
            break;
        }
    }
    assert!(found_created, "should receive session_created event");

    // Drain the create_session response if we haven't already
    // (it might come before or after the event)
    let mut saw_create_resp = false;
    while !saw_create_resp {
        if let Some(msg) = try_recv_json(&mut rx, Duration::from_millis(500)).await {
            if msg.get("method") == Some(&serde_json::json!("create_session")) {
                saw_create_resp = true;
            }
        } else {
            break;
        }
    }

    // Kill the session — should trigger session_destroyed event
    tx.send(Message::Text(
        serde_json::json!({
            "id": 2,
            "method": "kill_session",
            "params": {"name": "evt-test"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    let mut found_destroyed = false;
    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("event") == Some(&serde_json::json!("session_destroyed")) {
            assert_eq!(msg["params"]["name"], "evt-test");
            found_destroyed = true;
            break;
        }
    }
    assert!(found_destroyed, "should receive session_destroyed event");
}

// ── Test: missing session field ─────────────────────────────────

#[tokio::test]
async fn test_server_ws_missing_session_field() {
    let state = create_empty_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (mut tx, mut rx) = connect_server_ws(addr).await;

    // Send get_screen without a session field
    tx.send(Message::Text(
        serde_json::json!({
            "id": 1,
            "method": "get_screen",
            "params": {"format": "plain"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["error"]["code"], "session_required");
}

// ── Test: session not found ─────────────────────────────────────

#[tokio::test]
async fn test_server_ws_session_not_found() {
    let state = create_empty_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (mut tx, mut rx) = connect_server_ws(addr).await;

    // Send get_screen for a nonexistent session
    tx.send(Message::Text(
        serde_json::json!({
            "id": 1,
            "method": "get_screen",
            "session": "ghost",
            "params": {"format": "plain"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["error"]["code"], "session_not_found");
}

// ── Test: set_server_mode ────────────────────────────────────────

#[tokio::test]
async fn test_server_ws_set_server_mode() {
    let state = create_empty_state();
    assert!(!state.server_config.is_persistent());
    let app = api::router(state.clone(), None);
    let addr = start_server(app).await;

    let (mut tx, mut rx) = connect_server_ws(addr).await;

    // Set persistent to true
    tx.send(Message::Text(
        serde_json::json!({
            "id": 1,
            "method": "set_server_mode",
            "params": {"persistent": true}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["method"], "set_server_mode");
    assert!(resp["result"].is_object());
    assert_eq!(resp["result"]["persistent"], true);
    assert!(state.server_config.is_persistent());

    // Set persistent back to false
    tx.send(Message::Text(
        serde_json::json!({
            "id": 2,
            "method": "set_server_mode",
            "params": {"persistent": false}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["id"], 2);
    assert_eq!(resp["result"]["persistent"], false);
    assert!(!state.server_config.is_persistent());
}

// ── Test: duplicate session name ────────────────────────────────

#[tokio::test]
async fn test_server_ws_create_duplicate_name() {
    let state = create_empty_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (mut tx, mut rx) = connect_server_ws(addr).await;

    // Create first
    tx.send(Message::Text(
        serde_json::json!({
            "id": 1,
            "method": "create_session",
            "params": {"name": "dup"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("method") == Some(&serde_json::json!("create_session")) {
            break;
        }
    }

    // Create duplicate
    tx.send(Message::Text(
        serde_json::json!({
            "id": 2,
            "method": "create_session",
            "params": {"name": "dup"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    let mut dup_resp = None;
    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("method") == Some(&serde_json::json!("create_session"))
            && msg.get("id") == Some(&serde_json::json!(2))
        {
            dup_resp = Some(msg);
            break;
        }
    }
    let dup_resp = dup_resp.expect("should get second create_session response");
    assert_eq!(dup_resp["error"]["code"], "session_name_conflict");
}

// ── Test: send_input via server WS ──────────────────────────────

#[tokio::test]
async fn test_server_ws_send_input() {
    let state = create_empty_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (mut tx, mut rx) = connect_server_ws(addr).await;

    // Create a session
    tx.send(Message::Text(
        serde_json::json!({
            "id": 1,
            "method": "create_session",
            "params": {"name": "input-test"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("method") == Some(&serde_json::json!("create_session")) {
            break;
        }
    }

    // Send input
    tx.send(Message::Text(
        serde_json::json!({
            "id": 2,
            "method": "send_input",
            "session": "input-test",
            "params": {"data": "echo hello\r"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    let mut input_resp = None;
    for _ in 0..10 {
        let msg = recv_json(&mut rx).await;
        if msg.get("method") == Some(&serde_json::json!("send_input")) {
            input_resp = Some(msg);
            break;
        }
    }
    let input_resp = input_resp.expect("should get send_input response");
    assert_eq!(input_resp["id"], 2);
    assert!(input_resp["result"].is_object());
    assert!(input_resp.get("error").is_none());
}

// ── Test: kill nonexistent session ──────────────────────────────

#[tokio::test]
async fn test_server_ws_kill_nonexistent() {
    let state = create_empty_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (mut tx, mut rx) = connect_server_ws(addr).await;

    tx.send(Message::Text(
        serde_json::json!({
            "id": 1,
            "method": "kill_session",
            "params": {"name": "ghost"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["error"]["code"], "session_not_found");
}

// ── Test: malformed request ─────────────────────────────────────

#[tokio::test]
async fn test_server_ws_malformed_request() {
    let state = create_empty_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (mut tx, mut rx) = connect_server_ws(addr).await;

    // Send JSON without method field
    tx.send(Message::Text(r#"{"id": 1}"#.to_string()))
        .await
        .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["error"]["code"], "invalid_request");
}

// ── Test: subscribe with activity events on server-level WS ────

#[tokio::test]
async fn test_server_ws_subscribe_activity_events() {
    let state = create_empty_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (mut tx, mut rx) = connect_server_ws(addr).await;

    // Create a session
    tx.send(Message::Text(
        serde_json::json!({"id": 1, "method": "create_session", "params": {"name": "act"}})
            .to_string(),
    ))
    .await
    .unwrap();

    // Drain until create response
    loop {
        let msg = recv_json(&mut rx).await;
        if msg.get("method") == Some(&serde_json::json!("create_session")) {
            assert_eq!(msg["result"]["name"], "act");
            break;
        }
    }

    // Let the session go idle (shell prompt takes time to settle)
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // Subscribe to activity events with a short idle timeout
    tx.send(Message::Text(
        serde_json::json!({
            "id": 2,
            "method": "subscribe",
            "session": "act",
            "params": {"events": ["lines", "activity"], "idle_timeout_ms": 500, "format": "plain"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    // Drain until we see the subscribe response, sync, and initial activity state
    let mut got_subscribe = false;
    let mut got_sync = false;
    let mut got_initial_activity = false;
    let mut initial_activity_type = String::new();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !got_initial_activity {
        let msg = recv_json(&mut rx).await;

        if msg.get("method") == Some(&serde_json::json!("subscribe")) {
            got_subscribe = true;
            continue;
        }

        if let Some(event) = msg.get("event").and_then(|e| e.as_str()) {
            match event {
                "sync" => {
                    got_sync = true;
                    assert_eq!(msg["session"], "act");
                }
                "idle" | "running" => {
                    got_initial_activity = true;
                    initial_activity_type = event.to_string();
                    assert_eq!(msg["session"], "act");
                }
                _ => {}
            }
        }
    }

    assert!(got_subscribe, "Should receive subscribe response");
    assert!(got_sync, "Should receive sync event");
    assert!(got_initial_activity, "Should receive initial activity state event");
    // Session has been idle for 1500ms with idle_timeout of 500ms → should be idle
    assert_eq!(initial_activity_type, "idle", "Session should be initially idle");

    // Now send input to trigger a Running event
    tx.send(Message::Text(
        serde_json::json!({
            "id": 3,
            "method": "send_input",
            "session": "act",
            "params": {"data": "echo hello\n"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    // Collect events — expect to see Running (from activity), then eventually Idle
    let mut got_running = false;
    let mut got_idle_after_running = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !got_idle_after_running {
        match try_recv_json(&mut rx, Duration::from_millis(200)).await {
            Some(msg) => {
                if let Some(event) = msg.get("event").and_then(|e| e.as_str()) {
                    match event {
                        "running" => {
                            got_running = true;
                            assert_eq!(msg["session"], "act");
                        }
                        "idle" if got_running => {
                            got_idle_after_running = true;
                            assert_eq!(msg["session"], "act");
                            assert!(msg.get("screen").is_some(), "Idle event should have screen data");
                        }
                        _ => {}
                    }
                }
            }
            None => {}
        }
    }

    assert!(got_running, "Should receive Running event after input");
    assert!(got_idle_after_running, "Should receive Idle event after activity settles");
}
