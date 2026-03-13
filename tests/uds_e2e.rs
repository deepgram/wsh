//! End-to-end test for HTTP-over-UDS basic operations.
//!
//! Spawns a real `wsh server` process with `--ephemeral`, connects to the
//! HTTP-over-UDS socket, and exercises basic API operations: health check,
//! session create, session list, and session delete.

use std::time::Duration;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Wait for the HTTP UDS socket to appear on disk and respond to health checks.
async fn wait_for_http_socket(http_socket_path: &std::path::Path) -> Result<(), &'static str> {
    let client = wsh::uds_client::UdsHttpClient::new(http_socket_path);
    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;

    while tokio::time::Instant::now() < deadline {
        if client.health_check().await {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err("HTTP UDS socket did not become ready in time")
}

#[tokio::test]
async fn test_http_over_uds_basic_operations() {
    let socket_dir = tempfile::TempDir::new().unwrap();
    let socket_path = socket_dir.path().join("uds-e2e.sock");
    let http_socket_path = socket_path.with_extension("http.sock");

    // Spawn wsh server (ephemeral, UDS only — no --bind)
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("server")
        .arg("--ephemeral")
        .arg("--socket")
        .arg(&socket_path)
        .arg("--server-name")
        .arg("uds-e2e-test")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn wsh server");

    // Wait for the HTTP UDS socket to be ready
    wait_for_http_socket(&http_socket_path)
        .await
        .expect("wsh HTTP UDS socket should become ready");

    let client = wsh::uds_client::UdsHttpClient::new(&http_socket_path);

    // ── 1. GET /health -> 200 ────────────────────────────────────
    let resp = client.get("/health").await.expect("health request failed");
    assert_eq!(resp.status, 200, "GET /health should return 200");
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["status"], "ok", "health should report status ok");

    // ── 2. POST /sessions -> 201 ─────────────────────────────────
    let resp = client
        .post_json("/sessions", &serde_json::json!({"name": "test"}))
        .await
        .expect("session create request failed");
    assert_eq!(
        resp.status, 201,
        "POST /sessions should return 201 Created"
    );
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["name"], "test", "created session name should be 'test'");

    // ── 3. GET /sessions -> 200, list contains "test" ────────────
    let resp = client
        .get("/sessions")
        .await
        .expect("session list request failed");
    assert_eq!(resp.status, 200, "GET /sessions should return 200");
    let json: serde_json::Value = resp.json().await.unwrap();
    let sessions = json.as_array().expect("sessions should be an array");
    let has_test = sessions
        .iter()
        .any(|s| s["name"].as_str() == Some("test"));
    assert!(
        has_test,
        "session list should contain 'test', got: {:?}",
        sessions
    );

    // ── 4. DELETE /sessions/test -> 200 ──────────────────────────
    let resp = client
        .delete("/sessions/test")
        .await
        .expect("session delete request failed");
    assert_eq!(
        resp.status, 204,
        "DELETE /sessions/test should return 204 No Content"
    );

    // ── Cleanup: server should exit (ephemeral with no sessions) ─
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().expect("try_wait failed") {
            eprintln!("wsh server exited with status: {:?}", status);
            break;
        }
        if start.elapsed() > timeout {
            child.kill().ok();
            panic!("wsh server did not exit after last session was deleted (ephemeral mode)");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // HTTP socket file should be cleaned up
    assert!(
        !http_socket_path.exists(),
        "HTTP socket file should be removed after server exits"
    );
}
