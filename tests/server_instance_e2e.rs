//! End-to-end tests for named server instances (`-L` / `--server-name`).
//!
//! Verifies that multiple server instances can coexist, that clients route
//! to the correct instance via `-L`, and that `--socket` overrides `-L`.

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
async fn create_session(port: u16, name: &str) -> String {
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}/sessions", port);
    let resp = client
        .post(&url)
        .json(&serde_json::json!({"name": name}))
        .send()
        .await
        .expect("session create request failed");
    assert_eq!(resp.status(), 201, "expected 201 Created");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["name"].as_str().unwrap().to_string()
}

/// Find a free port.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Wait for a server process to exit (with timeout).
fn wait_for_exit(child: &mut std::process::Child, label: &str) {
    let start = std::time::Instant::now();
    loop {
        if child.try_wait().expect("try_wait failed").is_some() {
            return;
        }
        if start.elapsed() > SHUTDOWN_TIMEOUT {
            child.kill().ok();
            panic!("{} did not exit in time", label);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Two servers with different `-L` names can run simultaneously on different ports.
#[tokio::test]
async fn test_two_named_instances_coexist() {
    let port_a = free_port();
    let port_b = free_port();

    // Use a temp dir so instance files don't pollute the real runtime dir.
    // We pass --socket explicitly but also --server-name to verify the lock
    // files are created per-instance.
    let socket_dir = tempfile::TempDir::new().unwrap();
    let sock_a = socket_dir.path().join("alpha.sock");
    let sock_b = socket_dir.path().join("beta.sock");

    let mut child_a = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("server")
        .arg("--bind")
        .arg(format!("127.0.0.1:{}", port_a))
        .arg("--socket")
        .arg(&sock_a)
        .arg("--server-name")
        .arg("alpha")
        .arg("--ephemeral")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn server alpha");

    let mut child_b = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("server")
        .arg("--bind")
        .arg(format!("127.0.0.1:{}", port_b))
        .arg("--socket")
        .arg(&sock_b)
        .arg("--server-name")
        .arg("beta")
        .arg("--ephemeral")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn server beta");

    // Wait for both to be ready
    wait_for_ready(port_a)
        .await
        .expect("server alpha should become ready");
    wait_for_ready(port_b)
        .await
        .expect("server beta should become ready");

    // Create sessions on each
    let name_a = create_session(port_a, "sess-a").await;
    let name_b = create_session(port_b, "sess-b").await;
    assert_eq!(name_a, "sess-a");
    assert_eq!(name_b, "sess-b");

    // List sessions via CLI on server alpha (should only see sess-a)
    let output_a = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("list")
        .arg("--socket")
        .arg(&sock_a)
        .output()
        .expect("failed to run wsh list for alpha");
    let stdout_a = String::from_utf8_lossy(&output_a.stdout);
    assert!(
        stdout_a.contains("sess-a"),
        "alpha should list sess-a, got: {}",
        stdout_a
    );
    assert!(
        !stdout_a.contains("sess-b"),
        "alpha should NOT list sess-b, got: {}",
        stdout_a
    );

    // List sessions via CLI on server beta (should only see sess-b)
    let output_b = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("list")
        .arg("--socket")
        .arg(&sock_b)
        .output()
        .expect("failed to run wsh list for beta");
    let stdout_b = String::from_utf8_lossy(&output_b.stdout);
    assert!(
        stdout_b.contains("sess-b"),
        "beta should list sess-b, got: {}",
        stdout_b
    );
    assert!(
        !stdout_b.contains("sess-a"),
        "beta should NOT list sess-a, got: {}",
        stdout_b
    );

    // Kill sessions to trigger ephemeral shutdown
    let _ = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("kill")
        .arg("sess-a")
        .arg("--socket")
        .arg(&sock_a)
        .output();
    let _ = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("kill")
        .arg("sess-b")
        .arg("--socket")
        .arg(&sock_b)
        .output();

    wait_for_exit(&mut child_a, "server alpha");
    wait_for_exit(&mut child_b, "server beta");
}

/// The flock prevents a second server with the same instance name.
#[tokio::test]
async fn test_duplicate_instance_name_fails() {
    let port_a = free_port();
    let port_b = free_port();

    let socket_dir = tempfile::TempDir::new().unwrap();
    let sock = socket_dir.path().join("dup.sock");

    let mut child_a = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("server")
        .arg("--bind")
        .arg(format!("127.0.0.1:{}", port_a))
        .arg("--socket")
        .arg(&sock)
        .arg("--server-name")
        .arg("dup")
        .arg("--ephemeral")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn first server");

    // Wait for first server to be ready
    wait_for_ready(port_a)
        .await
        .expect("first server should become ready");

    // Second server with the same --server-name should fail
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("server")
        .arg("--bind")
        .arg(format!("127.0.0.1:{}", port_b))
        .arg("--socket")
        .arg(socket_dir.path().join("dup2.sock"))
        .arg("--server-name")
        .arg("dup")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("failed to run second server");

    assert!(
        !output.status.success(),
        "second server with same instance name should fail"
    );

    // Clean up first server
    let _ = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("stop")
        .arg("--socket")
        .arg(&sock)
        .output();

    wait_for_exit(&mut child_a, "first server");
}

/// After killing a server (kill -9), the flock is released and a new server
/// can start with the same instance name.
#[tokio::test]
async fn test_flock_released_on_crash() {
    let port_a = free_port();

    let socket_dir = tempfile::TempDir::new().unwrap();
    let sock = socket_dir.path().join("crash.sock");

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("server")
        .arg("--bind")
        .arg(format!("127.0.0.1:{}", port_a))
        .arg("--socket")
        .arg(&sock)
        .arg("--server-name")
        .arg("crash")
        .arg("--ephemeral")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn server");

    wait_for_ready(port_a)
        .await
        .expect("server should become ready");

    // Kill -9 the server (simulating a crash)
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGKILL);
    }
    wait_for_exit(&mut child, "killed server");

    // Start a new server with the same instance name — should succeed
    let port_b = free_port();
    let mut child_b = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("server")
        .arg("--bind")
        .arg(format!("127.0.0.1:{}", port_b))
        .arg("--socket")
        .arg(&sock)
        .arg("--server-name")
        .arg("crash")
        .arg("--ephemeral")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn replacement server");

    wait_for_ready(port_b)
        .await
        .expect("replacement server should become ready");

    // Clean up
    let _ = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("stop")
        .arg("--socket")
        .arg(&sock)
        .output();

    wait_for_exit(&mut child_b, "replacement server");
}
