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
            server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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

/// Assert that a tool call result is NOT an error.
///
/// The MCP spec allows `isError` to be `false`, absent, or `null` when
/// a tool call succeeds. This helper accepts all three.
fn assert_not_error(json: &serde_json::Value) {
    let is_error = &json["result"]["isError"];
    assert!(
        is_error.is_null() || is_error == false,
        "Expected isError to be false/absent/null, got: {}",
        is_error
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
            server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    // Create router WITH auth token
    let app = router(state, Some("secret-token".to_string()));
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // MCP endpoint should require auth when token is set (C4 fix)
    let resp = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}"#,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "MCP endpoint should require auth when token is set"
    );

    // REST API should also reject without auth
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

// ── Test 9: MCP list prompts ────────────────────────────────────

#[tokio::test]
async fn test_mcp_list_prompts() {
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

    // List prompts
    let body = send_mcp_request_with_session(
        &client,
        addr,
        r#"{"jsonrpc":"2.0","id":2,"method":"prompts/list"}"#,
        &session_id,
    )
    .await;

    let json = extract_jsonrpc_from_sse(&body);

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 2);

    let prompts = json["result"]["prompts"]
        .as_array()
        .expect("Expected prompts array in list prompts response");

    // Should have exactly 9 prompts (one per skill)
    assert_eq!(
        prompts.len(),
        9,
        "Expected 9 prompts, got {}",
        prompts.len()
    );

    // Verify expected prompt names exist
    let prompt_names: Vec<&str> = prompts
        .iter()
        .filter_map(|p| p["name"].as_str())
        .collect();

    assert!(prompt_names.contains(&"wsh:core"), "Missing wsh:core prompt");
    assert!(
        prompt_names.contains(&"wsh:drive-process"),
        "Missing wsh:drive-process prompt"
    );
    assert!(prompt_names.contains(&"wsh:tui"), "Missing wsh:tui prompt");
    assert!(
        prompt_names.contains(&"wsh:multi-session"),
        "Missing wsh:multi-session prompt"
    );
    assert!(
        prompt_names.contains(&"wsh:agent-orchestration"),
        "Missing wsh:agent-orchestration prompt"
    );
    assert!(
        prompt_names.contains(&"wsh:monitor"),
        "Missing wsh:monitor prompt"
    );
    assert!(
        prompt_names.contains(&"wsh:visual-feedback"),
        "Missing wsh:visual-feedback prompt"
    );
    assert!(
        prompt_names.contains(&"wsh:input-capture"),
        "Missing wsh:input-capture prompt"
    );
    assert!(
        prompt_names.contains(&"wsh:generative-ui"),
        "Missing wsh:generative-ui prompt"
    );

    // Verify all prompts have descriptions
    for prompt in prompts {
        assert!(
            prompt["description"].is_string(),
            "Prompt {} should have a description",
            prompt["name"]
        );
    }
}

// ── Test 10: MCP get prompt ─────────────────────────────────────

#[tokio::test]
async fn test_mcp_get_prompt() {
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

    // Get the wsh:core prompt
    let body = send_mcp_request_with_session(
        &client,
        addr,
        r#"{"jsonrpc":"2.0","id":2,"method":"prompts/get","params":{"name":"wsh:core"}}"#,
        &session_id,
    )
    .await;

    let json = extract_jsonrpc_from_sse(&body);

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 2);

    let result = &json["result"];
    assert!(
        result["description"].is_string(),
        "Expected description in get_prompt result"
    );

    let messages = result["messages"]
        .as_array()
        .expect("Expected messages array in get_prompt result");

    assert_eq!(messages.len(), 1, "Expected exactly one message");

    let message = &messages[0];
    assert_eq!(
        message["role"].as_str(),
        Some("user"),
        "Message role should be 'user'"
    );

    let content_text = message["content"]["text"]
        .as_str()
        .expect("Expected text content in message");

    // Verify this is the MCP-adapted core skill
    assert!(
        content_text.contains("wsh:core-mcp"),
        "Core prompt content should contain 'wsh:core-mcp'"
    );
    assert!(
        content_text.contains("wsh_run_command"),
        "Core prompt content should reference wsh_run_command tool"
    );
}

// ── Test 11: MCP get prompt with unknown name ───────────────────

#[tokio::test]
async fn test_mcp_get_prompt_unknown_name() {
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

    // Get a prompt that doesn't exist
    let body = send_mcp_request_with_session(
        &client,
        addr,
        r#"{"jsonrpc":"2.0","id":2,"method":"prompts/get","params":{"name":"nonexistent"}}"#,
        &session_id,
    )
    .await;

    let json = extract_jsonrpc_from_sse(&body);

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 2);

    // Should return a JSON-RPC error
    assert!(
        json["error"].is_object(),
        "Expected error object for unknown prompt name"
    );
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("unknown prompt"),
        "Error message should mention 'unknown prompt'"
    );
}

// ─────────────────────────────────────────────────────────────────
// MCP Tool Call Integration Test Helpers
// ─────────────────────────────────────────────────────────────────

use std::sync::atomic::{AtomicU64, Ordering};

/// Global counter to generate unique request IDs across tests.
static REQUEST_ID_COUNTER: AtomicU64 = AtomicU64::new(100);

fn next_request_id() -> u64 {
    REQUEST_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Set up an MCP session ready for tool calls (initialize + notifications/initialized).
async fn setup_mcp_session(client: &reqwest::Client, addr: SocketAddr) -> String {
    let (_, session_id) = send_initialize_and_get_session(client, addr).await;

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

    session_id
}

/// Helper to call an MCP tool and extract the result.
async fn call_tool(
    client: &reqwest::Client,
    addr: SocketAddr,
    session_id: &str,
    tool_name: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let request_id = next_request_id();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": arguments,
        }
    });

    let response_body = send_mcp_request_with_session(
        client,
        addr,
        &serde_json::to_string(&body).unwrap(),
        session_id,
    )
    .await;

    extract_jsonrpc_from_sse(&response_body)
}

/// Extract the text content from a tool call result.
fn extract_tool_text(json: &serde_json::Value) -> &str {
    json["result"]["content"][0]["text"]
        .as_str()
        .expect("Expected text content in tool result")
}

/// Parse the text content from a tool call result as JSON.
fn parse_tool_result(json: &serde_json::Value) -> serde_json::Value {
    let text = extract_tool_text(json);
    serde_json::from_str(text).expect("Expected valid JSON in tool result text")
}

/// Helper to kill a session during cleanup (best-effort).
async fn cleanup_session(
    client: &reqwest::Client,
    addr: SocketAddr,
    session_id: &str,
    session_name: &str,
) {
    let _ = call_tool(
        client,
        addr,
        session_id,
        "wsh_manage_session",
        serde_json::json!({
            "session": session_name,
            "action": "kill",
        }),
    )
    .await;
}

// ─────────────────────────────────────────────────────────────────
// MCP Tool Call Integration Tests
// ─────────────────────────────────────────────────────────────────

// ── Test 12: Session lifecycle (create + list + manage/kill) ──────

#[tokio::test]
async fn test_mcp_tool_session_lifecycle() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let mcp_session = setup_mcp_session(&client, addr).await;

    let sess_name = "mcp-lifecycle-test";

    // 1. Create a session
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_create_session",
        serde_json::json!({"name": sess_name}),
    )
    .await;
    assert_not_error(&json);
    let result = parse_tool_result(&json);
    assert_eq!(result["name"], sess_name);
    assert!(result["rows"].is_number());
    assert!(result["cols"].is_number());

    // 2. List all sessions — should include our session
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_list_sessions",
        serde_json::json!({}),
    )
    .await;
    assert_not_error(&json);
    let list: Vec<serde_json::Value> =
        serde_json::from_str(extract_tool_text(&json)).unwrap();
    assert!(
        list.iter().any(|s| s["name"] == sess_name),
        "Session list should contain {}",
        sess_name
    );

    // 3. Get detail for specific session
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_list_sessions",
        serde_json::json!({"session": sess_name}),
    )
    .await;
    assert_not_error(&json);
    let detail = parse_tool_result(&json);
    assert_eq!(detail["name"], sess_name);
    assert!(detail["rows"].is_number());
    assert!(detail["cols"].is_number());

    // 4. Kill the session
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_manage_session",
        serde_json::json!({
            "session": sess_name,
            "action": "kill",
        }),
    )
    .await;
    assert_not_error(&json);
    let result = parse_tool_result(&json);
    assert_eq!(result["status"], "killed");

    // 5. List sessions — should be empty
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_list_sessions",
        serde_json::json!({}),
    )
    .await;
    let list: Vec<serde_json::Value> =
        serde_json::from_str(extract_tool_text(&json)).unwrap();
    assert!(
        list.is_empty(),
        "Session list should be empty after kill"
    );
}

// ── Test 13: Duplicate session name → error ──────────────────────

#[tokio::test]
async fn test_mcp_tool_create_duplicate_session() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let mcp_session = setup_mcp_session(&client, addr).await;

    let sess_name = "mcp-dup-test";

    // Create first session
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_create_session",
        serde_json::json!({"name": sess_name}),
    )
    .await;
    assert_not_error(&json);

    // Create second session with same name — should fail
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_create_session",
        serde_json::json!({"name": sess_name}),
    )
    .await;

    // Should be a protocol-level error (invalid_params) since RegistryError::NameExists
    assert!(
        json["error"].is_object(),
        "Expected JSON-RPC error for duplicate session name, got: {}",
        json
    );
    let err_msg = json["error"]["message"].as_str().unwrap_or("");
    assert!(
        err_msg.contains("already exists"),
        "Error message should mention 'already exists', got: {}",
        err_msg
    );

    // Cleanup
    cleanup_session(&client, addr, &mcp_session, sess_name).await;
}

// ── Test 14: wsh_run_command (core agent loop) ───────────────────

#[tokio::test]
async fn test_mcp_tool_run_command() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let mcp_session = setup_mcp_session(&client, addr).await;

    let sess_name = "mcp-runcmd-test";

    // Create session
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_create_session",
        serde_json::json!({"name": sess_name}),
    )
    .await;
    assert_not_error(&json);

    // Give the shell a moment to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Run a command
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_run_command",
        serde_json::json!({
            "session": sess_name,
            "input": "echo hello_wsh_test\n",
            "timeout_ms": 2000,
            "max_wait_ms": 15000,
            "format": "plain",
        }),
    )
    .await;

    // The response should have result.content (success or error)
    let text = extract_tool_text(&json);
    let result: serde_json::Value = serde_json::from_str(text).unwrap();

    // Should have a screen field regardless of idle outcome
    assert!(
        result.get("screen").is_some(),
        "run_command response should contain 'screen' field, got: {}",
        result
    );

    // Cleanup
    cleanup_session(&client, addr, &mcp_session, sess_name).await;
}

// ── Test 15: wsh_send_input + wsh_get_screen ─────────────────────

#[tokio::test]
async fn test_mcp_tool_send_input_and_get_screen() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let mcp_session = setup_mcp_session(&client, addr).await;

    let sess_name = "mcp-input-screen-test";

    // Create session
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_create_session",
        serde_json::json!({"name": sess_name}),
    )
    .await;
    assert_not_error(&json);

    // Send input
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_send_input",
        serde_json::json!({
            "session": sess_name,
            "input": "echo test_marker_123\n",
        }),
    )
    .await;
    assert_not_error(&json);
    let result = parse_tool_result(&json);
    assert_eq!(result["status"], "sent");
    assert!(result["bytes"].is_number());
    assert!(
        result["bytes"].as_u64().unwrap() > 0,
        "Should have sent at least 1 byte"
    );

    // Wait a bit for output to appear
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Get screen
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_get_screen",
        serde_json::json!({
            "session": sess_name,
            "format": "plain",
        }),
    )
    .await;
    assert_not_error(&json);
    let text = extract_tool_text(&json);
    // The response should be valid JSON (screen data)
    let screen: serde_json::Value = serde_json::from_str(text)
        .expect("Screen response should be valid JSON");
    // Screen should have some structural data (rows, cols, cursor, etc.)
    assert!(
        screen.is_object(),
        "Screen response should be a JSON object"
    );

    // Cleanup
    cleanup_session(&client, addr, &mcp_session, sess_name).await;
}

// ── Test 16: Overlay lifecycle (list → create → list → remove → list) ──

#[tokio::test]
async fn test_mcp_tool_overlay_lifecycle() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let mcp_session = setup_mcp_session(&client, addr).await;

    let sess_name = "mcp-overlay-test";

    // Create session
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_create_session",
        serde_json::json!({"name": sess_name}),
    )
    .await;
    assert_not_error(&json);

    // 1. List overlays — should be empty
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_overlay",
        serde_json::json!({
            "session": sess_name,
            "list": true,
        }),
    )
    .await;
    assert_not_error(&json);
    let result = parse_tool_result(&json);
    let overlays = result["overlays"]
        .as_array()
        .expect("overlays should be an array");
    assert!(overlays.is_empty(), "Initially no overlays");

    // 2. Create an overlay
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_overlay",
        serde_json::json!({
            "session": sess_name,
            "x": 0,
            "y": 0,
            "width": 20,
            "height": 3,
            "spans": [{"text": "hello overlay"}],
        }),
    )
    .await;
    assert_not_error(&json);
    let result = parse_tool_result(&json);
    assert_eq!(result["status"], "created");
    let overlay_id = result["id"]
        .as_str()
        .expect("overlay create should return id");

    // 3. List overlays — should have 1
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_overlay",
        serde_json::json!({
            "session": sess_name,
            "list": true,
        }),
    )
    .await;
    let result = parse_tool_result(&json);
    let overlays = result["overlays"].as_array().unwrap();
    assert_eq!(overlays.len(), 1, "Should have 1 overlay after create");

    // 4. Remove the overlay by ID
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_remove_overlay",
        serde_json::json!({
            "session": sess_name,
            "id": overlay_id,
        }),
    )
    .await;
    assert_not_error(&json);
    let result = parse_tool_result(&json);
    assert_eq!(result["status"], "removed");

    // 5. List overlays — should be empty again
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_overlay",
        serde_json::json!({
            "session": sess_name,
            "list": true,
        }),
    )
    .await;
    let result = parse_tool_result(&json);
    let overlays = result["overlays"].as_array().unwrap();
    assert!(overlays.is_empty(), "Overlays should be empty after remove");

    // Cleanup
    cleanup_session(&client, addr, &mcp_session, sess_name).await;
}

// ── Test 17: Panel lifecycle (list → create → list → remove → list) ──

#[tokio::test]
async fn test_mcp_tool_panel_lifecycle() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let mcp_session = setup_mcp_session(&client, addr).await;

    let sess_name = "mcp-panel-test";

    // Create session
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_create_session",
        serde_json::json!({"name": sess_name}),
    )
    .await;
    assert_not_error(&json);

    // 1. List panels — should be empty
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_panel",
        serde_json::json!({
            "session": sess_name,
            "list": true,
        }),
    )
    .await;
    assert_not_error(&json);
    let result = parse_tool_result(&json);
    let panels = result["panels"]
        .as_array()
        .expect("panels should be an array");
    assert!(panels.is_empty(), "Initially no panels");

    // 2. Create a panel
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_panel",
        serde_json::json!({
            "session": sess_name,
            "position": "bottom",
            "height": 2,
            "spans": [{"text": "status panel"}],
        }),
    )
    .await;
    assert_not_error(&json);
    let result = parse_tool_result(&json);
    assert_eq!(result["status"], "created");
    assert!(
        result["id"].is_string(),
        "panel create should return an id"
    );

    // 3. List panels — should have 1
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_panel",
        serde_json::json!({
            "session": sess_name,
            "list": true,
        }),
    )
    .await;
    let result = parse_tool_result(&json);
    let panels = result["panels"].as_array().unwrap();
    assert_eq!(panels.len(), 1, "Should have 1 panel after create");

    // 4. Remove all panels (no id = clear all)
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_remove_panel",
        serde_json::json!({
            "session": sess_name,
        }),
    )
    .await;
    assert_not_error(&json);
    let result = parse_tool_result(&json);
    assert_eq!(result["status"], "cleared");

    // 5. List panels — should be empty again
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_panel",
        serde_json::json!({
            "session": sess_name,
            "list": true,
        }),
    )
    .await;
    let result = parse_tool_result(&json);
    let panels = result["panels"].as_array().unwrap();
    assert!(panels.is_empty(), "Panels should be empty after clear");

    // Cleanup
    cleanup_session(&client, addr, &mcp_session, sess_name).await;
}

// ── Test 18: Input mode (query → capture → release) ─────────────

#[tokio::test]
async fn test_mcp_tool_input_mode() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let mcp_session = setup_mcp_session(&client, addr).await;

    let sess_name = "mcp-inputmode-test";

    // Create session
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_create_session",
        serde_json::json!({"name": sess_name}),
    )
    .await;
    assert_not_error(&json);

    // 1. Query current mode — should be passthrough
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_input_mode",
        serde_json::json!({"session": sess_name}),
    )
    .await;
    assert_not_error(&json);
    let result = parse_tool_result(&json);
    assert_eq!(result["mode"], "passthrough");
    assert!(
        result["focused_element"].is_null(),
        "focused_element should be null initially"
    );

    // 2. Switch to capture mode
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_input_mode",
        serde_json::json!({
            "session": sess_name,
            "mode": "capture",
        }),
    )
    .await;
    assert_not_error(&json);
    let result = parse_tool_result(&json);
    assert_eq!(result["mode"], "capture");

    // 3. Release back to passthrough
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_input_mode",
        serde_json::json!({
            "session": sess_name,
            "mode": "release",
        }),
    )
    .await;
    assert_not_error(&json);
    let result = parse_tool_result(&json);
    assert_eq!(result["mode"], "passthrough");

    // Cleanup
    cleanup_session(&client, addr, &mcp_session, sess_name).await;
}

// ── Test 19: Screen mode (query → enter_alt → exit_alt) ─────────

#[tokio::test]
async fn test_mcp_tool_screen_mode() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let mcp_session = setup_mcp_session(&client, addr).await;

    let sess_name = "mcp-screenmode-test";

    // Create session
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_create_session",
        serde_json::json!({"name": sess_name}),
    )
    .await;
    assert_not_error(&json);

    // 1. Query current mode — should be normal
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_screen_mode",
        serde_json::json!({"session": sess_name}),
    )
    .await;
    assert_not_error(&json);
    let result = parse_tool_result(&json);
    assert_eq!(result["mode"], "normal");

    // 2. Enter alternate screen mode
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_screen_mode",
        serde_json::json!({
            "session": sess_name,
            "action": "enter_alt",
        }),
    )
    .await;
    assert_not_error(&json);
    let result = parse_tool_result(&json);
    assert_eq!(result["mode"], "alt");

    // 3. Exit alternate screen mode
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_screen_mode",
        serde_json::json!({
            "session": sess_name,
            "action": "exit_alt",
        }),
    )
    .await;
    assert_not_error(&json);
    let result = parse_tool_result(&json);
    assert_eq!(result["mode"], "normal");

    // Cleanup
    cleanup_session(&client, addr, &mcp_session, sess_name).await;
}

// ── Test 20: Tool call for nonexistent session → error ───────────

#[tokio::test]
async fn test_mcp_tool_nonexistent_session_error() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let mcp_session = setup_mcp_session(&client, addr).await;

    // Call get_screen for a nonexistent session
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_get_screen",
        serde_json::json!({"session": "nonexistent-session"}),
    )
    .await;

    // Should be a protocol-level error (invalid_params)
    assert!(
        json["error"].is_object(),
        "Expected JSON-RPC error for nonexistent session, got: {}",
        json
    );
    let err_msg = json["error"]["message"].as_str().unwrap_or("");
    assert!(
        err_msg.contains("session not found"),
        "Error message should mention 'session not found', got: {}",
        err_msg
    );
}

// ── Test 21: HTTP API and MCP coexist on the same server ─────────

#[tokio::test]
async fn test_http_and_mcp_coexist() {
    let registry = SessionRegistry::new();
    let state = AppState {
        sessions: registry,
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(ServerConfig::new(false)),
            server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    let app = router(state, None);
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();

    // ── Step 1: Create a session via the HTTP API ───────────────
    let resp = client
        .post(format!("http://{addr}/sessions"))
        .json(&serde_json::json!({"name": "coexist-test"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        201,
        "HTTP create session should return 201 Created"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "coexist-test");

    // ── Step 2: List sessions via the HTTP API ──────────────────
    let resp = client
        .get(format!("http://{addr}/sessions"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let sessions: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert_eq!(sessions.len(), 1, "Should have exactly one session via HTTP");
    assert_eq!(sessions[0]["name"], "coexist-test");

    // ── Step 3: MCP initialize on the same server ───────────────
    let (json, _session_id) = send_initialize_and_get_session(&client, addr).await;
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 1);
    assert!(json["result"]["serverInfo"]["name"].is_string());

    // ── Step 4: Set up full MCP session for tool calls ──────────
    let mcp_session = setup_mcp_session(&client, addr).await;

    // ── Step 5: List sessions via MCP tool — should see the HTTP-created session
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_list_sessions",
        serde_json::json!({}),
    )
    .await;
    assert_not_error(&json);
    let list: Vec<serde_json::Value> =
        serde_json::from_str(extract_tool_text(&json)).unwrap();
    assert!(
        list.iter().any(|s| s["name"] == "coexist-test"),
        "MCP should see session created via HTTP API, got: {:?}",
        list
    );

    // ── Step 6: Create a second session via MCP ─────────────────
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_create_session",
        serde_json::json!({"name": "mcp-created"}),
    )
    .await;
    assert_not_error(&json);
    let result = parse_tool_result(&json);
    assert_eq!(result["name"], "mcp-created");

    // ── Step 7: Verify HTTP API sees the MCP-created session ────
    let resp = client
        .get(format!("http://{addr}/sessions"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let sessions: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert_eq!(
        sessions.len(),
        2,
        "HTTP should see both sessions (HTTP-created + MCP-created)"
    );
    let names: Vec<&str> = sessions
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    assert!(
        names.contains(&"coexist-test"),
        "HTTP list should contain 'coexist-test'"
    );
    assert!(
        names.contains(&"mcp-created"),
        "HTTP list should contain 'mcp-created'"
    );

    // ── Step 8: Access the HTTP-created session via HTTP endpoint ─
    let resp = client
        .get(format!("http://{addr}/sessions/coexist-test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "coexist-test");

    // ── Step 9: Access the MCP-created session via HTTP endpoint ─
    let resp = client
        .get(format!("http://{addr}/sessions/mcp-created"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "mcp-created");

    // ── Step 10: Clean up — delete both sessions via HTTP ────────
    let resp = client
        .delete(format!("http://{addr}/sessions/coexist-test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204, "HTTP delete of coexist-test should succeed");

    let resp = client
        .delete(format!("http://{addr}/sessions/mcp-created"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204, "HTTP delete of mcp-created should succeed");

    // ── Step 11: Verify both APIs see empty state ────────────────
    let resp = client
        .get(format!("http://{addr}/sessions"))
        .send()
        .await
        .unwrap();
    let sessions: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert!(
        sessions.is_empty(),
        "HTTP should see no sessions after cleanup"
    );

    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_list_sessions",
        serde_json::json!({}),
    )
    .await;
    let list: Vec<serde_json::Value> =
        serde_json::from_str(extract_tool_text(&json)).unwrap();
    assert!(
        list.is_empty(),
        "MCP should see no sessions after cleanup"
    );
}

// ── Test 22: Manage session rename ───────────────────────────────

#[tokio::test]
async fn test_mcp_tool_manage_session_rename() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let mcp_session = setup_mcp_session(&client, addr).await;

    let old_name = "mcp-rename-old";
    let new_name = "mcp-rename-new";

    // Create session with old name
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_create_session",
        serde_json::json!({"name": old_name}),
    )
    .await;
    assert_not_error(&json);

    // Rename the session
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_manage_session",
        serde_json::json!({
            "session": old_name,
            "action": "rename",
            "new_name": new_name,
        }),
    )
    .await;
    assert_not_error(&json);
    let result = parse_tool_result(&json);
    assert_eq!(result["status"], "renamed");
    assert_eq!(result["old_name"], old_name);
    assert_eq!(result["new_name"], new_name);

    // Verify list shows new name, not old
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_list_sessions",
        serde_json::json!({}),
    )
    .await;
    let list: Vec<serde_json::Value> =
        serde_json::from_str(extract_tool_text(&json)).unwrap();

    let names: Vec<&str> = list
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();

    assert!(
        names.contains(&new_name),
        "Session list should contain new name '{}', got: {:?}",
        new_name,
        names
    );
    assert!(
        !names.contains(&old_name),
        "Session list should NOT contain old name '{}', got: {:?}",
        old_name,
        names
    );

    // Cleanup
    cleanup_session(&client, addr, &mcp_session, new_name).await;
}

// ── Test 23: wsh_get_scrollback ──────────────────────────────────

#[tokio::test]
async fn test_mcp_tool_get_scrollback() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let mcp_session = setup_mcp_session(&client, addr).await;

    let sess_name = "mcp-scrollback-test";

    // Create session
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_create_session",
        serde_json::json!({"name": sess_name}),
    )
    .await;
    assert_not_error(&json);

    // Give the shell a moment to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Get scrollback with offset and limit
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_get_scrollback",
        serde_json::json!({
            "session": sess_name,
            "offset": 0,
            "limit": 10,
            "format": "plain",
        }),
    )
    .await;
    assert_not_error(&json);

    // The result should be a valid JSON object (parser response)
    let result = parse_tool_result(&json);
    assert!(
        result.is_object(),
        "Scrollback response should be a JSON object, got: {}",
        result
    );

    // Cleanup
    cleanup_session(&client, addr, &mcp_session, sess_name).await;
}

// ── Test 24: wsh_await_idle ───────────────────────────────────────

#[tokio::test]
async fn test_mcp_tool_await_idle() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let mcp_session = setup_mcp_session(&client, addr).await;

    let sess_name = "mcp-idle-test";

    // Create session
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_create_session",
        serde_json::json!({"name": sess_name}),
    )
    .await;
    assert_not_error(&json);

    // Give the shell a moment to start and settle
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Await idle -- a freshly created session should settle quickly
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_await_idle",
        serde_json::json!({
            "session": sess_name,
            "timeout_ms": 500,
            "max_wait_ms": 5000,
        }),
    )
    .await;
    assert_not_error(&json);

    let result = parse_tool_result(&json);
    assert_eq!(
        result["status"], "idle",
        "Expected status 'idle', got: {}",
        result
    );
    assert!(
        result["generation"].is_number(),
        "Expected generation number in idle response, got: {}",
        result
    );

    // Cleanup
    cleanup_session(&client, addr, &mcp_session, sess_name).await;
}

// ── Test 25: wsh_send_input with base64 encoding ─────────────────

#[tokio::test]
async fn test_mcp_tool_send_input_base64() {
    let app = create_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let mcp_session = setup_mcp_session(&client, addr).await;

    let sess_name = "mcp-base64-input-test";

    // Create session
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_create_session",
        serde_json::json!({"name": sess_name}),
    )
    .await;
    assert_not_error(&json);

    // Send base64-encoded Ctrl-C (byte 0x03 = "Aw==" in base64)
    let json = call_tool(
        &client,
        addr,
        &mcp_session,
        "wsh_send_input",
        serde_json::json!({
            "session": sess_name,
            "input": "Aw==",
            "encoding": "base64",
        }),
    )
    .await;
    assert_not_error(&json);

    let result = parse_tool_result(&json);
    assert_eq!(
        result["status"], "sent",
        "Expected status 'sent', got: {}",
        result
    );
    assert_eq!(
        result["bytes"], 1,
        "Expected exactly 1 byte sent (Ctrl-C), got: {}",
        result["bytes"]
    );

    // Cleanup
    cleanup_session(&client, addr, &mcp_session, sess_name).await;
}
