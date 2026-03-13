//! HTTP client over Unix domain sockets.
//!
//! Uses `hyper` with a custom Unix socket connector to make HTTP requests
//! over UDS. The `Host` header and URL host are ignored — the socket path
//! determines the destination.

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use std::path::{Path, PathBuf};
use tokio::net::UnixStream;

/// An HTTP client that connects over a Unix domain socket.
#[derive(Clone)]
pub struct UdsHttpClient {
    socket_path: PathBuf,
}

/// Response from a UDS HTTP request.
pub struct UdsResponse {
    pub status: hyper::StatusCode,
    pub headers: hyper::HeaderMap,
    inner: Incoming,
}

impl UdsResponse {
    /// Read the entire response body as bytes.
    pub async fn bytes(self) -> Result<Bytes, hyper::Error> {
        Ok(self.inner.collect().await?.to_bytes())
    }

    /// Read the entire response body as a string.
    pub async fn text(self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let bytes = self.bytes().await?;
        Ok(String::from_utf8(bytes.to_vec())?)
    }

    /// Parse the response body as JSON.
    pub async fn json<T: serde::de::DeserializeOwned>(self) -> Result<T, Box<dyn std::error::Error + Send + Sync>> {
        let bytes = self.bytes().await?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

impl UdsHttpClient {
    /// Create a new UDS HTTP client for the given socket path.
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    /// Get the socket path.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Send an HTTP request over the Unix socket.
    async fn send_request(
        &self,
        req: hyper::Request<http_body_util::Full<Bytes>>,
    ) -> Result<UdsResponse, Box<dyn std::error::Error + Send + Sync>> {
        let stream = UnixStream::connect(&self.socket_path).await?;
        let io = TokioIo::new(stream);

        let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;

        // Spawn connection driver
        tokio::spawn(async move {
            if let Err(e) = conn.with_upgrades().await {
                tracing::debug!(?e, "UDS HTTP connection error");
            }
        });

        let resp = sender.send_request(req).await?;
        let (parts, body) = resp.into_parts();

        Ok(UdsResponse {
            status: parts.status,
            headers: parts.headers,
            inner: body,
        })
    }

    /// Make a GET request.
    pub async fn get(
        &self,
        path: &str,
    ) -> Result<UdsResponse, Box<dyn std::error::Error + Send + Sync>> {
        let req = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(path)
            .header("Host", "localhost")
            .body(http_body_util::Full::new(Bytes::new()))?;
        self.send_request(req).await
    }

    /// Make a POST request with a JSON body.
    pub async fn post_json(
        &self,
        path: &str,
        body: &impl serde::Serialize,
    ) -> Result<UdsResponse, Box<dyn std::error::Error + Send + Sync>> {
        let body_bytes = serde_json::to_vec(body)?;
        let req = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(path)
            .header("Host", "localhost")
            .header("Content-Type", "application/json")
            .body(http_body_util::Full::new(Bytes::from(body_bytes)))?;
        self.send_request(req).await
    }

    /// Make a PUT request with a JSON body.
    pub async fn put_json(
        &self,
        path: &str,
        body: &impl serde::Serialize,
    ) -> Result<UdsResponse, Box<dyn std::error::Error + Send + Sync>> {
        let body_bytes = serde_json::to_vec(body)?;
        let req = hyper::Request::builder()
            .method(hyper::Method::PUT)
            .uri(path)
            .header("Host", "localhost")
            .header("Content-Type", "application/json")
            .body(http_body_util::Full::new(Bytes::from(body_bytes)))?;
        self.send_request(req).await
    }

    /// Make a DELETE request.
    pub async fn delete(
        &self,
        path: &str,
    ) -> Result<UdsResponse, Box<dyn std::error::Error + Send + Sync>> {
        let req = hyper::Request::builder()
            .method(hyper::Method::DELETE)
            .uri(path)
            .header("Host", "localhost")
            .body(http_body_util::Full::new(Bytes::new()))?;
        self.send_request(req).await
    }

    /// Make a PATCH request with a JSON body.
    pub async fn patch_json(
        &self,
        path: &str,
        body: &impl serde::Serialize,
    ) -> Result<UdsResponse, Box<dyn std::error::Error + Send + Sync>> {
        let body_bytes = serde_json::to_vec(body)?;
        let req = hyper::Request::builder()
            .method(hyper::Method::PATCH)
            .uri(path)
            .header("Host", "localhost")
            .header("Content-Type", "application/json")
            .body(http_body_util::Full::new(Bytes::from(body_bytes)))?;
        self.send_request(req).await
    }

    /// Make a POST request with a raw body and custom headers.
    pub async fn post_raw(
        &self,
        path: &str,
        content_type: &str,
        body: Bytes,
        extra_headers: &[(&str, &str)],
    ) -> Result<UdsResponse, Box<dyn std::error::Error + Send + Sync>> {
        let mut builder = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(path)
            .header("Host", "localhost")
            .header("Content-Type", content_type);
        for (k, v) in extra_headers {
            builder = builder.header(*k, *v);
        }
        let req = builder.body(http_body_util::Full::new(body))?;
        self.send_request(req).await
    }

    /// Make a DELETE request with custom headers.
    pub async fn delete_with_headers(
        &self,
        path: &str,
        extra_headers: &[(&str, &str)],
    ) -> Result<UdsResponse, Box<dyn std::error::Error + Send + Sync>> {
        let mut builder = hyper::Request::builder()
            .method(hyper::Method::DELETE)
            .uri(path)
            .header("Host", "localhost");
        for (k, v) in extra_headers {
            builder = builder.header(*k, *v);
        }
        let req = builder.body(http_body_util::Full::new(Bytes::new()))?;
        self.send_request(req).await
    }

    /// Check if the server is healthy by hitting GET /health.
    pub async fn health_check(&self) -> bool {
        match self.get("/health").await {
            Ok(resp) => resp.status.is_success(),
            Err(_) => false,
        }
    }
}
