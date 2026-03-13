//! Transport-awareness middleware for distinguishing UDS from TCP connections.
//!
//! Injects a [`Transport`] extension into each request so handlers can check
//! whether the connection arrived over a Unix domain socket (local, privileged)
//! or TCP (potentially remote). UDS connections carry peer credentials (UID, GID, PID)
//! from `SO_PEERCRED`.

use axum::extract::connect_info::ConnectInfo;
use std::net::SocketAddr;

/// Transport metadata injected into every request by the transport middleware.
#[derive(Debug, Clone)]
pub enum Transport {
    /// Connection arrived over a Unix domain socket.
    Uds {
        uid: u32,
        gid: u32,
        pid: Option<i32>,
    },
    /// Connection arrived over TCP.
    Tcp { addr: SocketAddr },
}

impl Transport {
    /// Returns `true` if this is a local UDS connection (privileged).
    pub fn is_local(&self) -> bool {
        matches!(self, Transport::Uds { .. })
    }
}

/// Connect info extracted from Unix domain socket connections.
///
/// Captures peer credentials via `SO_PEERCRED` when available.
#[derive(Debug, Clone)]
pub struct UdsConnectInfo {
    pub peer_cred: Option<tokio::net::unix::UCred>,
}

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, tokio::net::UnixListener>>
    for UdsConnectInfo
{
    fn connect_info(stream: axum::serve::IncomingStream<'_, tokio::net::UnixListener>) -> Self {
        let peer_cred = stream.io().peer_cred().ok();
        UdsConnectInfo { peer_cred }
    }
}

/// Middleware that injects [`Transport::Uds`] for UDS connections.
///
/// Applied to the router serving on `tokio::net::UnixListener`. Extracts
/// `UdsConnectInfo` from the connection and injects `Transport::Uds`.
pub async fn uds_transport_middleware(
    connect_info: ConnectInfo<UdsConnectInfo>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let cred = connect_info.0.peer_cred.as_ref();
    let transport = Transport::Uds {
        uid: cred.map(|c| c.uid()).unwrap_or(0),
        gid: cred.map(|c| c.gid()).unwrap_or(0),
        pid: cred.and_then(|c| c.pid()),
    };
    req.extensions_mut().insert(transport);
    next.run(req).await
}

/// Middleware that injects [`Transport::Tcp`] for TCP connections.
///
/// Applied to the router serving on `tokio::net::TcpListener`. Extracts
/// `ConnectInfo<SocketAddr>` from the connection and injects `Transport::Tcp`.
pub async fn tcp_transport_middleware(
    connect_info: ConnectInfo<SocketAddr>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let transport = Transport::Tcp {
        addr: connect_info.0,
    };
    req.extensions_mut().insert(transport);
    next.run(req).await
}
