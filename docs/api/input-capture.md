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
POST /input/capture
```

**Response:** `204 No Content`

After this call, keyboard input from the local terminal is broadcast to API
subscribers but **not** forwarded to the PTY. The terminal program (shell,
vim, etc.) sees no input until you release.

This is idempotent -- calling it multiple times has no additional effect.

**Example:**

```bash
curl -X POST http://localhost:8080/input/capture
```

## Releasing Back to Passthrough

```
POST /input/release
```

**Response:** `204 No Content`

Restores normal input flow. The PTY receives keystrokes again.

This is idempotent.

**Example:**

```bash
curl -X POST http://localhost:8080/input/release
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

## Escape Hatch

The `Ctrl+\` key combination is treated as an escape hatch. Even in capture
mode, it is handled specially to allow the user to regain control if an API
client has captured input and become unresponsive.

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
