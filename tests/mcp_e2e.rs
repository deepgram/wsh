//! End-to-end tests for `wsh mcp` stdio bridge + ephemeral server.
//!
//! These tests spawn real `wsh server --ephemeral` and `wsh mcp` processes and
//! verify that:
//! - The full MCP initialize → tool call → response cycle works over the WS bridge
//! - The ephemeral server stays alive while `wsh mcp` is connected
//! - Killing `wsh mcp` (abrupt disconnect) causes the ephemeral server to exit
//! - Multiple concurrent `wsh mcp` instances: last disconnect triggers shutdown

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(50);

const INITIALIZE_REQUEST: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}"#;
const INITIALIZED_NOTIFICATION: &str =
    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
const TOOLS_LIST_REQUEST: &str =
    r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;

/// Waits for wsh to be ready by polling the health endpoint.
async fn wait_for_ready(port: u16) -> Result<(), &'static str> {
    let url = format!("http://127.0.0.1:{}/health", port);
    let client = reqwest::Client::new();

    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
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

/// Spawns `wsh server --bind ... --ephemeral` with unique socket and server name.
fn spawn_server(
    port: u16,
    socket_path: &std::path::Path,
    instance_name: &str,
) -> Child {
    Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("server")
        .arg("--bind")
        .arg(format!("127.0.0.1:{}", port))
        .arg("--socket")
        .arg(socket_path)
        .arg("--server-name")
        .arg(instance_name)
        .arg("--ephemeral")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn wsh server")
}

/// Spawns `wsh mcp --socket ...` with piped stdin/stdout.
fn spawn_mcp_bridge(socket_path: &std::path::Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("mcp")
        .arg("--socket")
        .arg(socket_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn wsh mcp")
}

/// Send a line of JSON to the child's stdin.
fn send_line(child: &mut Child, line: &str) {
    let stdin = child.stdin.as_mut().expect("stdin not piped");
    writeln!(stdin, "{}", line).expect("write to stdin failed");
    stdin.flush().expect("flush stdin failed");
}

/// Read one line of JSON from the child's stdout (blocking, with timeout).
/// Returns the parsed JSON value.
fn read_json_line(child: &mut Child, timeout: Duration) -> serde_json::Value {
    use std::io::BufRead;

    let stdout = child.stdout.as_mut().expect("stdout not piped");

    // Set a read timeout by spawning a thread (std::process stdout is blocking)
    let stdout_fd = {
        use std::os::unix::io::AsRawFd;
        stdout.as_raw_fd()
    };
    // Safety: we duplicate the fd to create an independent File for the reader thread
    let dup_fd = unsafe { libc::dup(stdout_fd) };
    assert!(dup_fd >= 0, "dup() failed");
    let file = unsafe { std::fs::File::from_raw_fd(dup_fd) };

    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(file);
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => tx.send(Err("EOF on stdout".to_string())),
            Ok(_) => tx.send(Ok(line)),
            Err(e) => tx.send(Err(format!("read error: {}", e))),
        }
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(line)) => {
            let _ = handle.join();
            serde_json::from_str(line.trim())
                .unwrap_or_else(|e| panic!("invalid JSON: {}: {:?}", e, line.trim()))
        }
        Ok(Err(e)) => panic!("stdout read failed: {}", e),
        Err(_) => panic!("timeout reading from wsh mcp stdout"),
    }
}

use std::os::unix::io::FromRawFd;

/// Wait for a child process to exit within the given timeout.
/// Returns true if it exited, false if we had to kill it.
fn wait_for_exit(child: &mut Child, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        if let Some(_status) = child.try_wait().expect("try_wait failed") {
            return true;
        }
        if start.elapsed() > timeout {
            child.kill().ok();
            let _ = child.wait();
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ── Test 1: Full MCP cycle via wsh mcp process ─────────────────────

#[tokio::test]
async fn test_mcp_e2e_full_cycle() {
    // Allocate port and socket
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let socket_dir = tempfile::TempDir::new().unwrap();
    let socket_path = socket_dir.path().join("mcp-cycle.sock");

    // Spawn ephemeral server
    let mut server = spawn_server(port, &socket_path, "mcp-e2e-cycle");
    wait_for_ready(port).await.expect("server should start");

    // Spawn wsh mcp bridge
    let mut mcp = spawn_mcp_bridge(&socket_path);

    // Send initialize
    send_line(&mut mcp, INITIALIZE_REQUEST);
    let init_resp = read_json_line(&mut mcp, Duration::from_secs(10));
    assert_eq!(init_resp["jsonrpc"], "2.0");
    assert_eq!(init_resp["id"], 1);
    assert_eq!(init_resp["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(init_resp["result"]["serverInfo"]["name"], "wsh");

    // Send initialized notification
    send_line(&mut mcp, INITIALIZED_NOTIFICATION);

    // Send tools/list
    send_line(&mut mcp, TOOLS_LIST_REQUEST);
    let tools_resp = read_json_line(&mut mcp, Duration::from_secs(10));
    assert_eq!(tools_resp["jsonrpc"], "2.0");
    assert_eq!(tools_resp["id"], 2);
    let tools = tools_resp["result"]["tools"]
        .as_array()
        .expect("expected tools array");
    assert!(!tools.is_empty(), "should have tools");
    let tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(
        tool_names.contains(&"wsh_create_session"),
        "expected wsh_create_session, got: {:?}",
        tool_names
    );
    assert!(
        tool_names.contains(&"wsh_send_input"),
        "expected wsh_send_input, got: {:?}",
        tool_names
    );

    // Close stdin → wsh mcp should exit
    drop(mcp.stdin.take());
    assert!(
        wait_for_exit(&mut mcp, Duration::from_secs(5)),
        "wsh mcp should exit after stdin EOF"
    );

    // Ephemeral server should exit (no sessions, no connections)
    assert!(
        wait_for_exit(&mut server, SHUTDOWN_TIMEOUT),
        "ephemeral server should exit after wsh mcp disconnects"
    );
}

// ── Test 2: wsh mcp keeps ephemeral server alive ────────────────────

#[tokio::test]
async fn test_mcp_e2e_keeps_server_alive() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let socket_dir = tempfile::TempDir::new().unwrap();
    let socket_path = socket_dir.path().join("mcp-alive.sock");

    let mut server = spawn_server(port, &socket_path, "mcp-e2e-alive");
    wait_for_ready(port).await.expect("server should start");

    // Spawn wsh mcp bridge and initialize
    let mut mcp = spawn_mcp_bridge(&socket_path);
    send_line(&mut mcp, INITIALIZE_REQUEST);
    let _init = read_json_line(&mut mcp, Duration::from_secs(10));

    // Wait a bit — server should still be alive (no sessions, but MCP
    // connection holds active_count > 0, preventing idle shutdown)
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        server.try_wait().unwrap().is_none(),
        "ephemeral server should stay alive while wsh mcp is connected"
    );

    // Now close the bridge
    drop(mcp.stdin.take());
    assert!(
        wait_for_exit(&mut mcp, Duration::from_secs(5)),
        "wsh mcp should exit"
    );

    // Server should exit now
    assert!(
        wait_for_exit(&mut server, SHUTDOWN_TIMEOUT),
        "ephemeral server should exit after wsh mcp disconnects"
    );
}

// ── Test 3: Kill wsh mcp → ephemeral server exits ───────────────────

#[tokio::test]
async fn test_mcp_e2e_kill_bridge_triggers_shutdown() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let socket_dir = tempfile::TempDir::new().unwrap();
    let socket_path = socket_dir.path().join("mcp-kill.sock");

    let mut server = spawn_server(port, &socket_path, "mcp-e2e-kill");
    wait_for_ready(port).await.expect("server should start");

    // Spawn and initialize
    let mut mcp = spawn_mcp_bridge(&socket_path);
    send_line(&mut mcp, INITIALIZE_REQUEST);
    let _init = read_json_line(&mut mcp, Duration::from_secs(10));

    // SIGKILL the bridge (simulates crash — no close frame, no cleanup)
    unsafe {
        libc::kill(mcp.id() as libc::pid_t, libc::SIGKILL);
    }
    let _ = mcp.wait(); // reap zombie

    // Ephemeral server should detect the broken connection and exit.
    // Over UDS, the kernel closes the socket immediately on process death,
    // so the server detects it promptly (no TCP keepalive delay).
    assert!(
        wait_for_exit(&mut server, SHUTDOWN_TIMEOUT),
        "ephemeral server should exit after wsh mcp is killed"
    );
}

// ── Test 4: Multiple wsh mcp instances, last disconnect triggers shutdown ──

#[tokio::test]
async fn test_mcp_e2e_multiple_bridges_last_disconnect() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let socket_dir = tempfile::TempDir::new().unwrap();
    let socket_path = socket_dir.path().join("mcp-multi.sock");

    let mut server = spawn_server(port, &socket_path, "mcp-e2e-multi");
    wait_for_ready(port).await.expect("server should start");

    // Spawn two bridges
    let mut mcp1 = spawn_mcp_bridge(&socket_path);
    send_line(&mut mcp1, INITIALIZE_REQUEST);
    let _init1 = read_json_line(&mut mcp1, Duration::from_secs(10));

    let mut mcp2 = spawn_mcp_bridge(&socket_path);
    send_line(&mut mcp2, INITIALIZE_REQUEST);
    let _init2 = read_json_line(&mut mcp2, Duration::from_secs(10));

    // Kill the first bridge
    drop(mcp1.stdin.take());
    assert!(
        wait_for_exit(&mut mcp1, Duration::from_secs(5)),
        "first wsh mcp should exit"
    );

    // Server should still be alive (second bridge holds interest)
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        server.try_wait().unwrap().is_none(),
        "server should stay alive while second wsh mcp is connected"
    );

    // Kill the second bridge
    drop(mcp2.stdin.take());
    assert!(
        wait_for_exit(&mut mcp2, Duration::from_secs(5)),
        "second wsh mcp should exit"
    );

    // Now the server should exit
    assert!(
        wait_for_exit(&mut server, SHUTDOWN_TIMEOUT),
        "ephemeral server should exit after all wsh mcp bridges disconnect"
    );
}
