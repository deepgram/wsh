//! End-to-end tests for `wsh stop`.
//!
//! These tests spawn a real `wsh server` process and verify that `wsh stop`
//! shuts it down gracefully, and that `wsh stop` with no server running
//! exits cleanly.

use std::time::Duration;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(50);

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

/// Creates a session via POST /sessions. Returns the session name.
async fn create_session(port: u16) -> String {
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}/sessions", port);
    let resp = client
        .post(&url)
        .json(&serde_json::json!({"name": "test"}))
        .send()
        .await
        .expect("session create request failed");
    assert_eq!(resp.status(), 201, "expected 201 Created");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["name"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_wsh_stop_shuts_down_server() {
    // Find an available port
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    // Use a unique socket path
    let socket_dir = tempfile::TempDir::new().unwrap();
    let socket_path = socket_dir.path().join("test-stop.sock");

    // Spawn wsh server (NOT ephemeral, so it won't exit on session kill)
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("server")
        .arg("--bind")
        .arg(format!("127.0.0.1:{}", port))
        .arg("--socket")
        .arg(&socket_path)
        .arg("--server-name")
        .arg("stop-e2e-test")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("Failed to spawn wsh server");

    // Wait for server to be ready
    wait_for_ready(port)
        .await
        .expect("wsh should become ready");

    // Create a session to verify the server is functional
    let _session_name = create_session(port).await;

    // Run `wsh stop`
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("stop")
        .arg("--socket")
        .arg(&socket_path)
        .output()
        .expect("failed to run wsh stop");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "wsh stop should succeed. stdout: {}, stderr: {}",
        stdout,
        stderr,
    );
    assert!(
        stdout.contains("Server stopped."),
        "expected 'Server stopped.' in output, got: {}",
        stdout,
    );

    // Server process should have exited
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("try_wait failed") {
            println!("wsh exited with status: {:?}", status);
            break;
        }
        if start.elapsed() > SHUTDOWN_TIMEOUT {
            child.kill().ok();
            panic!("wsh server did not exit after stop");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Socket file should be gone
    assert!(
        !socket_path.exists(),
        "socket file should be removed after stop"
    );
}

#[tokio::test]
async fn test_wsh_stop_no_server_running() {
    // Point at a nonexistent socket
    let socket_dir = tempfile::TempDir::new().unwrap();
    let socket_path = socket_dir.path().join("nonexistent.sock");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("stop")
        .arg("--socket")
        .arg(&socket_path)
        .output()
        .expect("failed to run wsh stop");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "wsh stop should exit 0 when no server is running. stderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        stdout.contains("No server running."),
        "expected 'No server running.' in output, got: {}",
        stdout,
    );
}
