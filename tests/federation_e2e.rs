//! End-to-end federation test.
//!
//! This test exercises the full federation lifecycle by spawning real wsh server
//! processes and interacting with them over HTTP. It covers:
//!
//! 1. Starting two server instances (hub and backend) with unique names
//! 2. Verifying both servers are healthy independently
//! 3. Testing federation management endpoints on the hub
//! 4. Cross-server session proxy (via in-process test with `add_unchecked`)
//!
//! The cross-server proxy test uses an in-process hub because the SSRF
//! validation correctly blocks registering localhost backends via the HTTP API.
//! In production, backends run on non-loopback addresses.
//!
//! Run with: `cargo test --test federation_e2e -- --ignored`

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Parse the listening address from a wsh server's stderr output.
/// The server prints a line like: `Listening on 127.0.0.1:XXXXX`
fn parse_listen_address(child: &mut Child) -> String {
    let stderr = child.stderr.take().expect("child should have stderr");
    let reader = BufReader::new(stderr);

    let deadline = std::time::Instant::now() + STARTUP_TIMEOUT;
    for line in reader.lines() {
        if std::time::Instant::now() > deadline {
            break;
        }
        let line = line.expect("should read stderr line");
        eprintln!("[server stderr] {}", line);
        // Look for the listening address in the output.
        // Format varies but typically includes the bind address.
        if let Some(addr) = extract_address(&line) {
            return addr;
        }
    }
    panic!("failed to parse listen address from server output within timeout");
}

/// Try to extract a socket address from a log line.
fn extract_address(line: &str) -> Option<String> {
    // The server logs something like: `Listening on 127.0.0.1:12345`
    // or the tracing output contains the address.
    if line.contains("Listening on") || line.contains("listening on") {
        // Find a pattern like 127.0.0.1:NNNNN
        for word in line.split_whitespace() {
            let word = word.trim_end_matches(|c: char| !c.is_ascii_digit());
            if word.contains(':') {
                let parts: Vec<&str> = word.split(':').collect();
                if parts.len() == 2 {
                    if parts[1].parse::<u16>().is_ok() {
                        return Some(word.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Spawns a wsh server with the given instance name.
fn spawn_server(instance_name: &str) -> Child {
    let socket_dir = tempfile::TempDir::new().unwrap();
    let socket_path = socket_dir.path().join("test.sock");
    // Keep the TempDir alive for the duration of the server process.
    let _socket_dir = socket_dir;

    Command::new(env!("CARGO_BIN_EXE_wsh"))
        .arg("server")
        .arg("--bind")
        .arg("127.0.0.1:0")
        .arg("--socket")
        .arg(&socket_path)
        .arg("--server-name")
        .arg(instance_name)
        .arg("--ephemeral")
        .arg("--hostname")
        .arg(instance_name)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn wsh server")
}

/// Wait for a server to be healthy by polling GET /health.
async fn wait_for_ready(addr: &str) -> Result<(), String> {
    let url = format!("http://{}/health", addr);
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
    Err(format!("server at {} did not become ready", addr))
}

// ── Test 1: Two independent servers start and are healthy ───────────

#[ignore]
#[tokio::test]
async fn federation_e2e_both_servers_start() {
    // Spawn hub and backend servers.
    let mut hub = spawn_server("fed-e2e-hub-1");
    let mut backend = spawn_server("fed-e2e-backend-1");

    // Parse addresses from stderr output (blocking, so run in threads).
    let hub_addr = tokio::task::spawn_blocking(move || parse_listen_address(&mut hub))
        .await
        .unwrap();
    let backend_addr = tokio::task::spawn_blocking(move || parse_listen_address(&mut backend))
        .await
        .unwrap();

    eprintln!("hub listening on {}", hub_addr);
    eprintln!("backend listening on {}", backend_addr);

    // Both should become healthy.
    wait_for_ready(&hub_addr).await.expect("hub should be ready");
    wait_for_ready(&backend_addr)
        .await
        .expect("backend should be ready");

    // Verify both return healthy.
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/health", hub_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let resp = client
        .get(format!("http://{}/health", backend_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Note: We can't clean up the spawned children here since we moved them
    // into spawn_blocking. The processes will be killed when the test ends
    // since they're ephemeral and we haven't created any sessions.
}

// ── Test 2: Federation management endpoints ─────────────────────────

#[ignore]
#[tokio::test]
async fn federation_e2e_server_management() {
    let mut hub = spawn_server("fed-e2e-hub-2");
    let hub_addr = tokio::task::spawn_blocking(move || parse_listen_address(&mut hub))
        .await
        .unwrap();

    wait_for_ready(&hub_addr).await.expect("hub should be ready");

    let client = reqwest::Client::new();
    let base = format!("http://{}", hub_addr);

    // GET /servers should include self.
    let resp = client
        .get(format!("{}/servers", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let servers = body.as_array().expect("should be array");
    assert_eq!(servers.len(), 1, "should have exactly self");
    assert_eq!(servers[0]["address"], "local");
    assert_eq!(servers[0]["health"], "healthy");
    assert_eq!(servers[0]["hostname"], "fed-e2e-hub-2");

    // POST /servers with a valid non-loopback address (will fail to connect
    // but should be accepted by the API).
    let resp = client
        .post(format!("{}/servers", base))
        .json(&serde_json::json!({
            "address": "10.99.99.99:9999"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["address"], "10.99.99.99:9999");
    assert_eq!(body["health"], "connecting");

    // GET /servers should now have 2 entries.
    let resp = client
        .get(format!("{}/servers", base))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let servers = body.as_array().unwrap();
    assert_eq!(servers.len(), 2);

    // POST /servers with localhost should be rejected (SSRF prevention).
    let resp = client
        .post(format!("{}/servers", base))
        .json(&serde_json::json!({
            "address": "127.0.0.1:9999"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // POST /servers with duplicate address should be rejected.
    let resp = client
        .post(format!("{}/servers", base))
        .json(&serde_json::json!({
            "address": "10.99.99.99:9999"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);

    // GET /servers/{hostname} for self.
    let resp = client
        .get(format!("{}/servers/fed-e2e-hub-2", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["hostname"], "fed-e2e-hub-2");

    // GET /servers/{hostname} for non-existent server.
    let resp = client
        .get(format!("{}/servers/nonexistent", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // DELETE /servers by hostname -- the dummy backend won't have resolved
    // its hostname, so we can't delete by hostname. Delete by address
    // would require a different endpoint. We verify the management works
    // by removing a non-existent hostname.
    let resp = client
        .delete(format!("{}/servers/nonexistent", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// ── Test 3: Session operations on local server ──────────────────────

#[ignore]
#[tokio::test]
async fn federation_e2e_local_sessions() {
    let mut hub = spawn_server("fed-e2e-hub-3");
    let hub_addr = tokio::task::spawn_blocking(move || parse_listen_address(&mut hub))
        .await
        .unwrap();

    wait_for_ready(&hub_addr).await.expect("hub should be ready");

    let client = reqwest::Client::new();
    let base = format!("http://{}", hub_addr);

    // Create a session locally (no ?server= parameter).
    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({
            "name": "local-test",
            "tags": ["e2e"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "local-test");

    // Create session targeting self by hostname.
    let resp = client
        .post(format!("{}/sessions?server=fed-e2e-hub-3", base))
        .json(&serde_json::json!({
            "name": "self-targeted",
            "tags": ["e2e"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "self-targeted");

    // List sessions (should aggregate, but only local exists).
    let resp = client
        .get(format!("{}/sessions", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let sessions = body.as_array().unwrap();
    assert_eq!(sessions.len(), 2);

    // Filter by tag.
    let resp = client
        .get(format!("{}/sessions?tag=e2e", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let sessions = body.as_array().unwrap();
    assert_eq!(sessions.len(), 2);

    // Target unknown server should return 404.
    let resp = client
        .post(format!("{}/sessions?server=nonexistent", base))
        .json(&serde_json::json!({
            "name": "remote-test"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // Kill sessions.
    let resp = client
        .delete(format!("{}/sessions/local-test", base))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let resp = client
        .delete(format!("{}/sessions/self-targeted", base))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // Verify sessions are gone.
    let resp = client
        .get(format!("{}/sessions", base))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let sessions = body.as_array().unwrap();
    assert_eq!(sessions.len(), 0);
}

// ── Test 4: Cross-server proxy via in-process AppState ──────────────
//
// This test verifies the actual federation proxy path by constructing an
// in-process hub with `add_unchecked` to bypass SSRF validation (which
// correctly blocks localhost in production). A real backend wsh server
// is spawned as a child process.

#[ignore]
#[tokio::test]
async fn federation_e2e_cross_server_proxy() {
    use std::sync::{atomic::AtomicUsize, Arc};
    use tokio::net::TcpListener;
    use wsh::api::{router, AppState, RouterConfig, ServerConfig};
    use wsh::federation::manager::FederationManager;
    use wsh::federation::registry::{BackendEntry, BackendHealth, BackendRole};
    use wsh::session::SessionRegistry;
    use wsh::shutdown::ShutdownCoordinator;

    // 1. Spawn a real backend server.
    let mut backend = spawn_server("fed-e2e-backend-4");
    let backend_addr = tokio::task::spawn_blocking(move || parse_listen_address(&mut backend))
        .await
        .unwrap();

    wait_for_ready(&backend_addr)
        .await
        .expect("backend should be ready");

    // 2. Create an in-process hub with the backend registered via add_unchecked.
    let federation_manager = FederationManager::new();
    let backends = federation_manager.registry().clone();

    // Register the real backend, bypassing SSRF validation for localhost.
    backends
        .add_unchecked(BackendEntry {
            address: backend_addr.clone(),
            token: None,
            hostname: Some("fed-e2e-backend-4".into()),
            health: BackendHealth::Healthy,
            role: BackendRole::Member,
        })
        .unwrap();

    let state = AppState {
        sessions: SessionRegistry::new(),
        shutdown: ShutdownCoordinator::new(),
        server_config: Arc::new(ServerConfig::new(false)),
        server_ws_count: Arc::new(AtomicUsize::new(0)),
        mcp_session_count: Arc::new(AtomicUsize::new(0)),
        ticket_store: Arc::new(wsh::api::ticket::TicketStore::new()),
        backends: backends.clone(),
        federation: Arc::new(tokio::sync::Mutex::new(federation_manager)),
        hostname: "fed-e2e-hub-4".to_string(),
        federation_config_path: None,
        local_token: None,
        default_backend_token: None,
    };

    // Start the in-process hub.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hub_addr = listener.local_addr().unwrap();
    let app = router(state, RouterConfig::default());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let hub_base = format!("http://{}", hub_addr);

    // 3. GET /servers should show both hub (self) and backend.
    let resp = client
        .get(format!("{}/servers", hub_base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let servers = body.as_array().unwrap();
    assert_eq!(servers.len(), 2, "should have hub + backend");

    // 4. Create a session on the backend through the hub.
    let resp = client
        .post(format!(
            "{}/sessions?server=fed-e2e-backend-4",
            hub_base
        ))
        .json(&serde_json::json!({
            "name": "remote-session",
            "tags": ["proxy-test"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        201,
        "session creation on backend should succeed: {:?}",
        resp.text().await
    );

    // 5. List sessions on hub -- should include sessions from both servers.
    let resp = client
        .get(format!("{}/sessions", hub_base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let sessions = body.as_array().unwrap();

    // We should see the remote session (plus any local sessions).
    let remote_sessions: Vec<_> = sessions
        .iter()
        .filter(|s| s["name"] == "remote-session")
        .collect();
    assert_eq!(
        remote_sessions.len(),
        1,
        "should find the remote session in aggregated list"
    );

    // The session should have a "server" field indicating the backend.
    let remote = &remote_sessions[0];
    assert_eq!(
        remote["server"], "fed-e2e-backend-4",
        "remote session should indicate which server it's on"
    );

    // 6. Get screen from the remote session through the hub.
    let resp = client
        .get(format!(
            "{}/sessions/remote-session/screen?format=plain&server=fed-e2e-backend-4",
            hub_base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "screen read from remote session should succeed"
    );

    // 7. Send input to the remote session through the hub.
    let resp = client
        .post(format!(
            "{}/sessions/remote-session/input?server=fed-e2e-backend-4",
            hub_base
        ))
        .body("echo hello\n")
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "input injection to remote session should succeed"
    );

    // 8. Kill the remote session through the hub.
    let resp = client
        .delete(format!(
            "{}/sessions/remote-session?server=fed-e2e-backend-4",
            hub_base
        ))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "killing remote session through hub should succeed"
    );

    // 9. Verify the session is gone from the backend.
    let resp = client
        .get(format!("http://{}/sessions", backend_addr))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let sessions = body.as_array().unwrap();
    let remaining: Vec<_> = sessions
        .iter()
        .filter(|s| s["name"] == "remote-session")
        .collect();
    assert_eq!(
        remaining.len(),
        0,
        "remote session should be gone from backend"
    );
}

// ── Test 5: Server identity (hostname) ──────────────────────────────

#[ignore]
#[tokio::test]
async fn federation_e2e_server_hostname() {
    let mut server = spawn_server("fed-e2e-custom-host");
    let addr = tokio::task::spawn_blocking(move || parse_listen_address(&mut server))
        .await
        .unwrap();

    wait_for_ready(&addr).await.expect("server should be ready");

    let client = reqwest::Client::new();

    // GET /server/info should return the custom hostname.
    let resp = client
        .get(format!("http://{}/server/info", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["hostname"], "fed-e2e-custom-host",
        "server should report the configured hostname"
    );

    // GET /servers should use the custom hostname.
    let resp = client
        .get(format!("http://{}/servers", addr))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let servers = body.as_array().unwrap();
    assert_eq!(servers[0]["hostname"], "fed-e2e-custom-host");
}
