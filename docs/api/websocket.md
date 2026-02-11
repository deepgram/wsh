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

**Result:**

```json
{"epoch": 42, "lines": [...], "cursor": {...}, "cols": 80, "rows": 24, "alternate_active": false, ...}
```

### `get_scrollback`

Get scrollback buffer contents. Same response shape as `GET /scrollback`.

**Params:** `format` (default `"styled"`), `offset` (default `0`), `limit` (default `100`)

**Result:**

```json
{"epoch": 42, "lines": [...], "total_lines": 500, "offset": 0}
```

### `send_input`

Inject bytes into the terminal's PTY.

**Params:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `data` | string | (required) | The data to send |
| `encoding` | `"utf8"` \| `"base64"` | `"utf8"` | How `data` is encoded |

**Result:** `{}`

### `get_input_mode`

Get the current input mode.

**Result:** `{"mode": "passthrough"}` or `{"mode": "capture"}`

### `capture_input`

Switch to capture mode. Keyboard input is intercepted and broadcast as events
instead of being sent to the PTY.

**Result:** `{}`

### `release_input`

Switch back to passthrough mode. Keyboard input goes to the PTY normally.

**Result:** `{}`

### `create_overlay`

Create a positioned text overlay on the terminal.

**Params:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `x` | integer | yes | Column position |
| `y` | integer | yes | Row position |
| `z` | integer | no | Z-order (stacking) |
| `spans` | array | yes | Array of span objects (see overlay docs) |

**Result:** `{"id": "overlay-uuid"}`

### `list_overlays`

List all active overlays.

**Result:** Array of overlay objects.

### `get_overlay`

Get a single overlay by id.

**Params:** `id` (string, required)

**Result:** Overlay object.

### `update_overlay`

Replace an overlay's spans.

**Params:** `id` (string, required), `spans` (array, required)

**Result:** `{}`

### `patch_overlay`

Move or reorder an overlay without replacing its content.

**Params:** `id` (string, required), `x` (integer, optional), `y` (integer, optional), `z` (integer, optional)

**Result:** `{}`

### `delete_overlay`

Delete an overlay.

**Params:** `id` (string, required)

**Result:** `{}`

### `clear_overlays`

Delete all overlays.

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

**Result:** `{"id": "panel-uuid"}`

### `list_panels`

List all active panels.

**Result:** Array of panel objects.

### `get_panel`

Get a single panel by id.

**Params:** `id` (string, required)

**Result:** Panel object.

### `update_panel`

Fully replace a panel's properties.

**Params:** `id` (string, required), `position` (string, required), `height` (integer, required), `z` (integer, required), `spans` (array, required)

**Result:** `{}`

### `patch_panel`

Partially update a panel. Only provided fields are changed.

**Params:** `id` (string, required), `position` (string, optional), `height` (integer, optional), `z` (integer, optional), `spans` (array, optional)

**Result:** `{}`

### `delete_panel`

Delete a panel.

**Params:** `id` (string, required)

**Result:** `{}`

### `clear_panels`

Delete all panels.

**Result:** `{}`

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
