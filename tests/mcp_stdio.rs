//! Integration tests for the `wsh mcp` stdio bridge subcommand.
//!
//! These tests verify that:
//! - The MCP server responds to initialize requests over stdin/stdout
//! - Full tool exercise works (create session, list, manage/kill)
//! - Clean shutdown occurs when stdin is closed
//!
//! Protocol: rmcp's stdio transport uses newline-delimited JSON.
//! Each message is a single JSON object on one line, terminated by `\n`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Spawn `wsh mcp --bind 127.0.0.1:0` as a child process with piped stdin/stdout.
fn spawn_mcp() -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("mcp")
        .arg("--bind")
        .arg("127.0.0.1:0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn wsh mcp")
}

/// Send a JSON-RPC message over stdin using newline-delimited JSON framing.
fn send_jsonrpc(stdin: &mut impl Write, msg: &serde_json::Value) {
    let payload = serde_json::to_string(msg).unwrap();
    writeln!(stdin, "{}", payload).unwrap();
    stdin.flush().unwrap();
}

/// Read a single JSON-RPC response from stdout using newline-delimited JSON.
/// Reads lines until a valid JSON-RPC response (containing "jsonrpc" and "id") is found.
fn read_jsonrpc(reader: &mut BufReader<impl std::io::Read>) -> serde_json::Value {
    loop {
        let mut line = String::new();
        let bytes_read = reader
            .read_line(&mut line)
            .expect("failed to read line from stdout");
        if bytes_read == 0 {
            panic!("unexpected EOF while reading JSON-RPC response from stdout");
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
            // Return any valid JSON-RPC response (has "jsonrpc" field)
            if json.get("jsonrpc").is_some() && json.get("id").is_some() {
                return json;
            }
        }
    }
}

/// Send an initialize request and return the response.
fn initialize(
    stdin: &mut impl Write,
    reader: &mut BufReader<impl std::io::Read>,
) -> serde_json::Value {
    let init_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-stdio",
                "version": "0.1"
            }
        }
    });
    send_jsonrpc(stdin, &init_request);
    read_jsonrpc(reader)
}

/// Send the notifications/initialized notification (required by MCP protocol).
fn send_initialized_notification(stdin: &mut impl Write) {
    let notification = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    send_jsonrpc(stdin, &notification);
}

// ── Test 1: Initialize over stdio ──────────────────────────────────

#[test]
fn test_mcp_stdio_initialize() {
    let mut child = spawn_mcp();
    let mut stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());

    // Use a thread with a timeout to avoid hanging
    let handle = std::thread::spawn(move || {
        let response = initialize(&mut stdin, &mut reader);

        // Verify the response structure
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 1);

        let result = &response["result"];
        assert!(
            result.is_object(),
            "Expected result object in initialize response, got: {}",
            response
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

        // Clean up: drop stdin to close the pipe
        drop(stdin);
    });

    // Wait with timeout
    let timeout = Duration::from_secs(15);
    let start = std::time::Instant::now();
    loop {
        if handle.is_finished() {
            break;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            panic!("test timed out after {:?}", timeout);
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let result = handle.join();
    let _ = child.kill();
    let _ = child.wait();

    result.expect("test thread panicked");
}

// ── Test 2: Full tool exercise over stdio ──────────────────────────

#[test]
fn test_mcp_stdio_full_tool_exercise() {
    let mut child = spawn_mcp();
    let mut stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());

    let handle = std::thread::spawn(move || {
        // 1. Initialize
        let response = initialize(&mut stdin, &mut reader);
        assert_eq!(response["jsonrpc"], "2.0");
        assert!(
            response["result"].is_object(),
            "Initialize should succeed, got: {}",
            response
        );

        // 2. Send notifications/initialized
        send_initialized_notification(&mut stdin);

        // Small delay to let the server process the notification
        std::thread::sleep(Duration::from_millis(200));

        // 3. Create a session via wsh_create_session tool
        let create_request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "wsh_create_session",
                "arguments": {
                    "name": "stdio-test-session"
                }
            }
        });
        send_jsonrpc(&mut stdin, &create_request);
        let create_response = read_jsonrpc(&mut reader);

        assert_eq!(create_response["jsonrpc"], "2.0");
        assert_eq!(create_response["id"], 2);

        // Verify the tool result
        let content = &create_response["result"]["content"];
        assert!(
            content.is_array(),
            "Expected content array in tool result, got: {}",
            create_response
        );
        let text = content[0]["text"]
            .as_str()
            .expect("Expected text content");
        let parsed: serde_json::Value =
            serde_json::from_str(text).expect("Expected valid JSON in tool result");
        assert_eq!(parsed["name"], "stdio-test-session");
        assert!(parsed["rows"].is_number());
        assert!(parsed["cols"].is_number());

        // 4. List sessions via wsh_list_sessions
        let list_request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "wsh_list_sessions",
                "arguments": {}
            }
        });
        send_jsonrpc(&mut stdin, &list_request);
        let list_response = read_jsonrpc(&mut reader);

        assert_eq!(list_response["jsonrpc"], "2.0");
        assert_eq!(list_response["id"], 3);

        let list_text = list_response["result"]["content"][0]["text"]
            .as_str()
            .expect("Expected text content in list response");
        let sessions: Vec<serde_json::Value> =
            serde_json::from_str(list_text).expect("Expected JSON array");
        assert!(
            sessions
                .iter()
                .any(|s| s["name"] == "stdio-test-session"),
            "Session list should contain stdio-test-session, got: {:?}",
            sessions
        );

        // 5. Kill the session via wsh_manage_session
        let kill_request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "wsh_manage_session",
                "arguments": {
                    "session": "stdio-test-session",
                    "action": "kill"
                }
            }
        });
        send_jsonrpc(&mut stdin, &kill_request);
        let kill_response = read_jsonrpc(&mut reader);

        assert_eq!(kill_response["jsonrpc"], "2.0");
        assert_eq!(kill_response["id"], 4);

        let kill_text = kill_response["result"]["content"][0]["text"]
            .as_str()
            .expect("Expected text content in kill response");
        let kill_parsed: serde_json::Value =
            serde_json::from_str(kill_text).expect("Expected valid JSON");
        assert_eq!(kill_parsed["status"], "killed");

        // 6. Verify session is removed by listing again
        let verify_request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "wsh_list_sessions",
                "arguments": {}
            }
        });
        send_jsonrpc(&mut stdin, &verify_request);
        let verify_response = read_jsonrpc(&mut reader);

        assert_eq!(verify_response["jsonrpc"], "2.0");
        assert_eq!(verify_response["id"], 5);

        let verify_text = verify_response["result"]["content"][0]["text"]
            .as_str()
            .expect("Expected text content in verify response");
        let remaining: Vec<serde_json::Value> =
            serde_json::from_str(verify_text).expect("Expected JSON array");
        assert!(
            remaining.is_empty(),
            "Session list should be empty after kill, got: {:?}",
            remaining
        );

        // Clean shutdown
        drop(stdin);
    });

    // Wait with timeout (generous: 30s for process spawn + tool calls)
    let timeout = Duration::from_secs(30);
    let start = std::time::Instant::now();
    loop {
        if handle.is_finished() {
            break;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            panic!("test timed out after {:?}", timeout);
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let result = handle.join();
    let _ = child.kill();
    let _ = child.wait();

    result.expect("test thread panicked");
}

// ── Test 3: Clean shutdown when stdin is closed ────────────────────

#[test]
fn test_mcp_stdio_clean_shutdown() {
    let mut child = spawn_mcp();
    let mut stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());

    // Complete the full initialization handshake, then close stdin
    let handle = std::thread::spawn(move || {
        let response = initialize(&mut stdin, &mut reader);
        assert_eq!(response["jsonrpc"], "2.0");
        assert!(response["result"].is_object());

        // Send initialized notification to complete the handshake
        send_initialized_notification(&mut stdin);

        // Small delay to let the server process the notification
        std::thread::sleep(Duration::from_millis(100));

        // Now close stdin to trigger shutdown
        drop(stdin);
        // reader is also dropped here
    });

    // Wait for the initialization thread to finish
    let timeout = Duration::from_secs(15);
    let start = std::time::Instant::now();
    loop {
        if handle.is_finished() {
            break;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            panic!("initialization timed out");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    handle.join().expect("test thread panicked");

    // Wait for the child process to exit with a timeout
    let exit_timeout = Duration::from_secs(10);
    let exit_start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                assert!(
                    status.success(),
                    "wsh mcp should exit cleanly (exit code 0), got: {:?}",
                    status.code()
                );
                return;
            }
            Ok(None) => {
                if exit_start.elapsed() > exit_timeout {
                    let _ = child.kill();
                    panic!(
                        "wsh mcp did not exit within {:?} after stdin was closed",
                        exit_timeout
                    );
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                panic!("error waiting for child process: {}", e);
            }
        }
    }
}
