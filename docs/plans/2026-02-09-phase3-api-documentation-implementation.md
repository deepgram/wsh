# Phase 3: API Documentation & Onboarding — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make wsh's API thoroughly documented and properly authenticated so humans and AI agents can build against it immediately.

**Architecture:** Split monolithic `src/api.rs` into a module directory (`src/api/`), add an `ApiError` enum with `IntoResponse` for structured errors, add conditional auth middleware based on bind address, and embed hand-written docs/OpenAPI via `include_str!()`.

**Tech Stack:** Rust, axum 0.7, clap 4 (with `env` feature), `rand` crate for token generation. Nix development environment — all cargo commands must be wrapped: `nix develop -c sh -c "cargo ..."`.

**Working directory:** `/home/ajsyp/Projects/deepgram/wsh/.worktrees/phase3-api-docs`

**Branch:** `phase3-api-docs`

---

## Task 1: Split `src/api.rs` into module directory

Pure refactor. Move existing code into a module directory without changing any behavior. All 131 tests must still pass.

**Files:**
- Delete: `src/api.rs`
- Create: `src/api/mod.rs`
- Create: `src/api/handlers.rs`
- Modify: `src/lib.rs` (no change needed — `pub mod api` already works with a directory)

**Step 1: Create the module directory and move handler code**

Create `src/api/handlers.rs` containing all handler functions and their supporting types (everything except `AppState`, `router()`, and the test module). This file should contain:

```rust
use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::input::{InputBroadcaster, InputMode, Mode};
use crate::overlay::{Overlay, OverlaySpan, OverlayStore};
use crate::parser::{
    events::{Event, EventType, Subscribe},
    state::{Format, Query, QueryResponse},
    Parser,
};
use crate::shutdown::ShutdownCoordinator;

use super::AppState;

// All handler functions and their request/response types go here:
// - HealthResponse, health()
// - input()
// - ws_raw(), handle_ws_raw()
// - ws_json(), handle_ws_json()
// - ScreenQuery, screen()
// - ScrollbackQuery, default_limit(), scrollback()
// - CreateOverlayRequest, CreateOverlayResponse, UpdateOverlayRequest, PatchOverlayRequest
// - overlay_create(), overlay_list(), overlay_get(), overlay_update(), overlay_patch(), overlay_delete(), overlay_clear()
// - InputModeResponse, input_mode_get(), input_capture(), input_release()
```

Create `src/api/mod.rs` containing `AppState`, `router()`, and re-exports:

```rust
mod handlers;

use axum::{
    routing::{get, post},
    Router,
};
use bytes::Bytes;
use tokio::sync::{broadcast, mpsc};

use crate::input::{InputBroadcaster, InputMode};
use crate::overlay::OverlayStore;
use crate::parser::Parser;
use crate::shutdown::ShutdownCoordinator;

use handlers::*;

#[derive(Clone)]
pub struct AppState {
    pub input_tx: mpsc::Sender<Bytes>,
    pub output_rx: broadcast::Sender<Bytes>,
    pub shutdown: ShutdownCoordinator,
    pub parser: Parser,
    pub overlays: OverlayStore,
    pub input_mode: InputMode,
    pub input_broadcaster: InputBroadcaster,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/input", post(input))
        .route("/input/mode", get(input_mode_get))
        .route("/input/capture", post(input_capture))
        .route("/input/release", post(input_release))
        .route("/ws/raw", get(ws_raw))
        .route("/ws/json", get(ws_json))
        .route("/screen", get(screen))
        .route("/scrollback", get(scrollback))
        .route(
            "/overlay",
            get(overlay_list)
                .post(overlay_create)
                .delete(overlay_clear),
        )
        .route(
            "/overlay/:id",
            get(overlay_get)
                .put(overlay_update)
                .patch(overlay_patch)
                .delete(overlay_delete),
        )
        .with_state(state)
}
```

**Step 2: Delete the old `src/api.rs`**

Remove the original file. The module system now uses `src/api/mod.rs`.

**Step 3: Move the test module**

Move the `#[cfg(test)] mod tests` block from the old `api.rs` into `src/api/mod.rs` (or a separate `src/api/tests.rs` if preferred). The tests reference `super::*` so they need to live in the `mod.rs` or import from the right place.

Alternatively, create `src/api/tests.rs` and add `#[cfg(test)] mod tests;` to `mod.rs`, adjusting imports to use `super::*` → `super::super::*` or explicit paths.

Simplest approach: keep the test module in `mod.rs`.

**Step 4: Run tests to verify no behavior change**

Run: `nix develop -c sh -c "cargo test 2>&1"`
Expected: All 131 tests pass. No compilation errors.

**Step 5: Commit**

```bash
git add src/api/ src/api.rs
git commit -m "refactor(api): split api.rs into module directory

Move handlers into src/api/handlers.rs, keep AppState and router
in src/api/mod.rs. Pure refactor, no behavior change."
```

---

## Task 2: Add `ApiError` type with structured JSON responses

Create a structured error type that all handlers will use. This task only creates the type and its tests — handlers are migrated in Task 3.

**Files:**
- Create: `src/api/error.rs`
- Modify: `src/api/mod.rs` (add `pub mod error;`)

**Step 1: Write tests for ApiError**

Add to `src/api/error.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    #[tokio::test]
    async fn test_auth_required_status_and_body() {
        let err = ApiError::AuthRequired;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "auth_required");
        assert!(json["error"]["message"].is_string());
    }

    #[tokio::test]
    async fn test_auth_invalid_status_and_body() {
        let err = ApiError::AuthInvalid;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "auth_invalid");
    }

    #[tokio::test]
    async fn test_not_found_status_and_body() {
        let err = ApiError::NotFound;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn test_overlay_not_found_status_and_body() {
        let err = ApiError::OverlayNotFound("abc-123".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "overlay_not_found");
        let msg = json["error"]["message"].as_str().unwrap();
        assert!(msg.contains("abc-123"));
    }

    #[tokio::test]
    async fn test_invalid_request_status_and_body() {
        let err = ApiError::InvalidRequest("missing field 'x'".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "invalid_request");
    }

    #[tokio::test]
    async fn test_channel_full_status() {
        let err = ApiError::ChannelFull;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_parser_unavailable_status() {
        let err = ApiError::ParserUnavailable;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_input_send_failed_status() {
        let err = ApiError::InputSendFailed;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn test_internal_error_status() {
        let err = ApiError::InternalError("something broke".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "internal_error");
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `nix develop -c sh -c "cargo test api::error 2>&1"`
Expected: FAIL — `ApiError` does not exist yet.

**Step 3: Implement ApiError**

In `src/api/error.rs`:

```rust
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Structured API error type.
///
/// Every variant maps to an HTTP status code and a machine-readable error code.
/// The `code` field is stable and meant for programmatic matching.
/// The `message` field is human-readable and may change.
#[derive(Debug)]
pub enum ApiError {
    /// No token provided on a protected route
    AuthRequired,
    /// Token provided but doesn't match
    AuthInvalid,
    /// Unknown route
    NotFound,
    /// Overlay ID doesn't exist
    OverlayNotFound(String),
    /// Malformed JSON, missing fields, bad query params
    InvalidRequest(String),
    /// Overlay validation failure
    InvalidOverlay(String),
    /// Bad value for input mode
    InvalidInputMode(String),
    /// Unknown format query param
    InvalidFormat(String),
    /// Internal channel at capacity
    ChannelFull,
    /// Parser task has died
    ParserUnavailable,
    /// Failed to send input to PTY
    InputSendFailed,
    /// Unexpected/unclassified error
    InternalError(String),
}

impl ApiError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::AuthRequired => StatusCode::UNAUTHORIZED,
            Self::AuthInvalid => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::OverlayNotFound(_) => StatusCode::NOT_FOUND,
            Self::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            Self::InvalidOverlay(_) => StatusCode::BAD_REQUEST,
            Self::InvalidInputMode(_) => StatusCode::BAD_REQUEST,
            Self::InvalidFormat(_) => StatusCode::BAD_REQUEST,
            Self::ChannelFull => StatusCode::SERVICE_UNAVAILABLE,
            Self::ParserUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::InputSendFailed => StatusCode::INTERNAL_SERVER_ERROR,
            Self::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::AuthRequired => "auth_required",
            Self::AuthInvalid => "auth_invalid",
            Self::NotFound => "not_found",
            Self::OverlayNotFound(_) => "overlay_not_found",
            Self::InvalidRequest(_) => "invalid_request",
            Self::InvalidOverlay(_) => "invalid_overlay",
            Self::InvalidInputMode(_) => "invalid_input_mode",
            Self::InvalidFormat(_) => "invalid_format",
            Self::ChannelFull => "channel_full",
            Self::ParserUnavailable => "parser_unavailable",
            Self::InputSendFailed => "input_send_failed",
            Self::InternalError(_) => "internal_error",
        }
    }

    fn message(&self) -> String {
        match self {
            Self::AuthRequired => "Authentication required. Provide a token via Authorization header or ?token= query parameter.".to_string(),
            Self::AuthInvalid => "Invalid authentication token.".to_string(),
            Self::NotFound => "Not found.".to_string(),
            Self::OverlayNotFound(id) => format!("No overlay exists with id '{}'.", id),
            Self::InvalidRequest(detail) => format!("Invalid request: {}.", detail),
            Self::InvalidOverlay(detail) => format!("Invalid overlay: {}.", detail),
            Self::InvalidInputMode(detail) => format!("Invalid input mode: {}.", detail),
            Self::InvalidFormat(detail) => format!("Invalid format: {}.", detail),
            Self::ChannelFull => "Server is overloaded. Try again shortly.".to_string(),
            Self::ParserUnavailable => "Terminal parser is unavailable.".to_string(),
            Self::InputSendFailed => "Failed to send input to terminal.".to_string(),
            Self::InternalError(detail) => format!("Internal error: {}.", detail),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({
            "error": {
                "code": self.code(),
                "message": self.message(),
            }
        });
        (self.status_code(), Json(body)).into_response()
    }
}
```

Add to `src/api/mod.rs`:

```rust
pub mod error;
```

**Step 4: Run tests to verify they pass**

Run: `nix develop -c sh -c "cargo test api::error 2>&1"`
Expected: All ApiError tests pass.

Run: `nix develop -c sh -c "cargo test 2>&1"`
Expected: All 131+ tests pass.

**Step 5: Commit**

```bash
git add src/api/error.rs src/api/mod.rs
git commit -m "feat(api): add ApiError type with structured JSON responses

Each variant maps to an HTTP status, machine-readable code, and
human-readable message. Implements IntoResponse for axum handlers."
```

---

## Task 3: Migrate handlers to use ApiError

Update all handlers in `src/api/handlers.rs` to return `Result<T, ApiError>` instead of ad-hoc error types. Also update existing tests.

**Files:**
- Modify: `src/api/handlers.rs`
- Modify: `src/api/mod.rs` (test assertions for new error format)

**Step 1: Migrate handlers**

Update each handler in `src/api/handlers.rs`:

**`input()`** — currently returns `StatusCode` on error:
```rust
pub(super) async fn input(State(state): State<AppState>, body: Bytes) -> Result<StatusCode, ApiError> {
    state.input_tx.send(body).await.map_err(|e| {
        tracing::error!("Failed to send input to PTY: {}", e);
        ApiError::InputSendFailed
    })?;
    Ok(StatusCode::NO_CONTENT)
}
```

**`screen()`** — currently returns `(StatusCode, Json<Value>)` error:
```rust
pub(super) async fn screen(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<ScreenQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let response = state
        .parser
        .query(Query::Screen { format: params.format })
        .await
        .map_err(|_| ApiError::ParserUnavailable)?;
    Ok(Json(response))
}
```

**`scrollback()`** — same pattern as `screen()`:
```rust
pub(super) async fn scrollback(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<ScrollbackQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let response = state
        .parser
        .query(Query::Scrollback {
            format: params.format,
            offset: params.offset,
            limit: params.limit,
        })
        .await
        .map_err(|_| ApiError::ParserUnavailable)?;
    Ok(Json(response))
}
```

**`overlay_get()`** — currently returns `StatusCode::NOT_FOUND`:
```rust
pub(super) async fn overlay_get(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<Overlay>, ApiError> {
    state.overlays.get(&id).map(Json).ok_or_else(|| ApiError::OverlayNotFound(id))
}
```

**`overlay_update()`** — currently returns bare `StatusCode`:
```rust
pub(super) async fn overlay_update(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<UpdateOverlayRequest>,
) -> Result<StatusCode, ApiError> {
    if state.overlays.update(&id, req.spans) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::OverlayNotFound(id))
    }
}
```

**`overlay_patch()`** — same pattern:
```rust
pub(super) async fn overlay_patch(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<PatchOverlayRequest>,
) -> Result<StatusCode, ApiError> {
    if state.overlays.move_to(&id, req.x, req.y, req.z) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::OverlayNotFound(id))
    }
}
```

**`overlay_delete()`** — same pattern:
```rust
pub(super) async fn overlay_delete(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<StatusCode, ApiError> {
    if state.overlays.delete(&id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::OverlayNotFound(id))
    }
}
```

Handlers that already return infallible types (`health()`, `overlay_list()`, `overlay_create()`, `overlay_clear()`, `input_mode_get()`, `input_capture()`, `input_release()`) do not need to change.

**Step 2: Update unit tests in `src/api/mod.rs`**

The `test_overlay_delete` test for a non-existent overlay should now return a structured error. No test currently checks for error bodies on overlay handlers, so existing tests should still pass. But verify that the `test_overlay_delete` test (which deletes a real overlay) still returns `NO_CONTENT`.

**Step 3: Update integration tests**

In `tests/overlay_integration.rs`, the `test_overlay_not_found` test likely checks for `StatusCode::NOT_FOUND`. It should still pass because `ApiError::OverlayNotFound` returns 404. Verify the response body now includes structured JSON.

**Step 4: Run all tests**

Run: `nix develop -c sh -c "cargo test 2>&1"`
Expected: All tests pass. Existing status code assertions are unchanged (404 is still 404, etc.).

**Step 5: Commit**

```bash
git add src/api/handlers.rs src/api/mod.rs
git commit -m "feat(api): migrate handlers to structured ApiError responses

All error-returning handlers now use Result<T, ApiError> with
consistent JSON error format: {error: {code, message}}."
```

---

## Task 4: Add `--token` and `--shell` CLI flags

**Files:**
- Modify: `Cargo.toml` (add `rand` dep, clap `env` feature)
- Modify: `src/main.rs` (Args struct, token logic, shell flag threading)
- Modify: `src/pty.rs` (accept shell override in SpawnCommand)

**Step 1: Add dependencies**

In `Cargo.toml`, update clap and add rand:

```toml
clap = { version = "4", features = ["derive", "env"] }
rand = "0.8"
```

**Step 2: Update Args struct**

In `src/main.rs`, add new fields to `Args`:

```rust
#[derive(ClapParser, Debug)]
#[command(name = "wsh", version, about, long_about = None)]
struct Args {
    /// Address to bind the HTTP/WebSocket API server
    #[arg(long, default_value = "127.0.0.1:8080")]
    bind: SocketAddr,

    /// Authentication token for non-localhost bindings
    #[arg(long, env = "WSH_TOKEN")]
    token: Option<String>,

    /// Shell to spawn (overrides $SHELL)
    #[arg(long)]
    shell: Option<String>,

    /// Command string to execute (like sh -c)
    #[arg(short = 'c')]
    command: Option<String>,

    /// Force interactive mode
    #[arg(short = 'i')]
    interactive: bool,
}
```

**Step 3: Add token resolution logic**

In `src/main.rs`, after parsing args, add token resolution:

```rust
fn is_loopback(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

fn resolve_token(args: &Args) -> Option<String> {
    if is_loopback(&args.bind) {
        return None; // No auth needed for localhost
    }

    match &args.token {
        Some(token) => Some(token.clone()),
        None => {
            // Auto-generate a random token
            use rand::Rng;
            let token: String = rand::thread_rng()
                .sample_iter(&rand::distributions::Alphanumeric)
                .take(32)
                .map(char::from)
                .collect();
            eprintln!("wsh: API token (required for non-localhost): {}", token);
            Some(token)
        }
    }
}
```

**Step 4: Thread shell override to SpawnCommand**

Update the spawn command construction in `main()`:

```rust
let spawn_cmd = match &args.command {
    Some(cmd) => SpawnCommand::Command {
        command: cmd.clone(),
        interactive: args.interactive,
    },
    None => SpawnCommand::Shell {
        interactive: args.interactive,
        shell: args.shell.clone(),
    },
};
```

Update `SpawnCommand::Shell` in `src/pty.rs`:

```rust
pub enum SpawnCommand {
    Shell { interactive: bool, shell: Option<String> },
    Command { command: String, interactive: bool },
}

impl Default for SpawnCommand {
    fn default() -> Self {
        Self::Shell { interactive: false, shell: None }
    }
}
```

Update `build_command()` in `src/pty.rs`:

```rust
SpawnCommand::Shell { interactive, shell } => {
    let shell_path = shell.as_deref()
        .or(std::env::var("SHELL").ok().as_deref())
        .unwrap_or("/bin/sh")
        .to_string();
    let mut cmd = CommandBuilder::new(&shell_path);
    if *interactive {
        cmd.arg("-i");
    }
    cmd
}
```

Note: The `or()` chain won't work directly with `env::var` returning a `Result`. Use:

```rust
SpawnCommand::Shell { interactive, shell } => {
    let shell_path = match shell {
        Some(s) => s.clone(),
        None => std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
    };
    let mut cmd = CommandBuilder::new(&shell_path);
    if *interactive {
        cmd.arg("-i");
    }
    cmd
}
```

**Step 5: Update PTY tests that construct SpawnCommand**

Search for `SpawnCommand::Shell { interactive:` and `SpawnCommand::default()` in tests. The `Default` impl needs updating. Tests using `SpawnCommand::Shell { interactive: true }` need `shell: None` added.

Update in `src/pty.rs` tests:
- `test_spawn_interactive_shell`: `SpawnCommand::Shell { interactive: true, shell: None }`

Update in `src/main.rs`:
- `spawn_cmd` construction already handled above.

**Step 6: Run tests**

Run: `nix develop -c sh -c "cargo test 2>&1"`
Expected: All tests pass. (Token logic is not yet wired to the API — that's Task 5.)

**Step 7: Commit**

```bash
git add Cargo.toml src/main.rs src/pty.rs
git commit -m "feat(cli): add --token and --shell flags

--token (or WSH_TOKEN env var) sets auth token. Auto-generates if
binding to non-localhost without one. --shell overrides \$SHELL."
```

---

## Task 5: Add auth middleware

Conditional middleware that validates bearer tokens for non-localhost bindings.

**Files:**
- Create: `src/api/auth.rs`
- Modify: `src/api/mod.rs` (add module, update router to accept optional token)

**Step 1: Write auth middleware tests**

In `src/api/auth.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request, routing::get, Router};
    use tower::ServiceExt;

    async fn ok_handler() -> &'static str {
        "ok"
    }

    fn test_app(token: String) -> Router {
        Router::new()
            .route("/protected", get(ok_handler))
            .layer(auth_layer(token))
    }

    #[tokio::test]
    async fn test_valid_bearer_token() {
        let app = test_app("secret".to_string());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("Authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_valid_query_param_token() {
        let app = test_app("secret".to_string());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/protected?token=secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_missing_token_returns_401() {
        let app = test_app("secret".to_string());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "auth_required");
    }

    #[tokio::test]
    async fn test_wrong_token_returns_403() {
        let app = test_app("secret".to_string());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("Authorization", "Bearer wrong")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "auth_invalid");
    }

    #[tokio::test]
    async fn test_bearer_takes_precedence_over_query() {
        let app = test_app("secret".to_string());
        // Bearer is correct, query is wrong — should succeed (bearer checked first)
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/protected?token=wrong")
                    .header("Authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `nix develop -c sh -c "cargo test api::auth 2>&1"`
Expected: FAIL — module does not exist.

**Step 3: Implement auth middleware**

In `src/api/auth.rs`:

```rust
use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::error::ApiError;

/// Extract token from request: check Authorization header first, then ?token= query param.
fn extract_token(req: &Request) -> Option<String> {
    // Check Authorization: Bearer <token> header
    if let Some(auth_header) = req.headers().get("authorization") {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                return Some(token.to_string());
            }
        }
    }

    // Check ?token=<token> query parameter
    if let Some(query) = req.uri().query() {
        for pair in query.split('&') {
            if let Some(value) = pair.strip_prefix("token=") {
                return Some(value.to_string());
            }
        }
    }

    None
}

/// Middleware that validates the bearer token.
pub async fn require_auth(
    req: Request,
    next: Next,
) -> Response {
    // Token is stored in request extensions by the layer
    let expected_token = req
        .extensions()
        .get::<ExpectedToken>()
        .expect("ExpectedToken extension missing — auth_layer not applied?");

    match extract_token(&req) {
        None => ApiError::AuthRequired.into_response(),
        Some(token) if token != expected_token.0 => ApiError::AuthInvalid.into_response(),
        Some(_) => next.run(req).await,
    }
}

/// Wrapper to store expected token in request extensions.
#[derive(Clone)]
pub struct ExpectedToken(pub String);

/// Create an auth middleware layer for the given token.
pub fn auth_layer(token: String) -> axum::middleware::from_fn_with_state::IntoLayer<...> {
    // Actually, using extensions is cleaner. Use AddExtension + from_fn.
}
```

Actually, the simplest approach with axum is to use `axum::middleware::from_fn` with a closure that captures the token. But closures can't be used with `from_fn` directly. Instead, use `from_fn_with_state`:

```rust
use axum::middleware;

pub fn auth_layer(token: String) -> middleware::FromFnLayer<..., String, ...> {
    middleware::from_fn_with_state(token, require_auth)
}

pub async fn require_auth(
    axum::extract::State(expected_token): axum::extract::State<String>,
    req: Request,
    next: Next,
) -> Response {
    match extract_token(&req) {
        None => ApiError::AuthRequired.into_response(),
        Some(token) if token != expected_token => ApiError::AuthInvalid.into_response(),
        Some(_) => next.run(req).await,
    }
}
```

Hmm, this conflicts with the existing `AppState` state. Better approach: use `Extension`:

```rust
use axum::{Extension, middleware};

#[derive(Clone)]
pub struct AuthToken(pub String);

pub async fn require_auth(
    Extension(expected): Extension<AuthToken>,
    req: Request,
    next: Next,
) -> Response {
    match extract_token(&req) {
        None => ApiError::AuthRequired.into_response(),
        Some(token) if token != expected.0 => ApiError::AuthInvalid.into_response(),
        Some(_) => next.run(req).await,
    }
}

/// Create the middleware stack: Extension layer + from_fn layer.
pub fn auth_layer(token: String) -> (Extension<AuthToken>, middleware::FromFnLayer<...>) {
    // Return a tuple of layers
}
```

Simplest approach: just return a `Router` wrapper function. Update `router()` in `mod.rs` to conditionally apply auth. Let's use the simplest pattern that works:

```rust
// src/api/auth.rs
use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::Response,
};

use super::error::ApiError;

fn extract_token(req: &Request) -> Option<String> {
    // Check Authorization: Bearer <token> header
    if let Some(auth_header) = req.headers().get("authorization") {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                return Some(token.to_string());
            }
        }
    }

    // Check ?token=<token> query parameter
    if let Some(query) = req.uri().query() {
        for pair in query.split('&') {
            if let Some(value) = pair.strip_prefix("token=") {
                return Some(value.to_string());
            }
        }
    }

    None
}

/// Auth middleware. The expected token is captured in the closure via `from_fn`.
/// Use `make_auth_middleware(token)` to create it.
pub async fn check_auth(
    expected_token: String,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    match extract_token(&req) {
        None => Err(ApiError::AuthRequired),
        Some(token) if token != expected_token => Err(ApiError::AuthInvalid),
        Some(_) => Ok(next.run(req).await),
    }
}
```

The actual integration: in `src/api/mod.rs`, update `router()` to accept an optional token and conditionally apply a middleware layer. Use `axum::middleware::from_fn` with a closure. Since `from_fn` needs an async fn (not a closure), we'll use `tower::ServiceBuilder` with a custom layer, or we'll store the token in the router state and extract it.

Cleanest axum pattern — store in the router and use `from_fn`:

```rust
// src/api/mod.rs

pub fn router(state: AppState, token: Option<String>) -> Router {
    let health_route = Router::new().route("/health", get(health));

    let protected_routes = Router::new()
        .route("/input", post(input))
        // ... all other routes
        .with_state(state);

    let protected_routes = if let Some(token) = token {
        protected_routes.layer(axum::middleware::from_fn(move |req, next| {
            let token = token.clone();
            auth::check_auth(token, req, next)
        }))
    } else {
        protected_routes
    };

    Router::new()
        .merge(health_route.with_state(/* need state for health too */))
        .merge(protected_routes)
}
```

Wait, this doesn't work cleanly because `health` doesn't use state. Let me reconsider.

Actually `health()` doesn't use `State` at all, so it can live on a stateless router:

```rust
pub fn router(state: AppState, token: Option<String>) -> Router {
    let mut app = Router::new()
        .route("/health", get(health))
        .route("/input", post(input))
        .route("/input/mode", get(input_mode_get))
        .route("/input/capture", post(input_capture))
        .route("/input/release", post(input_release))
        .route("/ws/raw", get(ws_raw))
        .route("/ws/json", get(ws_json))
        .route("/screen", get(screen))
        .route("/scrollback", get(scrollback))
        .route(
            "/overlay",
            get(overlay_list).post(overlay_create).delete(overlay_clear),
        )
        .route(
            "/overlay/:id",
            get(overlay_get).put(overlay_update).patch(overlay_patch).delete(overlay_delete),
        )
        .with_state(state);

    if let Some(token) = token {
        // Wrap everything except /health with auth
        // Unfortunately axum doesn't support per-route middleware easily.
        // Use a route-level approach: split into two routers.
    }

    app
}
```

The cleanest axum approach for "auth on all routes except /health":

```rust
pub fn router(state: AppState, token: Option<String>) -> Router {
    let protected = Router::new()
        .route("/input", post(input))
        .route("/input/mode", get(input_mode_get))
        .route("/input/capture", post(input_capture))
        .route("/input/release", post(input_release))
        .route("/ws/raw", get(ws_raw))
        .route("/ws/json", get(ws_json))
        .route("/screen", get(screen))
        .route("/scrollback", get(scrollback))
        .route(
            "/overlay",
            get(overlay_list).post(overlay_create).delete(overlay_clear),
        )
        .route(
            "/overlay/:id",
            get(overlay_get).put(overlay_update).patch(overlay_patch).delete(overlay_delete),
        )
        .with_state(state);

    let protected = match token {
        Some(token) => {
            protected.layer(axum::middleware::from_fn(move |req, next| {
                let t = token.clone();
                async move { auth::check_auth(t, req, next).await }
            }))
        }
        None => protected,
    };

    Router::new()
        .route("/health", get(health))
        .merge(protected)
}
```

Note: `GET /docs` and `GET /openapi.yaml` (added in Task 7) should also be unauthenticated — they are documentation endpoints. We'll handle that then.

**Step 4: Update `main()` to pass token**

In `src/main.rs`:

```rust
let token = resolve_token(&args);
let app = api::router(state, token);
```

**Step 5: Update test helper `create_test_state` / `create_test_app`**

In unit tests (`src/api/mod.rs`) and integration tests, the `router()` call now needs a second argument. Pass `None` for all existing tests (no auth):

```rust
let app = router(state, None);
```

Update in:
- `src/api/mod.rs` tests: `router(state)` → `router(state, None)`
- `tests/api_integration.rs`: `router(state)` → `router(state, None)`
- `tests/input_capture_integration.rs`: `router(state)` → `router(state, None)`
- `tests/overlay_integration.rs`: `router(state)` → `router(state, None)`
- `tests/graceful_shutdown.rs`: check if it uses `router()`
- Any other test file that calls `router()`

**Step 6: Run tests**

Run: `nix develop -c sh -c "cargo test 2>&1"`
Expected: All tests pass, including new auth tests.

**Step 7: Commit**

```bash
git add src/api/auth.rs src/api/mod.rs src/main.rs tests/
git commit -m "feat(api): add conditional auth middleware

Bearer token auth required for non-localhost bindings. Token checked
from Authorization header first, then ?token= query param. Health
endpoint is exempt."
```

---

## Task 6: Add auth integration tests

Verify auth works end-to-end with the full router.

**Files:**
- Create: `tests/auth_integration.rs`

**Step 1: Write integration tests**

```rust
//! Integration tests for authentication.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use bytes::Bytes;
use tokio::sync::mpsc;
use tower::ServiceExt;
use wsh::api::{router, AppState};
use wsh::broker::Broker;
use wsh::input::{InputBroadcaster, InputMode};
use wsh::overlay::OverlayStore;
use wsh::parser::Parser;
use wsh::shutdown::ShutdownCoordinator;

fn create_test_state() -> AppState {
    let (input_tx, _) = mpsc::channel::<Bytes>(64);
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    AppState {
        input_tx,
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
    }
}

#[tokio::test]
async fn test_auth_required_on_protected_routes() {
    let state = create_test_state();
    let app = router(state, Some("test-token".to_string()));

    // /screen without token should be 401
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/screen").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_health_exempt_from_auth() {
    let state = create_test_state();
    let app = router(state, Some("test-token".to_string()));

    let response = app
        .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_bearer_token_grants_access() {
    let state = create_test_state();
    let app = router(state, Some("test-token".to_string()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/screen")
                .header("Authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_query_param_token_grants_access() {
    let state = create_test_state();
    let app = router(state, Some("test-token".to_string()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/screen?token=test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_wrong_token_returns_403() {
    let state = create_test_state();
    let app = router(state, Some("test-token".to_string()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/screen")
                .header("Authorization", "Bearer wrong-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_no_auth_when_token_is_none() {
    let state = create_test_state();
    let app = router(state, None);

    // Should work without any token
    let response = app
        .oneshot(Request::builder().uri("/screen").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
```

**Step 2: Run tests**

Run: `nix develop -c sh -c "cargo test auth_integration 2>&1"`
Expected: All 6 tests pass.

**Step 3: Commit**

```bash
git add tests/auth_integration.rs
git commit -m "test: add auth integration tests

Verify auth enforcement end-to-end: protected routes require token,
health is exempt, bearer and query param both work, wrong token
returns 403, no auth when token is None."
```

---

## Task 7: Write human-readable API documentation

Create the markdown documentation files for API consumers.

**Files:**
- Create: `docs/api/README.md`
- Create: `docs/api/authentication.md`
- Create: `docs/api/websocket.md`
- Create: `docs/api/errors.md`
- Create: `docs/api/overlays.md`
- Create: `docs/api/input-capture.md`

**Step 1: Write `docs/api/README.md`**

API overview, quick start (curl examples), endpoint table, navigation links. Include examples of hitting `/health`, `/screen`, `POST /input`, and subscribing to `/ws/json`. Show the 30-second quick start: start wsh, curl health, curl screen.

**Step 2: Write `docs/api/authentication.md`**

Cover: when auth applies (localhost vs remote), token lifecycle (--token, WSH_TOKEN, auto-generate), HTTP auth (header vs query param), WebSocket auth (?token=), health exemption. Include curl examples with and without auth.

**Step 3: Write `docs/api/websocket.md`**

Cover: `/ws/raw` (binary protocol, xterm.js compatible), `/ws/json` (subscription protocol, event types, shapes). Document the subscribe message format, all event types with example JSON payloads, reconnection behavior.

**Step 4: Write `docs/api/errors.md`**

Cover: error response format, complete error code table (from the design doc), example error responses for common scenarios.

**Step 5: Write `docs/api/overlays.md`**

Cover: what overlays are, CRUD lifecycle, positioning and z-index, styling (colors, bold, italic, underline), curl examples for create, list, get, update, patch, delete, clear.

**Step 6: Write `docs/api/input-capture.md`**

Cover: passthrough vs capture modes, switching modes via API, escape hatch (Ctrl+\), subscribing to input events via WebSocket, key event format, curl and wscat examples.

**Step 7: Commit**

```bash
git add docs/api/
git commit -m "docs: add comprehensive API documentation

Human-readable guides for API consumers: overview with quick start,
authentication, WebSocket protocols, error codes, overlays, and
input capture. Includes curl examples throughout."
```

---

## Task 8: Write OpenAPI spec

Hand-written OpenAPI 3.1 spec covering all endpoints.

**Files:**
- Create: `docs/api/openapi.yaml`

**Step 1: Write the OpenAPI spec**

Document all endpoints with:
- Path, method, summary, description
- Request body schemas (JSON)
- Response schemas (JSON) for success and error cases
- Query parameter schemas
- Authentication scheme (bearer token, query param)
- WebSocket endpoints documented as GET with upgrade note

Cover all endpoints:
- `GET /health`
- `POST /input`
- `GET /screen`
- `GET /scrollback`
- `GET /ws/raw`
- `GET /ws/json`
- `POST /overlay`, `GET /overlay`, `DELETE /overlay`
- `GET /overlay/:id`, `PUT /overlay/:id`, `PATCH /overlay/:id`, `DELETE /overlay/:id`
- `GET /input/mode`, `POST /input/capture`, `POST /input/release`
- `GET /openapi.yaml`
- `GET /docs`

Define component schemas for all request/response types:
- `ScreenResponse`, `ScrollbackResponse`, `CursorResponse`
- `FormattedLine` (plain or styled), `Span`, `Style`, `Color`
- `Overlay`, `OverlaySpan`
- `InputModeResponse`, `InputEvent`
- `ErrorResponse` (the `{error: {code, message}}` shape)
- `Subscribe` message for WebSocket

**Step 2: Validate YAML syntax**

Run: `nix develop -c sh -c "python3 -c \"import yaml; yaml.safe_load(open('docs/api/openapi.yaml'))\""`
(or use any YAML parser available in the Nix environment)

**Step 3: Commit**

```bash
git add docs/api/openapi.yaml
git commit -m "docs: add OpenAPI 3.1 specification

Hand-written spec covering all HTTP and WebSocket endpoints,
request/response schemas, authentication, and error codes."
```

---

## Task 9: Serve docs and OpenAPI at runtime

Add `GET /docs` and `GET /openapi.yaml` endpoints that serve the embedded documentation.

**Files:**
- Modify: `src/api/handlers.rs` (add doc handlers)
- Modify: `src/api/mod.rs` (add routes, outside auth layer)

**Step 1: Write tests for doc endpoints**

Add to the test module in `src/api/mod.rs` (or `tests/api_integration.rs`):

```rust
#[tokio::test]
async fn test_openapi_endpoint() {
    let state = create_test_state();
    let app = router(state, None);
    let response = app
        .oneshot(Request::builder().uri("/openapi.yaml").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(content_type.contains("yaml") || content_type.contains("text/plain"));
}

#[tokio::test]
async fn test_docs_endpoint() {
    let state = create_test_state();
    let app = router(state, None);
    let response = app
        .oneshot(Request::builder().uri("/docs").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(content_type.contains("text/markdown") || content_type.contains("text/plain"));
}
```

**Step 2: Run tests to verify they fail**

Expected: 404 on both endpoints.

**Step 3: Implement doc handlers**

In `src/api/handlers.rs`:

```rust
use axum::http::header;

const OPENAPI_SPEC: &str = include_str!("../../docs/api/openapi.yaml");
const API_DOCS: &str = include_str!("../../docs/api/README.md");

pub(super) async fn openapi() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/yaml; charset=utf-8")], OPENAPI_SPEC)
}

pub(super) async fn docs() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/markdown; charset=utf-8")], API_DOCS)
}
```

In `src/api/mod.rs`, add routes outside the auth layer (alongside `/health`):

```rust
Router::new()
    .route("/health", get(health))
    .route("/openapi.yaml", get(openapi))
    .route("/docs", get(docs))
    .merge(protected)
```

**Step 4: Run tests**

Run: `nix develop -c sh -c "cargo test 2>&1"`
Expected: All tests pass including new doc endpoint tests.

**Step 5: Commit**

```bash
git add src/api/handlers.rs src/api/mod.rs
git commit -m "feat(api): serve docs and OpenAPI spec at runtime

GET /openapi.yaml serves the OpenAPI spec (text/yaml).
GET /docs serves the API guide (text/markdown).
Both embedded via include_str!() and exempt from auth."
```

---

## Task 10: Update root README

Update the repo root README for end-user onboarding.

**Files:**
- Modify: `README.md`

**Step 1: Update README**

Rewrite to cover:
- What wsh is (one paragraph)
- Quick start (install, run, verify with curl)
- CLI flags reference
- Architecture overview (brief)
- Link to `docs/api/` for API documentation
- Link to `docs/VISION.md` for the full vision
- Security model summary

Keep it concise — this is a landing page, not exhaustive documentation.

**Step 2: Commit**

```bash
git add README.md
git commit -m "docs: update README with end-user onboarding guide

Quick start, CLI reference, architecture overview, and links to
API documentation and project vision."
```

---

## Task 11: Update roadmap with Phase 3 completion status

**Files:**
- Modify: `docs/plans/2026-02-03-implementation-roadmap.md`

**Step 1: Mark Phase 3 as complete**

Update the roadmap to reflect Phase 3 completion, similar to how Phase 1 and Phase 2 are marked.

**Step 2: Commit**

```bash
git add docs/plans/2026-02-03-implementation-roadmap.md
git commit -m "docs: mark Phase 3 as complete in roadmap"
```

---

## Task 12: Final verification

Run the full test suite one final time and verify everything is clean.

**Step 1: Run all tests**

Run: `nix develop -c sh -c "cargo test 2>&1"`
Expected: All tests pass (should be 140+ with new tests).

**Step 2: Run clippy**

Run: `nix develop -c sh -c "cargo clippy 2>&1"`
Expected: No warnings.

**Step 3: Verify the binary runs**

Run: `nix develop -c sh -c "cargo build 2>&1"`
Expected: Clean build.

**Step 4: Manual smoke test (optional)**

```bash
# In one terminal:
nix develop -c sh -c "cargo run"

# In another:
curl http://localhost:8080/health
curl http://localhost:8080/docs
curl http://localhost:8080/openapi.yaml
curl http://localhost:8080/screen
```

---

## Summary

| Task | Description | Estimated Tests Added |
|------|-------------|----------------------|
| 1 | Split api.rs into module | 0 (refactor) |
| 2 | ApiError type | ~10 |
| 3 | Migrate handlers to ApiError | 0 (update existing) |
| 4 | CLI flags (--token, --shell) | 0 (runtime only) |
| 5 | Auth middleware | ~5 |
| 6 | Auth integration tests | ~6 |
| 7 | API documentation (markdown) | 0 (docs) |
| 8 | OpenAPI spec | 0 (docs) |
| 9 | Serve docs at runtime | ~2 |
| 10 | Update README | 0 (docs) |
| 11 | Update roadmap | 0 (docs) |
| 12 | Final verification | 0 (verification) |
