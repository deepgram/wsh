//! Integration tests for the MCP Streamable HTTP endpoint at `/mcp`.
//!
//! These tests verify that:
//! - The MCP endpoint responds to initialize requests
//! - Server info and capabilities are returned correctly
//! - Tool listing works through the MCP protocol
//! - The endpoint is accessible without authentication (separate from the REST API)

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpListener;
use wsh::api::{router, AppState, ServerConfig};
use wsh::session::SessionRegistry;
use wsh::shutdown::ShutdownCoordinator;

/// Creates a test app with an empty session registry.
fn create_test_app() -> axum::Router {
    let registry = SessionRegistry::new();
    let state = AppState {
        sessions: registry,
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(ServerConfig::new(false)),
    };
    router(state, None)
}

async fn start_test_server(app: axum::Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    addr
}

/// Helper to send an MCP JSON-RPC request with a session ID header.
async fn send_mcp_request_with_session(
    client: &reqwest::Client,
    addr: SocketAddr,
    body: &str,
    session_id: &str,
) -> String {
    let response = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", session_id)
        .body(body.to_string())
        .send()
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        200,
        "MCP endpoint should return 200 OK"
    );

    response.text().await.unwrap()
}

/// Extract the JSON-RPC response from an SSE event stream body.
/// SSE events look like:
///   id: 0\nretry: 3000\ndata: \n\nevent: message\ndata: {"jsonrpc":"2.0",...}\n\n
fn extract_jsonrpc_from_sse(body: &str) -> serde_json::Value {
    // Find the last event that contains actual JSON-RPC data
    let events: Vec<&str> = body.split("\n\n").collect();
    for event in events.iter().rev() {
        for line in event.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                    if json.get("jsonrpc").is_some() {
                        return json;
                    }
                }
            }
        }
    }
    panic!(
        "No JSON-RPC response found in SSE body:\n{}",
        body
    );
}

/// Extract the session ID from an SSE response's headers (for stateful mode).
async fn send_initialize_and_get_session(
    client: &reqwest::Client,
    addr: SocketAddr,
) -> (serde_json::Value, String) {
    let response = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}"#,
        )
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    let session_id = response
        .headers()
        .get("mcp-session-id")
        .expect("initialize response should have Mcp-Session-Id header")
        .to_str()
        .unwrap()
        .to_string();

    let body = response.text().await.unwrap();
    let json = extract_jsonrpc_from_sse(&body);

    (json, session_id)
}

// ── Test 1: MCP initialize returns server info ─────────────────

#[tokio::test]
async fn test_mcp_initialize_returns_server_info() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let (json, _session_id) = send_initialize_and_get_session(&client, addr).await;

    // Verify the response is a valid JSON-RPC result
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 1);

    let result = &json["result"];
    assert!(
        result.is_object(),
        "Expected result object in initialize response"
    );

    // Verify protocol version
    assert_eq!(result["protocolVersion"], "2024-11-05");

    // Verify server info
    assert_eq!(result["serverInfo"]["name"], "wsh");
    assert!(
        result["serverInfo"]["version"].is_string(),
        "Expected version string in server info"
    );

    // Verify capabilities include tools
    assert!(
        result["capabilities"]["tools"].is_object(),
        "Expected tools capability"
    );

    // Verify instructions are present
    assert!(
        result["instructions"].is_string(),
        "Expected instructions string"
    );
    let instructions = result["instructions"].as_str().unwrap();
    assert!(
        instructions.contains("wsh_run_command"),
        "Instructions should mention wsh_run_command"
    );
}

// ── Test 2: MCP list tools returns our tools ───────────────────

#[tokio::test]
async fn test_mcp_list_tools() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // First initialize to get a session
    let (_, session_id) = send_initialize_and_get_session(&client, addr).await;

    // Send initialized notification (required by MCP protocol before other requests)
    let _notif_resp = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .send()
        .await
        .unwrap();

    // Now list tools
    let body = send_mcp_request_with_session(
        &client,
        addr,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        &session_id,
    )
    .await;

    let json = extract_jsonrpc_from_sse(&body);

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 2);

    let tools = json["result"]["tools"]
        .as_array()
        .expect("Expected tools array in list tools response");

    // We should have all 14 tools
    assert!(
        tools.len() >= 14,
        "Expected at least 14 tools, got {}",
        tools.len()
    );

    // Verify some key tools exist
    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();

    assert!(
        tool_names.contains(&"wsh_create_session"),
        "Missing wsh_create_session tool"
    );
    assert!(
        tool_names.contains(&"wsh_run_command"),
        "Missing wsh_run_command tool"
    );
    assert!(
        tool_names.contains(&"wsh_get_screen"),
        "Missing wsh_get_screen tool"
    );
    assert!(
        tool_names.contains(&"wsh_overlay"),
        "Missing wsh_overlay tool"
    );
    assert!(
        tool_names.contains(&"wsh_panel"),
        "Missing wsh_panel tool"
    );
    assert!(
        tool_names.contains(&"wsh_input_mode"),
        "Missing wsh_input_mode tool"
    );
}

// ── Test 3: MCP endpoint is accessible without auth ────────────

#[tokio::test]
async fn test_mcp_endpoint_exempt_from_auth() {
    let registry = SessionRegistry::new();
    let state = AppState {
        sessions: registry,
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(ServerConfig::new(false)),
    };
    // Create router WITH auth token
    let app = router(state, Some("secret-token".to_string()));
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // MCP endpoint should work without auth token
    let (json, _session_id) = send_initialize_and_get_session(&client, addr).await;
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 1);
    assert!(json["result"]["serverInfo"]["name"].is_string());

    // Meanwhile, REST API should reject without auth
    let resp = client
        .get(format!("http://{addr}/sessions"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "REST API should require auth when token is set"
    );
}

// ── Test 4: MCP returns error for wrong content type ───────────

#[tokio::test]
async fn test_mcp_rejects_wrong_content_type() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "text/plain")
        .header("Accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        415,
        "Should reject non-JSON content type"
    );
}

// ── Test 5: MCP returns error for missing accept header ────────

#[tokio::test]
async fn test_mcp_rejects_missing_accept() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        // Only accept JSON, not event-stream
        .header("Accept", "application/json")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        406,
        "Should reject missing text/event-stream in Accept header"
    );
}

// ── Test 6: MCP list resources ──────────────────────────────────

#[tokio::test]
async fn test_mcp_list_resources() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Initialize to get a session ID
    let (_, session_id) = send_initialize_and_get_session(&client, addr).await;

    // Send initialized notification
    let _ = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .send()
        .await
        .unwrap();

    // List resources
    let body = send_mcp_request_with_session(
        &client,
        addr,
        r#"{"jsonrpc":"2.0","id":2,"method":"resources/list"}"#,
        &session_id,
    )
    .await;

    let json = extract_jsonrpc_from_sse(&body);

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 2);

    let resources = json["result"]["resources"]
        .as_array()
        .expect("Expected resources array in list resources response");

    // Should have at least the wsh://sessions resource
    assert!(
        !resources.is_empty(),
        "Expected at least one resource"
    );

    // Verify the sessions resource exists
    let has_sessions = resources
        .iter()
        .any(|r| r["uri"].as_str() == Some("wsh://sessions"));
    assert!(has_sessions, "Missing wsh://sessions resource");
}

// ── Test 7: MCP list resource templates ─────────────────────────

#[tokio::test]
async fn test_mcp_list_resource_templates() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Initialize to get a session ID
    let (_, session_id) = send_initialize_and_get_session(&client, addr).await;

    // Send initialized notification
    let _ = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .send()
        .await
        .unwrap();

    // List resource templates
    let body = send_mcp_request_with_session(
        &client,
        addr,
        r#"{"jsonrpc":"2.0","id":2,"method":"resources/templates/list"}"#,
        &session_id,
    )
    .await;

    let json = extract_jsonrpc_from_sse(&body);

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 2);

    let templates = json["result"]["resourceTemplates"]
        .as_array()
        .expect("Expected resourceTemplates array");

    // Should have exactly 2 templates (screen and scrollback)
    assert_eq!(
        templates.len(),
        2,
        "Expected 2 resource templates, got {}",
        templates.len()
    );

    let uri_templates: Vec<&str> = templates
        .iter()
        .filter_map(|t| t["uriTemplate"].as_str())
        .collect();

    assert!(
        uri_templates.contains(&"wsh://sessions/{name}/screen"),
        "Missing screen template"
    );
    assert!(
        uri_templates.contains(&"wsh://sessions/{name}/scrollback"),
        "Missing scrollback template"
    );
}

// ── Test 8: MCP read sessions resource ──────────────────────────

#[tokio::test]
async fn test_mcp_read_sessions_resource() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // Initialize to get a session ID
    let (_, session_id) = send_initialize_and_get_session(&client, addr).await;

    // Send initialized notification
    let _ = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .send()
        .await
        .unwrap();

    // Read the sessions resource
    let body = send_mcp_request_with_session(
        &client,
        addr,
        r#"{"jsonrpc":"2.0","id":2,"method":"resources/read","params":{"uri":"wsh://sessions"}}"#,
        &session_id,
    )
    .await;

    let json = extract_jsonrpc_from_sse(&body);

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 2);

    let contents = json["result"]["contents"]
        .as_array()
        .expect("Expected contents array in read resource response");

    assert_eq!(contents.len(), 1, "Expected exactly one content entry");

    // The text content should be a JSON array (empty, since no sessions created)
    let text = contents[0]["text"]
        .as_str()
        .expect("Expected text field in content");
    let parsed: serde_json::Value =
        serde_json::from_str(text).expect("Content should be valid JSON");
    assert!(parsed.is_array(), "Expected JSON array");
    assert_eq!(
        parsed.as_array().unwrap().len(),
        0,
        "Expected empty array (no sessions created)"
    );
}
