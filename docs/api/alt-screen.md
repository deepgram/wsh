# Alternate Screen Mode

wsh supports an API-controlled alternate screen mode that lets agents create
temporary UI contexts. Overlays and panels created in alt screen mode are
isolated from normal-mode elements and are automatically cleaned up when
exiting alt screen.

## Concepts

### Screen Mode

A session is always in one of two screen modes:

| Mode | Description |
|------|-------------|
| `normal` | Default mode. Elements created here persist normally. |
| `alt` | Alternate mode. Elements created here are temporary. |

This is distinct from the terminal emulator's alternate screen buffer (used by
programs like vim, htop, etc.). The API-level screen mode is a separate concept
that controls which overlays and panels are visible and how they are managed.

### Element Tagging

Every overlay and panel is tagged with a `screen_mode` at creation time,
matching the session's current mode. The `screen_mode` field is informational
and read-only -- you cannot set it directly on creation. It is automatically
determined by the session's current mode.

The `screen_mode` field is omitted from JSON responses when the value is
`"normal"` (the default).

### List Filtering

List endpoints (`GET /overlay`, `GET /panel`) only return elements whose
`screen_mode` matches the session's current mode. When in normal mode, you
only see normal-mode elements. When in alt mode, you only see alt-mode
elements.

`GET /overlay/:id` and `GET /panel/:id` return any element regardless of mode.

### Exit Cleanup

Exiting alt screen mode (`POST /screen_mode/exit_alt`) deletes all overlays
and panels tagged with `screen_mode: "alt"`. The PTY reclaims any space that
was allocated to alt-mode panels.

## HTTP Endpoints

### Get Current Screen Mode

```
GET /screen_mode
```

Returns the session's current screen mode.

**Response:** `200 OK`

```json
{"mode": "normal"}
```

**Example:**

```bash
curl http://localhost:8080/screen_mode
```

### Enter Alternate Screen Mode

```
POST /screen_mode/enter_alt
```

Switches the session to alternate screen mode. New overlays and panels will be
tagged with `screen_mode: "alt"`.

**Response:** `204 No Content`

**Error:** `409` with code `already_in_alt_screen` if the session is already in
alt mode.

**Example:**

```bash
curl -X POST http://localhost:8080/screen_mode/enter_alt
```

### Exit Alternate Screen Mode

```
POST /screen_mode/exit_alt
```

Switches the session back to normal screen mode. All alt-mode overlays and
panels are deleted. Normal-mode elements become visible again in list
endpoints.

**Response:** `204 No Content`

**Error:** `409` with code `not_in_alt_screen` if the session is already in
normal mode.

**Example:**

```bash
curl -X POST http://localhost:8080/screen_mode/exit_alt
```

## WebSocket Methods

### `get_screen_mode`

Get the session's current screen mode.

```json
{"id": 1, "method": "get_screen_mode"}
```

**Result:**

```json
{"id": 1, "method": "get_screen_mode", "result": {"mode": "normal"}}
```

### `enter_alt_screen`

Switch to alternate screen mode.

```json
{"id": 2, "method": "enter_alt_screen"}
```

**Result:** `{}`

**Error:** `already_in_alt_screen` if already in alt mode.

### `exit_alt_screen`

Switch back to normal screen mode. Deletes all alt-mode elements.

```json
{"id": 3, "method": "exit_alt_screen"}
```

**Result:** `{}`

**Error:** `not_in_alt_screen` if already in normal mode.

## Example: Temporary Agent UI

```bash
# Enter alt screen mode for a temporary UI context
curl -X POST http://localhost:8080/screen_mode/enter_alt

# Create alt-mode UI elements (auto-tagged with screen_mode: "alt")
curl -X POST http://localhost:8080/panel \
  -H 'Content-Type: application/json' \
  -d '{"position": "top", "height": 1, "spans": [{"text": " Agent Menu "}], "background": {"bg": "blue"}}'

curl -X POST http://localhost:8080/overlay \
  -H 'Content-Type: application/json' \
  -d '{"x": 5, "y": 3, "width": 40, "height": 5, "background": {"bg": {"r": 30, "g": 30, "b": 30}}, "spans": [{"text": "Option 1: Build\nOption 2: Test\nOption 3: Deploy"}]}'

# ... agent interaction happens ...

# Exit alt screen -- all alt-mode elements are automatically deleted
curl -X POST http://localhost:8080/screen_mode/exit_alt
# Normal-mode overlays and panels reappear
```

## Server Mode

In server mode, the screen mode endpoints are nested under the session path:

| Method | Path |
|--------|------|
| `GET` | `/sessions/:name/screen_mode` |
| `POST` | `/sessions/:name/screen_mode/enter_alt` |
| `POST` | `/sessions/:name/screen_mode/exit_alt` |
