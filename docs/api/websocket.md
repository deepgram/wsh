# WebSocket Protocol

wsh exposes two WebSocket endpoints for real-time terminal interaction.

## Raw Binary WebSocket

```
GET /ws/raw
```

A bidirectional byte stream mirroring the terminal's PTY.

### Output (server -> client)

Binary frames containing raw PTY output. This includes ANSI escape sequences,
control characters, and UTF-8 text exactly as the terminal emits them.

### Input (client -> server)

Send binary or text frames to inject bytes into the PTY. The data is forwarded
verbatim -- no JSON encoding.

### Lifecycle

1. Client sends HTTP upgrade request to `/ws/raw`
2. Connection opens; output frames begin immediately
3. Client sends input frames at any time
4. Either side closes the connection

### Use Cases

- Building custom terminal emulators
- Piping raw terminal I/O to/from external tools
- Low-overhead monitoring

---

## JSON Event WebSocket

```
GET /ws/json
```

A structured protocol for querying terminal state, injecting input, managing
overlays, and subscribing to real-time events -- all over a single connection.

### Connection Flow

```
Client                           Server
  |                                |
  |  ---- WS upgrade ---------->  |
  |  <--- { "connected": true } - |
  |                                |
  |  ---- method call ---------->  |
  |  <--- method response ------  |
  |                                |
  |  ---- subscribe method ----->  |
  |  <--- subscribe response ---  |
  |  <--- sync event              |
  |  <--- events (continuous) ---  |
  |                                |
```

### Step 1: Connect

After the WebSocket handshake, the server sends:

```json
{"connected": true}
```

### Request/Response Protocol

All client messages use a JSON-RPC-like envelope:

```json
{"id": 1, "method": "get_screen", "params": {"format": "styled"}}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `method` | string | yes | Method name to invoke |
| `id` | any | no | Request identifier, echoed in response |
| `params` | object | no | Method-specific parameters (defaults to `{}`) |

The server responds with:

```json
{"id": 1, "method": "get_screen", "result": { ... }}
```

On error:

```json
{"id": 1, "method": "get_screen", "error": {"code": "parser_unavailable", "message": "..."}}
```

**Distinguishing message types** -- server messages are one of three kinds:

| Kind | Discriminator | Example |
|------|---------------|---------|
| Connected | has `connected` | `{"connected": true}` |
| Response | has `method` | `{"method": "get_screen", "result": {...}}` |
| Event | has `event` | `{"event": "line", "seq": 5, ...}` |

**Protocol errors** -- if the client sends invalid JSON or a message without a
`method` field, the server returns an error with no `method` or `id`:

```json
{"error": {"code": "invalid_request", "message": "Invalid JSON or missing 'method' field."}}
```

### Step 2: Subscribe to Events

Subscribing is a method call like any other. Send:

```json
{"id": 1, "method": "subscribe", "params": {"events": ["lines", "cursor", "diffs"]}}
```

Response:

```json
{"id": 1, "method": "subscribe", "result": {"events": ["lines", "cursor", "diffs"]}}
```

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `events` | array of strings | (required) | Event types to subscribe to |
| `interval_ms` | integer | `100` | Minimum interval between events (ms) |
| `format` | `"plain"` \| `"styled"` | `"styled"` | Line format for events containing lines |
| `quiesce_ms` | integer | `0` | When > 0, emit a `sync` event after this many ms of inactivity |

**Available event types:**

| Type | Description |
|------|-------------|
| `lines` | Individual line updates |
| `cursor` | Cursor position changes |
| `mode` | Alternate screen enter/exit |
| `diffs` | Batched screen diffs (changed line indices + full screen) |
| `input` | Keyboard input events (requires input capture) |

### Step 3: Initial Sync

Immediately after subscribing, the server sends a `sync` event with the
complete current screen state:

```json
{
  "event": "sync",
  "seq": 0,
  "screen": {
    "epoch": 42,
    "first_line_index": 0,
    "total_lines": 24,
    "lines": ["$ "],
    "cursor": {"row": 0, "col": 2, "visible": true},
    "cols": 80,
    "rows": 24,
    "alternate_active": false
  },
  "scrollback_lines": 150
}
```

Use this to initialize your local state before processing incremental events.

### Step 4: Receive Events

Events arrive as JSON text frames. Every event has an `event` field
(discriminator) and a `seq` field (monotonically increasing sequence number).

---

## WebSocket Methods

All methods can be called at any time after receiving `{"connected": true}`.
Subscribe is not required first.

### `subscribe`

Start receiving real-time events. See Step 2 above for full details.

**Params:** `events` (required), `interval_ms`, `format`

**Result:** `{"events": ["lines", "cursor"]}`

### `get_screen`

Get the current visible screen. Same response shape as `GET /screen`.

**Params:** `format` (`"plain"` | `"styled"`, default `"styled"`)

```json
{"id": 1, "method": "get_screen", "params": {"format": "styled"}}
```

**Result:**

```json
{"id": 1, "method": "get_screen", "result": {"epoch": 42, "lines": [...], "cursor": {...}, "cols": 80, "rows": 24, "alternate_active": false, ...}}
```

### `get_scrollback`

Get scrollback buffer contents. Same response shape as `GET /scrollback`.

**Params:** `format` (default `"styled"`), `offset` (default `0`), `limit` (default `100`)

```json
{"id": 2, "method": "get_scrollback", "params": {"format": "plain", "offset": 0, "limit": 50}}
```

**Result:**

```json
{"id": 2, "method": "get_scrollback", "result": {"epoch": 42, "lines": [...], "total_lines": 500, "offset": 0}}
```

### `send_input`

Inject bytes into the terminal's PTY.

**Params:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `data` | string | (required) | The data to send |
| `encoding` | `"utf8"` \| `"base64"` | `"utf8"` | How `data` is encoded |

```json
{"id": 3, "method": "send_input", "params": {"data": "ls\n"}}
```

To send binary data (e.g., Ctrl+C):

```json
{"id": 4, "method": "send_input", "params": {"data": "Aw==", "encoding": "base64"}}
```

**Result:** `{}`

### `get_input_mode`

Get the current input mode.

```json
{"id": 5, "method": "get_input_mode"}
```

**Result:** `{"mode": "passthrough"}` or `{"mode": "capture"}`

### `capture_input`

Switch to capture mode. Keyboard input is intercepted and broadcast as events
instead of being sent to the PTY.

```json
{"id": 6, "method": "capture_input"}
```

**Result:** `{}`

### `release_input`

Switch back to passthrough mode. Keyboard input goes to the PTY normally.

```json
{"id": 7, "method": "release_input"}
```

**Result:** `{}`

### `await_quiesce`

Wait for the terminal to become quiescent (no activity for the specified
duration), then return a screen state snapshot. The connection remains
responsive while waiting — other events and method calls continue normally.

**Params:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `timeout_ms` | integer | (required) | Quiescence threshold in milliseconds |
| `format` | `"plain"` \| `"styled"` | `"styled"` | Line format for screen snapshot |
| `max_wait_ms` | integer | (none) | Overall deadline; omit for no deadline |
| `last_generation` | integer | (none) | Generation from a previous response; if it matches current state, waits for new activity first |
| `fresh` | boolean | `false` | Always observe real silence for `timeout_ms` before responding |

```json
{"id": 8, "method": "await_quiesce", "params": {"timeout_ms": 2000, "format": "plain"}}
```

**Result:**

```json
{"screen": { ... }, "scrollback_lines": 150, "generation": 42}
```

The `generation` field is a monotonic counter that increments on each activity
event. Pass it back as `last_generation` on subsequent requests to prevent
busy-loop storms when the terminal is already idle:

```json
{"id": 9, "method": "await_quiesce", "params": {"timeout_ms": 2000, "last_generation": 42}}
```

Alternatively, set `fresh: true` to always observe real silence without tracking
generation state — at the cost of always waiting at least `timeout_ms`:

```json
{"id": 10, "method": "await_quiesce", "params": {"timeout_ms": 2000, "fresh": true}}
```

**Error (on timeout):**

```json
{"error": {"code": "quiesce_timeout", "message": "Terminal did not become quiescent within the deadline."}}
```

A new `await_quiesce` request replaces any pending one. Only one can be active
at a time per connection.

### Quiescence Sync Subscription

When `quiesce_ms > 0` is passed to `subscribe`, the server automatically emits
a `sync` event whenever the terminal has been idle for that duration after any
activity. This provides a continuous "command finished" signal without polling.

```json
{"method": "subscribe", "params": {"events": ["lines"], "quiesce_ms": 2000}}
```

Each time the terminal goes quiet for 2 seconds, you receive:

```json
{"event": "sync", "seq": 0, "screen": { ... }, "scrollback_lines": 150}
```

The quiescence subscription is reset on re-subscribe. Set `quiesce_ms` to `0`
(or omit it) to disable.

### `create_overlay`

Create a positioned text overlay on the terminal.

**Params:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `x` | integer | yes | Column position |
| `y` | integer | yes | Row position |
| `z` | integer | no | Z-order (stacking) |
| `spans` | array | yes | Array of span objects (see overlay docs) |

```json
{"id": 10, "method": "create_overlay", "params": {"x": 60, "y": 0, "z": 100, "spans": [{"text": "Status: OK", "fg": "green"}]}}
```

**Result:** `{"id": "overlay-uuid"}`

### `list_overlays`

List all active overlays.

```json
{"id": 11, "method": "list_overlays"}
```

**Result:** Array of overlay objects.

### `get_overlay`

Get a single overlay by id.

**Params:** `id` (string, required)

```json
{"id": 12, "method": "get_overlay", "params": {"id": "overlay-uuid"}}
```

**Result:** Overlay object.

### `update_overlay`

Replace an overlay's spans.

**Params:** `id` (string, required), `spans` (array, required)

```json
{"id": 13, "method": "update_overlay", "params": {"id": "overlay-uuid", "spans": [{"text": "Updated", "bold": true}]}}
```

**Result:** `{}`

### `patch_overlay`

Move or reorder an overlay without replacing its content.

**Params:** `id` (string, required), `x` (integer, optional), `y` (integer, optional), `z` (integer, optional)

```json
{"id": 14, "method": "patch_overlay", "params": {"id": "overlay-uuid", "x": 0, "y": 23}}
```

**Result:** `{}`

### `delete_overlay`

Delete an overlay.

**Params:** `id` (string, required)

```json
{"id": 15, "method": "delete_overlay", "params": {"id": "overlay-uuid"}}
```

**Result:** `{}`

### `clear_overlays`

Delete all overlays.

```json
{"id": 16, "method": "clear_overlays"}
```

**Result:** `{}`

### `create_panel`

Create a panel (agent-owned screen region that shrinks the PTY).

**Params:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `position` | `"top"` \| `"bottom"` | yes | Edge of the terminal |
| `height` | integer | yes | Number of rows |
| `z` | integer | no | Z-order (auto-assigned if omitted) |
| `spans` | array | no | Array of span objects (default: empty) |

```json
{"id": 20, "method": "create_panel", "params": {"position": "bottom", "height": 1, "spans": [{"text": "Ready", "fg": "green"}]}}
```

**Result:** `{"id": "panel-uuid"}`

### `list_panels`

List all active panels.

```json
{"id": 21, "method": "list_panels"}
```

**Result:** Array of panel objects.

### `get_panel`

Get a single panel by id.

**Params:** `id` (string, required)

```json
{"id": 22, "method": "get_panel", "params": {"id": "panel-uuid"}}
```

**Result:** Panel object.

### `update_panel`

Fully replace a panel's properties.

**Params:** `id` (string, required), `position` (string, required), `height` (integer, required), `z` (integer, required), `spans` (array, required)

```json
{"id": 23, "method": "update_panel", "params": {"id": "panel-uuid", "position": "bottom", "height": 1, "z": 10, "spans": [{"text": "Updated"}]}}
```

**Result:** `{}`

### `patch_panel`

Partially update a panel. Only provided fields are changed.

**Params:** `id` (string, required), `position` (string, optional), `height` (integer, optional), `z` (integer, optional), `spans` (array, optional)

```json
{"id": 24, "method": "patch_panel", "params": {"id": "panel-uuid", "spans": [{"text": "Patched", "bold": true}]}}
```

**Result:** `{}`

### `delete_panel`

Delete a panel.

**Params:** `id` (string, required)

```json
{"id": 25, "method": "delete_panel", "params": {"id": "panel-uuid"}}
```

**Result:** `{}`

### `clear_panels`

Delete all panels.

```json
{"id": 26, "method": "clear_panels"}
```

**Result:** `{}`

### `update_overlay_spans`

Partial update of overlay spans by ID. Only spans with a matching `id` are
updated; unmatched spans are left unchanged.

**Params:** `id` (string, required), `spans` (array, required)

```json
{"id": 30, "method": "update_overlay_spans", "params": {"id": "overlay-uuid", "spans": [{"id": "value", "text": "Updated", "fg": "green"}]}}
```

**Result:** `{}`

### `overlay_region_write`

Write styled text at specific (row, col) positions within an overlay.

**Params:** `id` (string, required), `writes` (array, required)

```json
{"id": 31, "method": "overlay_region_write", "params": {"id": "overlay-uuid", "writes": [{"row": 0, "col": 0, "text": "X", "fg": "red"}]}}
```

**Result:** `{}`

### `update_panel_spans`

Partial update of panel spans by ID. Only spans with a matching `id` are
updated; unmatched spans are left unchanged.

**Params:** `id` (string, required), `spans` (array, required)

```json
{"id": 32, "method": "update_panel_spans", "params": {"id": "panel-uuid", "spans": [{"id": "value", "text": "Updated", "fg": "green"}]}}
```

**Result:** `{}`

### `panel_region_write`

Write styled text at specific (row, col) positions within a panel.

**Params:** `id` (string, required), `writes` (array, required)

```json
{"id": 33, "method": "panel_region_write", "params": {"id": "panel-uuid", "writes": [{"row": 0, "col": 0, "text": "X", "fg": "red"}]}}
```

**Result:** `{}`

### `batch_update`

Atomically update both spans and region writes on an overlay or panel in a
single call. Useful for reducing round trips when updating both at once.

**Params:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | yes | Overlay or panel ID |
| `type` | `"overlay"` \| `"panel"` | yes | Target element type |
| `spans` | array | no | Spans to update by ID (same as `update_*_spans`) |
| `writes` | array | no | Region writes (same as `*_region_write`) |

```json
{"id": 34, "method": "batch_update", "params": {"id": "overlay-uuid", "type": "overlay", "spans": [{"id": "value", "text": "OK"}], "writes": [{"row": 0, "col": 0, "text": "X"}]}}
```

**Result:** `{}`

### `focus`

Set input focus to a focusable overlay or panel.

**Params:** `id` (string, required)

```json
{"id": 35, "method": "focus", "params": {"id": "overlay-uuid"}}
```

**Result:** `{}`

**Errors:** `invalid_request` if element not found, `not_focusable` if element
is not focusable.

### `unfocus`

Clear input focus.

```json
{"id": 36, "method": "unfocus"}
```

**Result:** `{}`

### `get_focus`

Get the currently focused element's ID.

```json
{"id": 37, "method": "get_focus"}
```

**Result:**

```json
{"focused": "overlay-uuid"}
```

or `{"focused": null}` when nothing is focused.

### `get_screen_mode`

Get the session's current screen mode.

```json
{"id": 38, "method": "get_screen_mode"}
```

**Result:**

```json
{"mode": "normal"}
```

### `enter_alt_screen`

Switch to alternate screen mode.

```json
{"id": 39, "method": "enter_alt_screen"}
```

**Result:** `{}`

**Error:** `already_in_alt_screen` if already in alt mode.

### `exit_alt_screen`

Switch back to normal screen mode. Deletes all alt-mode overlays and panels.

```json
{"id": 40, "method": "exit_alt_screen"}
```

**Result:** `{}`

**Error:** `not_in_alt_screen` if already in normal mode.

---

## Error Responses

Method errors include the request `id` and `method`:

```json
{"id": 3, "method": "get_overlay", "error": {"code": "overlay_not_found", "message": "No overlay exists with id 'abc-123'."}}
```

Protocol errors (malformed request) omit `id` and `method`:

```json
{"error": {"code": "invalid_request", "message": "Invalid JSON or missing 'method' field."}}
```

See [errors.md](errors.md) for the full error code reference.

---

## Event Types

### `line`

A single line was updated.

```json
{
  "event": "line",
  "seq": 5,
  "index": 3,
  "total_lines": 24,
  "line": [
    {"text": "$ ", "bold": true},
    {"text": "ls"}
  ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `index` | integer | Line number (0-based from top of visible screen) |
| `total_lines` | integer | Total lines in the terminal |
| `line` | FormattedLine | The line content (string or array of spans) |

### `cursor`

Cursor position changed.

```json
{
  "event": "cursor",
  "seq": 6,
  "row": 0,
  "col": 5,
  "visible": true
}
```

### `mode`

Terminal switched between normal and alternate screen buffer.

```json
{
  "event": "mode",
  "seq": 7,
  "alternate_active": true
}
```

When `alternate_active` is `true`, a full-screen TUI (vim, htop, etc.) is
running. When `false`, the terminal is in normal scrollback mode.

### `reset`

Terminal state was reset. Clients should re-fetch full state.

```json
{
  "event": "reset",
  "seq": 8,
  "reason": "clear_screen"
}
```

**Reset reasons:**

| Reason | Description |
|--------|-------------|
| `clear_screen` | Screen was cleared (Ctrl+L or `\e[2J`) |
| `clear_scrollback` | Scrollback buffer was cleared |
| `hard_reset` | Full terminal reset |
| `alternate_screen_enter` | Entered alternate screen buffer |
| `alternate_screen_exit` | Exited alternate screen buffer |
| `resize` | Terminal was resized |

### `sync`

Full screen state snapshot. Sent on initial connection and after resets.

```json
{
  "event": "sync",
  "seq": 9,
  "screen": { ... },
  "scrollback_lines": 150
}
```

The `screen` object has the same shape as the `GET /screen` response.

### `diff`

Batched screen update with changed line indices and full screen state.

```json
{
  "event": "diff",
  "seq": 10,
  "changed_lines": [0, 1, 23],
  "screen": { ... }
}
```

`changed_lines` lists the indices of lines that changed since the last diff.
The `screen` object contains the complete current screen.

### Input Events

When subscribed to `input` events, you receive keyboard input as it arrives.
These are broadcast from the input event system.

**Input event (keystroke):**

```json
{
  "event": "input",
  "mode": "passthrough",
  "raw": [27, 91, 65],
  "parsed": {
    "key": "ArrowUp",
    "modifiers": []
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `mode` | `"passthrough"` \| `"capture"` | Current input mode |
| `raw` | array of integers | Raw bytes of the input |
| `parsed` | object \| null | Parsed key if recognized |
| `parsed.key` | string \| null | Key name |
| `parsed.modifiers` | array of strings | Active modifiers (e.g., `["ctrl"]`) |

**Mode change event:**

```json
{
  "event": "mode",
  "mode": "capture"
}
```

Sent when the input mode changes between `passthrough` and `capture`.

---

## Server-Level WebSocket

In server mode, the top-level `/ws/json` endpoint (not nested under
`/sessions/:name`) provides a multiplexed WebSocket that can interact with
any session and receive session lifecycle events.

### Session Field

Per-session methods require a `session` field in the request to identify the
target session:

```json
{"id": 1, "method": "get_screen", "session": "dev", "params": {"format": "styled"}}
```

Server-level methods do not require the `session` field.

### Server-Level Methods

#### `list_sessions`

List all active sessions.

```json
{"id": 1, "method": "list_sessions"}
```

**Result:**

```json
{"id": 1, "method": "list_sessions", "result": [{"name": "dev", "pid": 12345, "command": "/bin/bash", "rows": 24, "cols": 80, "clients": 1}, {"name": "build", "pid": 12346, "command": "/bin/bash", "rows": 24, "cols": 80, "clients": 0}]}
```

#### `create_session`

Create a new session.

**Params:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | no | Session name (auto-generated if omitted) |
| `command` | string | no | Command to run (defaults to user's shell) |
| `rows` | integer | no | Terminal rows (default: 24) |
| `cols` | integer | no | Terminal columns (default: 80) |
| `cwd` | string | no | Working directory |
| `env` | object | no | Additional environment variables |

```json
{"id": 2, "method": "create_session", "params": {"name": "dev", "command": "bash", "cwd": "/home/user/project"}}
```

**Result:**

```json
{"id": 2, "method": "create_session", "result": {"name": "dev", "pid": 12345, "command": "/bin/bash", "rows": 24, "cols": 80, "clients": 0}}
```

#### `kill_session`

Destroy a session.

**Params:** `name` (string, required)

```json
{"id": 3, "method": "kill_session", "params": {"name": "dev"}}
```

**Result:** `{}`

#### `detach_session`

Detach all connected clients from a session. The session remains alive.

**Params:** `name` (string, required)

```json
{"id": 5, "method": "detach_session", "params": {"name": "dev"}}
```

**Result:** `{}`

**Errors:** `session_not_found` if the session doesn't exist.

#### `rename_session`

Rename an existing session.

**Params:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Current session name |
| `new_name` | string | yes | New session name |

```json
{"id": 4, "method": "rename_session", "params": {"name": "dev", "new_name": "prod"}}
```

**Result:**

```json
{"id": 4, "method": "rename_session", "result": {"name": "prod"}}
```

**Errors:** `session_not_found` if the session doesn't exist, `session_name_conflict` if the new name is already taken.

#### `set_server_mode`

Set the server's persistence mode.

**Params:**

| Param | Type | Description |
|-------|------|-------------|
| `persistent` | boolean | `true` for persistent, `false` for ephemeral |

```json
{"id": 4, "method": "set_server_mode", "params": {"persistent": true}}
```

**Result:**

```json
{"id": 4, "method": "set_server_mode", "result": {"persistent": true}}
```

### Session Lifecycle Events

The server-level WebSocket automatically broadcasts session lifecycle events:

**Session created:**

```json
{"event": "session_created", "params": {"name": "dev"}}
```

**Session renamed:**

```json
{"event": "session_renamed", "params": {"old_name": "dev", "new_name": "prod"}}
```

**Session destroyed** (killed via API, or PTY process exited):

```json
{"event": "session_destroyed", "params": {"name": "dev"}}
```

### Per-Session Subscriptions

On the server-level WebSocket, `subscribe` requires a `session` field to
specify which session to subscribe to. You can subscribe to multiple sessions
by sending multiple subscribe requests with different session names.

```json
{"id": 10, "method": "subscribe", "session": "dev", "params": {"events": ["lines", "cursor"]}}
```

---

## Graceful Shutdown

When wsh shuts down, it sends a WebSocket close frame with code `1000`
(normal closure) and reason `"server shutting down"` before terminating
the connection.

## Reconnection

wsh sessions are stateful on the server side but stateless on the client side.
If your WebSocket disconnects:

1. Reconnect to the same endpoint
2. The server sends `{"connected": true}` again
3. Re-send your subscribe message (or any method calls you need)
4. The server sends a fresh `sync` event with current state

No state is lost. The terminal session continues regardless of client
connections.
