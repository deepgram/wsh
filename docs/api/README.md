# wsh API Reference

wsh exposes terminal I/O via HTTP and WebSocket. This document covers every
endpoint, request format, and response shape you need to build against the API.

**Base URL:** `http://localhost:8080` (default)

## Endpoints at a Glance

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Health check (no auth) |
| `POST` | `/input` | Inject bytes into the terminal |
| `GET` | `/screen` | Current screen state |
| `GET` | `/scrollback` | Scrollback buffer contents |
| `GET` | `/ws/raw` | Raw binary WebSocket |
| `GET` | `/ws/json` | JSON event WebSocket |
| `POST` | `/overlay` | Create an overlay |
| `GET` | `/overlay` | List all overlays |
| `DELETE` | `/overlay` | Clear all overlays |
| `GET` | `/overlay/:id` | Get a single overlay |
| `PUT` | `/overlay/:id` | Replace overlay spans |
| `PATCH` | `/overlay/:id` | Move/reorder an overlay |
| `DELETE` | `/overlay/:id` | Delete an overlay |
| `POST` | `/panel` | Create a panel |
| `GET` | `/panel` | List all panels |
| `DELETE` | `/panel` | Clear all panels |
| `GET` | `/panel/:id` | Get a single panel |
| `PUT` | `/panel/:id` | Replace a panel |
| `PATCH` | `/panel/:id` | Partially update a panel |
| `DELETE` | `/panel/:id` | Delete a panel |
| `GET` | `/input/mode` | Get current input mode |
| `POST` | `/input/capture` | Switch to capture mode |
| `POST` | `/input/release` | Switch to passthrough mode |
| `GET` | `/openapi.yaml` | OpenAPI specification (no auth) |
| `GET` | `/docs` | This documentation (no auth) |

## Quick Start

```bash
# Start wsh (localhost, no auth required)
wsh

# Check health
curl http://localhost:8080/health
# {"status":"ok"}

# Get current screen contents
curl http://localhost:8080/screen
# {"epoch":1,"first_line_index":0,"total_lines":1,"lines":["$ "],"cursor":{"row":0,"col":2,"visible":true},"cols":80,"rows":24,"alternate_active":false}

# Send input (type "ls\n")
curl -X POST http://localhost:8080/input -d 'ls\n'

# Get scrollback with pagination
curl 'http://localhost:8080/scrollback?offset=0&limit=50'

# Connect to raw WebSocket (using websocat)
websocat ws://localhost:8080/ws/raw
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

## Related Documents

- [authentication.md](authentication.md) -- Auth model and token configuration
- [websocket.md](websocket.md) -- WebSocket protocol and event types
- [errors.md](errors.md) -- Error code reference
- [overlays.md](overlays.md) -- Overlay system
- [panels.md](panels.md) -- Panel system
- [input-capture.md](input-capture.md) -- Input capture mode
- [openapi.yaml](openapi.yaml) -- Machine-readable OpenAPI 3.1 spec
