use crate::federation::registry::{BackendEntry, BackendHealth, BackendRegistry};
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::MaybeTlsStream;

type WsStream = tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// A persistent WebSocket connection to a single backend server.
///
/// Spawns a tokio task that:
/// - Connects to `ws://{address}/ws/json` (with optional Bearer token)
/// - On success: queries `GET /server/info` for hostname, updates registry health to Healthy
/// - Runs a select! loop: ping timer (30s), incoming messages, shutdown signal
/// - On disconnect: marks health Unavailable, retries with exponential backoff (1s..60s)
pub struct BackendConnection {
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl BackendConnection {
    /// Spawn the persistent connection task for the given backend address.
    pub fn spawn(
        address: String,
        token: Option<String>,
        registry: BackendRegistry,
        local_server_id: String,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(connection_loop(
            address,
            token,
            registry,
            shutdown_rx,
            local_server_id,
        ));
        Self { shutdown_tx, task }
    }

    /// Signal the connection task to shut down.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Wait for the connection task to complete. Consumes the handle.
    pub async fn join(self) {
        let _ = self.task.await;
    }
}

async fn connection_loop(
    address: String,
    token: Option<String>,
    registry: BackendRegistry,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    local_server_id: String,
) {
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);

    loop {
        if *shutdown_rx.borrow() {
            return;
        }

        // Build WS URL from the backend's base address.
        let backend_stub = BackendEntry {
            address: address.clone(),
            token: token.clone(),
            hostname: None,
            health: BackendHealth::Connecting,
            role: crate::federation::registry::BackendRole::Member,
            server_id: None,
        };
        let ws_url = backend_stub.ws_url_for("/ws/json");

        // Extract host authority for the Host header.
        let host_authority = address
            .strip_prefix("https://")
            .or_else(|| address.strip_prefix("http://"))
            .unwrap_or(&address)
            .split('/')
            .next()
            .unwrap_or(&address)
            .to_string();

        // Build connection request with optional auth header.
        let connect_result = if let Some(ref tok) = token {
            use tokio_tungstenite::tungstenite::http::Request;
            let req = Request::builder()
                .uri(&ws_url)
                .header("Authorization", format!("Bearer {}", tok))
                .header("Connection", "Upgrade")
                .header("Upgrade", "websocket")
                .header("Sec-WebSocket-Version", "13")
                .header(
                    "Sec-WebSocket-Key",
                    tokio_tungstenite::tungstenite::handshake::client::generate_key(),
                )
                .header("Host", &host_authority)
                .body(())
                .unwrap();
            tokio_tungstenite::connect_async(req).await
        } else {
            tokio_tungstenite::connect_async(&ws_url).await
        };

        match connect_result {
            Ok((ws_stream, _)) => {
                backoff = Duration::from_secs(1);

                // Query server info (hostname + server_id) via HTTP endpoint.
                let server_info = fetch_server_info(&address, token.as_deref()).await;

                // Self-loop detection: if remote server_id matches local, reject permanently.
                if let Ok(ref info) = server_info {
                    if let Some(ref remote_id) = info.server_id {
                        if *remote_id == local_server_id {
                            tracing::warn!(
                                backend = %address,
                                server_id = %remote_id,
                                "self-loop detected — backend is this server, marking rejected"
                            );
                            registry.set_server_id(&address, remote_id);
                            registry.set_health(&address, BackendHealth::Rejected);
                            return; // Permanent — no retry.
                        }
                    }
                }

                // Not a self-loop — mark healthy.
                registry.set_health(&address, BackendHealth::Healthy);
                tracing::info!(backend = %address, "backend connected");

                // Enrich with hostname and server_id from the response.
                if let Ok(info) = server_info {
                    if let Some(hostname) = info.hostname {
                        let _ = registry.set_hostname(&address, &hostname);
                    }
                    if let Some(ref remote_id) = info.server_id {
                        registry.set_server_id(&address, remote_id);
                    }
                }

                // Run until disconnect or shutdown.
                run_connection(ws_stream, &mut shutdown_rx).await;

                if *shutdown_rx.borrow() {
                    return;
                }
                registry.set_health(&address, BackendHealth::Unavailable);
                tracing::warn!(backend = %address, "backend disconnected");
            }
            Err(e) => {
                tracing::debug!(backend = %address, error = %e, "connection failed");
                registry.set_health(&address, BackendHealth::Unavailable);
            }
        }

        // Wait before retry with exponential backoff.
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = shutdown_rx.changed() => { return; }
        }
        backoff = (backoff * 2).min(max_backoff);
    }
}

async fn run_connection(
    ws_stream: WsStream,
    shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
) {
    let (mut sink, mut stream) = ws_stream.split();
    let mut ping_interval = tokio::time::interval(Duration::from_secs(30));
    ping_interval.tick().await; // Skip the first immediate tick.

    loop {
        tokio::select! {
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Ping(data))) => {
                        if sink.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // Ignore other messages for now.
                    Some(Err(_)) => break,
                }
            }
            _ = ping_interval.tick() => {
                if sink.send(Message::Ping(vec![].into())).await.is_err() {
                    break;
                }
            }
            _ = shutdown_rx.changed() => {
                let _ = sink.send(Message::Close(None)).await;
                break;
            }
        }
    }
}

struct ServerInfo {
    hostname: Option<String>,
    server_id: Option<String>,
}

async fn fetch_server_info(
    address: &str,
    token: Option<&str>,
) -> Result<ServerInfo, Box<dyn std::error::Error + Send + Sync>> {
    let backend_stub = BackendEntry {
        address: address.to_string(),
        token: None,
        hostname: None,
        health: BackendHealth::Connecting,
        role: crate::federation::registry::BackendRole::Member,
        server_id: None,
    };
    let url = backend_stub.url_for("/server/info");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()?;

    let mut req = client.get(&url);
    if let Some(tok) = token {
        req = req.bearer_auth(tok);
    }

    let resp = req.send().await?;
    let body: serde_json::Value = resp.json().await?;
    Ok(ServerInfo {
        hostname: body["hostname"].as_str().map(|s| s.to_string()),
        server_id: body["server_id"].as_str().map(|s| s.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::federation::registry::{BackendEntry, BackendHealth, BackendRegistry, BackendRole};
    use tokio::time::timeout;

    /// Spawn a minimal WebSocket server that accepts connections and stays open.
    async fn spawn_ws_server() -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                        let (_, mut rx) = ws.split();
                        // Just drain incoming messages to keep connection alive.
                        while rx.next().await.is_some() {}
                    }
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn connects_and_becomes_healthy() {
        let addr = spawn_ws_server().await;

        let registry = BackendRegistry::new();
        registry
            .add_unchecked(BackendEntry {
                address: addr.to_string(),
                token: None,
                hostname: None,
                health: BackendHealth::Connecting,
                role: BackendRole::Member,
                server_id: None,
            })
            .unwrap();

        let conn = BackendConnection::spawn(addr.to_string(), None, registry.clone(), "test-local-id".into());

        // Wait for healthy.
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some(entry) = registry.get_by_address(&addr.to_string()) {
                    if entry.health == BackendHealth::Healthy {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("should become healthy within 5s");

        conn.shutdown();
        conn.join().await;
    }

    #[tokio::test]
    async fn detects_disconnect_and_marks_unavailable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Accept one connection, then drop it.
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let (_, mut rx) = ws.split();
            // Read one message then close.
            let _ = rx.next().await;
            // Drop WS to trigger disconnect.
        });

        let registry = BackendRegistry::new();
        registry
            .add_unchecked(BackendEntry {
                address: addr.to_string(),
                token: None,
                hostname: None,
                health: BackendHealth::Connecting,
                role: BackendRole::Member,
                server_id: None,
            })
            .unwrap();

        let conn = BackendConnection::spawn(addr.to_string(), None, registry.clone(), "test-local-id".into());

        // Wait for healthy first.
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some(entry) = registry.get_by_address(&addr.to_string()) {
                    if entry.health == BackendHealth::Healthy {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("should become healthy");

        // Wait for server to close.
        server_handle.await.unwrap();

        // Wait for unavailable.
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some(entry) = registry.get_by_address(&addr.to_string()) {
                    if entry.health == BackendHealth::Unavailable {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("should become unavailable after disconnect");

        conn.shutdown();
        conn.join().await;
    }

    #[tokio::test]
    async fn shutdown_stops_connection() {
        let addr = spawn_ws_server().await;

        let registry = BackendRegistry::new();
        registry
            .add_unchecked(BackendEntry {
                address: addr.to_string(),
                token: None,
                hostname: None,
                health: BackendHealth::Connecting,
                role: BackendRole::Member,
                server_id: None,
            })
            .unwrap();

        let conn = BackendConnection::spawn(addr.to_string(), None, registry.clone(), "test-local-id".into());

        // Wait for healthy.
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some(entry) = registry.get_by_address(&addr.to_string()) {
                    if entry.health == BackendHealth::Healthy {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("should become healthy");

        conn.shutdown();
        // Should complete within reasonable time.
        timeout(Duration::from_secs(5), conn.join())
            .await
            .expect("should shut down within 5s");
    }
}
