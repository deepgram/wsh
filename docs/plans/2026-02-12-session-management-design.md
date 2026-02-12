# Fix Session Management: Server Process Model + Overlay/Panel Forwarding

## Process Model (Canonical Reference)

This section defines the wsh process model. All implementation decisions derive from this.

### Architecture

```
┌──────────────────────────────────────────────────────────┐
│                  wsh server process                       │
│  (either forked by `wsh` or started via `wsh server`)    │
│                                                          │
│  Owns ALL state:                                         │
│  ├── SessionRegistry (all sessions)                      │
│  │   └── Session                                         │
│  │       ├── PTY (master/slave pair, child process)      │
│  │       ├── Parser (terminal state machine)             │
│  │       │   ├── Scrollback buffer (10k lines)           │
│  │       │   ├── Screen grid (cells, colors, cursor)     │
│  │       │   └── Mode flags (alt screen, etc.)           │
│  │       ├── OverlayStore (all overlay state)            │
│  │       ├── PanelStore (all panel state)                │
│  │       ├── ActivityTracker (quiescence)                │
│  │       ├── InputMode (capture toggle)                  │
│  │       ├── FocusTracker (overlay/panel focus)          │
│  │       └── TerminalSize                                │
│  ├── HTTP/WS API server (agents, web UI)                 │
│  └── Unix socket server (CLI clients)                    │
│                                                          │
│  Modes:                                                  │
│  • Ephemeral (default from `wsh`): exits when last       │
│    session ends                                          │
│  • Persistent (`wsh server` or `wsh persist`): stays     │
│    alive until Ctrl+C                                    │
└──────────────────────────────────────────────────────────┘
         │ Unix socket
         │ (binary frames: PtyOutput, StdinInput, Resize,
         │  Detach, OverlaySync, PanelSync, ...)
         ▼
┌──────────────────────────────────────────────────────────┐
│                  wsh client process                       │
│  (started by `wsh`, `wsh attach`, etc.)                  │
│                                                          │
│  Owns NO session state. Is a thin terminal proxy:        │
│  ├── Reads stdin → sends StdinInput frames               │
│  ├── Receives PtyOutput frames → writes to stdout        │
│  ├── Receives OverlaySync → renders ANSI to stdout       │
│  ├── Receives PanelSync → renders ANSI to stdout         │
│  ├── Handles SIGWINCH → sends Resize frames              │
│  └── Handles Ctrl+\ double-tap → sends Detach            │
│                                                          │
│  Terminal management (raw mode, screen guard) is local.  │
│  All state queries go through the server's HTTP API.     │
└──────────────────────────────────────────────────────────┘
```

### Command Behaviors

| Command | Server exists? | Action |
|---------|---------------|--------|
| `wsh` | No | Fork/exec `wsh server --ephemeral` as background daemon, wait for socket, then create session + attach |
| `wsh` | Yes | Connect to socket, create session, attach |
| `wsh server` | No | Start server in foreground, persistent mode |
| `wsh server` | Yes | Error: "server already running" |
| `wsh list` | Yes | Connect to socket, list sessions |
| `wsh list` | No | Error: "no server running" |
| `wsh attach <name>` | Yes | Connect to socket, attach to session |
| `wsh persist` | Yes | HTTP POST to toggle persistence |
| `wsh kill <name>` | Yes | Connect to socket, kill session |

### Key Invariants

1. **Only one server process per user** (one Unix socket at `default_socket_path()`)
2. **Only the server owns the HTTP API** — clients never bind HTTP ports
3. **All session state lives on the server** — scrollback, overlays, panels, PTY, parser
4. **Clients are disposable** — disconnect and reconnect without losing state
5. **Overlay/panel rendering happens on the client** — the server sends state updates, the client writes ANSI to its terminal

---

## Context

Running `wsh` (standalone) creates an isolated session invisible to `wsh list` because
standalone mode never starts a Unix socket server. Running `wsh` after `wsh server`
panics on port 8080. The current standalone mode conflates server and client
responsibilities in one process. This plan fixes the process model and adds
overlay/panel forwarding through the socket protocol.

## Phases

### Phase 1: Fix the process model (session management)

#### 1a. Refactor `run_standalone()` into smart client

**File: `src/main.rs`**

Replace `run_standalone()` with:

```rust
async fn run_standalone(cli: Cli) -> Result<(), WshError> {
    let socket_path = server::default_socket_path();

    // Try connecting to an existing server
    let client = match client::Client::connect(&socket_path).await {
        Ok(c) => c,
        Err(_) => {
            // No server running — fork/exec one
            spawn_server_daemon(&socket_path)?;
            wait_for_socket(&socket_path).await?;
            client::Client::connect(&socket_path).await?
        }
    };

    run_as_client(cli, client).await
}
```

#### 1b. Implement `spawn_server_daemon()`

**File: `src/main.rs`**

Fork/exec `wsh server --ephemeral` as a background process using
`std::process::Command`:

- `args: ["server", "--ephemeral"]`
- stdout/stderr redirected to a log file or `/dev/null`
- Detached from parent process group (setsid or equivalent)
- The `--socket` path passed through if non-default

#### 1c. Implement `run_as_client()`

**File: `src/main.rs`**

New client attach flow:

1. Get terminal size
2. Build `CreateSessionMsg` from CLI args:
   - `--name` → `msg.name`
   - `-c` → `msg.command`
   - `--shell` → `msg.command` (pass shell path as command)
   - terminal size → `msg.rows`, `msg.cols`
3. `client.create_session(msg)` → `CreateSessionResponse`
4. Enter raw mode (`RawModeGuard`)
5. Set up screen guard (clear / alt screen based on `--alt-screen`)
6. `client.run_streaming()` — bidirectional I/O
7. On return: restore terminal, exit

#### 1d. Make `wsh server` persistent by default

**File: `src/main.rs`**

Change `run_server()` to use `ServerConfig::new(true)` (persistent).

Add `--ephemeral` flag to `Commands::Server`:

```rust
Server {
    ...
    #[arg(long)]
    ephemeral: bool,
}
```

Use `ServerConfig::new(!ephemeral)` so:
- `wsh server` → persistent (stays alive until Ctrl+C)
- `wsh server --ephemeral` → exits when last session ends
- Auto-spawn from `wsh` passes `--ephemeral`

#### 1e. Remove old standalone code

The old `run_standalone()` body (PTY spawn, output loop, stdin reader,
SIGWINCH handler, HTTP server) is no longer needed. Remove it entirely.
The server handles PTY management; the client handles terminal I/O.

#### 1f. Socket cleanup

`server::serve()` already handles stale socket detection. The server
process should clean up its socket on exit (add cleanup in `run_server()`
shutdown path).

### Phase 2: Fix panel coordinator for headless server

The panel coordinator (`src/panel/coordinator.rs`) writes directly to
stdout unconditionally. In the headless server, there's no terminal.

#### 2a. Gate panel stdout writes

**File: `src/panel/coordinator.rs`**

Add `is_local: bool` parameter to `reconfigure_layout()` and
`flush_panel_content()`. Only write ANSI to stdout when
`is_local == true`.

The PTY resize and parser resize in `reconfigure_layout()` remain
unconditional (they affect the virtual terminal state, not stdout).

#### 2b. Update all callers

**Files: `src/api/handlers.rs`, `src/api/ws_methods.rs`, `src/main.rs`**

Pass `session.is_local` to `reconfigure_layout()` and
`flush_panel_content()` at every call site (~15 call sites across
handlers.rs and ws_methods.rs, plus 1 in main.rs SIGWINCH handler).

### Phase 3: Forward overlay/panel state to socket clients

Currently the socket streaming protocol only sends `PtyOutput` frames.
Clients can't see overlays or panels.

#### 3a. Add new frame types

**File: `src/protocol.rs`**

```rust
OverlaySync = 0x12,   // Server → Client: full overlay state
PanelSync = 0x13,     // Server → Client: full panel state + layout
```

Message structs:

```rust
#[derive(Serialize, Deserialize)]
pub struct OverlaySyncMsg {
    pub overlays: Vec<OverlayData>,
}

#[derive(Serialize, Deserialize)]
pub struct PanelSyncMsg {
    pub panels: Vec<PanelData>,
    pub scroll_region_top: u16,
    pub scroll_region_bottom: u16,
}
```

Full-state sync (not deltas) — overlay/panel counts are small.

#### 3b. Add visual change notification channel

**File: `src/session.rs`**

Add broadcast channel to Session:

```rust
pub visual_update_tx: broadcast::Sender<VisualUpdate>,
```

```rust
pub enum VisualUpdate {
    OverlaysChanged,
    PanelsChanged,
}
```

API handlers fire `visual_update_tx.send(...)` after modifying
overlays/panels.

#### 3c. Forward visual updates in server streaming loop

**File: `src/server.rs` — `run_streaming()`**

Add a new arm to the `select!` loop that subscribes to
`visual_update_rx` and sends `OverlaySync`/`PanelSync` frames to
the client.

#### 3d. Render overlay/panel state on the client

**File: `src/client.rs` — `streaming_loop()`**

Handle new frame types. Client maintains a local cache of
overlay/panel state so it can compute erase sequences before
rendering new state.

#### 3e. Send initial state on attach/create

**File: `src/server.rs`**

After sending AttachSessionResponse/CreateSessionResponse, send
initial OverlaySync and PanelSync frames before entering the
streaming loop.

#### 3f. Interleave overlay rendering with PTY output

**File: `src/client.rs` — `streaming_loop()`**

When writing PtyOutput to stdout, wrap with overlay erase/re-render
using DEC2026 synchronized output (matching the current standalone
behavior in main.rs:383-399).

### Phase 4: Remove `is_local` from Session

Once overlay/panel rendering is fully client-side, `is_local` is
unnecessary. Remove from Session and all conditional checks in
handlers/ws_methods. The server never writes ANSI to stdout.

### Phase 5: Tests

#### Unit tests
- `src/server.rs`: OverlaySync/PanelSync frames sent on state change
- `src/client.rs`: Client handles OverlaySync/PanelSync frames
- `src/protocol.rs`: New frame type serialization

#### Integration tests
- Server auto-spawn: `wsh` as subprocess, `wsh list` shows session
- Multiple sessions: Two `wsh` clients, both visible in `wsh list`
- Session lifecycle: create, attach, detach, re-attach, kill
- Overlay forwarding: Create via HTTP API, client receives OverlaySync
- Panel forwarding: Create via HTTP API, client receives PanelSync
- Ephemeral exit: Auto-spawned server exits after last session ends
- Persistent mode: `wsh server` stays alive after sessions end

### Phase 6: Documentation and skills

- Update API documentation for new socket frame types
- Update skills to reflect that `wsh` always connects to a server
- Ensure VISION.md is consistent with the process model above

---

## Files Modified

| File | Changes |
|------|---------|
| `src/main.rs` | Replace `run_standalone()` with smart client; add `spawn_server_daemon()`, `run_as_client()`; add `--ephemeral` to Server command; make Server persistent by default; remove old standalone code |
| `src/server.rs` | Send OverlaySync/PanelSync in streaming loop; send initial state on attach/create; socket cleanup on exit |
| `src/client.rs` | Handle OverlaySync/PanelSync frames; local overlay/panel cache; render overlays around PtyOutput |
| `src/protocol.rs` | Add OverlaySync (0x12), PanelSync (0x13) frame types and message structs |
| `src/session.rs` | Add `visual_update_tx` broadcast channel; remove `is_local` field |
| `src/panel/coordinator.rs` | Gate stdout writes with `is_local` parameter (Phase 2), then remove entirely (Phase 4) |
| `src/api/handlers.rs` | Fire `visual_update_tx` after overlay/panel mutations; pass `is_local` to panel coordinator |
| `src/api/ws_methods.rs` | Same as handlers.rs |
| `src/overlay/types.rs` | Add Serialize derives for socket transport (if not already present) |
| `src/panel/types.rs` | Add Serialize derives for socket transport (if not already present) |
| `tests/` | New integration tests for process model, overlay/panel forwarding |

## Verification

1. `wsh` (no server running) → server auto-spawns, session created, terminal works
2. `wsh list` → shows the session
3. Second `wsh` → creates second session on same server, no port conflict
4. `wsh list` → shows both sessions
5. Exit second `wsh` → session removed
6. Exit first `wsh` → last session ends, ephemeral server exits
7. `wsh list` → "no server running" error
8. `wsh server` → starts persistent server
9. `wsh` → connects, creates session
10. Exit `wsh` → server stays alive (persistent)
11. Create overlay via HTTP API → appears on attached `wsh` client terminal
12. Create panel via HTTP API → appears on attached `wsh` client terminal
13. All existing tests pass
