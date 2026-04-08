//! Terminal session recording in asciinema v2 format.
//!
//! [`RecordingRegistry`] manages all recordings on the server. Each recording
//! subscribes to the session's broker channel and writes asciinema v2 events to
//! a file on disk as output arrives. Recordings outlive their sessions, remaining
//! accessible via the API after the session is destroyed.
//!
//! ## Format
//!
//! asciinema v2 is a newline-delimited JSON file:
//! - **Line 1**: header object `{"version":2,"width":W,"height":H,"timestamp":T,...}`
//! - **Subsequent lines**: event arrays `[elapsed_secs, "o", "data"]`
//!
//! The format is append-only, so partial recordings (from unclean shutdowns)
//! are valid and playable up to the last complete line.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// ── Public types ─────────────────────────────────────────────────────────────

/// Current state of a recording.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingStatus {
    /// Actively capturing output.
    Recording,
    /// Cleanly finalized and fully playable.
    Stopped,
    /// Session exited uncleanly; partial file exists and is playable up to
    /// the last complete event line.
    Failed,
}

/// Wire-format recording metadata returned by the API.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecordingInfo {
    pub id: String,
    pub session: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
    pub bytes_written: u64,
    pub status: RecordingStatus,
    pub width: u16,
    pub height: u16,
    /// Relative URL paths for this recording. Callers can prepend the server
    /// base URL to form absolute links.
    pub urls: RecordingUrls,
}

/// Relative URL paths for a single recording.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecordingUrls {
    /// Raw asciinema v2 cast file.
    pub cast: String,
    /// Self-contained HTML page with an embedded player.
    pub player: String,
    /// Copy-pasteable HTML embed snippet.
    pub embed: String,
}

impl RecordingUrls {
    fn for_id(id: &str) -> Self {
        Self {
            cast: format!("/recordings/{id}/cast"),
            player: format!("/recordings/{id}/player"),
            embed: format!("/recordings/{id}/embed"),
        }
    }
}

/// Error returned when attempting to start a recording.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("session already has an active recording")]
    AlreadyRecording,
}

// ── Internal registry entry ───────────────────────────────────────────────────

struct RecordingEntry {
    id: String,
    session: String,
    title: Option<String>,
    started_at: u64,
    stopped_at: Option<u64>,
    bytes_written: Arc<AtomicU64>,
    status: RecordingStatus,
    pub path: PathBuf,
    width: u16,
    height: u16,
    /// Cancels the recording task (used by `stop()` and session-kill).
    cancel: CancellationToken,
}

impl RecordingEntry {
    fn to_info(&self) -> RecordingInfo {
        let bytes_written = self.bytes_written.load(Ordering::Relaxed);
        let duration_secs = match (self.stopped_at, self.started_at) {
            (Some(stop), start) if stop >= start => Some((stop - start) as f64),
            _ => None,
        };
        RecordingInfo {
            id: self.id.clone(),
            session: self.session.clone(),
            title: self.title.clone(),
            started_at: self.started_at,
            stopped_at: self.stopped_at,
            duration_secs,
            bytes_written,
            status: self.status.clone(),
            width: self.width,
            height: self.height,
            urls: RecordingUrls::for_id(&self.id),
        }
    }
}

// ── Registry ─────────────────────────────────────────────────────────────────

struct RegistryInner {
    /// All recordings keyed by recording ID.
    recordings: HashMap<String, RecordingEntry>,
    /// Maps session name → active recording ID (at most one per session).
    active: HashMap<String, String>,
    recordings_dir: PathBuf,
}

/// Shared, cloneable registry of all recordings on the server.
///
/// Clone is cheap (Arc under the hood). All mutations go through the write
/// lock; the lock is never held across async points.
#[derive(Clone)]
pub struct RecordingRegistry {
    inner: Arc<RwLock<RegistryInner>>,
}

impl RecordingRegistry {
    /// Create a registry that stores cast files in the platform data directory.
    pub fn new() -> Self {
        let dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("wsh/recordings");
        Self::with_dir(dir)
    }

    /// Create a registry that stores cast files in `recordings_dir`.
    ///
    /// The directory is created on demand when the first recording starts.
    pub fn with_dir(recordings_dir: PathBuf) -> Self {
        Self {
            inner: Arc::new(RwLock::new(RegistryInner {
                recordings: HashMap::new(),
                active: HashMap::new(),
                recordings_dir,
            })),
        }
    }

    /// Return the directory where cast files are stored.
    pub fn recordings_dir(&self) -> PathBuf {
        self.inner.read().recordings_dir.clone()
    }

    /// Return the active recording ID for a session, if any.
    pub fn active_for_session(&self, session: &str) -> Option<String> {
        self.inner.read().active.get(session).cloned()
    }

    /// Return info for a single recording by ID.
    pub fn get(&self, id: &str) -> Option<RecordingInfo> {
        self.inner.read().recordings.get(id).map(|e| e.to_info())
    }

    /// Return all recordings, optionally filtered by session name.
    pub fn list(&self, session: Option<&str>) -> Vec<RecordingInfo> {
        let inner = self.inner.read();
        let mut infos: Vec<RecordingInfo> = inner
            .recordings
            .values()
            .filter(|e| session.is_none() || Some(e.session.as_str()) == session)
            .map(|e| e.to_info())
            .collect();
        // Stable order: newest first.
        infos.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        infos
    }

    /// Return the filesystem path for a recording's cast file.
    pub fn recording_path(&self, id: &str) -> Option<PathBuf> {
        self.inner.read().recordings.get(id).map(|e| e.path.clone())
    }

    /// Return `true` if the recording with `id` is currently active.
    pub fn is_active(&self, id: &str) -> bool {
        self.inner
            .read()
            .recordings
            .get(id)
            .map(|e| e.status == RecordingStatus::Recording)
            .unwrap_or(false)
    }

    /// Start a new recording for `session`.
    ///
    /// Subscribes to `output_rx` for PTY bytes and writes them as asciinema v2
    /// events to a new cast file. The recording task exits when:
    /// - `stop()` is called (via the recording's own cancel token), or
    /// - the session is killed (via `session_cancelled`), or
    /// - the broker channel closes (session destroyed).
    ///
    /// Returns `Err(StartError::AlreadyRecording)` if the session already has
    /// an active recording.
    pub fn start(
        &self,
        session: &str,
        title: Option<String>,
        width: u16,
        height: u16,
        output_rx: broadcast::Receiver<bytes::Bytes>,
        session_cancelled: CancellationToken,
    ) -> Result<RecordingInfo, StartError> {
        let mut inner = self.inner.write();

        if inner.active.contains_key(session) {
            return Err(StartError::AlreadyRecording);
        }

        let id = Uuid::new_v4().to_string();
        let started_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let path = inner.recordings_dir.join(format!("{id}.cast"));
        let cancel = CancellationToken::new();
        let bytes_written = Arc::new(AtomicU64::new(0));

        let entry = RecordingEntry {
            id: id.clone(),
            session: session.to_string(),
            title: title.clone(),
            started_at,
            stopped_at: None,
            bytes_written: bytes_written.clone(),
            status: RecordingStatus::Recording,
            path: path.clone(),
            width,
            height,
            cancel: cancel.clone(),
        };
        let info = entry.to_info();

        inner.recordings.insert(id.clone(), entry);
        inner.active.insert(session.to_string(), id.clone());
        let recordings_dir = inner.recordings_dir.clone();
        drop(inner);

        // Spawn the async recording task.
        let registry = self.clone();
        let id_clone = id.clone();
        let session_clone = session.to_string();
        tokio::spawn(async move {
            if let Err(e) = tokio::fs::create_dir_all(&recordings_dir).await {
                tracing::error!(%e, dir = %recordings_dir.display(), "failed to create recordings directory");
                registry.mark_failed(&id_clone, &session_clone);
                return;
            }
            match tokio::fs::File::create(&path).await {
                Err(e) => {
                    tracing::error!(%e, path = %path.display(), "failed to create recording file");
                    registry.mark_failed(&id_clone, &session_clone);
                }
                Ok(file) => {
                    run_recording(
                        file,
                        output_rx,
                        title,
                        width,
                        height,
                        started_at,
                        bytes_written,
                        cancel,
                        session_cancelled,
                    )
                    .await;
                    registry.finalize(&id_clone, &session_clone);
                }
            }
        });

        Ok(info)
    }

    /// Stop the active recording for `session`.
    ///
    /// Cancels the recording task, which flushes and syncs the file before
    /// exiting. Returns the current recording info (status may briefly still
    /// show `Recording` until the task confirms finalization). Returns `None`
    /// if no recording is active for the session.
    pub fn stop(&self, session: &str) -> Option<RecordingInfo> {
        let mut inner = self.inner.write();
        let id = inner.active.remove(session)?;
        let info = inner.recordings.get(&id).map(|e| e.to_info());
        if let Some(entry) = inner.recordings.get(&id) {
            entry.cancel.cancel();
        }
        info
    }

    /// Delete a recording by ID, cancelling it if active.
    ///
    /// Returns the path to the cast file so the caller can delete it from disk.
    /// Returns `None` if no recording exists with `id`.
    pub fn delete(&self, id: &str) -> Option<PathBuf> {
        let mut inner = self.inner.write();
        // Collect what we need before mutating.
        let (cancel, session_name, is_active) =
            if let Some(entry) = inner.recordings.get(id) {
                (
                    entry.cancel.clone(),
                    entry.session.clone(),
                    entry.status == RecordingStatus::Recording,
                )
            } else {
                return None;
            };
        cancel.cancel();
        if is_active {
            inner.active.remove(&session_name);
        }
        inner.recordings.remove(id).map(|e| e.path)
    }

    // ── Internal helpers (called from the recording task) ────────────────────

    fn finalize(&self, id: &str, session: &str) {
        let mut inner = self.inner.write();
        // Only remove from active map if we're still listed there (stop() may
        // have already removed us).
        inner.active.remove(session);
        if let Some(entry) = inner.recordings.get_mut(id) {
            let stopped_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            entry.stopped_at = Some(stopped_at);
            entry.status = RecordingStatus::Stopped;
        }
    }

    fn mark_failed(&self, id: &str, session: &str) {
        let mut inner = self.inner.write();
        inner.active.remove(session);
        if let Some(entry) = inner.recordings.get_mut(id) {
            let stopped_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            entry.stopped_at = Some(stopped_at);
            entry.status = RecordingStatus::Failed;
        }
    }
}

impl Default for RecordingRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Recording task ────────────────────────────────────────────────────────────

/// Write PTY output from `output_rx` to `file` in asciinema v2 format.
///
/// Runs until `cancel` fires, `session_cancelled` fires, or the broker channel
/// closes. Flushes and syncs the file before returning.
async fn run_recording(
    mut file: tokio::fs::File,
    mut output_rx: broadcast::Receiver<bytes::Bytes>,
    title: Option<String>,
    width: u16,
    height: u16,
    started_at_unix: u64,
    bytes_written: Arc<AtomicU64>,
    cancel: CancellationToken,
    session_cancelled: CancellationToken,
) {
    // Write asciinema v2 header.
    let title_field = match &title {
        Some(t) => format!(
            r#", "title": {}"#,
            serde_json::to_string(t).unwrap_or_else(|_| "\"\"".into())
        ),
        None => String::new(),
    };
    let header = format!(
        r#"{{"version": 2, "width": {width}, "height": {height}, "timestamp": {started_at_unix}{title_field}, "env": {{"TERM": "xterm-256color"}}}}
"#
    );

    if let Err(e) = file.write_all(header.as_bytes()).await {
        tracing::error!(%e, "failed to write recording header");
        return;
    }
    bytes_written.fetch_add(header.len() as u64, Ordering::Relaxed);

    let start = Instant::now();

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = session_cancelled.cancelled() => break,
            result = output_rx.recv() => {
                match result {
                    Ok(data) => {
                        let elapsed = start.elapsed().as_secs_f64();
                        // Terminal output is almost always valid UTF-8; lossy
                        // conversion handles the rare non-UTF-8 byte safely.
                        let text = String::from_utf8_lossy(&data);
                        let Ok(json_str) = serde_json::to_string(&*text) else {
                            continue;
                        };
                        // Event line: [elapsed, "o", "data"]\n
                        let event = format!("[{elapsed:.6}, \"o\", {json_str}]\n");
                        let event_bytes = event.as_bytes();
                        if let Err(e) = file.write_all(event_bytes).await {
                            tracing::error!(%e, "recording write error");
                            break;
                        }
                        bytes_written.fetch_add(event_bytes.len() as u64, Ordering::Relaxed);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            skipped = n,
                            "recording lagged, some output not captured"
                        );
                    }
                }
            }
        }
    }

    // Ensure all written data is on disk before the task exits.
    let _ = file.flush().await;
    let _ = file.sync_all().await;
}

// ── Player page HTML ──────────────────────────────────────────────────────────

const PLAYER_CSS_CDN: &str =
    "https://cdn.jsdelivr.net/npm/asciinema-player@3/dist/bundle/player.css";
const PLAYER_JS_CDN: &str =
    "https://cdn.jsdelivr.net/npm/asciinema-player@3/dist/bundle/player.js";

/// Build the full standalone player HTML page for a recording.
///
/// `cast_url` should be the absolute or relative URL of the cast file.
/// `width` / `height` are the terminal dimensions recorded in the header.
pub fn player_html(cast_url: &str, title: Option<&str>, width: u16, height: u16) -> String {
    let page_title = title.unwrap_or("Terminal Recording");
    let cast_url_json = serde_json::to_string(cast_url).unwrap_or_default();
    let title_json = serde_json::to_string(page_title).unwrap_or_default();
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{page_title}</title>
  <link rel="stylesheet" type="text/css" href="{PLAYER_CSS_CDN}">
  <style>
    * {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{ background: #1a1a1a; display: flex; flex-direction: column;
            align-items: center; justify-content: flex-start;
            min-height: 100vh; padding: 24px 16px; font-family: sans-serif; }}
    h1 {{ color: #ccc; font-size: 1rem; margin-bottom: 16px;
          font-weight: normal; letter-spacing: 0.02em; }}
    #player {{ width: 100%; max-width: 960px; }}
  </style>
</head>
<body>
  <h1>{page_title}</h1>
  <div id="player"></div>
  <script src="{PLAYER_JS_CDN}"></script>
  <script>
    AsciinemaPlayer.create({cast_url_json}, document.getElementById('player'), {{
      cols: {width},
      rows: {height},
      title: {title_json},
      autoPlay: false,
      fit: 'width'
    }});
  </script>
</body>
</html>
"#
    )
}

/// Build the HTML embed snippet for a recording.
///
/// `cast_url` must be an absolute URL reachable from wherever the snippet
/// will be embedded.
pub fn embed_html(cast_url: &str, title: Option<&str>, width: u16, height: u16) -> String {
    let container_id = format!("wsh-player-{}", Uuid::new_v4().to_string().replace('-', ""));
    let cast_url_json = serde_json::to_string(cast_url).unwrap_or_default();
    let title_json = match title {
        Some(t) => serde_json::to_string(t).unwrap_or_else(|_| "\"\"".into()),
        None => "\"Terminal Recording\"".into(),
    };
    format!(
        r#"<div id="{container_id}"></div>
<link rel="stylesheet" type="text/css" href="{PLAYER_CSS_CDN}">
<script src="{PLAYER_JS_CDN}"></script>
<script>
  AsciinemaPlayer.create({cast_url_json}, document.getElementById('{container_id}'), {{
    cols: {width}, rows: {height}, title: {title_json}, autoPlay: false, fit: 'width'
  }});
</script>"#
    )
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::sync::broadcast;

    fn registry_in_tempdir() -> (RecordingRegistry, TempDir) {
        let dir = TempDir::new().unwrap();
        let reg = RecordingRegistry::with_dir(dir.path().to_path_buf());
        (reg, dir)
    }

    #[tokio::test]
    async fn start_creates_cast_file_with_valid_header() {
        let (registry, _dir) = registry_in_tempdir();
        let (tx, rx) = broadcast::channel::<bytes::Bytes>(16);
        let cancel = CancellationToken::new();

        let info = registry
            .start("test-session", Some("My Test".into()), 80, 24, rx, cancel.clone())
            .unwrap();

        assert_eq!(info.session, "test-session");
        assert_eq!(info.title.as_deref(), Some("My Test"));
        assert_eq!(info.status, RecordingStatus::Recording);
        assert!(info.urls.cast.contains("/cast"));
        assert!(info.urls.player.contains("/player"));
        assert!(info.urls.embed.contains("/embed"));

        // Send a line of output then stop.
        tx.send(bytes::Bytes::from("hello world\r\n")).unwrap();
        drop(tx); // close the broadcast channel → recording task exits

        // Wait for the task to finalize.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let updated = registry.get(&info.id).unwrap();
        assert_eq!(updated.status, RecordingStatus::Stopped);
        assert!(updated.bytes_written > 0);

        // Read the cast file and validate format.
        let path = registry.recording_path(&info.id).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        let mut lines = contents.lines();

        // Header must be valid JSON with version:2
        let header: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(header["version"], 2);
        assert_eq!(header["width"], 80);
        assert_eq!(header["height"], 24);
        assert_eq!(header["title"], "My Test");

        // Event line must be a JSON array [elapsed, "o", data]
        let event_line = lines.next().unwrap();
        let event: serde_json::Value = serde_json::from_str(event_line).unwrap();
        assert!(event.is_array());
        let arr = event.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert!(arr[0].is_number());
        assert_eq!(arr[1], "o");
        assert!(arr[2].as_str().unwrap().contains("hello world"));
    }

    #[tokio::test]
    async fn start_returns_error_if_session_already_recording() {
        let (registry, _dir) = registry_in_tempdir();
        let (_tx1, rx1) = broadcast::channel::<bytes::Bytes>(16);
        let (_tx2, rx2) = broadcast::channel::<bytes::Bytes>(16);
        let cancel = CancellationToken::new();

        registry
            .start("sess", None, 80, 24, rx1, cancel.clone())
            .unwrap();
        let err = registry.start("sess", None, 80, 24, rx2, cancel).unwrap_err();
        assert!(matches!(err, StartError::AlreadyRecording));
    }

    #[tokio::test]
    async fn stop_cancels_recording_and_finalizes() {
        let (registry, _dir) = registry_in_tempdir();
        let (_tx, rx) = broadcast::channel::<bytes::Bytes>(16);
        let cancel = CancellationToken::new();

        let info = registry
            .start("sess", None, 80, 24, rx, cancel)
            .unwrap();
        assert_eq!(info.status, RecordingStatus::Recording);
        assert_eq!(registry.active_for_session("sess"), Some(info.id.clone()));

        registry.stop("sess");

        // Give the task time to finalize.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert_eq!(registry.active_for_session("sess"), None);
        let updated = registry.get(&info.id).unwrap();
        assert_eq!(updated.status, RecordingStatus::Stopped);
    }

    #[tokio::test]
    async fn delete_removes_entry_and_returns_path() {
        let (registry, _dir) = registry_in_tempdir();
        let (_tx, rx) = broadcast::channel::<bytes::Bytes>(16);
        let cancel = CancellationToken::new();

        let info = registry
            .start("sess", None, 80, 24, rx, cancel)
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let path = registry.delete(&info.id).unwrap();
        assert!(path.to_string_lossy().ends_with(".cast"));
        assert!(registry.get(&info.id).is_none());
    }

    #[test]
    fn list_filters_by_session() {
        // Just verify the filter logic works without spawning recording tasks.
        let (registry, _dir) = registry_in_tempdir();
        // registry is empty; list returns empty vec.
        assert!(registry.list(Some("nonexistent")).is_empty());
        assert!(registry.list(None).is_empty());
    }

    #[test]
    fn player_html_contains_cast_url_and_dimensions() {
        let html = player_html("/recordings/abc/cast", Some("Test"), 120, 40);
        assert!(html.contains("/recordings/abc/cast"));
        assert!(html.contains("cols: 120"));
        assert!(html.contains("rows: 40"));
        assert!(html.contains("Test"));
        assert!(html.contains(PLAYER_JS_CDN));
    }

    #[test]
    fn embed_html_contains_absolute_cast_url() {
        let html = embed_html("http://localhost:8080/recordings/abc/cast", Some("Test"), 80, 24);
        assert!(html.contains("http://localhost:8080/recordings/abc/cast"));
        assert!(html.contains("cols: 80"));
        assert!(html.contains(PLAYER_JS_CDN));
    }
}
