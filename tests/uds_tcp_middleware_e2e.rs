//! End-to-end test verifying that UDS connections bypass TCP-only middleware.
//!
//! Spawns a `wsh server` with --bind 0.0.0.0 (non-loopback, so auth is enforced),
//! --token, and --base-prefix. Verifies:
//! - UDS connections work without auth, at bare paths
//! - TCP connections require auth, use prefixed paths
//! - TCP connections without auth are rejected

use std::time::Duration;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

async fn wait_for_ready(port: u16) -> Result<(), &'static str> {
    // /health is always available at the root, even with --base-prefix
    let url = format!("http://127.0.0.1:{}/health", port);
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err("wsh did not become ready in time")
}

async fn wait_for_http_socket(path: &std::path::Path) -> Result<(), &'static str> {
    let client = wsh::uds_client::UdsHttpClient::new(path);
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
async fn uds_bypasses_tcp_middleware() {
    let port = free_port();
    let token = "test-secret-token-1234";
    let socket_dir = tempfile::TempDir::new().unwrap();
    let socket_path = socket_dir.path().join("mw.sock");
    let http_socket_path = socket_path.with_extension("http.sock");

    // Bind to 0.0.0.0 (non-loopback) so that --token is respected and auth
    // middleware is applied to TCP connections.
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("server")
        .arg("--ephemeral")
        .arg("--bind")
        .arg(format!("0.0.0.0:{}", port))
        .arg("--token")
        .arg(token)
        .arg("--base-prefix")
        .arg("/wsh")
        .arg("--socket")
        .arg(&socket_path)
        .arg("--server-name")
        .arg("uds-mw-e2e")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn wsh server");

    wait_for_http_socket(&http_socket_path)
        .await
        .unwrap_or_else(|msg| {
            // If the socket never appeared, check if the child already exited
            let status = child.try_wait().expect("try_wait");
            let stderr = child.stderr.take().map(|mut s| {
                let mut buf = String::new();
                std::io::Read::read_to_string(&mut s, &mut buf).ok();
                buf
            });
            panic!(
                "{}\nchild status: {:?}\nstderr: {}",
                msg,
                status,
                stderr.unwrap_or_default()
            );
        });
    wait_for_ready(port).await.expect("TCP should be ready");

    // ── UDS: works WITHOUT auth, at BARE paths ──────────────
    let uds = wsh::uds_client::UdsHttpClient::new(&http_socket_path);

    let resp = uds.get("/health").await.expect("UDS health");
    assert_eq!(resp.status, 200, "UDS /health should work without auth");

    let resp = uds.get("/sessions").await.expect("UDS sessions");
    assert_eq!(resp.status, 200, "UDS /sessions should work without auth");

    let resp = uds
        .post_json("/sessions", &serde_json::json!({"name": "uds-test"}))
        .await
        .expect("UDS session create");
    assert_eq!(resp.status, 201, "UDS should create session without auth");

    // ── TCP: requires auth, uses PREFIXED paths ─────────────
    let http = reqwest::Client::new();

    let resp = http
        .get(format!("http://127.0.0.1:{}/wsh/sessions", port))
        .send()
        .await
        .expect("TCP sessions no-auth");
    assert_eq!(resp.status(), 401, "TCP /wsh/sessions without auth should be 401");

    let resp = http
        .get(format!("http://127.0.0.1:{}/wsh/sessions", port))
        .header("authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("TCP sessions with-auth");
    assert_eq!(resp.status(), 200, "TCP /wsh/sessions with auth should be 200");

    let resp = http
        .get(format!("http://127.0.0.1:{}/sessions", port))
        .header("authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("TCP bare sessions");
    assert_eq!(resp.status(), 404, "TCP /sessions (no prefix) should be 404");

    // ── Cleanup ─────────────────────────────────────────────
    let resp = uds.delete("/sessions/uds-test").await.expect("delete");
    assert_eq!(resp.status, 204);

    let start = std::time::Instant::now();
    loop {
        if child.try_wait().expect("try_wait").is_some() {
            break;
        }
        if start.elapsed() > Duration::from_secs(10) {
            child.kill().ok();
            panic!("server did not exit");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[tokio::test]
async fn uds_websocket_works_without_auth() {
    let port = free_port();
    let token = "test-secret-token-5678";
    let socket_dir = tempfile::TempDir::new().unwrap();
    let socket_path = socket_dir.path().join("ws-mw.sock");
    let http_socket_path = socket_path.with_extension("http.sock");

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("server")
        .arg("--ephemeral")
        .arg("--bind")
        .arg(format!("0.0.0.0:{}", port))
        .arg("--token")
        .arg(token)
        .arg("--base-prefix")
        .arg("/wsh")
        .arg("--socket")
        .arg(&socket_path)
        .arg("--server-name")
        .arg("ws-mw-e2e")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn wsh server");

    wait_for_http_socket(&http_socket_path)
        .await
        .expect("UDS should be ready");
    wait_for_ready(port).await.expect("TCP should be ready");

    // Create session via UDS
    let uds = wsh::uds_client::UdsHttpClient::new(&http_socket_path);
    let resp = uds
        .post_json("/sessions", &serde_json::json!({"name": "ws-test"}))
        .await
        .expect("create session");
    assert_eq!(resp.status, 201);

    // ── UDS WebSocket: works without auth ───────────────────
    let ws_stream = tokio::net::UnixStream::connect(&http_socket_path)
        .await
        .expect("UDS connect");
    let (mut ws, _resp) =
        tokio_tungstenite::client_async("ws://localhost/sessions/ws-test/ws/json", ws_stream)
            .await
            .expect("UDS WS upgrade should succeed without auth");

    // Read the initial {"connected": true} message
    use futures::StreamExt;
    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("should get message in time")
        .expect("stream should have message")
        .expect("message should be valid");
    let text = msg.into_text().expect("should be text");
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["connected"], true, "UDS WS should connect");

    // Close WS
    let _ = futures::SinkExt::close(&mut ws).await;

    // ── TCP WebSocket without auth: rejected ────────────────
    use tokio_tungstenite::tungstenite;
    let tcp_url = format!("ws://127.0.0.1:{}/wsh/sessions/ws-test/ws/json", port);
    let result = tokio_tungstenite::connect_async(&tcp_url).await;
    match result {
        Err(tungstenite::Error::Http(resp)) => {
            assert_eq!(resp.status(), 401, "TCP WS without auth should be 401");
        }
        other => panic!("expected HTTP 401 error, got: {:?}", other),
    }

    // ── Cleanup ─────────────────────────────────────────────
    let _ = uds.delete("/sessions/ws-test").await;
    let start = std::time::Instant::now();
    loop {
        if child.try_wait().expect("try_wait").is_some() {
            break;
        }
        if start.elapsed() > Duration::from_secs(10) {
            child.kill().ok();
            panic!("server did not exit");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
