//! HTTP handlers for the recording API.
//!
//! ## Session recording endpoints (nested under `/sessions/:name`)
//!
//! | Method   | Path        | Description                     |
//! |----------|-------------|---------------------------------|
//! | `POST`   | `/recording`| Start recording a session       |
//! | `GET`    | `/recording`| Get active recording status     |
//! | `DELETE` | `/recording`| Stop active recording           |
//!
//! ## Recording management endpoints
//!
//! | Method   | Path                     | Description                     |
//! |----------|--------------------------|---------------------------------|
//! | `GET`    | `/recordings`            | List all recordings             |
//! | `GET`    | `/recordings/:id`        | Get a single recording          |
//! | `DELETE` | `/recordings/:id`        | Delete a recording              |
//! | `GET`    | `/recordings/:id/cast`   | Serve the raw `.cast` file      |
//! | `GET`    | `/recordings/:id/player` | Self-contained player page      |
//! | `GET`    | `/recordings/:id/embed`  | HTML embed snippet              |

use axum::{
    body::Body,
    extract::{Path, Query as AxumQuery, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

use crate::recording::{self, RecordingInfo};

use super::{error::ApiError, get_session, AppState};

// ── Request / response types ──────────────────────────────────────────────────

#[derive(Deserialize)]
pub(super) struct StartRecordingRequest {
    /// Optional display title embedded in the asciinema header.
    pub title: Option<String>,
    /// Also record "i" (input) events. Currently reserved; always false.
    #[serde(default)]
    pub _capture_input: bool,
}

#[derive(Deserialize, Default)]
pub(super) struct ListRecordingsQuery {
    /// Filter by session name.
    pub session: Option<String>,
    /// Filter by status: "recording", "stopped", "failed".
    pub status: Option<String>,
}

#[derive(Serialize)]
struct RecordingResponse {
    #[serde(flatten)]
    info: RecordingInfo,
}

fn info_response(info: RecordingInfo) -> Json<RecordingResponse> {
    Json(RecordingResponse { info })
}

// ── Helper ────────────────────────────────────────────────────────────────────

fn recording_not_found(_id: &str) -> ApiError {
    ApiError::RecordingNotFound
}

fn get_recording(state: &AppState, id: &str) -> Result<RecordingInfo, ApiError> {
    state
        .recordings
        .get(id)
        .ok_or_else(|| recording_not_found(id))
}

/// Compute the absolute base URL from the incoming request headers.
///
/// Falls back to the server's TCP address if the `Host` header is absent.
fn absolute_base_url(headers: &HeaderMap, state: &AppState) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| state.tcp_addr.map(|a| a.to_string()))
        .unwrap_or_else(|| "localhost:8080".to_string());

    // Use https if the host looks like it has a port > 443, otherwise http.
    // In practice agents and CI will hit HTTP on localhost.
    let scheme = if host.starts_with("localhost") || host.starts_with("127.") {
        "http"
    } else {
        "https"
    };
    format!("{scheme}://{host}")
}

// ── Session-scoped recording endpoints ───────────────────────────────────────

/// `POST /sessions/:name/recording` — start recording.
pub(super) async fn recording_start(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<StartRecordingRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    let (rows, cols) = session.terminal_size.get();
    let output_rx = session.output_rx.subscribe();
    let session_cancelled = session.cancelled.clone();

    state
        .recordings
        .start(
            &name,
            req.title,
            cols,
            rows,
            output_rx,
            session_cancelled,
        )
        .map(|info| (StatusCode::CREATED, info_response(info)).into_response())
        .map_err(|e| match e {
            recording::StartError::AlreadyRecording => ApiError::RecordingAlreadyActive,
        })
}

/// `GET /sessions/:name/recording` — get active recording for a session.
pub(super) async fn recording_status(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    // Ensure the session exists.
    get_session(&state.sessions, &name)?;

    let id = state
        .recordings
        .active_for_session(&name)
        .ok_or(ApiError::RecordingNotFound)?;

    let info = state.recordings.get(&id).ok_or(ApiError::RecordingNotFound)?;
    Ok(info_response(info))
}

/// `DELETE /sessions/:name/recording` — stop active recording.
pub(super) async fn recording_stop(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    // Ensure the session exists.
    get_session(&state.sessions, &name)?;

    state
        .recordings
        .stop(&name)
        .map(info_response)
        .ok_or(ApiError::RecordingNotFound)
}

// ── Global recording endpoints ────────────────────────────────────────────────

/// `GET /recordings` — list all recordings.
pub(super) async fn recording_list(
    State(state): State<AppState>,
    AxumQuery(query): AxumQuery<ListRecordingsQuery>,
) -> Json<serde_json::Value> {
    let mut infos = state.recordings.list(query.session.as_deref());

    // Optional status filter.
    if let Some(ref status_filter) = query.status {
        infos.retain(|i| {
            let s = match i.status {
                recording::RecordingStatus::Recording => "recording",
                recording::RecordingStatus::Stopped => "stopped",
                recording::RecordingStatus::Failed => "failed",
            };
            s == status_filter.as_str()
        });
    }

    Json(serde_json::json!({ "recordings": infos }))
}

/// `GET /recordings/:id` — get a single recording.
pub(super) async fn recording_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let info = get_recording(&state, &id)?;
    Ok(info_response(info))
}

/// `DELETE /recordings/:id` — delete a recording and its cast file.
pub(super) async fn recording_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let path = state
        .recordings
        .delete(&id)
        .ok_or(ApiError::RecordingNotFound)?;

    // Best-effort file deletion; don't fail the request if the file is gone.
    if path.exists() {
        if let Err(e) = tokio::fs::remove_file(&path).await {
            tracing::warn!(%e, path = %path.display(), "failed to delete recording cast file");
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

/// `GET /recordings/:id/cast` — serve the raw asciinema cast file.
///
/// Serves the file as it currently exists. When the recording is still active,
/// this returns the bytes written so far (a valid partial cast playable up to
/// the last complete event line).
pub(super) async fn recording_cast(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let path = state
        .recordings
        .recording_path(&id)
        .ok_or(ApiError::RecordingNotFound)?;

    let mut file = tokio::fs::File::open(&path).await.map_err(|e| {
        tracing::error!(%e, path = %path.display(), "failed to open cast file");
        ApiError::InternalError("cast file not readable".into())
    })?;

    let mut contents = Vec::new();
    file.read_to_end(&mut contents).await.map_err(|e| {
        tracing::error!(%e, "failed to read cast file");
        ApiError::InternalError("cast file read failed".into())
    })?;

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-asciicast; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Body::from(contents))
        .unwrap())
}

/// `GET /recordings/:id/player` — self-contained player HTML page.
pub(super) async fn recording_player(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let info = get_recording(&state, &id)?;

    // The player page is served by wsh itself, so we use a relative cast URL.
    // This works because the page is at /recordings/:id/player and the cast
    // endpoint is at /recordings/:id/cast — the browser resolves `./cast`.
    let html = recording::player_html("./cast", info.title.as_deref(), info.width, info.height);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        // Allow embedding in iframes on other origins so CI dashboards can
        // display the player without opening a new tab.
        .header("x-frame-options", "ALLOWALL")
        .header("content-security-policy", format!(
            "default-src 'self' https://cdn.jsdelivr.net; script-src 'self' 'unsafe-inline' https://cdn.jsdelivr.net; style-src 'self' 'unsafe-inline' https://cdn.jsdelivr.net; connect-src 'self' {}",
            absolute_base_url(&headers, &state)
        ))
        .body(Body::from(html))
        .unwrap())
}

/// `GET /recordings/:id/embed` — HTML embed snippet.
///
/// Returns a copy-pasteable fragment that embeds an asciinema player pointing
/// at this server's cast endpoint. Uses the request `Host` header to build an
/// absolute cast URL that works when the snippet is pasted into external pages.
pub(super) async fn recording_embed(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let info = get_recording(&state, &id)?;
    let base = absolute_base_url(&headers, &state);
    let cast_url = format!("{base}/recordings/{id}/cast");
    let html = recording::embed_html(&cast_url, info.title.as_deref(), info.width, info.height);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        // Convenience header: the full standalone player URL.
        .header("x-player-url", format!("{base}/recordings/{id}/player"))
        .body(Body::from(html))
        .unwrap())
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn make_headers(host: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_str(host).unwrap());
        headers
    }

    #[test]
    fn absolute_base_url_uses_host_header() {
        let state = crate::api::AppState {
            sessions: crate::session::SessionRegistry::new(),
            shutdown: crate::shutdown::ShutdownCoordinator::new(),
            server_config: std::sync::Arc::new(crate::api::ServerConfig::new(false)),
            server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            ticket_store: std::sync::Arc::new(crate::api::ticket::TicketStore::new()),
            backends: crate::federation::registry::BackendRegistry::new(),
            federation: std::sync::Arc::new(tokio::sync::Mutex::new(
                crate::federation::manager::FederationManager::new(),
            )),
            ip_access: None,
            hostname: "test".into(),
            federation_config_path: None,
            local_token: None,
            default_backend_token: None,
            server_id: "test-id".into(),
            shutdown_notify: tokio_util::sync::CancellationToken::new(),
            tcp_addr: None,
            instance_name: "test".into(),
            http_socket_path: std::path::PathBuf::from("/tmp/test.http.sock"),
            recordings: crate::recording::RecordingRegistry::new(),
        };

        let headers = make_headers("example.com:8080");
        let base = absolute_base_url(&headers, &state);
        assert!(base.starts_with("https://example.com:8080"));

        let local_headers = make_headers("localhost:8080");
        let local_base = absolute_base_url(&local_headers, &state);
        assert!(local_base.starts_with("http://localhost:8080"));
    }
}
