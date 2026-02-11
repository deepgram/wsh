# Server Mode Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Refactor wsh from a single-session, single-process model to a client/server architecture where a server process manages multiple named sessions, each with its own PTY, exposed through a unified HTTP/WS API.

**Architecture:** Extract per-session state into a `Session` struct, introduce a `SessionRegistry` to manage multiple sessions by name, refactor the API to scope endpoints under `/sessions/:name/`, and add server-level endpoints and a multiplexed WebSocket. The CLI becomes a thin client communicating over a Unix domain socket.

**Tech Stack:** Rust, Tokio, Axum 0.7, Unix domain sockets (`tokio::net::UnixListener`), existing deps (portable-pty, crossterm, avt)

**Design doc:** `docs/plans/2026-02-11-server-mode-design.md`

---

## Phase 1: Session Abstraction & Multi-Session API

Phase 1 keeps the single-process model but refactors internals so the API
supports multiple named sessions. After this phase, `wsh` starts a server
in-process, creates one session, and the API works under `/sessions/:name/`.

### Task 1: Create `Session` struct

Extract per-session state from `AppState` into a new `Session` struct.

**Files:**
- Create: `src/session.rs`
- Modify: `src/lib.rs` (add `pub mod session;`)

**Step 1: Write the failing test**

Create `src/session.rs` with a test module:

```rust
use std::sync::Arc;
use bytes::Bytes;
use tokio::sync::{broadcast, mpsc};

use crate::activity::ActivityTracker;
use crate::broker::Broker;
use crate::input::{InputBroadcaster, InputMode};
use crate::overlay::OverlayStore;
use crate::panel::PanelStore;
use crate::parser::Parser;
use crate::pty::Pty;
use crate::shutdown::ShutdownCoordinator;
use crate::terminal::TerminalSize;

/// Per-session state. Each session owns a PTY and all associated state.
#[derive(Clone)]
pub struct Session {
    pub name: String,
    pub input_tx: mpsc::Sender<Bytes>,
    pub output_rx: broadcast::Sender<Bytes>,
    pub shutdown: ShutdownCoordinator,
    pub parser: Parser,
    pub overlays: OverlayStore,
    pub panels: PanelStore,
    pub pty: Arc<Pty>,
    pub terminal_size: TerminalSize,
    pub input_mode: InputMode,
    pub input_broadcaster: InputBroadcaster,
    pub activity: ActivityTracker,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::SpawnCommand;

    #[test]
    fn session_has_name() {
        let (input_tx, _input_rx) = mpsc::channel(64);
        let broker = Broker::new();
        let parser = Parser::spawn(&broker, 80, 24, 1000);
        let session = Session {
            name: "test-session".to_string(),
            input_tx,
            output_rx: broker.sender(),
            shutdown: ShutdownCoordinator::new(),
            parser,
            overlays: OverlayStore::new(),
            panels: PanelStore::new(),
            pty: Arc::new(Pty::spawn(24, 80, SpawnCommand::default()).unwrap()),
            terminal_size: TerminalSize::new(24, 80),
            input_mode: InputMode::new(),
            input_broadcaster: InputBroadcaster::new(),
            activity: ActivityTracker::new(),
        };
        assert_eq!(session.name, "test-session");
    }

    #[test]
    fn session_is_cloneable() {
        let (input_tx, _input_rx) = mpsc::channel(64);
        let broker = Broker::new();
        let parser = Parser::spawn(&broker, 80, 24, 1000);
        let session = Session {
            name: "clone-test".to_string(),
            input_tx,
            output_rx: broker.sender(),
            shutdown: ShutdownCoordinator::new(),
            parser,
            overlays: OverlayStore::new(),
            panels: PanelStore::new(),
            pty: Arc::new(Pty::spawn(24, 80, SpawnCommand::default()).unwrap()),
            terminal_size: TerminalSize::new(24, 80),
            input_mode: InputMode::new(),
            input_broadcaster: InputBroadcaster::new(),
            activity: ActivityTracker::new(),
        };
        let cloned = session.clone();
        assert_eq!(cloned.name, "clone-test");
    }
}
```

**Step 2: Add module to `src/lib.rs`**

Add `pub mod session;` to `src/lib.rs`.

**Step 3: Run tests to verify they pass**

Run: `nix develop -c sh -c "cargo test session::tests"`
Expected: PASS (2 tests)

**Step 4: Commit**

```bash
git add src/session.rs src/lib.rs
git commit -m "feat: add Session struct for per-session state"
```

---

### Task 2: Create `SessionRegistry`

A concurrent map managing sessions by name, with auto-generated numeric names.

**Files:**
- Modify: `src/session.rs`

**Step 1: Write the failing tests**

Add to `src/session.rs`:

```rust
use std::collections::HashMap;
use std::sync::RwLock;
use tokio::sync::broadcast as tokio_broadcast;

/// Manages multiple sessions by name.
#[derive(Clone)]
pub struct SessionRegistry {
    inner: Arc<RwLock<RegistryInner>>,
    /// Broadcast channel for server-level events (session created/destroyed/exited).
    events_tx: tokio_broadcast::Sender<SessionEvent>,
}

struct RegistryInner {
    sessions: HashMap<String, Session>,
    next_id: u64,
}

/// Server-level session lifecycle events.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    Created { name: String },
    Exited { name: String },
    Destroyed { name: String },
}

impl SessionRegistry {
    pub fn new() -> Self { ... }

    /// Insert a session. If name is None, auto-generate a numeric name.
    /// Returns the assigned name, or an error if the name is already taken.
    pub fn insert(&self, name: Option<String>, session: Session) -> Result<String, RegistryError> { ... }

    /// Look up a session by name.
    pub fn get(&self, name: &str) -> Option<Session> { ... }

    /// Remove a session by name. Returns the removed session if it existed.
    pub fn remove(&self, name: &str) -> Option<Session> { ... }

    /// Rename a session. Returns error if old name doesn't exist or new name is taken.
    pub fn rename(&self, old_name: &str, new_name: &str) -> Result<(), RegistryError> { ... }

    /// List all session names.
    pub fn list(&self) -> Vec<String> { ... }

    /// Number of active sessions.
    pub fn len(&self) -> usize { ... }

    /// Subscribe to server-level session events.
    pub fn subscribe_events(&self) -> tokio_broadcast::Receiver<SessionEvent> { ... }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("session name already exists: {0}")]
    NameExists(String),
    #[error("session not found: {0}")]
    NotFound(String),
}
```

Add tests:

```rust
#[cfg(test)]
mod tests {
    // ... existing tests ...

    // --- SessionRegistry tests ---

    fn make_test_session(name: &str) -> Session {
        let (input_tx, _input_rx) = mpsc::channel(64);
        let broker = Broker::new();
        let parser = Parser::spawn(&broker, 80, 24, 1000);
        Session {
            name: name.to_string(),
            input_tx,
            output_rx: broker.sender(),
            shutdown: ShutdownCoordinator::new(),
            parser,
            overlays: OverlayStore::new(),
            panels: PanelStore::new(),
            pty: Arc::new(Pty::spawn(24, 80, SpawnCommand::default()).unwrap()),
            terminal_size: TerminalSize::new(24, 80),
            input_mode: InputMode::new(),
            input_broadcaster: InputBroadcaster::new(),
            activity: ActivityTracker::new(),
        }
    }

    #[test]
    fn registry_insert_with_name() {
        let reg = SessionRegistry::new();
        let s = make_test_session("my-session");
        let name = reg.insert(Some("my-session".into()), s).unwrap();
        assert_eq!(name, "my-session");
        assert!(reg.get("my-session").is_some());
    }

    #[test]
    fn registry_insert_auto_name() {
        let reg = SessionRegistry::new();
        let s = make_test_session("");
        let name = reg.insert(None, s).unwrap();
        assert_eq!(name, "0");
        let s2 = make_test_session("");
        let name2 = reg.insert(None, s2).unwrap();
        assert_eq!(name2, "1");
    }

    #[test]
    fn registry_insert_duplicate_name_fails() {
        let reg = SessionRegistry::new();
        let s1 = make_test_session("dup");
        let s2 = make_test_session("dup");
        reg.insert(Some("dup".into()), s1).unwrap();
        let err = reg.insert(Some("dup".into()), s2).unwrap_err();
        assert!(matches!(err, RegistryError::NameExists(_)));
    }

    #[test]
    fn registry_remove() {
        let reg = SessionRegistry::new();
        let s = make_test_session("rm-me");
        reg.insert(Some("rm-me".into()), s).unwrap();
        assert!(reg.remove("rm-me").is_some());
        assert!(reg.get("rm-me").is_none());
    }

    #[test]
    fn registry_remove_nonexistent() {
        let reg = SessionRegistry::new();
        assert!(reg.remove("nope").is_none());
    }

    #[test]
    fn registry_rename() {
        let reg = SessionRegistry::new();
        let s = make_test_session("old-name");
        reg.insert(Some("old-name".into()), s).unwrap();
        reg.rename("old-name", "new-name").unwrap();
        assert!(reg.get("old-name").is_none());
        assert!(reg.get("new-name").is_some());
        assert_eq!(reg.get("new-name").unwrap().name, "new-name");
    }

    #[test]
    fn registry_rename_to_existing_fails() {
        let reg = SessionRegistry::new();
        reg.insert(Some("a".into()), make_test_session("a")).unwrap();
        reg.insert(Some("b".into()), make_test_session("b")).unwrap();
        let err = reg.rename("a", "b").unwrap_err();
        assert!(matches!(err, RegistryError::NameExists(_)));
    }

    #[test]
    fn registry_rename_nonexistent_fails() {
        let reg = SessionRegistry::new();
        let err = reg.rename("nope", "new").unwrap_err();
        assert!(matches!(err, RegistryError::NotFound(_)));
    }

    #[test]
    fn registry_list() {
        let reg = SessionRegistry::new();
        reg.insert(Some("b".into()), make_test_session("b")).unwrap();
        reg.insert(Some("a".into()), make_test_session("a")).unwrap();
        let mut names = reg.list();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn registry_len() {
        let reg = SessionRegistry::new();
        assert_eq!(reg.len(), 0);
        reg.insert(None, make_test_session("")).unwrap();
        assert_eq!(reg.len(), 1);
        reg.insert(None, make_test_session("")).unwrap();
        assert_eq!(reg.len(), 2);
        reg.remove("0");
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn registry_auto_name_skips_taken_names() {
        let reg = SessionRegistry::new();
        // Manually take name "0"
        reg.insert(Some("0".into()), make_test_session("0")).unwrap();
        // Auto-name should skip "0" and assign "1"
        let name = reg.insert(None, make_test_session("")).unwrap();
        assert_eq!(name, "1");
    }

    #[tokio::test]
    async fn registry_emits_events() {
        let reg = SessionRegistry::new();
        let mut events = reg.subscribe_events();
        reg.insert(Some("ev-test".into()), make_test_session("ev-test")).unwrap();
        let event = events.recv().await.unwrap();
        assert!(matches!(event, SessionEvent::Created { name } if name == "ev-test"));
        reg.remove("ev-test");
        let event = events.recv().await.unwrap();
        assert!(matches!(event, SessionEvent::Destroyed { name } if name == "ev-test"));
    }
}
```

**Step 2: Implement `SessionRegistry`**

Write the implementation to make all tests pass. Key details:
- `insert()` with `None` name: start from `self.next_id`, increment until a
  name that isn't in the map is found, then use it. Update `next_id` to
  `found + 1`.
- `insert()` updates `session.name` to the assigned name before inserting.
- `rename()` removes the session, updates its `name` field, and re-inserts.
- `insert()` and `remove()` send events on `events_tx` (ignore send errors —
  no receivers is fine).

**Step 3: Run tests**

Run: `nix develop -c sh -c "cargo test session::tests"`
Expected: PASS (all registry tests)

**Step 4: Commit**

```bash
git add src/session.rs
git commit -m "feat: add SessionRegistry for multi-session management"
```

---

### Task 3: Refactor `AppState` to hold `SessionRegistry`

Replace per-session fields in `AppState` with the registry.

**Files:**
- Modify: `src/api/mod.rs`
- Modify: `src/api/error.rs` (add `SessionNotFound` variant)

**Step 1: Add error variant**

Add to `ApiError` in `src/api/error.rs`:

```rust
#[error("session not found: {0}")]
SessionNotFound(String),
```

With: `status_code() => 404`, `code() => "session_not_found"`,
`message() => "Session not found: {name}"`.

Add a unit test for the new variant in the existing test module.

**Step 2: Refactor `AppState`**

Replace `AppState` in `src/api/mod.rs`:

```rust
use crate::session::SessionRegistry;

#[derive(Clone)]
pub struct AppState {
    pub sessions: SessionRegistry,
    pub shutdown: ShutdownCoordinator,
}
```

The `ShutdownCoordinator` here is server-level (for overall server shutdown).

**Step 3: Add a helper to extract a session from the path**

Add a helper function (or Axum extractor) in `src/api/mod.rs`:

```rust
use axum::extract::Path;

/// Extract a session from the registry by name path parameter.
pub(crate) fn get_session(
    sessions: &SessionRegistry,
    name: &str,
) -> Result<crate::session::Session, ApiError> {
    sessions.get(name).ok_or_else(|| ApiError::SessionNotFound(name.to_string()))
}
```

**Step 4: Run `cargo check` — expect errors**

The handlers still reference old `AppState` fields. This is expected; we fix
them in the next task.

**Step 5: Commit (WIP)**

```bash
git add src/api/mod.rs src/api/error.rs
git commit -m "refactor: replace AppState fields with SessionRegistry (WIP)"
```

---

### Task 4: Update all handlers to use session from registry

Every handler currently does `State(state): State<AppState>` and accesses
`state.input_tx`, `state.parser`, etc. They need to extract the session
name from the path and look it up in the registry.

**Files:**
- Modify: `src/api/mod.rs` (router — nest under `/sessions/:name`)
- Modify: `src/api/handlers.rs` (all handlers)
- Modify: `src/api/ws_methods.rs` (`dispatch()` signature)

**Step 1: Update the router**

Restructure `router()` in `src/api/mod.rs` to nest per-session routes:

```rust
pub fn router(state: AppState, token: Option<String>) -> Router {
    let session_routes = Router::new()
        .route("/input", post(input))
        .route("/input/mode", get(input_mode_get))
        .route("/input/capture", post(input_capture))
        .route("/input/release", post(input_release))
        .route("/quiesce", get(quiesce))
        .route("/ws/raw", get(ws_raw))
        .route("/ws/json", get(ws_json))
        .route("/screen", get(screen))
        .route("/scrollback", get(scrollback))
        .route("/overlay", get(overlay_list).post(overlay_create).delete(overlay_clear))
        .route("/overlay/:id", get(overlay_get).put(overlay_update).patch(overlay_patch).delete(overlay_delete))
        .route("/panel", get(panel_list).post(panel_create).delete(panel_clear))
        .route("/panel/:id", get(panel_get).put(panel_update).patch(panel_patch).delete(panel_delete));

    let server_routes = Router::new()
        .route("/sessions", get(session_list).post(session_create))
        .route("/sessions/:name", get(session_get).patch(session_rename).delete(session_kill))
        .route("/server/persist", post(server_persist))
        .route("/ws/json", get(ws_json_server))
        .nest("/sessions/:name", session_routes);

    let protected = server_routes.with_state(state);

    let protected = match token {
        Some(token) => protected.layer(axum::middleware::from_fn(move |req, next| {
            let t = token.clone();
            async move { auth::require_auth(t, req, next).await }
        })),
        None => protected,
    };

    Router::new()
        .route("/health", get(health))
        .route("/openapi.yaml", get(openapi_spec))
        .route("/docs", get(docs_index))
        .merge(protected)
}
```

**Step 2: Update handler signatures**

Every per-session handler changes from:

```rust
pub(super) async fn input(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    state.input_tx.send(body).await...
}
```

To:

```rust
pub(super) async fn input(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    session.input_tx.send(body).await...
}
```

Apply this pattern to ALL per-session handlers. The change is mechanical:
add `Path(name): Path<String>`, call `get_session()`, replace `state.X`
with `session.X`.

For WebSocket handlers (`ws_raw`, `ws_json`), the session name must be
captured before the upgrade and moved into the closure:

```rust
pub(super) async fn ws_json(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let session = get_session(&state.sessions, &name)?;
    Ok(ws.on_upgrade(move |socket| handle_ws_json(socket, session)))
}
```

Note: `handle_ws_json` and `handle_ws_raw` now take `Session` instead of
`AppState`. Update their signatures accordingly. They only use per-session
state, so this is a direct field-for-field replacement.

**Step 3: Update `dispatch()` in `ws_methods.rs`**

Change signature from:

```rust
pub async fn dispatch(req: &WsRequest, state: &AppState) -> WsResponse
```

To:

```rust
pub async fn dispatch(req: &WsRequest, session: &Session) -> WsResponse
```

All internal field accesses (`state.parser`, `state.overlays`, etc.) become
`session.parser`, `session.overlays`, etc. This is a mechanical find-replace.

**Step 4: Run `cargo check`**

Fix any remaining compilation errors.

**Step 5: Commit**

```bash
git add src/api/
git commit -m "refactor: scope all handlers under /sessions/:name"
```

---

### Task 5: Add server-level HTTP handlers

Implement the session management endpoints.

**Files:**
- Modify: `src/api/handlers.rs`

**Step 1: Define request/response types**

```rust
#[derive(Deserialize)]
pub(super) struct CreateSessionRequest {
    pub name: Option<String>,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub rows: Option<u16>,
    pub cols: Option<u16>,
}

#[derive(Serialize)]
pub(super) struct SessionInfo {
    pub name: String,
    pub created_at: String,  // or omit for now
}

#[derive(Deserialize)]
pub(super) struct RenameSessionRequest {
    pub name: String,
}
```

**Step 2: Implement handlers**

```rust
pub(super) async fn session_list(
    State(state): State<AppState>,
) -> Json<Vec<SessionInfo>> { ... }

pub(super) async fn session_create(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<SessionInfo>), ApiError> { ... }

pub(super) async fn session_get(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<SessionInfo>, ApiError> { ... }

pub(super) async fn session_rename(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<RenameSessionRequest>,
) -> Result<Json<SessionInfo>, ApiError> { ... }

pub(super) async fn session_kill(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> { ... }

pub(super) async fn server_persist(
    State(state): State<AppState>,
) -> StatusCode { ... }
```

`session_create` needs to:
1. Spawn a PTY with the given command/size (or defaults).
2. Create a Broker, Parser, channels, and all per-session state.
3. Spawn PTY reader/writer tasks (factored out of main.rs).
4. Insert the session into the registry.
5. Return the session info.

This means factoring the PTY setup + task spawning out of `main.rs` into a
reusable function. Add a `Session::spawn()` constructor in `src/session.rs`:

```rust
impl Session {
    /// Spawn a new session with a PTY and all associated tasks.
    pub fn spawn(
        name: String,
        command: SpawnCommand,
        rows: u16,
        cols: u16,
    ) -> Result<Self, pty::PtyError> {
        let mut pty = Pty::spawn(rows, cols, command)?;
        let pty_reader = pty.take_reader()?;
        let pty_writer = pty.take_writer()?;
        let pty = Arc::new(pty);

        let broker = Broker::new();
        let parser = Parser::spawn(&broker, cols as usize, rows as usize, 10_000);
        let (input_tx, input_rx) = mpsc::channel::<Bytes>(64);
        let shutdown = ShutdownCoordinator::new();
        let overlays = OverlayStore::new();
        let panels = PanelStore::new();
        let input_mode = InputMode::new();
        let input_broadcaster = InputBroadcaster::new();
        let activity = ActivityTracker::new();
        let terminal_size = TerminalSize::new(rows, cols);

        // Spawn PTY reader (no stdout — server mode, output goes to broker only)
        // Spawn PTY writer
        // Spawn child monitor
        // ...

        Ok(Session { name, input_tx, output_rx: broker.sender(), ... })
    }
}
```

Note: In server mode, the PTY reader does NOT write to stdout (there is no
local terminal). It only publishes to the broker. Factor the PTY reader to
accept an optional stdout writer. For now, in the server's `Session::spawn`,
the reader only calls `broker.publish()` and `activity.touch()`.

**Step 3: Write tests for session management endpoints**

Add integration tests (or unit tests in `src/api/mod.rs`) that:
- `POST /sessions` creates a session and returns its name
- `GET /sessions` lists sessions
- `GET /sessions/:name` returns session details
- `PATCH /sessions/:name` renames a session
- `DELETE /sessions/:name` kills a session
- Creating a duplicate name returns 409 or 400
- Getting/deleting a nonexistent session returns 404

**Step 4: Run tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: PASS

**Step 5: Commit**

```bash
git add src/session.rs src/api/
git commit -m "feat: add session management HTTP endpoints"
```

---

### Task 6: Update internal unit tests (`src/api/mod.rs` tests)

The `create_test_state()` in the internal test module constructs the old
`AppState`. Update it to create the new `AppState` with a `SessionRegistry`
containing one session.

**Files:**
- Modify: `src/api/mod.rs` (test module)

**Step 1: Update `create_test_state()`**

```rust
fn create_test_state() -> (AppState, mpsc::Receiver<Bytes>, String) {
    let registry = SessionRegistry::new();
    let (input_tx, input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    let session = Session {
        name: "test".to_string(),
        input_tx,
        output_rx: broker.sender(),
        // ... all other fields ...
    };
    registry.insert(Some("test".into()), session).unwrap();
    let state = AppState {
        sessions: registry,
        shutdown: ShutdownCoordinator::new(),
    };
    (state, input_rx, "test".to_string())
}
```

**Step 2: Update all test functions**

Every test that does `router(state, None)` and hits e.g. `/input` must now
hit `/sessions/test/input`. Update all request URIs.

**Step 3: Run tests**

Run: `nix develop -c sh -c "cargo test api::tests"`
Expected: PASS

**Step 4: Commit**

```bash
git add src/api/mod.rs
git commit -m "test: update api::mod internal tests for multi-session"
```

---

### Task 7: Update `ws_methods.rs` dispatch tests

**Files:**
- Modify: `src/api/ws_methods.rs` (test module)

**Step 1: Update `create_test_state()`**

The dispatch tests create `AppState` to pass to `dispatch()`. Since
`dispatch()` now takes `&Session`, update the test helper to create a
`Session` directly:

```rust
fn create_test_session() -> (Session, mpsc::Receiver<Bytes>) {
    let (input_tx, input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    let session = Session {
        name: "test".to_string(),
        input_tx,
        output_rx: broker.sender(),
        // ... all fields ...
    };
    (session, input_rx)
}
```

Update all `dispatch()` calls from `dispatch(&req, &state)` to
`dispatch(&req, &session)`.

**Step 2: Run tests**

Run: `nix develop -c sh -c "cargo test ws_methods::tests"`
Expected: PASS

**Step 3: Commit**

```bash
git add src/api/ws_methods.rs
git commit -m "test: update ws_methods dispatch tests for Session"
```

---

### Task 8: Update integration tests

All 15 integration test files in `tests/` construct `AppState` inline. Each
needs updating.

**Files:**
- Modify: All files in `tests/`

**Step 1: Update test helpers**

For each integration test file, update `create_test_app()` / `create_test_state()`:

- Create a `SessionRegistry`, insert a test session, build new `AppState`
- Update all request URIs from e.g. `/input` to `/sessions/test/input`
- Update WebSocket URLs similarly

The files to update (grouped by pattern):

**Tests that create AppState + router directly:**
- `tests/api_integration.rs`
- `tests/auth_integration.rs`
- `tests/ws_json_methods.rs`
- `tests/panel_integration.rs`
- `tests/overlay_integration.rs`
- `tests/input_capture_integration.rs`
- `tests/quiesce_integration.rs`

**Tests that use end-to-end patterns (may spawn actual wsh or use AppState):**
- `tests/parser_integration.rs`
- `tests/graceful_shutdown.rs`
- `tests/pty_integration.rs`
- `tests/interactive_shell.rs`
- `tests/e2e_input.rs`
- `tests/e2e_concurrent_input.rs`
- `tests/e2e_http.rs`
- `tests/e2e_websocket_input.rs`

For each file, the change is mechanical:
1. Import `SessionRegistry` and `Session`
2. Build a `Session` instead of `AppState` for the per-session fields
3. Build `AppState` with a `SessionRegistry` containing that session
4. Prefix all endpoint paths with `/sessions/test`

**Step 2: Run all tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: PASS

**Step 3: Commit**

```bash
git add tests/
git commit -m "test: update all integration tests for multi-session AppState"
```

---

### Task 9: Add server-level WebSocket (multiplexed)

Implement `GET /ws/json` at the server level for multiplexed session access.

**Files:**
- Modify: `src/api/handlers.rs` (add `ws_json_server`, `handle_ws_json_server`)
- Modify: `src/api/ws_methods.rs` (add server-level method types)

**Step 1: Define server-level WS request/response types**

In `ws_methods.rs`, add:

```rust
/// Server-level WebSocket request — includes optional session field.
#[derive(Debug, Deserialize)]
pub struct ServerWsRequest {
    pub id: Option<serde_json::Value>,
    pub method: String,
    pub session: Option<String>,
    pub params: Option<serde_json::Value>,
}
```

**Step 2: Implement `handle_ws_json_server`**

This handler:
- Receives `ServerWsRequest` messages
- For session management methods (`create_session`, `list_sessions`,
  `kill_session`, `rename_session`, `set_server_mode`): handle directly
  using the registry
- For per-session methods (those with a `session` field): look up the session
  in the registry and delegate to the existing `dispatch()`
- For subscriptions with a `session` field: manage per-session subscriptions
  (subscribe to that session's broker/parser)
- Broadcast `SessionEvent`s to all connected server-level WebSocket clients

**Step 3: Write integration tests**

Create `tests/ws_server_integration.rs`:

- Connect to server-level `/ws/json`
- Create a session via `create_session` method
- List sessions via `list_sessions`
- Send input to the session via `send_input` with `session` field
- Receive `session_created` event
- Kill session, receive `session_destroyed` event
- Rename session via `rename_session`

**Step 4: Run tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: PASS

**Step 5: Commit**

```bash
git add src/api/ tests/ws_server_integration.rs
git commit -m "feat: add server-level multiplexed WebSocket"
```

---

### Task 10: Refactor `main.rs` for in-process server

Update `main.rs` to create a `SessionRegistry`, insert the initial session,
and use the new `AppState`.

**Files:**
- Modify: `src/main.rs`

**Step 1: Refactor main()**

Key changes:
- Create `SessionRegistry`
- Use `Session::spawn()` (or build one manually) for the initial session
- Build new `AppState` with registry
- Keep stdin reader, SIGWINCH handler, and shutdown logic
- PTY reader/writer are now started by `Session::spawn()`

The stdin reader stays in `main.rs` because it's specific to the CLI
(in-process mode). It sends input to the session's `input_tx`.

The SIGWINCH handler needs to resize the current session's PTY/parser.

**Step 2: Verify the binary still works**

Run: `nix develop -c sh -c "cargo build"`
Then manually test: `nix develop -c sh -c "./target/debug/wsh"`

**Step 3: Run all tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: PASS

**Step 4: Commit**

```bash
git add src/main.rs src/session.rs
git commit -m "refactor: main.rs uses SessionRegistry with initial session"
```

---

### Task 11: Phase 1 integration tests

Write integration tests validating the full multi-session API flow.

**Files:**
- Create: `tests/session_management.rs`

**Step 1: Write tests**

```rust
// Test: create multiple sessions via HTTP, list them, verify isolation
// Test: send input to one session, verify other session is unaffected
// Test: rename a session, verify accessible by new name, old name 404s
// Test: delete a session, verify it's gone from the list
// Test: creating a session with duplicate name returns error
// Test: session auto-naming produces sequential integers
// Test: per-session endpoints return 404 for nonexistent session
```

**Step 2: Run tests**

Run: `nix develop -c sh -c "cargo test session_management"`
Expected: PASS

**Step 3: Commit**

```bash
git add tests/session_management.rs
git commit -m "test: add multi-session integration tests"
```

---

## Phase 2: Client/Server Process Split

Phase 2 splits `wsh` into a daemon server and a thin CLI client
communicating over a Unix domain socket.

### Task 12: Unix socket protocol types

Define the frame types and serialization for the Unix socket protocol.

**Files:**
- Create: `src/protocol.rs`
- Modify: `src/lib.rs` (add `pub mod protocol;`)

**Step 1: Define frame types**

```rust
/// Frame type byte values.
#[repr(u8)]
pub enum FrameType {
    // Control frames (JSON payload)
    CreateSession = 0x01,
    CreateSessionResponse = 0x02,
    AttachSession = 0x03,
    AttachSessionResponse = 0x04,
    Detach = 0x05,
    Resize = 0x06,
    Error = 0x07,

    // Data frames (raw bytes payload)
    PtyOutput = 0x10,
    StdinInput = 0x11,
}

/// Wire format: [type: u8][length: u32 big-endian][payload: bytes]
pub struct Frame {
    pub frame_type: FrameType,
    pub payload: Bytes,
}
```

**Step 2: Implement encode/decode**

```rust
impl Frame {
    pub async fn write_to<W: AsyncWriteExt + Unpin>(
        &self, writer: &mut W,
    ) -> io::Result<()> { ... }

    pub async fn read_from<R: AsyncReadExt + Unpin>(
        reader: &mut R,
    ) -> io::Result<Self> { ... }
}
```

**Step 3: Define control message types**

```rust
#[derive(Serialize, Deserialize)]
pub struct CreateSessionMsg {
    pub name: Option<String>,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Serialize, Deserialize)]
pub struct CreateSessionResponseMsg {
    pub name: String,
}

#[derive(Serialize, Deserialize)]
pub struct AttachSessionMsg {
    pub name: String,
    pub scrollback: ScrollbackRequest,
}

#[derive(Serialize, Deserialize)]
pub enum ScrollbackRequest {
    None,
    Lines(usize),
    All,
}

#[derive(Serialize, Deserialize)]
pub struct AttachSessionResponseMsg {
    pub name: String,
    pub rows: u16,
    pub cols: u16,
    pub scrollback: Vec<u8>,  // raw terminal bytes to replay
    pub screen: Vec<u8>,      // current screen state as terminal bytes
}

#[derive(Serialize, Deserialize)]
pub struct ResizeMsg {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Serialize, Deserialize)]
pub struct ErrorMsg {
    pub code: String,
    pub message: String,
}
```

**Step 4: Write tests**

Test round-trip encode/decode for each frame type. Test boundary conditions
(empty payload, large payload, invalid frame type).

**Step 5: Run tests**

Run: `nix develop -c sh -c "cargo test protocol::tests"`
Expected: PASS

**Step 6: Commit**

```bash
git add src/protocol.rs src/lib.rs
git commit -m "feat: add Unix socket protocol frame types"
```

---

### Task 13: Unix socket server listener

Add a Unix socket listener to the server that handles CLI client connections.

**Files:**
- Create: `src/server.rs`
- Modify: `src/lib.rs` (add `pub mod server;`)

**Step 1: Implement the listener**

```rust
use tokio::net::UnixListener;

pub struct UnixSocketServer {
    sessions: SessionRegistry,
    listener_path: PathBuf,
}

impl UnixSocketServer {
    pub async fn start(
        sessions: SessionRegistry,
        path: PathBuf,
    ) -> io::Result<()> { ... }

    async fn handle_client(
        stream: UnixStream,
        sessions: SessionRegistry,
    ) { ... }
}
```

`handle_client`:
1. Read the first control frame (CreateSession or AttachSession).
2. For CreateSession: spawn a new session, insert into registry, send
   response with name.
3. For AttachSession: look up session, send response with scrollback/screen
   data.
4. Enter streaming loop: forward StdinInput frames to session's `input_tx`,
   forward session output to PtyOutput frames.
5. Handle Resize and Detach control frames inline.
6. On disconnect: clean up.

**Step 2: Write tests**

Test with `UnixStream::connect()`:
- Connect, send CreateSession, verify response
- Connect, send AttachSession, verify response
- Send StdinInput, verify it reaches session's input_tx
- Verify PtyOutput frames arrive when broker publishes

**Step 3: Run tests**

Run: `nix develop -c sh -c "cargo test server::tests"`
Expected: PASS

**Step 4: Commit**

```bash
git add src/server.rs src/lib.rs
git commit -m "feat: add Unix socket server for CLI clients"
```

---

### Task 14: Server daemon mode (`wsh server`)

Add the `wsh server` subcommand that starts the server as a foreground
process with both HTTP and Unix socket listeners.

**Files:**
- Modify: `src/main.rs`

**Step 1: Add CLI subcommands**

Restructure CLI args using clap subcommands:

```rust
#[derive(ClapParser)]
#[command(name = "wsh")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    // Default (no subcommand) args:
    #[arg(long, default_value = "127.0.0.1:8080")]
    bind: SocketAddr,
    #[arg(short = 'c')]
    command_str: Option<String>,
    #[arg(short = 'i')]
    interactive: bool,
    #[arg(long, env = "WSH_TOKEN")]
    token: Option<String>,
    #[arg(long)]
    shell: Option<String>,
    #[arg(long)]
    name: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the server daemon
    Server {
        #[arg(long, default_value = "127.0.0.1:8080")]
        bind: SocketAddr,
        #[arg(long, env = "WSH_TOKEN")]
        token: Option<String>,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Attach to an existing session
    Attach {
        name: String,
        #[arg(long, default_value = "all")]
        scrollback: String,  // "all", "0", or a number
    },
    /// List active sessions
    List,
    /// Kill a session
    Kill { name: String },
}
```

**Step 2: Implement `wsh server`**

When `Commands::Server` is matched:
1. Create `SessionRegistry`
2. Build `AppState`, create Axum router
3. Start HTTP listener
4. Start Unix socket listener
5. Wait for shutdown signal (Ctrl+C)
6. No PTY, no stdin reader, no raw mode

**Step 3: Test manually**

Run: `nix develop -c sh -c "cargo run -- server"`
Verify: server starts, listens on HTTP and Unix socket, no PTY spawned.

**Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: add 'wsh server' subcommand for daemon mode"
```

---

### Task 15: Thin CLI client mode

When `wsh` is run without a subcommand (the default), it acts as a thin
client: discovers/starts the server, creates a session, and proxies I/O.

**Files:**
- Create: `src/client.rs`
- Modify: `src/lib.rs` (add `pub mod client;`)
- Modify: `src/main.rs`

**Step 1: Implement the client**

```rust
pub struct Client {
    stream: UnixStream,
}

impl Client {
    /// Connect to an existing server.
    pub async fn connect(socket_path: &Path) -> io::Result<Self> { ... }

    /// Create a new session on the server.
    pub async fn create_session(&mut self, msg: CreateSessionMsg) -> Result<String, ...> { ... }

    /// Attach to an existing session.
    pub async fn attach(&mut self, msg: AttachSessionMsg) -> Result<AttachSessionResponseMsg, ...> { ... }

    /// Enter streaming mode: proxy stdin/stdout.
    pub async fn run_streaming(
        self,
        // stdin/stdout handles, resize signal, etc.
    ) -> Result<(), ...> { ... }
}
```

**Step 2: Implement server discovery and auto-start**

In `main.rs`, when no subcommand:
1. Compute socket path
2. Try `Client::connect()`
3. If connection fails: spawn server as background process
   (`std::process::Command::new(current_exe).arg("server").spawn()`)
4. Retry connection with backoff
5. Send CreateSession
6. Enter raw mode
7. Call `client.run_streaming()`
8. On session exit or detach: restore terminal

**Step 3: Implement `wsh attach`, `wsh list`, `wsh kill`**

- `wsh attach <name>`: Connect to server, send AttachSession, enter
  streaming mode
- `wsh list`: Connect to server's HTTP API, `GET /sessions`, print table
- `wsh kill <name>`: Connect to server's HTTP API, `DELETE /sessions/:name`

**Step 4: Test manually**

- Run `wsh server` in one terminal
- Run `wsh --name test1` in another — should connect and create session
- Run `wsh attach test1` in a third — should attach to same session
- Run `wsh list` — should show `test1`
- Run `wsh kill test1` — should kill the session

**Step 5: Commit**

```bash
git add src/client.rs src/lib.rs src/main.rs
git commit -m "feat: add thin CLI client with server auto-start"
```

---

### Task 16: Server shutdown behavior (ephemeral vs persistent)

Implement the shutdown mode logic.

**Files:**
- Modify: `src/session.rs` or `src/server.rs`
- Modify: `src/api/handlers.rs` (`server_persist` handler)

**Step 1: Add shutdown mode to server state**

```rust
use std::sync::atomic::{AtomicBool, Ordering};

pub struct ServerConfig {
    pub persistent: AtomicBool,
}
```

Add `ServerConfig` to `AppState`.

**Step 2: Wire up session removal callback**

When a session is removed from the registry (either via exit or kill):
- If `!persistent` and `registry.len() == 0`: signal server shutdown.

**Step 3: Implement `server_persist` handler**

The HTTP handler and WS method both set `persistent = true`.

**Step 4: Write tests**

- Test: ephemeral server shuts down when last session removed
- Test: persistent server stays alive when last session removed
- Test: upgrading from ephemeral to persistent works

**Step 5: Run tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: PASS

**Step 6: Commit**

```bash
git add src/
git commit -m "feat: add ephemeral/persistent server shutdown modes"
```

---

### Task 17: `wsh server persist` CLI command

**Files:**
- Modify: `src/main.rs`

**Step 1: Add subcommand**

Add `Persist` as a subcommand under `Server` (or as `ServerPersist`):

```rust
/// Upgrade an implicit server to persistent mode
ServerPersist {
    #[arg(long)]
    socket: Option<PathBuf>,
},
```

**Step 2: Implement**

Connect to server's HTTP API, `POST /server/persist`.

**Step 3: Test manually**

- Start `wsh` (implicit server start)
- Run `wsh server persist`
- Exit all sessions — server should stay alive

**Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: add 'wsh server persist' CLI command"
```

---

### Task 18: Scrollback on attach

Implement scrollback replay when a CLI client attaches to a session.

**Files:**
- Modify: `src/server.rs` (handle_client attach logic)
- Modify: `src/client.rs` (replay logic with synchronized output)

**Step 1: Server side**

When handling an AttachSession request with scrollback:
- Query the session's parser for scrollback lines (all, N lines, or none)
- Query current screen state
- Encode as raw terminal bytes in the response

**Step 2: Client side**

When receiving the attach response:
1. Write `\x1b[?2026h` to stdout (begin synchronized update)
2. Write scrollback bytes
3. Write screen state bytes
4. Write `\x1b[?2026l` to stdout (end synchronized update)

**Step 3: Write tests**

- Attach with `--scrollback 0`: verify only current screen received
- Attach with `--scrollback all`: verify scrollback included
- Attach with `--scrollback N`: verify N lines of scrollback

**Step 4: Run tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: PASS

**Step 5: Commit**

```bash
git add src/server.rs src/client.rs
git commit -m "feat: scrollback replay on session attach"
```

---

### Task 19: Phase 2 end-to-end tests

Write comprehensive end-to-end tests for the full client/server flow.

**Files:**
- Create: `tests/server_client_e2e.rs`

**Step 1: Write tests**

```rust
// Test: start server, create session via client, send input, verify output
// Test: start server, create two sessions, verify isolation
// Test: implicit server start (wsh with no server running)
// Test: attach to session, verify output streaming
// Test: detach and reattach
// Test: multiple clients attached to same session
// Test: session exit cleans up properly
// Test: ephemeral server shuts down after last session exits
// Test: persistent server stays alive after last session exits
// Test: server persist upgrade
// Test: scrollback replay on attach
```

**Step 2: Run tests**

Run: `nix develop -c sh -c "cargo test server_client_e2e"`
Expected: PASS

**Step 3: Commit**

```bash
git add tests/server_client_e2e.rs
git commit -m "test: add server/client end-to-end tests"
```

---

### Task 20: Update documentation

Update API docs and OpenAPI spec for the new multi-session endpoints.

**Files:**
- Modify: `docs/api/openapi.yaml`
- Modify: `docs/api/README.md`
- Modify: any other doc files referencing the API

**Step 1: Update OpenAPI spec**

Add session management endpoints, update all existing endpoints to include
`/sessions/:name` prefix, add server-level WebSocket docs.

**Step 2: Update README**

Document the new session management API, server-level WebSocket, and CLI
subcommands.

**Step 3: Commit**

```bash
git add docs/
git commit -m "docs: update API documentation for multi-session server mode"
```
