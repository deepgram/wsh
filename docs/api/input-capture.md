# Input Capture

Input capture lets API clients intercept keyboard input before it reaches the
terminal's PTY. This enables building custom key handlers, approval workflows,
and agent-driven interactions.

## Input Modes

wsh has two input modes:

| Mode | Behavior |
|------|----------|
| `passthrough` (default) | Input goes to both API subscribers and the PTY |
| `capture` | Input goes only to API subscribers; the PTY receives nothing |

In both modes, subscribers on the JSON WebSocket (subscribed to `input` events)
receive every keystroke. The difference is whether the PTY also gets the input.

## Checking the Current Mode

```
GET /input/mode
```

**Response:**

```json
{"mode": "passthrough"}
```

**Example:**

```bash
curl http://localhost:8080/input/mode
```

## Switching to Capture Mode

```
POST /input/capture?owner=<owner-id>
```

**Response:** `204 No Content`

After this call, keyboard input from the local terminal is broadcast to API
subscribers but **not** forwarded to the PTY. The terminal program (shell,
vim, etc.) sees no input until you release.

**Query parameters:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `owner` | string | (auto-generated UUID) | Identifier for the capturing client |

The `owner` parameter prevents one client from accidentally releasing another
client's capture. If omitted, a random UUID is assigned. If another owner
already holds the capture, the request returns `409 Conflict` with error code
`input_capture_failed`.

Calling capture multiple times with the **same** owner is idempotent.

**Example:**

```bash
curl -X POST 'http://localhost:8080/sessions/default/input/capture?owner=my-agent'
```

**Error (already captured by another owner):**

```json
{"error": {"code": "input_capture_failed", "message": "Input capture failed: input already captured by other-agent."}}
```

## Releasing Back to Passthrough

```
POST /input/release?owner=<owner-id>
```

**Response:** `204 No Content`

Restores normal input flow. The PTY receives keystrokes again.

**Query parameters:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `owner` | string | (auto-generated UUID) | Must match the owner that captured |

Only the owner that captured input can release it. If the caller is not the
current owner, the request returns `409 Conflict`.

**Example:**

```bash
curl -X POST 'http://localhost:8080/sessions/default/input/release?owner=my-agent'
```

## Subscribing to Input Events

To receive input events, connect to the JSON WebSocket and include `"input"`
in your subscription:

```json
{"id": 1, "method": "subscribe", "params": {"events": ["input"]}}
```

You will then receive events for every keystroke:

```json
{
  "event": "input",
  "mode": "capture",
  "raw": [97],
  "parsed": {
    "key": "a",
    "modifiers": []
  }
}
```

And mode change notifications:

```json
{
  "event": "mode",
  "mode": "passthrough"
}
```

### Parsed Keys

The `parsed` field attempts to identify the key from raw bytes:

| Input | `key` | `modifiers` |
|-------|-------|-------------|
| `a` | `"a"` | `[]` |
| Ctrl+C | `"c"` | `["ctrl"]` |
| Escape | `"Escape"` | `[]` |
| Enter | `"Enter"` | `[]` |
| Tab | `"Tab"` | `[]` |
| Backspace | `"Backspace"` | `[]` |
| Arrow Up | `"ArrowUp"` | `[]` |
| Arrow Down | `"ArrowDown"` | `[]` |
| Arrow Left | `"ArrowLeft"` | `[]` |
| Arrow Right | `"ArrowRight"` | `[]` |
| Home | `"Home"` | `[]` |
| End | `"End"` | `[]` |
| Ctrl+A..Z | letter | `["ctrl"]` |
| Unknown | `null` | `[]` |

When the key cannot be identified (multi-byte sequences, non-standard
terminals), `parsed` is `null` and you can fall back to inspecting `raw`.

## Keyboard Toggle

The `Ctrl+\` key combination **toggles** input capture mode. Pressing it
switches from passthrough to capture, or from capture back to passthrough.
`Ctrl+\` is never forwarded to the PTY — it is always consumed by wsh.

This gives the user a physical escape hatch: if an agent has captured input
and become unresponsive, the user presses `Ctrl+\` to regain control. It also
gives agents a signal when the user *wants* to interact — entering capture mode
manually is a hint that the user is seeking agent attention.

In server mode (attached client), double-tapping `Ctrl+\` detaches from the
session. Each press still toggles capture mode on the server (so two rapid
presses cancel out, leaving capture mode unchanged after re-attach).

## Example: Approval Workflow

An agent watching a terminal session can use input capture to intercept
user responses:

```python
import websockets
import json
import requests

BASE = "http://localhost:8080"

async def approval_flow():
    # 1. Capture input
    requests.post(f"{BASE}/input/capture")

    # 2. Show overlay asking for approval
    resp = requests.post(f"{BASE}/overlay", json={
        "x": 0, "y": 23, "z": 999,
        "spans": [
            {"text": " Approve? [y/n] ", "bg": "yellow", "bold": True}
        ]
    })
    overlay_id = resp.json()["id"]

    # 3. Listen for input
    async with websockets.connect(f"ws://localhost:8080/ws/json") as ws:
        await ws.recv()  # {"connected": true}
        await ws.send(json.dumps({"events": ["input"]}))
        await ws.recv()  # {"subscribed": [...]}
        await ws.recv()  # sync event

        while True:
            msg = json.loads(await ws.recv())
            if msg.get("event") == "input" and msg.get("parsed"):
                key = msg["parsed"]["key"]
                if key == "y":
                    print("Approved!")
                    break
                elif key == "n":
                    print("Denied!")
                    break

    # 4. Clean up
    requests.delete(f"{BASE}/overlay/{overlay_id}")
    requests.post(f"{BASE}/input/release")
```

## Focus Tracking

Focus tracking lets API clients direct captured input to a specific overlay or
panel. At most one element has focus at a time.

### Set Focus

```
POST /input/focus
Content-Type: application/json
```

**Request body:**

```json
{"id": "overlay-or-panel-uuid"}
```

Sets input focus to the specified overlay or panel. The element must exist and
have `focusable: true`.

**Response:** `204 No Content`

**Errors:**

| Status | Code | When |
|--------|------|------|
| 400 | `invalid_request` | No overlay or panel with that ID |
| 400 | `not_focusable` | Element exists but `focusable` is false |

**Example:**

```bash
curl -X POST http://localhost:8080/input/focus \
  -H 'Content-Type: application/json' \
  -d '{"id": "f47ac10b-58cc-4372-a567-0e02b2c3d479"}'
```

### Remove Focus

```
POST /input/unfocus
```

Clears input focus. No element receives directed input.

**Response:** `204 No Content`

**Example:**

```bash
curl -X POST http://localhost:8080/input/unfocus
```

### Get Current Focus

```
GET /input/focus
```

Returns the ID of the currently focused element, or `null` if nothing has focus.

**Response:** `200 OK`

```json
{"focused": "f47ac10b-58cc-4372-a567-0e02b2c3d479"}
```

or when nothing is focused:

```json
{"focused": null}
```

**Example:**

```bash
curl http://localhost:8080/input/focus
```

### Focus Auto-Clear

Focus is automatically cleared when:

- Input mode is released (`POST /input/release`) -- returning to passthrough
  mode clears focus
- The focused element is deleted -- deleting an overlay or panel that has focus
  clears focus
- All overlays or panels are cleared (`DELETE /overlay`, `DELETE /panel`)

## Notes

- Input injected via `POST /input` always reaches the PTY regardless of input
  mode. Capture mode only affects keyboard input from the local terminal.
- State is shared across all API clients. If one client captures input, it
  affects all clients and the local terminal.
- Mode changes are broadcast to all WebSocket subscribers watching `input`
  events.
- Focus requires an overlay or panel with `focusable: true`. Non-focusable
  elements cannot receive focus.
- **Ownership:** Each capture has an owner. Only the owner can release it.
  WebSocket clients use their connection ID as the owner automatically. HTTP
  clients should pass `?owner=` explicitly to maintain consistent identity
  across requests.
- **Auto-release on disconnect:** When a WebSocket client disconnects, its
  input capture is automatically released if it was the owner. This prevents
  orphaned captures from blocking the terminal.
- **Ctrl+\\ override:** The local keyboard toggle (`Ctrl+\\`) uses the owner
  `"local"` and can always toggle capture mode regardless of which API client
  owns the capture.
