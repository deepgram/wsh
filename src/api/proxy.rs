//! HTTP proxy helpers for forwarding session operations to remote backend servers.
//!
//! When a handler receives a `?server=X` query parameter targeting a remote
//! backend, it uses these helpers to construct an HTTP request to
//! `http://{backend.address}/{path}` and forward it. The response is returned
//! to the original caller with the same status code and JSON body.

use axum::http::StatusCode;

use crate::api::error::ApiError;
use crate::federation::registry::BackendEntry;

/// Shared connect and request timeouts for proxy requests.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Build a reqwest client with standard timeouts.
fn build_client() -> Result<reqwest::Client, ApiError> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| ApiError::InternalError(e.to_string()))
}

/// Proxy a GET request to a backend server.
///
/// Returns the HTTP status code and parsed JSON body from the backend.
pub(super) async fn proxy_get(
    backend: &BackendEntry,
    path: &str,
) -> Result<(StatusCode, serde_json::Value), ApiError> {
    let url = format!("http://{}{}", backend.address, path);
    let client = build_client()?;

    let mut req = client.get(&url);
    if let Some(ref token) = backend.token {
        req = req.bearer_auth(token);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| ApiError::ServerUnavailable(format!("{}: {}", backend.address, e)))?;

    let status = StatusCode::from_u16(resp.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ApiError::InternalError(format!("invalid response from backend: {}", e)))?;

    Ok((status, body))
}

/// Proxy a POST request with JSON body to a backend server.
///
/// Returns the HTTP status code and parsed JSON body from the backend.
pub(super) async fn proxy_post(
    backend: &BackendEntry,
    path: &str,
    body: serde_json::Value,
) -> Result<(StatusCode, serde_json::Value), ApiError> {
    let url = format!("http://{}{}", backend.address, path);
    let client = build_client()?;

    let mut req = client.post(&url).json(&body);
    if let Some(ref token) = backend.token {
        req = req.bearer_auth(token);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| ApiError::ServerUnavailable(format!("{}: {}", backend.address, e)))?;

    let status = StatusCode::from_u16(resp.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ApiError::InternalError(format!("invalid response from backend: {}", e)))?;

    Ok((status, body))
}

/// Proxy a POST request with raw bytes body to a backend server.
///
/// Returns the HTTP status code from the backend (no body expected).
pub(super) async fn proxy_post_bytes(
    backend: &BackendEntry,
    path: &str,
    body: bytes::Bytes,
) -> Result<StatusCode, ApiError> {
    let url = format!("http://{}{}", backend.address, path);
    let client = build_client()?;

    let mut req = client.post(&url).body(body);
    if let Some(ref token) = backend.token {
        req = req.bearer_auth(token);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| ApiError::ServerUnavailable(format!("{}: {}", backend.address, e)))?;

    Ok(StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
}

/// Proxy a DELETE request to a backend server.
///
/// Returns the HTTP status code from the backend.
pub(super) async fn proxy_delete(
    backend: &BackendEntry,
    path: &str,
) -> Result<StatusCode, ApiError> {
    let url = format!("http://{}{}", backend.address, path);
    let client = build_client()?;

    let mut req = client.delete(&url);
    if let Some(ref token) = backend.token {
        req = req.bearer_auth(token);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| ApiError::ServerUnavailable(format!("{}: {}", backend.address, e)))?;

    Ok(StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
}

/// Proxy a PATCH request with JSON body to a backend server.
///
/// Returns the HTTP status code and parsed JSON body from the backend.
pub(super) async fn proxy_patch(
    backend: &BackendEntry,
    path: &str,
    body: serde_json::Value,
) -> Result<(StatusCode, serde_json::Value), ApiError> {
    let url = format!("http://{}{}", backend.address, path);
    let client = build_client()?;

    let mut req = client.patch(&url).json(&body);
    if let Some(ref token) = backend.token {
        req = req.bearer_auth(token);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| ApiError::ServerUnavailable(format!("{}: {}", backend.address, e)))?;

    let status = StatusCode::from_u16(resp.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ApiError::InternalError(format!("invalid response from backend: {}", e)))?;

    Ok((status, body))
}
