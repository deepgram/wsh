//! Integration tests for TLS server support.
//!
//! Uses `rcgen` to generate self-signed certificates at test time,
//! starts a TLS server, and verifies HTTPS connectivity.

use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use wsh::api::{router, AppState, RouterConfig, ServerConfig};
use wsh::session::SessionRegistry;
use wsh::shutdown::ShutdownCoordinator;

fn create_test_app() -> axum::Router {
    create_test_app_with_config(RouterConfig::default())
}

fn create_test_app_with_config(config: RouterConfig) -> axum::Router {
    let federation_manager = wsh::federation::manager::FederationManager::new();
    let state = AppState {
        sessions: SessionRegistry::new(),
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(ServerConfig::new(false)),
        server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        ticket_store: std::sync::Arc::new(wsh::api::ticket::TicketStore::new()),
        backends: federation_manager.registry().clone(),
        federation: std::sync::Arc::new(tokio::sync::Mutex::new(federation_manager)),
        ip_access: None,
        hostname: "test-tls".to_string(),
        federation_config_path: None,
        local_token: None,
        default_backend_token: None,
    };
    router(state, config)
}

/// Generate a self-signed cert+key pair, write them to temp files, and return paths.
fn generate_test_cert(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    std::fs::write(&cert_path, cert.cert.pem()).unwrap();
    std::fs::write(&key_path, cert.key_pair.serialize_pem()).unwrap();
    (cert_path, key_path)
}

/// Start a TLS server and return its address.
async fn start_tls_server(
    app: axum::Router,
    acceptor: tokio_rustls::TlsAcceptor,
) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();

    tokio::spawn({
        let cancel = cancel.clone();
        async move {
            loop {
                let (tcp_stream, _peer_addr) = tokio::select! {
                    _ = cancel.cancelled() => break,
                    result = listener.accept() => {
                        match result {
                            Ok(conn) => conn,
                            Err(_) => continue,
                        }
                    }
                };

                let acceptor = acceptor.clone();
                let app = app.clone();

                tokio::spawn(async move {
                    let tls_stream = match acceptor.accept(tcp_stream).await {
                        Ok(s) => s,
                        Err(_) => return,
                    };

                    let io = hyper_util::rt::TokioIo::new(tls_stream);
                    let service = hyper_util::service::TowerToHyperService::new(app);
                    let builder = hyper_util::server::conn::auto::Builder::new(
                        hyper_util::rt::TokioExecutor::new(),
                    );
                    let conn = builder.serve_connection_with_upgrades(io, service);
                    let _ = conn.await;
                });
            }
        }
    });

    tokio::time::sleep(Duration::from_millis(10)).await;
    addr
}

#[tokio::test]
async fn tls_health_endpoint_works() {
    let dir = tempfile::tempdir().unwrap();
    let (cert_path, key_path) = generate_test_cert(dir.path());

    let acceptor = wsh::tls::load_tls_config(&cert_path, &key_path).unwrap();
    let app = create_test_app();
    let addr = start_tls_server(app, acceptor).await;

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();

    let resp = client
        .get(format!("https://127.0.0.1:{}/health", addr.port()))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn tls_sessions_endpoint_works() {
    let dir = tempfile::tempdir().unwrap();
    let (cert_path, key_path) = generate_test_cert(dir.path());

    let acceptor = wsh::tls::load_tls_config(&cert_path, &key_path).unwrap();
    let app = create_test_app();
    let addr = start_tls_server(app, acceptor).await;

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();

    let resp = client
        .get(format!("https://127.0.0.1:{}/sessions", addr.port()))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn plain_http_rejected_by_tls_server() {
    let dir = tempfile::tempdir().unwrap();
    let (cert_path, key_path) = generate_test_cert(dir.path());

    let acceptor = wsh::tls::load_tls_config(&cert_path, &key_path).unwrap();
    let app = create_test_app();
    let addr = start_tls_server(app, acceptor).await;

    let client = reqwest::Client::new();

    // Plain HTTP to a TLS server should fail (connection error or reset)
    let result = client
        .get(format!("http://127.0.0.1:{}/health", addr.port()))
        .timeout(Duration::from_secs(2))
        .send()
        .await;

    assert!(result.is_err(), "plain HTTP request to TLS server should fail");
}

// --- Combined TLS + base-prefix tests ---

#[tokio::test]
async fn tls_with_base_prefix_routes_work() {
    let dir = tempfile::tempdir().unwrap();
    let (cert_path, key_path) = generate_test_cert(dir.path());

    let acceptor = wsh::tls::load_tls_config(&cert_path, &key_path).unwrap();
    let app = create_test_app_with_config(RouterConfig {
        base_prefix: Some("/wsh".to_string()),
        ..RouterConfig::default()
    });
    let addr = start_tls_server(app, acceptor).await;

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();

    // Prefixed health endpoint should work
    let resp = client
        .get(format!("https://127.0.0.1:{}/wsh/health", addr.port()))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");

    // Root /health should still work (load balancer probe)
    let resp = client
        .get(format!("https://127.0.0.1:{}/health", addr.port()))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Prefixed sessions endpoint should work
    let resp = client
        .get(format!("https://127.0.0.1:{}/wsh/sessions", addr.port()))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Non-prefixed sessions should 404
    let resp = client
        .get(format!("https://127.0.0.1:{}/sessions", addr.port()))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// --- Combined TLS + federation tests ---

#[tokio::test]
async fn tls_federation_servers_endpoint() {
    use wsh::federation::registry::{BackendEntry, BackendHealth, BackendRole};

    let dir = tempfile::tempdir().unwrap();
    let (cert_path, key_path) = generate_test_cert(dir.path());

    // Create a hub with a registered backend (using add_unchecked to skip connection)
    let federation_manager = wsh::federation::manager::FederationManager::new();
    let backends = federation_manager.registry().clone();
    backends
        .add_unchecked(BackendEntry {
            address: "https://10.0.1.10:8080".to_string(),
            token: None,
            hostname: Some("backend-1".to_string()),
            health: BackendHealth::Healthy,
            role: BackendRole::Member,
        })
        .unwrap();

    let state = AppState {
        sessions: SessionRegistry::new(),
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(ServerConfig::new(false)),
        server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        ticket_store: std::sync::Arc::new(wsh::api::ticket::TicketStore::new()),
        backends,
        federation: std::sync::Arc::new(tokio::sync::Mutex::new(federation_manager)),
        ip_access: None,
        hostname: "test-tls-hub".to_string(),
        federation_config_path: None,
        local_token: None,
        default_backend_token: None,
    };
    let app = router(state, RouterConfig::default());

    let acceptor = wsh::tls::load_tls_config(&cert_path, &key_path).unwrap();
    let addr = start_tls_server(app, acceptor).await;

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();

    // List backends over HTTPS
    let resp = client
        .get(format!("https://127.0.0.1:{}/servers", addr.port()))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let servers = body.as_array().unwrap();
    // First entry is always the local server, second is the registered backend
    assert_eq!(servers.len(), 2);
    assert_eq!(servers[0]["address"], "local");
    assert_eq!(servers[0]["hostname"], "test-tls-hub");
    assert_eq!(servers[1]["address"], "https://10.0.1.10:8080");
    assert_eq!(servers[1]["hostname"], "backend-1");
    assert_eq!(servers[1]["health"], "healthy");
}

// --- Combined TLS + base-prefix + federation ---

#[tokio::test]
async fn tls_base_prefix_federation_combined() {
    use wsh::federation::registry::{BackendEntry, BackendHealth, BackendRole};

    let dir = tempfile::tempdir().unwrap();
    let (cert_path, key_path) = generate_test_cert(dir.path());

    let federation_manager = wsh::federation::manager::FederationManager::new();
    let backends = federation_manager.registry().clone();
    backends
        .add_unchecked(BackendEntry {
            address: "https://proxy.example.com/wsh-node-1".to_string(),
            token: None,
            hostname: Some("node-1".to_string()),
            health: BackendHealth::Healthy,
            role: BackendRole::Member,
        })
        .unwrap();

    let state = AppState {
        sessions: SessionRegistry::new(),
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(ServerConfig::new(false)),
        server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        ticket_store: std::sync::Arc::new(wsh::api::ticket::TicketStore::new()),
        backends,
        federation: std::sync::Arc::new(tokio::sync::Mutex::new(federation_manager)),
        ip_access: None,
        hostname: "test-combined".to_string(),
        federation_config_path: None,
        local_token: None,
        default_backend_token: None,
    };
    let app = router(
        state,
        RouterConfig {
            base_prefix: Some("/api".to_string()),
            ..RouterConfig::default()
        },
    );

    let acceptor = wsh::tls::load_tls_config(&cert_path, &key_path).unwrap();
    let addr = start_tls_server(app, acceptor).await;

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();

    // Root health still accessible over TLS
    let resp = client
        .get(format!("https://127.0.0.1:{}/health", addr.port()))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Prefixed federation endpoint over TLS
    let resp = client
        .get(format!("https://127.0.0.1:{}/api/servers", addr.port()))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let servers = body.as_array().unwrap();
    // First entry is the local server, second is the registered backend
    assert_eq!(servers.len(), 2);
    assert_eq!(servers[0]["address"], "local");
    assert_eq!(servers[1]["address"], "https://proxy.example.com/wsh-node-1");
    assert_eq!(servers[1]["hostname"], "node-1");

    // Prefixed sessions endpoint over TLS
    let resp = client
        .get(format!("https://127.0.0.1:{}/api/sessions", addr.port()))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Non-prefixed servers should 404
    let resp = client
        .get(format!("https://127.0.0.1:{}/servers", addr.port()))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}
