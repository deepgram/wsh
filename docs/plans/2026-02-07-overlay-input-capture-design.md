# Overlay & Input Capture System

> Enable API clients (agents, external tools) to display visual overlays on the terminal and optionally capture user input for sidebar interactions—without the underlying program knowing.

## Overview

### Core Principles

- **Pure compositing**: Overlays render on top of terminal content. The inner PTY is never resized and never knows overlays exist.
- **Stateful with IDs**: Each overlay has a unique ID. Clients create, update, move, and delete overlays by ID.
- **Reversible**: When an overlay is deleted, the underlying terminal content is restored by re-rendering from the parser's state.
- **Input visibility**: API subscribers can subscribe to all user input at any time—independent of capture mode. Capture mode only controls whether input *also* reaches the PTY (passthrough) or goes *exclusively* to subscribers (capture).
- **No ownership**: Any connected client can view, modify, or delete any overlay.

### Two Independent Features

1. **Overlays**: Visual content rendered on top of the terminal
2. **Input capture**: Routing user keystrokes exclusively to the API instead of both API and PTY

These features are orthogonal—you can use overlays without capturing input, capture input without overlays, or combine them for interactive agent experiences.

---

## Overlay Data Model

### Overlay Structure

```
Overlay {
  id: string          // Unique identifier, returned on create
  x: u16              // Column (0-indexed from left)
  y: u16              // Row (0-indexed from top)
  z: i32              // Layer order (higher renders on top)
  spans: Span[]       // Styled content
}

Span {
  text: string        // The text to display
  fg: Color?          // Foreground color (optional)
  bg: Color?          // Background color (optional)
  bold: bool
  italic: bool
  underline: bool
}

Color = "black" | "red" | "green" | "yellow" | "blue"
      | "magenta" | "cyan" | "white"
      | { r: u8, g: u8, b: u8 }  // True color
```

### Rendering Rules

- Overlays render at absolute screen coordinates
- Content extending beyond screen edges is clipped (no error)
- Overlays are opaque—they fully replace underlying content in their region
- Higher z-index renders on top; equal z-index uses creation order
- New overlays default to z-index higher than all existing overlays

Spans are inline—they flow left-to-right from (x, y). Newlines in span text move to the next row, same starting x column.

---

## Overlay API

### HTTP Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/overlay` | Create overlay, returns `{ id }` |
| `GET` | `/overlay` | List all overlays |
| `GET` | `/overlay/:id` | Get single overlay |
| `PUT` | `/overlay/:id` | Update overlay content |
| `PATCH` | `/overlay/:id` | Partial update (move, change z-index) |
| `DELETE` | `/overlay/:id` | Delete overlay, restore underlying content |
| `DELETE` | `/overlay` | Clear all overlays |

### Create Request

```json
{
  "x": 10,
  "y": 5,
  "z": 100,
  "spans": [
    {"text": "Warning: ", "fg": "yellow", "bold": true},
    {"text": "check your syntax"}
  ]
}
```

The `z` field is optional and defaults to top of stack.

### WebSocket Events

Broadcast to clients subscribed to `overlay` events:

```json
{"event": "overlay.created", "overlay": {...}}
{"event": "overlay.updated", "id": "...", "overlay": {...}}
{"event": "overlay.deleted", "id": "..."}
{"event": "overlay.cleared"}
```

---

## Input Capture

### Modes

- **Passthrough** (default): User input goes to both API subscribers and the PTY
- **Capture**: User input goes only to API subscribers; PTY receives nothing

### HTTP Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/input/capture` | Switch to capture mode |
| `POST` | `/input/release` | Switch to passthrough mode |
| `GET` | `/input/mode` | Get current mode |

### Escape Hatch

When in capture mode, `Ctrl+\` switches to passthrough mode without forwarding the keystroke to the PTY. In passthrough mode, `Ctrl+\` behaves normally (forwarded to PTY).

### WebSocket Input Subscription

Clients subscribe to input events via WebSocket. Events include both raw bytes and parsed representation:

```json
{
  "event": "input",
  "mode": "capture",
  "raw": [27, 91, 65],
  "parsed": {
    "key": "ArrowUp",
    "modifiers": []
  }
}
```

For unparseable sequences, `parsed` is `null` and the client uses `raw`.

### Mode Change Events

```json
{"event": "input.mode", "mode": "passthrough"}
{"event": "input.mode", "mode": "capture"}
```

---

## WebSocket Subscriptions

Subscriptions are opt-in. Clients send:

```json
{"subscribe": ["input", "overlay"]}
```

Or subscribe to everything:

```json
{"subscribe": ["*"]}
```

Available subscription types:
- `input` — keystroke events with mode and parsed key info
- `overlay` — overlay create/update/delete events
- Existing subscriptions (lines, cursor, screen, etc.) remain unchanged

---

## Rendering & Compositing

### Local Terminal (stdout)

wsh composites overlays in real-time when writing to the outer terminal:

1. PTY outputs data → parser updates terminal state
2. Forward original PTY output to stdout
3. After forwarding, render overlays:
   - Save cursor position (`\e[s`)
   - For each overlay (sorted by z-index):
     - Move cursor to overlay position (`\e[y;xH`)
     - Write styled spans (converted to ANSI SGR sequences)
   - Restore cursor position (`\e[u`)

### On Overlay Delete

When an overlay is removed, wsh re-renders the underlying region from the parser's terminal state:

1. Query parser for the cells in the overlay's region
2. Write those cells to stdout with proper positioning and styling
3. Restore cursor

### API Clients

Clients receive raw terminal state and overlay list separately:
- Terminal state: existing `lines`, `cursor`, `screen` subscriptions (unchanged)
- Overlay list: via `overlay` subscription or `GET /overlay`

Clients composite locally if they want a unified view.

---

## Edge Cases & Behavior

### Terminal Resize

- Overlays persist at their absolute coordinates
- If resize makes an overlay partially/fully off-screen, it's clipped
- API clients receive the existing resize event and can reposition overlays as needed

### Alternate Screen Mode

- Overlays render the same way—composited on top
- API clients already receive mode change events and can choose to hide/show/adjust overlays
- No special handling in wsh; the agent decides what's appropriate

### Multiple Overlays Overlapping

- Higher z-index wins
- For equal z-index, later-created overlay renders on top

### Rapid PTY Output

- Overlays are re-applied after each PTY read/forward cycle
- For performance, consider batching overlay rendering (coalesce rapid updates)

### Client Disconnect

- Overlays persist—they're owned by the session, not the client
- Reconnecting clients can query `GET /overlay` to see current state

### Empty Overlay

- Creating an overlay with empty spans is valid (no-op visually, but reserves the ID)
- Useful for "placeholder" patterns where content is added later via update

---

## Implementation Summary

### New Components

1. **Overlay store**: In-memory map of ID → Overlay, managed alongside parser state
2. **Compositing logic**: Inject overlay rendering into the PTY reader loop
3. **Restore logic**: Query parser for underlying cells, re-render region on delete
4. **Input routing**: Check mode before forwarding to PTY writer channel; always broadcast to input subscribers
5. **Escape hatch**: Intercept `Ctrl+\` in capture mode, switch to passthrough

### New API Surface

| Category | HTTP | WebSocket |
|----------|------|-----------|
| Overlays | CRUD at `/overlay` | Subscribe to `overlay` events |
| Input Capture | `/input/capture`, `/input/release`, `/input/mode` | Subscribe to `input` events |
| Subscriptions | — | `{"subscribe": ["input", "overlay", "*"]}` |

### Not In Scope

- Overlay persistence across wsh restarts
- Overlay animations or transitions
- Rich content (images, boxes, borders)—just styled text spans
- Namespacing or ownership of overlays
