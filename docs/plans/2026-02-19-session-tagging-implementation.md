# Session Tagging Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add tags to sessions — simple string labels for grouping — with CRUD operations, filtered listing, and group-scoped quiescence across all API surfaces (HTTP, WebSocket, MCP, socket protocol, CLI).

**Architecture:** Tags live on the Session struct as `Arc<RwLock<HashSet<String>>>`. The SessionRegistry maintains a reverse index (`HashMap<String, HashSet<String>>` — tag → session names) for O(1) lookups. All mutation paths (insert, remove, rename, tag add/remove) update the index atomically under the registry write lock.

**Tech Stack:** Rust, axum 0.7, serde, clap, rmcp (MCP), parking_lot RwLock

**Design doc:** `docs/plans/2026-02-19-session-tagging-design.md`

---

### Task 1: Add Tags to Session Struct and Registry Data Model

Add the `tags` field to Session, update `spawn_with_options` to initialize it, add `tags_index` to `RegistryInner`, and add tag validation.

**Files:**
- Modify: `src/session.rs:24-66` (Session struct), `src/session.rs:438-460` (spawn_with_options Session construction), `src/session.rs:535-540` (RegistryInner)

**Step 1: Write failing test — tag validation**

Add to the `#[cfg(test)]` module in `src/session.rs` (find it near the bottom):

```rust
#[test]
fn validate_tag_accepts_valid() {
    assert!(super::validate_tag("build").is_ok());
    assert!(super::validate_tag("my-tag_1.0").is_ok());
    assert!(super::validate_tag("a").is_ok());
}

#[test]
fn validate_tag_rejects_invalid() {
    assert!(super::validate_tag("").is_err());
    assert!(super::validate_tag(" spaces ").is_err());
    assert!(super::validate_tag("has space").is_err());
    assert!(super::validate_tag(&"x".repeat(65)).is_err());
    assert!(super::validate_tag("special!char").is_err());
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop -c sh -c "cargo test --lib session::tests::validate_tag -- --nocapture"`
Expected: FAIL — `validate_tag` function doesn't exist

**Step 3: Implement tag validation and Session struct changes**

In `src/session.rs`, add near the top (after imports):

```rust
use std::collections::HashSet;
```

Add the validation function (before the Session struct):

```rust
/// Validate a tag string. Tags must be 1-64 chars, alphanumeric/hyphens/underscores/dots.
pub fn validate_tag(tag: &str) -> Result<(), String> {
    if tag.is_empty() {
        return Err("tag must not be empty".to_string());
    }
    if tag.len() > 64 {
        return Err(format!("tag too long ({} chars, max 64)", tag.len()));
    }
    if !tag.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.') {
        return Err(format!("tag contains invalid characters: {tag}"));
    }
    Ok(())
}
```

Add to Session struct (after `client_count` field, line ~33):

```rust
/// User-defined tags for organizing and filtering sessions.
pub tags: Arc<RwLock<HashSet<String>>>,
```

Update Session construction in `spawn_with_options` (line ~438, add after `child_exited`):

```rust
tags: Arc::new(RwLock::new(HashSet::new())),
```

Add `tags_index` to `RegistryInner` (line ~537):

```rust
struct RegistryInner {
    sessions: HashMap<String, Session>,
    tags_index: HashMap<String, HashSet<String>>,
    next_id: u64,
    max_sessions: Option<usize>,
}
```

Update `SessionRegistry::new()` to initialize `tags_index: HashMap::new()`.

**Step 4: Run test to verify it passes**

Run: `nix develop -c sh -c "cargo test --lib session::tests::validate_tag -- --nocapture"`
Expected: PASS

**Step 5: Fix all test helpers that construct Session inline**

The following files construct Session structs directly and need `tags: Arc::new(parking_lot::RwLock::new(HashSet::new()))`:

- `src/api/ws_methods.rs:1174` — `create_test_session()` in the `#[cfg(test)]` module
- `tests/common/mod.rs:37` — `create_test_session_with_size()`

Also update `RegistryInner` construction in registry tests if any exist.

**Step 6: Verify full build**

Run: `nix develop -c sh -c "cargo test"`
Expected: All existing tests pass

**Step 7: Commit**

```
feat: add tags field to Session and tag validation

Adds the `tags: Arc<RwLock<HashSet<String>>>` field to Session,
`tags_index` reverse index to RegistryInner, and `validate_tag()`
function for tag string validation.
```

---

### Task 2: Registry Tag Operations and Index Maintenance

Add `add_tags()`, `remove_tags()`, `set_tags()`, `sessions_by_tags()` methods to SessionRegistry. Update `insert()`, `remove()`, and `rename()` to maintain the index. Add `SessionEvent::TagsChanged`.

**Files:**
- Modify: `src/session.rs:518-523` (SessionEvent enum), `src/session.rs:589-739` (SessionRegistry methods)

**Step 1: Write failing tests for tag index operations**

Add to the registry test module in `src/session.rs`:

```rust
#[test]
fn registry_add_tags() {
    let registry = SessionRegistry::new();
    let (session, _rx) = Session::spawn("test".into(), SpawnCommand::default(), 24, 80).unwrap();
    registry.insert(Some("s1".into()), session).unwrap();

    registry.add_tags("s1", &["build".into(), "test".into()]).unwrap();
    let s = registry.get("s1").unwrap();
    let tags: Vec<String> = {
        let t = s.tags.read();
        let mut v: Vec<_> = t.iter().cloned().collect();
        v.sort();
        v
    };
    assert_eq!(tags, vec!["build", "test"]);
}

#[test]
fn registry_sessions_by_tags_union() {
    let registry = SessionRegistry::new();
    let (s1, _) = Session::spawn("".into(), SpawnCommand::default(), 24, 80).unwrap();
    let (s2, _) = Session::spawn("".into(), SpawnCommand::default(), 24, 80).unwrap();
    let (s3, _) = Session::spawn("".into(), SpawnCommand::default(), 24, 80).unwrap();
    registry.insert(Some("s1".into()), s1).unwrap();
    registry.insert(Some("s2".into()), s2).unwrap();
    registry.insert(Some("s3".into()), s3).unwrap();

    registry.add_tags("s1", &["build".into()]).unwrap();
    registry.add_tags("s2", &["test".into()]).unwrap();
    registry.add_tags("s3", &["build".into(), "test".into()]).unwrap();

    let mut result = registry.sessions_by_tags(&["build".into()]);
    result.sort();
    assert_eq!(result, vec!["s1", "s3"]);

    let mut result = registry.sessions_by_tags(&["build".into(), "test".into()]);
    result.sort();
    assert_eq!(result, vec!["s1", "s2", "s3"]);
}

#[test]
fn registry_remove_cleans_index() {
    let registry = SessionRegistry::new();
    let (s, _) = Session::spawn("".into(), SpawnCommand::default(), 24, 80).unwrap();
    registry.insert(Some("s1".into()), s).unwrap();
    registry.add_tags("s1", &["build".into()]).unwrap();
    registry.remove("s1");
    assert!(registry.sessions_by_tags(&["build".into()]).is_empty());
}

#[test]
fn registry_rename_updates_index() {
    let registry = SessionRegistry::new();
    let (s, _) = Session::spawn("".into(), SpawnCommand::default(), 24, 80).unwrap();
    registry.insert(Some("s1".into()), s).unwrap();
    registry.add_tags("s1", &["build".into()]).unwrap();
    registry.rename("s1", "s2").unwrap();
    assert!(registry.sessions_by_tags(&["build".into()]).contains(&"s2".to_string()));
    assert!(!registry.sessions_by_tags(&["build".into()]).contains(&"s1".to_string()));
}

#[test]
fn registry_remove_tags() {
    let registry = SessionRegistry::new();
    let (s, _) = Session::spawn("".into(), SpawnCommand::default(), 24, 80).unwrap();
    registry.insert(Some("s1".into()), s).unwrap();
    registry.add_tags("s1", &["build".into(), "test".into()]).unwrap();
    registry.remove_tags("s1", &["build".into()]).unwrap();
    let s = registry.get("s1").unwrap();
    let tags: Vec<String> = s.tags.read().iter().cloned().collect();
    assert_eq!(tags, vec!["test"]);
    assert!(registry.sessions_by_tags(&["build".into()]).is_empty());
}

#[test]
fn registry_insert_with_initial_tags() {
    let registry = SessionRegistry::new();
    let (mut s, _) = Session::spawn("".into(), SpawnCommand::default(), 24, 80).unwrap();
    *s.tags.write() = HashSet::from(["build".into(), "ci".into()]);
    registry.insert(Some("s1".into()), s).unwrap();
    let mut result = registry.sessions_by_tags(&["build".into()]);
    result.sort();
    assert_eq!(result, vec!["s1"]);
}
```

**Step 2: Run tests to verify they fail**

Run: `nix develop -c sh -c "cargo test --lib session::tests::registry_add_tags -- --nocapture"`
Expected: FAIL — methods don't exist

**Step 3: Implement registry methods**

Add `TagsChanged` to `SessionEvent`:

```rust
#[derive(Debug, Clone)]
pub enum SessionEvent {
    Created { name: String },
    Renamed { old_name: String, new_name: String },
    Destroyed { name: String },
    TagsChanged { name: String, added: Vec<String>, removed: Vec<String> },
}
```

Add methods to `SessionRegistry`:

```rust
/// Add tags to a session. Validates tags, updates Session and index atomically.
pub fn add_tags(&self, name: &str, tags: &[String]) -> Result<(), RegistryError> {
    for tag in tags {
        validate_tag(tag).map_err(|e| RegistryError::NotFound(e))?; // reuse or add InvalidTag variant
    }
    let mut inner = self.inner.write();
    let session = inner.sessions.get(name)
        .ok_or_else(|| RegistryError::NotFound(name.to_string()))?;
    let mut session_tags = session.tags.write();
    let mut added = Vec::new();
    for tag in tags {
        if session_tags.insert(tag.clone()) {
            inner.tags_index.entry(tag.clone()).or_default().insert(name.to_string());
            added.push(tag.clone());
        }
    }
    drop(session_tags);
    drop(inner);
    if !added.is_empty() {
        let _ = self.events_tx.send(SessionEvent::TagsChanged {
            name: name.to_string(), added, removed: vec![],
        });
    }
    Ok(())
}

/// Remove tags from a session.
pub fn remove_tags(&self, name: &str, tags: &[String]) -> Result<(), RegistryError> {
    let mut inner = self.inner.write();
    let session = inner.sessions.get(name)
        .ok_or_else(|| RegistryError::NotFound(name.to_string()))?;
    let mut session_tags = session.tags.write();
    let mut removed = Vec::new();
    for tag in tags {
        if session_tags.remove(tag) {
            if let Some(set) = inner.tags_index.get_mut(tag) {
                set.remove(name);
                if set.is_empty() {
                    inner.tags_index.remove(tag);
                }
            }
            removed.push(tag.clone());
        }
    }
    drop(session_tags);
    drop(inner);
    if !removed.is_empty() {
        let _ = self.events_tx.send(SessionEvent::TagsChanged {
            name: name.to_string(), added: vec![], removed,
        });
    }
    Ok(())
}

/// Return session names matching ANY of the given tags (union).
pub fn sessions_by_tags(&self, tags: &[String]) -> Vec<String> {
    let inner = self.inner.read();
    let mut result = HashSet::new();
    for tag in tags {
        if let Some(names) = inner.tags_index.get(tag) {
            result.extend(names.iter().cloned());
        }
    }
    result.into_iter().collect()
}
```

Update `insert()` to index initial tags from the session's tags field.

Update `remove()` to clean up the index (iterate session's tags, remove name from each tag's entry).

Update `rename()` to update the name in each tag's index entry.

**Step 4: Run tests to verify they pass**

Run: `nix develop -c sh -c "cargo test --lib session::tests::registry_ -- --nocapture"`
Expected: All new registry tag tests PASS

**Step 5: Run full test suite**

Run: `nix develop -c sh -c "cargo test"`
Expected: All tests pass

**Step 6: Commit**

```
feat: add tag CRUD and index maintenance to SessionRegistry

Implements add_tags(), remove_tags(), sessions_by_tags() with reverse
index maintenance across insert/remove/rename. Adds TagsChanged event.
```

---

### Task 3: HTTP API — Tags in SessionInfo, Create, List, PATCH

Add tags to `SessionInfo` and `build_session_info`, add `tags` field to `CreateSessionRequest`, add tag filter to `session_list`, add tag modification to `PATCH /sessions/:name`.

**Files:**
- Modify: `src/api/handlers.rs:2285-2314` (request/response types, build_session_info)
- Modify: `src/api/handlers.rs:2323-2335` (session_list)
- Modify: `src/api/handlers.rs:2337-2398` (session_create)
- Modify: `src/api/handlers.rs:2316-2320` (RenameSessionRequest → UpdateSessionRequest)
- Modify: `src/api/handlers.rs:2408-2420` (session_rename → session_update)
- Modify: `src/api/mod.rs:134-140` (routes)

**Step 1: Write integration test for HTTP tag operations**

Create a new test or add to existing integration test file. Since these are HTTP tests that need a running server, this may be best placed in the handler module's `#[cfg(test)]` section which already has tests with AppState construction.

Test cases to write:
1. `session_create` with tags — verify tags appear in response
2. `session_list` with `?tag=X` filter
3. `PATCH /sessions/:name` with `add_tags` / `remove_tags`
4. `session_get` returns tags

**Step 2: Run test to verify it fails**

Expected: FAIL — tags field not in types

**Step 3: Implement HTTP changes**

Add `tags` to `CreateSessionRequest`:
```rust
#[derive(Deserialize)]
pub(super) struct CreateSessionRequest {
    pub name: Option<String>,
    pub command: Option<String>,
    pub rows: Option<u16>,
    pub cols: Option<u16>,
    pub cwd: Option<String>,
    pub env: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    pub tags: Vec<String>,
}
```

Add `tags` to `SessionInfo`:
```rust
#[derive(Serialize)]
pub(super) struct SessionInfo {
    pub name: String,
    pub pid: Option<u32>,
    pub command: String,
    pub rows: u16,
    pub cols: u16,
    pub clients: usize,
    pub tags: Vec<String>,
}
```

Update `build_session_info`:
```rust
fn build_session_info(session: &crate::session::Session) -> SessionInfo {
    let (rows, cols) = session.terminal_size.get();
    let mut tags: Vec<String> = session.tags.read().iter().cloned().collect();
    tags.sort();
    SessionInfo {
        name: session.name.clone(),
        pid: session.pid,
        command: session.command.clone(),
        rows,
        cols,
        clients: session.clients(),
        tags,
    }
}
```

Update `session_create` to set tags on session after spawn and before registry insert:
```rust
// After spawning, set initial tags
if !req.tags.is_empty() {
    for tag in &req.tags {
        validate_tag(tag).map_err(|e| ApiError::InvalidTag(e))?;
    }
    *session.tags.write() = req.tags.into_iter().collect();
}
```

Add query struct for `session_list`:
```rust
#[derive(Deserialize)]
pub(super) struct ListSessionsQuery {
    #[serde(default)]
    pub tag: Vec<String>,
}
```

Update `session_list` to accept query params and filter:
```rust
pub(super) async fn session_list(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<ListSessionsQuery>,
) -> Json<Vec<SessionInfo>> {
    let names = if params.tag.is_empty() {
        state.sessions.list()
    } else {
        state.sessions.sessions_by_tags(&params.tag)
    };
    // ... rest unchanged
}
```

Rename `RenameSessionRequest` to `UpdateSessionRequest` and extend:
```rust
#[derive(Deserialize)]
pub(super) struct UpdateSessionRequest {
    pub name: Option<String>,
    #[serde(default)]
    pub add_tags: Vec<String>,
    #[serde(default)]
    pub remove_tags: Vec<String>,
}
```

Update the PATCH handler (`session_rename` → `session_update`) to handle both rename and tag operations.

Add `InvalidTag` variant to `ApiError` in `src/api/error.rs`.

**Step 4: Run tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: PASS

**Step 5: Commit**

```
feat: add tags to HTTP API — create, list filter, PATCH update
```

---

### Task 4: WebSocket Server-Level Methods — Tags Support

Update the server-level WS handler's `create_session` and `list_sessions` methods to support tags. Add tag management methods.

**Files:**
- Modify: `src/api/handlers.rs:1108-1268` (handle_ws_json server-level dispatch)

**Step 1: Write test for WS tag operations**

Test in the handlers module's test section. Construct an AppState, call the WS dispatch function directly, verify:
1. `create_session` with tags param → session has tags
2. `list_sessions` with tag filter → only matching sessions
3. New `update_tags` method works

**Step 2: Run test to verify it fails**

Expected: FAIL

**Step 3: Implement WS changes**

Update `CreateParams` in the `"create_session"` match arm (line ~1115):
```rust
#[derive(Deserialize)]
struct CreateParams {
    name: Option<String>,
    command: Option<String>,
    rows: Option<u16>,
    cols: Option<u16>,
    cwd: Option<String>,
    env: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    tags: Vec<String>,
}
```

After `insert_and_get` succeeds, set tags if provided:
```rust
if !params_tags.is_empty() {
    let _ = state.sessions.add_tags(&assigned_name, &params_tags);
}
```

And include tags in the response JSON.

Update `"list_sessions"` to accept optional tag filter:
```rust
#[derive(Deserialize)]
struct ListParams {
    #[serde(default)]
    tag: Vec<String>,
}
```

Use `sessions_by_tags` when tag filter provided, `list()` otherwise. Include `tags` in each session's JSON.

Add `"update_tags"` method:
```rust
"update_tags" => {
    #[derive(Deserialize)]
    struct UpdateTagsParams {
        session: String,
        #[serde(default)]
        add: Vec<String>,
        #[serde(default)]
        remove: Vec<String>,
    }
    // parse params, call add_tags/remove_tags, return updated session info
}
```

**Step 4: Run tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: PASS

**Step 5: Commit**

```
feat: add tags to WebSocket server-level methods
```

---

### Task 5: MCP Tools — Tags Support

Update MCP tool params and implementations to support tags.

**Files:**
- Modify: `src/mcp/tools.rs:8-52` (param types)
- Modify: `src/mcp/mod.rs:130-270` (tool implementations)

**Step 1: Update param types**

In `src/mcp/tools.rs`:

Add `tags` to `CreateSessionParams`:
```rust
#[serde(default)]
pub tags: Vec<String>,
```

Add `tag` filter to `ListSessionsParams`:
```rust
#[serde(default)]
pub tag: Vec<String>,
```

Add `AddTags` and `RemoveTags` to `ManageAction`:
```rust
pub enum ManageAction {
    Kill,
    Rename,
    Detach,
    AddTags,
    RemoveTags,
}
```

Add `tags` field to `ManageSessionParams`:
```rust
pub tags: Option<Vec<String>>,
```

**Step 2: Update MCP tool implementations**

In `src/mcp/mod.rs`:

Update `wsh_create_session` to pass tags to session after spawn:
```rust
if !params.tags.is_empty() {
    let _ = self.state.sessions.add_tags(&assigned_name, &params.tags);
}
```

Include `tags` in the response JSON.

Update `wsh_list_sessions` to filter by tags:
```rust
let names = if params.tag.is_empty() {
    self.state.sessions.list()
} else {
    self.state.sessions.sessions_by_tags(&params.tag)
};
```

Include `tags` in session JSON output.

Update `wsh_manage_session` to handle `AddTags` and `RemoveTags` actions.

**Step 3: Run tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: PASS

**Step 4: Commit**

```
feat: add tags to MCP tools — create, list filter, manage
```

---

### Task 6: Socket Protocol — Tags in Messages

Update protocol message types and server socket handlers to include tags.

**Files:**
- Modify: `src/protocol.rs:220-237` (CreateSessionMsg, CreateSessionResponseMsg)
- Modify: `src/protocol.rs:306-313` (SessionInfoMsg)
- Modify: `src/server.rs:182-244` (handle_create_session)
- Modify: `src/server.rs:360-386` (handle_list_sessions)

**Step 1: Update protocol types**

In `src/protocol.rs`:

Add `tags` to `CreateSessionMsg`:
```rust
#[serde(default)]
pub tags: Vec<String>,
```

Add `tags` to `SessionInfoMsg`:
```rust
#[serde(default)]
pub tags: Vec<String>,
```

**Step 2: Update socket handlers**

In `src/server.rs`:

`handle_create_session`: After registry insert succeeds, add tags if provided. Include tags in response.

`handle_list_sessions`: Include `tags` field in each `SessionInfoMsg` (read from `session.tags`).

**Step 3: Fix socket protocol tests**

Update `CreateSessionMsg` construction in server tests (lines ~823, ~923, ~966, ~1001, ~1246, ~1373) to include `tags: vec![]`.

**Step 4: Run tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: PASS

**Step 5: Commit**

```
feat: add tags to socket protocol messages
```

---

### Task 7: Group-Scoped Quiescence

Add `tag` filter to `quiesce_any` handler so `GET /quiesce?tag=X` waits for any tagged session.

**Files:**
- Modify: `src/api/handlers.rs:1649-1669` (QuiesceAnyQuery)
- Modify: `src/api/handlers.rs:1671-1734` (quiesce_any handler)

**Step 1: Write integration test**

Create a test that:
1. Creates 3 sessions, tags 2 of them with "build"
2. Sends input to one "build" session to make it active, then lets it go quiescent
3. Calls `GET /quiesce?tag=build` and verifies it returns the quiescent build session

**Step 2: Implement**

Add `tag` to `QuiesceAnyQuery`:
```rust
#[derive(Deserialize)]
pub(super) struct QuiesceAnyQuery {
    timeout_ms: u64,
    #[serde(default)]
    format: Format,
    #[serde(default = "default_max_wait")]
    max_wait_ms: u64,
    last_generation: Option<u64>,
    last_session: Option<String>,
    #[serde(default)]
    fresh: bool,
    #[serde(default)]
    tag: Vec<String>,
}
```

Update `quiesce_any` to filter session names:
```rust
let names = if params.tag.is_empty() {
    state.sessions.list()
} else {
    state.sessions.sessions_by_tags(&params.tag)
};
```

The rest of the function (building quiescence futures per session, racing them) stays unchanged.

**Step 3: Run tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: PASS

**Step 4: Commit**

```
feat: add tag filter to group-scoped quiescence
```

---

### Task 8: CLI — `--tag` Flag and `wsh list` Tags Column

Add `--tag` to the main `wsh` command and `wsh list`, add `wsh tag` subcommand, display tags in list output.

**Files:**
- Modify: `src/main.rs:34-68` (Cli struct)
- Modify: `src/main.rs:115-120` (List subcommand)
- Modify: `src/main.rs:70-94` (Commands enum — add Tag subcommand)
- Modify: `src/main.rs:1007-1014` (CreateSessionMsg construction in run_default)
- Modify: `src/main.rs:1134-1174` (run_list display)
- Modify: `src/protocol.rs` (if needed for socket tag management frames)

**Step 1: Add CLI arg to Cli struct**

```rust
/// Tags for the initial session (can be specified multiple times)
#[arg(long = "tag")]
tags: Vec<String>,
```

**Step 2: Add Tag subcommand**

```rust
/// Manage tags on a session
Tag {
    /// Session name
    name: String,
    /// Action: add or remove
    #[command(subcommand)]
    action: TagAction,
},
```

With:
```rust
#[derive(Subcommand, Debug)]
enum TagAction {
    /// Add tags to a session
    Add {
        /// Tags to add
        tags: Vec<String>,
    },
    /// Remove tags from a session
    Remove {
        /// Tags to remove
        tags: Vec<String>,
    },
}
```

**Step 3: Wire tags into CreateSessionMsg**

In `run_default`:
```rust
let msg = protocol::CreateSessionMsg {
    name: cli.name.clone(),
    command,
    cwd: None,
    env: None,
    rows,
    cols,
    tags: cli.tags,
};
```

**Step 4: Update `wsh list` display**

In `run_list`, add TAGS column:
```rust
println!(
    "{:<20} {:<8} {:<20} {:<12} {:<8} {}",
    "NAME", "PID", "COMMAND", "SIZE", "CLIENTS", "TAGS"
);
for s in &sessions {
    let tags_str = s.tags.join(", ");
    println!(
        "{:<20} {:<8} {:<20} {:<12} {:<8} {}",
        s.name, pid_str, s.command, size, s.clients, tags_str
    );
}
```

**Step 5: Implement `wsh tag` subcommand**

Add `run_tag` function that connects to server socket and sends a tag management request. This requires a new socket protocol frame type for tag management, or we can use the HTTP API. The simplest approach: have `wsh tag` use the HTTP API since the socket already has a running server.

Alternatively, add a `ManageTags` frame type to the socket protocol. Decision: use whatever pattern `wsh kill` and `wsh detach` use (they go through the socket).

**Step 6: Run tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: PASS

**Step 7: Commit**

```
feat: add --tag CLI flag, wsh list tags column, wsh tag subcommand
```

---

### Task 9: Integration Tests

Write end-to-end integration tests covering the full tag lifecycle across API surfaces.

**Files:**
- Create: `tests/tagging_integration.rs`

**Tests to write:**

1. **HTTP tag lifecycle**: Create session with tags → list with filter → add tags via PATCH → verify → remove tags → verify → delete session → verify index clean
2. **Group quiescence**: Create 3 sessions, tag 2, send input to make them active, call `GET /quiesce?tag=X`, verify correct session returned
3. **Socket protocol tags**: Create session via socket with tags → list sessions → verify tags in response
4. **Tag validation**: Attempt to create session with invalid tag → verify 400 error
5. **Rename preserves tags**: Create with tags → rename → verify tags persist
6. **Empty tag filter returns all**: `GET /sessions?` (no tag param) returns all sessions

These should use the `wsh server --bind ... --ephemeral` pattern from existing graceful shutdown tests, hitting the HTTP API with reqwest.

**Step 1: Write the tests**

**Step 2: Run and verify**

Run: `nix develop -c sh -c "cargo test tagging -- --nocapture"`
Expected: PASS

**Step 3: Commit**

```
test: add integration tests for session tagging
```

---

### Task 10: Update Documentation and Skills

Update API docs, README, and skills to document the tagging feature.

**Files:**
- Modify: `docs/api.md` (or equivalent API documentation)
- Modify: skills files in `skills/wsh/` (core skill)
- Modify: `README.md` if it documents the API

**Step 1: Update API documentation**

Document new/changed endpoints:
- `POST /sessions` — `tags` field
- `GET /sessions?tag=X` — tag filter
- `PATCH /sessions/:name` — `add_tags`, `remove_tags`
- `GET /quiesce?tag=X` — tag-scoped quiescence
- Updated `SessionInfo` response shape

**Step 2: Update core skill**

Add tagging concepts to the core wsh skill — how agents should use tags to organize sessions.

**Step 3: Commit**

```
docs: update API documentation and skills for session tagging
```
