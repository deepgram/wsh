# wsh API Reference

wsh exposes terminal I/O via HTTP and WebSocket. This document covers every
endpoint, request format, and response shape you need to build against the API.

wsh operates in two modes:

- **Standalone mode** (default): A single session with local terminal I/O and
  an HTTP/WS API server. Run `wsh` with no subcommand.
- **Server mode**: A headless daemon managing multiple sessions via HTTP/WS and
  a Unix domain socket. Run `wsh server`.

**Base URL:** `http://localhost:8080` (default)

## Endpoints at a Glance

### Session Endpoints (nested under `/sessions/:name`)

In server mode, per-session endpoints are nested under `/sessions/:name`. In
standalone mode, the single session is accessible at the top level for backward
compatibility.

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/sessions/:name/input` | Inject bytes into the terminal |
| `GET` | `/sessions/:name/screen` | Current screen state |
| `GET` | `/sessions/:name/scrollback` | Scrollback buffer contents |
| `GET` | `/sessions/:name/ws/raw` | Raw binary WebSocket |
| `GET` | `/sessions/:name/ws/json` | JSON event WebSocket |
| `POST` | `/sessions/:name/overlay` | Create an overlay |
| `GET` | `/sessions/:name/overlay` | List all overlays |
| `DELETE` | `/sessions/:name/overlay` | Clear all overlays |
| `GET` | `/sessions/:name/overlay/:id` | Get a single overlay |
| `PUT` | `/sessions/:name/overlay/:id` | Replace overlay spans |
| `PATCH` | `/sessions/:name/overlay/:id` | Move/reorder an overlay |
| `DELETE` | `/sessions/:name/overlay/:id` | Delete an overlay |
| `POST` | `/sessions/:name/overlay/:id/spans` | Partial span update by ID |
| `POST` | `/sessions/:name/overlay/:id/write` | Region write (cell-level drawing) |
| `POST` | `/sessions/:name/panel` | Create a panel |
| `GET` | `/sessions/:name/panel` | List all panels |
| `DELETE` | `/sessions/:name/panel` | Clear all panels |
| `GET` | `/sessions/:name/panel/:id` | Get a single panel |
| `PUT` | `/sessions/:name/panel/:id` | Replace a panel |
| `PATCH` | `/sessions/:name/panel/:id` | Partially update a panel |
| `DELETE` | `/sessions/:name/panel/:id` | Delete a panel |
| `POST` | `/sessions/:name/panel/:id/spans` | Partial span update by ID |
| `POST` | `/sessions/:name/panel/:id/write` | Region write (cell-level drawing) |
| `GET` | `/sessions/:name/input/mode` | Get current input mode |
| `POST` | `/sessions/:name/input/capture` | Switch to capture mode |
| `POST` | `/sessions/:name/input/release` | Switch to passthrough mode |
| `GET` | `/sessions/:name/input/focus` | Get current input focus |
| `POST` | `/sessions/:name/input/focus` | Set input focus to an element |
| `POST` | `/sessions/:name/input/unfocus` | Clear input focus |
| `GET` | `/sessions/:name/screen_mode` | Get current screen mode |
| `POST` | `/sessions/:name/screen_mode/enter_alt` | Enter alternate screen mode |
| `POST` | `/sessions/:name/screen_mode/exit_alt` | Exit alternate screen mode |
| `GET` | `/sessions/:name/quiesce` | Wait for terminal quiescence |
| `POST` | `/sessions/:name/detach` | Detach all clients from the session |

### Session Management Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/sessions` | List all sessions |
| `POST` | `/sessions` | Create a new session |
| `GET` | `/sessions/:name` | Get session info |
| `PATCH` | `/sessions/:name` | Rename a session |
| `DELETE` | `/sessions/:name` | Kill (destroy) a session |

### Server Management Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/server/persist` | Query current persistence mode |
| `PUT` | `/server/persist` | Set persistence mode (on/off) |
| `GET` | `/ws/json` | Server-level JSON WebSocket (multi-session) |

### Global Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Health check (no auth) |
| `GET` | `/openapi.yaml` | OpenAPI specification (no auth) |
| `GET` | `/docs` | This documentation (no auth) |

## Quick Start

### Getting Started

Running `wsh` auto-spawns a server daemon if one isn't already running, then creates and attaches to a session. All API endpoints are available immediately.

```bash
# Start wsh (auto-spawns server, creates session, attaches)
wsh

# In another terminal:
# Check health
curl http://localhost:8080/health
# {"status":"ok"}

# List sessions
curl http://localhost:8080/sessions
# [{"name":"default"}]

# Get current screen contents
curl http://localhost:8080/sessions/default/screen
# {"epoch":1,"first_line_index":0,"total_lines":1,"lines":["$ "],...}

# Send input (type "ls\n")
curl -X POST http://localhost:8080/sessions/default/input -d 'ls\n'

# Connect to raw WebSocket (using websocat)
websocat ws://localhost:8080/sessions/default/ws/raw
```

### Server Mode

For persistent operation (e.g., hosting sessions for AI agents):

```bash
# Start the server daemon (persistent by default)
wsh server

# Create a session via HTTP
curl -X POST http://localhost:8080/sessions \
  -H 'Content-Type: application/json' \
  -d '{"name": "dev"}'
# {"name":"dev"}

# List sessions
curl http://localhost:8080/sessions
# [{"name":"dev"}]

# Get the session's screen
curl http://localhost:8080/sessions/dev/screen

# Send input to the session
curl -X POST http://localhost:8080/sessions/dev/input -d 'ls\n'

# Attach from another terminal via CLI
wsh attach dev

# Kill the session
curl -X DELETE http://localhost:8080/sessions/dev

# Use --ephemeral to auto-exit when last session ends
wsh server --ephemeral
```

## Health Check

```
GET /health
```

Always returns 200. Not subject to authentication.

**Response:**

```json
{"status": "ok"}
```

## Input Injection

```
POST /input
```

Sends raw bytes to the terminal's PTY. The request body is forwarded verbatim
-- there is no JSON wrapping. Use `Content-Type: application/octet-stream` or
`text/plain`.

**Response:** `204 No Content` on success.

**Errors:**

| Status | Code | When |
|--------|------|------|
| 500 | `input_send_failed` | PTY channel closed or broken |

**Example -- send Ctrl+C:**

```bash
printf '\x03' | curl -X POST http://localhost:8080/input --data-binary @-
```

## Screen State

```
GET /screen?format=styled
```

Returns the current visible screen, including cursor position and whether the
alternate screen buffer is active.

**Query parameters:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `format` | `plain` \| `styled` | `styled` | Line format (see below) |

**Response:**

```json
{
  "epoch": 42,
  "first_line_index": 0,
  "total_lines": 24,
  "lines": [ ... ],
  "cursor": {"row": 0, "col": 5, "visible": true},
  "cols": 80,
  "rows": 24,
  "alternate_active": false
}
```

`epoch` increments on each state change, useful for change detection.

### Line Formats

With `format=plain`, each line is a plain string:

```json
"lines": ["$ ls", "file.txt  README.md"]
```

With `format=styled` (default), lines that have styling are arrays of spans:

```json
"lines": [
  "$ ls",
  [
    {"text": "file.txt", "fg": {"indexed": 2}, "bold": true},
    {"text": "  README.md"}
  ]
]
```

A line with no styling may still appear as a plain string even in styled mode.
This keeps payloads compact.

### Span Object

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `text` | string | always | The text content |
| `fg` | Color | when set | Foreground color |
| `bg` | Color | when set | Background color |
| `bold` | boolean | when true | Bold |
| `faint` | boolean | when true | Dim/faint |
| `italic` | boolean | when true | Italic |
| `underline` | boolean | when true | Underline |
| `strikethrough` | boolean | when true | Strikethrough |
| `blink` | boolean | when true | Blink |
| `inverse` | boolean | when true | Reverse video |

Style fields use `skip_serializing_if`, so absent means `false` / unset.

### Color Object

Terminal colors serialize as one of:

```json
{"indexed": 2}       // 256-color palette index (0-255)
{"rgb": {"r": 255, "g": 128, "b": 0}}  // True color
```

## Scrollback Buffer

```
GET /scrollback?format=styled&offset=0&limit=100
```

Returns lines from the scrollback buffer (history above the visible screen).

**Query parameters:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `format` | `plain` \| `styled` | `styled` | Line format |
| `offset` | integer | `0` | Starting line index |
| `limit` | integer | `100` | Maximum lines to return |

**Response:**

```json
{
  "epoch": 42,
  "lines": [ ... ],
  "total_lines": 500,
  "offset": 0
}
```

Use `total_lines` and `offset` for pagination.

## WebSocket Endpoints

See [websocket.md](websocket.md) for the full WebSocket protocol documentation.

### Raw Binary WebSocket (`/ws/raw`)

Bidirectional byte stream. Output from the PTY arrives as binary frames. Send
binary or text frames to inject input.

### JSON Event WebSocket (`/ws/json`)

Structured request/response protocol over WebSocket. Supports method calls
(query state, inject input, manage overlays) and event subscriptions. After
connecting, you receive `{"connected": true}` and can send any method call:

```json
{"id": 1, "method": "get_screen", "params": {"format": "styled"}}
```

## Overlays

See [overlays.md](overlays.md) for the full overlay system documentation.

Overlays are positioned text elements rendered on top of terminal content.
Useful for floating notifications, tooltips, and agent-driven UI elements.

## Panels

See [panels.md](panels.md) for the full panel system documentation.

Panels are agent-owned screen regions anchored to the top or bottom of the
terminal. Unlike overlays, panels cause the PTY to shrink, creating dedicated
space that terminal output can never write to. Useful for persistent status
bars, toolbars, and progress indicators.

## Input Capture

See [input-capture.md](input-capture.md) for the full input capture documentation.

Input capture lets API clients intercept keyboard input before it reaches the
terminal's PTY. Useful for building custom key handlers and agent interactions.
Includes focus tracking for directing input to specific overlays or panels.

## Quiescence Sync

```
GET /quiesce?timeout_ms=2000
```

Long-polls until the terminal has been idle (no PTY output or input from any
source) for `timeout_ms` milliseconds, then returns a full screen state
snapshot. Useful for agents and automation that need to know when "the dust has
settled" after sending a command.

**Query parameters:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `timeout_ms` | integer | (required) | Quiescence threshold in milliseconds |
| `format` | `plain` \| `styled` | `styled` | Line format for response |
| `max_wait_ms` | integer | `30000` | Overall deadline before returning 408 |
| `last_generation` | integer | (none) | Generation from a previous response; blocks until new activity if it matches |
| `fresh` | boolean | `false` | Always observe real silence for `timeout_ms` before responding |

If the terminal has already been quiet for `timeout_ms` when the request
arrives, it responds immediately (unless `last_generation` or `fresh` are used).

**Response (200):**

```json
{
  "screen": { ... },
  "scrollback_lines": 150,
  "generation": 42
}
```

The `screen` object has the same shape as `GET /screen`. The `generation` field
is a monotonic counter that increments on each activity event.

**Preventing busy-loop storms:**

When polling quiescence repeatedly (e.g., waiting for a command that hasn't
finished), pass back the `generation` from the previous response as
`last_generation`. If no new activity has occurred, the server blocks until
something happens — preventing rapid-fire immediate responses:

```bash
# First call: may return immediately if already idle
curl 'http://localhost:8080/quiesce?timeout_ms=500&format=plain'
# Response: {"screen": ..., "generation": 42}

# Subsequent call: blocks until new activity, then waits for quiescence
curl 'http://localhost:8080/quiesce?timeout_ms=500&last_generation=42&format=plain'
```

Alternatively, use `fresh=true` to always observe real silence without tracking
generation state — at the cost of always waiting at least `timeout_ms`:

```bash
curl 'http://localhost:8080/quiesce?timeout_ms=500&fresh=true&format=plain'
```

**Errors:**

| Status | Code | When |
|--------|------|------|
| 408 | `quiesce_timeout` | `max_wait_ms` exceeded without quiescence |

The WebSocket equivalent is the `await_quiesce` method — see
[websocket.md](websocket.md). Subscriptions can also include automatic
quiescence sync via the `quiesce_ms` parameter.

### Server-Level Quiescence (Any Session)

```
GET /quiesce?timeout_ms=2000
```

Races quiescence detection across **all** sessions, returning the first
session to become quiescent. The response includes the session name so you
know which session settled.

**Query parameters:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `timeout_ms` | integer | (required) | Quiescence threshold in milliseconds |
| `format` | `plain` \| `styled` | `styled` | Line format for response |
| `max_wait_ms` | integer | `30000` | Overall deadline before returning 408 |
| `last_generation` | integer | (none) | Generation from a previous response; paired with `last_session` |
| `last_session` | string | (none) | Session name from a previous response; paired with `last_generation` |
| `fresh` | boolean | `false` | Always observe real silence for `timeout_ms` before responding |

**Response (200):**

```json
{
  "session": "build",
  "screen": { ... },
  "scrollback_lines": 150,
  "generation": 42
}
```

**Preventing busy-loop storms:**

Pass back both `last_session` and `last_generation` from the previous
response. The named session waits for new activity before checking
quiescence, while all other sessions are checked immediately:

```bash
# First call: returns whichever session becomes quiescent first
curl 'http://localhost:8080/quiesce?timeout_ms=500&format=plain'
# Response: {"session": "build", "screen": ..., "generation": 42}

# Subsequent call: "build" won't return until it has new activity,
# but other sessions can still win the race
curl 'http://localhost:8080/quiesce?timeout_ms=500&last_session=build&last_generation=42&format=plain'
```

**Errors:**

| Status | Code | When |
|--------|------|------|
| 404 | `no_sessions` | No sessions exist in the registry |
| 408 | `quiesce_timeout` | `max_wait_ms` exceeded without quiescence on any session |

## Server Mode

Server mode (`wsh server`) runs a headless daemon that manages multiple terminal
sessions. Sessions are created on demand via the HTTP API or Unix socket
protocol. Unlike standalone mode, no PTY is spawned automatically and no local
terminal I/O is performed.

### CLI Subcommands

wsh provides several subcommands for interacting with a running server:

| Subcommand | Description |
|------------|-------------|
| `wsh server` | Start the server daemon |
| `wsh attach <name>` | Attach to a session (local terminal I/O over Unix socket) |
| `wsh list` | List active sessions |
| `wsh kill <name>` | Destroy a session |
| `wsh detach <name>` | Detach all clients from a session |
| `wsh persist [on\|off]` | Query or set server persistence mode |

#### `wsh server`

```bash
wsh server [--bind <addr>] [--token <token>] [--socket <path>]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--bind` | `127.0.0.1:8080` | Address for the HTTP/WebSocket API server |
| `--token` | (auto-generated if non-localhost) | Authentication token |
| `--socket` | `$XDG_RUNTIME_DIR/wsh.sock` | Path to the Unix domain socket |

The server starts both an HTTP/WS listener and a Unix domain socket listener.
The HTTP/WS API serves session management, per-session endpoints, and the
server-level WebSocket. The Unix socket handles CLI client connections (`wsh
attach`).

#### `wsh attach`

```bash
wsh attach <name> [--scrollback <all|none|N>] [--socket <path>]
```

Attaches to a named session. The local terminal enters raw mode and proxies
I/O between your terminal and the session's PTY via the Unix socket. On attach,
scrollback and current screen content are replayed to bring your terminal up to
date.

| Flag | Default | Description |
|------|---------|-------------|
| `--scrollback` | `all` | Scrollback replay: `all`, `none`, or a line count |
| `--socket` | `$XDG_RUNTIME_DIR/wsh.sock` | Path to the Unix domain socket |
| `--alt-screen` | off | Use alternate screen buffer (restores previous screen on exit, but disables native terminal scrollback while attached) |

#### `wsh list`

```bash
wsh list [--socket <path>]
```

Lists active sessions on the server via the Unix socket.

#### `wsh kill`

```bash
wsh kill <name> [--socket <path>]
```

Destroys a named session on the server via the Unix socket.

#### `wsh detach`

```bash
wsh detach <name> [--socket <path>]
```

Detaches all connected clients from a named session via the Unix socket. The
session itself remains alive -- only the client connections are dropped.

#### `wsh persist`

```bash
wsh persist [on|off] [--bind <addr>] [--token <token>]
```

Query or set the server's persistence mode. With no argument, prints the current
state. `wsh persist on` enables persistent mode (server stays alive when all
sessions end). `wsh persist off` enables ephemeral mode (server exits when the
last session ends).

### Session Management

#### List Sessions

```
GET /sessions
```

Returns an array of all active sessions.

**Response:** `200 OK`

```json
[{"name": "dev"}, {"name": "build"}]
```

**Example:**

```bash
curl http://localhost:8080/sessions
```

#### Create a Session

```
POST /sessions
Content-Type: application/json
```

**Request body:**

```json
{
  "name": "dev",
  "command": "bash",
  "rows": 24,
  "cols": 80,
  "cwd": "/home/user/project",
  "env": {"TERM": "xterm-256color"}
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | no | Session name (auto-generated if omitted) |
| `command` | string | no | Command to run (defaults to user's shell) |
| `rows` | integer | no | Terminal rows (default: 24) |
| `cols` | integer | no | Terminal columns (default: 80) |
| `cwd` | string | no | Working directory |
| `env` | object | no | Additional environment variables |

**Response:** `201 Created`

```json
{"name": "dev"}
```

**Errors:**

| Status | Code | When |
|--------|------|------|
| 409 | `session_name_conflict` | Name already in use |
| 500 | `session_create_failed` | PTY spawn or other creation error |

**Example:**

```bash
curl -X POST http://localhost:8080/sessions \
  -H 'Content-Type: application/json' \
  -d '{"name": "dev", "command": "bash"}'
```

#### Get Session Info

```
GET /sessions/:name
```

**Response:** `200 OK`

```json
{"name": "dev"}
```

**Errors:**

| Status | Code | When |
|--------|------|------|
| 404 | `session_not_found` | No session with that name |

**Example:**

```bash
curl http://localhost:8080/sessions/dev
```

#### Rename a Session

```
PATCH /sessions/:name
Content-Type: application/json
```

**Request body:**

```json
{"name": "new-name"}
```

**Response:** `200 OK`

```json
{"name": "new-name"}
```

**Errors:**

| Status | Code | When |
|--------|------|------|
| 404 | `session_not_found` | No session with the original name |
| 409 | `session_name_conflict` | New name already in use |

**Example:**

```bash
curl -X PATCH http://localhost:8080/sessions/dev \
  -H 'Content-Type: application/json' \
  -d '{"name": "production"}'
```

#### Kill a Session

```
DELETE /sessions/:name
```

Destroys the session and its PTY.

**Response:** `204 No Content`

**Errors:**

| Status | Code | When |
|--------|------|------|
| 404 | `session_not_found` | No session with that name |

**Example:**

```bash
curl -X DELETE http://localhost:8080/sessions/dev
```

### Detach a Session

```
POST /sessions/:name/detach
```

Detaches all connected clients (Unix socket `wsh attach` sessions) from the
named session. The session itself remains alive -- only the client connections
are dropped. Useful for forcibly disconnecting attached terminals without
destroying the session.

**Response:** `204 No Content`

**Errors:**

| Status | Code | When |
|--------|------|------|
| 404 | `session_not_found` | No session with that name |

**Example:**

```bash
curl -X POST http://localhost:8080/sessions/dev/detach
```

### Server Persist

```
GET /server/persist
```

Returns the current persistence mode without changing it.

**Response:** `200 OK`

```json
{"persistent": false}
```

```
PUT /server/persist
```

Sets the server's persistence mode.

**Request body:**

```json
{"persistent": true}
```

**Response:** `200 OK`

```json
{"persistent": true}
```

**Examples:**

```bash
# Query current state
curl http://localhost:8080/server/persist

# Enable persistent mode
curl -X PUT http://localhost:8080/server/persist \
  -H 'Content-Type: application/json' \
  -d '{"persistent": true}'

# Enable ephemeral mode
curl -X PUT http://localhost:8080/server/persist \
  -H 'Content-Type: application/json' \
  -d '{"persistent": false}'
```

### Ephemeral vs Persistent Mode

By default, the server starts in **ephemeral mode**: it shuts down automatically
when its last session exits or is destroyed. This is useful for ad-hoc server
usage where you want automatic cleanup.

In **persistent mode**, the server stays alive indefinitely, waiting for new
sessions to be created. Toggle via `GET`/`PUT /server/persist`,
the `wsh persist [on|off]` CLI command, or the `set_server_mode` WebSocket method.

### Server-Level WebSocket

```
GET /ws/json
```

In server mode, the top-level `/ws/json` endpoint provides a multiplexed
WebSocket that can interact with any session and receive session lifecycle
events. After connecting, the server sends `{"connected": true}`.

**Server-level methods** (no `session` field needed):

| Method | Description |
|--------|-------------|
| `list_sessions` | List all active sessions |
| `create_session` | Create a new session |
| `kill_session` | Destroy a session |
| `detach_session` | Detach all clients from a session |
| `set_server_mode` | Query or set server mode (ephemeral/persistent) |

**Per-session methods** require a `session` field in the request:

```json
{"id": 1, "method": "get_screen", "session": "dev", "params": {"format": "styled"}}
```

All the standard per-session methods (`get_screen`, `get_scrollback`,
`send_input`, `subscribe`, `await_quiesce`, overlay/panel methods, etc.) work
the same as on the per-session `/sessions/:name/ws/json` endpoint.

**Session lifecycle events** are broadcast automatically:

```json
{"event": "session_created", "params": {"name": "dev"}}
{"event": "session_exited", "params": {"name": "dev"}}
{"event": "session_renamed", "params": {"old_name": "dev", "new_name": "prod"}}
{"event": "session_destroyed", "params": {"name": "dev"}}
```

#### `set_server_mode`

Query or set the server's persistence mode. If `params` is omitted, returns the
current mode without changing it.

**Params (optional):**

| Param | Type | Description |
|-------|------|-------------|
| `persistent` | boolean | `true` for persistent mode, `false` for ephemeral |

```json
// Set mode
{"id": 1, "method": "set_server_mode", "params": {"persistent": true}}

// Query mode (no params)
{"id": 2, "method": "set_server_mode"}
```

**Result:**

```json
{"id": 1, "method": "set_server_mode", "result": {"persistent": true}}
```

### Unix Socket Protocol

The Unix domain socket provides a binary framing protocol for CLI client
connections (`wsh attach`). It is designed for low-latency, bidirectional I/O
proxying between a local terminal and a server-managed PTY session.

#### Wire Format

Each frame consists of:

```
[type: u8][length: u32 big-endian][payload: bytes]
```

The maximum payload size is 16 MiB.

#### Frame Types

**Control frames** (JSON payload):

| Type | Byte | Direction | Description |
|------|------|-----------|-------------|
| `CreateSession` | `0x01` | Client -> Server | Request to create a new session |
| `CreateSessionResponse` | `0x02` | Server -> Client | Session creation response |
| `AttachSession` | `0x03` | Client -> Server | Request to attach to an existing session |
| `AttachSessionResponse` | `0x04` | Server -> Client | Attach response with scrollback/screen replay |
| `Detach` | `0x05` | Client -> Server | Cleanly detach from the session |
| `Resize` | `0x06` | Client -> Server | Terminal resize notification |
| `Error` | `0x07` | Server -> Client | Error response |

**Data frames** (raw bytes payload):

| Type | Byte | Direction | Description |
|------|------|-----------|-------------|
| `PtyOutput` | `0x10` | Server -> Client | PTY output data |
| `StdinInput` | `0x11` | Client -> Server | Keyboard input data |

#### Connection Lifecycle

1. Client connects to the Unix socket
2. Client sends a `CreateSession` or `AttachSession` control frame
3. Server responds with the corresponding response frame
4. Both sides enter streaming mode: `PtyOutput` and `StdinInput` frames flow
   bidirectionally
5. Client sends `Resize` frames when the terminal is resized
6. Client sends a `Detach` frame to cleanly disconnect (session remains alive)

#### Control Message Schemas

**CreateSession:**

```json
{
  "name": "dev",
  "command": "bash",
  "cwd": "/home/user",
  "env": {"KEY": "value"},
  "rows": 24,
  "cols": 80
}
```

**AttachSession:**

```json
{
  "name": "dev",
  "scrollback": "all",
  "rows": 24,
  "cols": 80
}
```

The `scrollback` field accepts `"none"`, `"all"`, or `{"lines": N}`.

**AttachSessionResponse:**

```json
{
  "name": "dev",
  "rows": 24,
  "cols": 80,
  "scrollback": "<base64-encoded raw terminal bytes>",
  "screen": "<base64-encoded raw terminal bytes>"
}
```

The `scrollback` and `screen` fields contain base64-encoded raw terminal bytes
(including ANSI escape sequences) for replaying into the client's terminal.

**Resize:**

```json
{"rows": 40, "cols": 120}
```

**Error:**

```json
{"code": "session_not_found", "message": "No session named 'foo'"}
```

## Authentication

See [authentication.md](authentication.md) for the full authentication documentation.

When wsh binds to a non-localhost address, bearer token authentication is
required on all endpoints except `/health`, `/docs`, and `/openapi.yaml`.

## Error Responses

See [errors.md](errors.md) for the complete error code reference.

All errors return JSON with a consistent structure:

```json
{
  "error": {
    "code": "machine_readable_code",
    "message": "Human-readable description."
  }
}
```

## Alternate Screen Mode

See [alt-screen.md](alt-screen.md) for full alternate screen mode documentation.

Alternate screen mode lets agents create temporary UI contexts. Overlays and
panels created in alt mode are isolated from normal-mode elements and are
automatically cleaned up when exiting alt screen.

## Related Documents

- [authentication.md](authentication.md) -- Auth model and token configuration
- [websocket.md](websocket.md) -- WebSocket protocol and event types
- [errors.md](errors.md) -- Error code reference
- [overlays.md](overlays.md) -- Overlay system
- [panels.md](panels.md) -- Panel system
- [input-capture.md](input-capture.md) -- Input capture and focus tracking
- [alt-screen.md](alt-screen.md) -- Alternate screen mode
- [openapi.yaml](openapi.yaml) -- Machine-readable OpenAPI 3.1 spec
