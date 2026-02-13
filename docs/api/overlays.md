# Overlays

Overlays are positioned text elements rendered on top of terminal content.
They are useful for status bars, notifications, debug info, and agent-driven
UI elements that shouldn't interfere with the terminal's own output.

## Concepts

An overlay has:

- **Position** (`x`, `y`): Column and row on the terminal grid (0-based)
- **Size** (`width`, `height`): Dimensions of the overlay's bounding rectangle
- **Z-order** (`z`): Stacking order when overlays overlap (higher = on top)
- **Background** (`background`): Optional fill color for the bounding rectangle
- **Spans**: One or more styled text segments, optionally named with `id`
- **Region writes**: Freeform styled text placed at specific (row, col) offsets
- **Focusable** (`focusable`): Whether the overlay can receive input focus
- **Screen mode** (`screen_mode`): Which screen mode the overlay belongs to (informational, auto-set at creation)
- **ID**: A unique identifier assigned on creation

Overlays exist independently of terminal content. They persist across screen
updates and are not affected by scrolling or screen clearing.

### Screen Mode

Every overlay is tagged with a `screen_mode` (`"normal"` or `"alt"`) at
creation time, matching the session's current screen mode. Overlays are only
returned by list endpoints when their mode matches the session's current mode.
When the session exits alt screen mode, all alt-mode overlays are deleted.
See [alt-screen.md](alt-screen.md) for details.

## Create an Overlay

```
POST /overlay
Content-Type: application/json
```

**Request body:**

```json
{
  "x": 10,
  "y": 0,
  "z": 100,
  "width": 30,
  "height": 3,
  "background": {"bg": "blue"},
  "spans": [
    {"id": "label", "text": "Status: ", "bold": true},
    {"id": "value", "text": "OK", "fg": "green"}
  ],
  "focusable": true
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `x` | integer | yes | Column position (0-based) |
| `y` | integer | yes | Row position (0-based) |
| `z` | integer | no | Z-order (default: auto-assigned) |
| `width` | integer | yes | Width in columns |
| `height` | integer | yes | Height in rows |
| `background` | BackgroundStyle | no | Background fill for the bounding rectangle |
| `spans` | array | yes | Styled text spans |
| `focusable` | boolean | no | Whether the overlay can receive input focus (default: false) |

**Response:** `201 Created`

```json
{"id": "f47ac10b-58cc-4372-a567-0e02b2c3d479"}
```

**Example:**

```bash
curl -X POST http://localhost:8080/overlay \
  -H 'Content-Type: application/json' \
  -d '{"x": 10, "y": 0, "z": 100, "width": 30, "height": 3, "background": {"bg": "blue"}, "spans": [{"text": "Status: OK", "fg": "green"}]}'
```

## List Overlays

```
GET /overlay
```

Returns overlays filtered by the session's current screen mode.

**Response:** `200 OK`

```json
[
  {
    "id": "f47ac10b-58cc-4372-a567-0e02b2c3d479",
    "x": 10,
    "y": 0,
    "z": 100,
    "width": 30,
    "height": 3,
    "background": {"bg": "blue"},
    "spans": [
      {"id": "label", "text": "Status: ", "bold": true},
      {"id": "value", "text": "OK", "fg": "green"}
    ],
    "focusable": true
  }
]
```

Note: `region_writes` is omitted when empty. `screen_mode` is omitted when
`"normal"` (it only appears in responses for alt-mode elements).

**Example:**

```bash
curl http://localhost:8080/overlay
```

## Get a Single Overlay

```
GET /overlay/:id
```

**Response:** `200 OK` with the overlay object.

**Error:** `404` with code `overlay_not_found` if the ID doesn't exist.

**Example:**

```bash
curl http://localhost:8080/overlay/f47ac10b-58cc-4372-a567-0e02b2c3d479
```

## Update Overlay Spans

```
PUT /overlay/:id
Content-Type: application/json
```

Replaces the overlay's spans while keeping its position, size, and z-order.

**Request body:**

```json
{
  "spans": [
    {"text": "Status: ", "bold": true},
    {"text": "Error", "fg": "red"}
  ]
}
```

**Response:** `204 No Content`

**Error:** `404` with code `overlay_not_found` if the ID doesn't exist.

**Example:**

```bash
curl -X PUT http://localhost:8080/overlay/f47ac10b-58cc-4372-a567-0e02b2c3d479 \
  -H 'Content-Type: application/json' \
  -d '{"spans": [{"text": "Status: ", "bold": true}, {"text": "Error", "fg": "red"}]}'
```

## Partial Span Update by ID

```
POST /overlay/:id/spans
Content-Type: application/json
```

Updates only the spans whose `id` matches a span in the request. Spans without
a matching `id` in the overlay are ignored. This avoids replacing the entire
span list when only one or two values change.

**Request body:**

```json
{
  "spans": [
    {"id": "value", "text": "Error", "fg": "red"}
  ]
}
```

**Response:** `204 No Content`

**Error:** `404` with code `overlay_not_found` if the ID doesn't exist.

**Example:**

```bash
curl -X POST http://localhost:8080/overlay/f47ac10b-58cc-4372-a567-0e02b2c3d479/spans \
  -H 'Content-Type: application/json' \
  -d '{"spans": [{"id": "value", "text": "Error", "fg": "red"}]}'
```

## Region Write

```
POST /overlay/:id/write
Content-Type: application/json
```

Writes styled text at specific (row, col) positions within the overlay's
bounding rectangle. Useful for charts, progress bars, and other non-linear
content. Each write replaces any existing region write at the same position.

**Request body:**

```json
{
  "writes": [
    {"row": 0, "col": 0, "text": "A", "fg": "red", "bold": true},
    {"row": 1, "col": 5, "text": "B", "fg": "blue"}
  ]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `writes` | array | yes | Array of region write objects |

Each region write object:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `row` | integer | yes | Row offset within the overlay (0-based) |
| `col` | integer | yes | Column offset within the overlay (0-based) |
| `text` | string | yes | Text to write |
| `fg` | OverlayColor | no | Foreground color |
| `bg` | OverlayColor | no | Background color |
| `bold` | boolean | no | Bold (default: false) |
| `italic` | boolean | no | Italic (default: false) |
| `underline` | boolean | no | Underline (default: false) |

**Response:** `204 No Content`

**Error:** `404` with code `overlay_not_found` if the ID doesn't exist.

**Example:**

```bash
curl -X POST http://localhost:8080/overlay/f47ac10b-58cc-4372-a567-0e02b2c3d479/write \
  -H 'Content-Type: application/json' \
  -d '{"writes": [{"row": 0, "col": 0, "text": "X", "fg": "red"}]}'
```

## Move or Reorder an Overlay

```
PATCH /overlay/:id
Content-Type: application/json
```

Updates position, size, and/or z-order without changing spans or region writes.
All fields are optional -- only provided fields are updated.

**Request body:**

```json
{
  "x": 20,
  "y": 5,
  "z": 200,
  "width": 40,
  "height": 5
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `x` | integer | no | New column position |
| `y` | integer | no | New row position |
| `z` | integer | no | New z-order |
| `width` | integer | no | New width |
| `height` | integer | no | New height |

**Response:** `204 No Content`

**Error:** `404` with code `overlay_not_found` if the ID doesn't exist.

**Example:**

```bash
curl -X PATCH http://localhost:8080/overlay/f47ac10b-58cc-4372-a567-0e02b2c3d479 \
  -H 'Content-Type: application/json' \
  -d '{"x": 20, "y": 5, "z": 200}'
```

## Delete an Overlay

```
DELETE /overlay/:id
```

**Response:** `204 No Content`

**Error:** `404` with code `overlay_not_found` if the ID doesn't exist.

If the deleted overlay had input focus, focus is automatically cleared.

**Example:**

```bash
curl -X DELETE http://localhost:8080/overlay/f47ac10b-58cc-4372-a567-0e02b2c3d479
```

## Clear All Overlays

```
DELETE /overlay
```

Removes every overlay. Focus is cleared if any overlay had focus.

**Response:** `204 No Content`

**Example:**

```bash
curl -X DELETE http://localhost:8080/overlay
```

## Overlay Spans

Each span in an overlay's `spans` array is a styled text segment:

```json
{
  "id": "label",
  "text": "Hello",
  "fg": "red",
  "bg": {"r": 0, "g": 0, "b": 0},
  "bold": true,
  "italic": false,
  "underline": false
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | no | Named identifier for partial updates via `/spans` |
| `text` | string | yes | The text content |
| `fg` | OverlayColor | no | Foreground color |
| `bg` | OverlayColor | no | Background color |
| `bold` | boolean | no | Bold (default: false) |
| `italic` | boolean | no | Italic (default: false) |
| `underline` | boolean | no | Underline (default: false) |

Boolean style fields default to `false` and are omitted from responses when
`false`. The `id` field is omitted when not set.

### Background Style

The `background` field fills the overlay's entire bounding rectangle with a
solid color before rendering spans and region writes:

```json
{"bg": "blue"}
```

or with RGB:

```json
{"bg": {"r": 30, "g": 30, "b": 30}}
```

### Overlay Colors

Overlay colors are either a named color string or an RGB object:

**Named colors:**

```json
"red"
"green"
"blue"
"yellow"
"cyan"
"magenta"
"black"
"white"
```

**RGB:**

```json
{"r": 255, "g": 128, "b": 0}
```

Note: Overlay colors differ from terminal span colors. Terminal spans use
`{"indexed": N}` or `{"rgb": {"r": N, "g": N, "b": N}}`. Overlay colors
use named strings or flat `{"r": N, "g": N, "b": N}` objects.

## Example: Agent Status Bar

```bash
# Create a status overlay at the top-right with background fill
curl -X POST http://localhost:8080/overlay \
  -H 'Content-Type: application/json' \
  -d '{
    "x": 60, "y": 0, "z": 100,
    "width": 20, "height": 1,
    "background": {"bg": "blue"},
    "spans": [
      {"id": "prefix", "text": " Agent: ", "bold": true},
      {"id": "status", "text": "watching "},
      {"id": "icon", "text": "\u2713", "fg": "green"}
    ]
  }'
# {"id":"abc123"}

# Update just the status text using partial span update
curl -X POST http://localhost:8080/overlay/abc123/spans \
  -H 'Content-Type: application/json' \
  -d '{
    "spans": [
      {"id": "status", "text": "action needed "},
      {"id": "icon", "text": "\u2717", "fg": "red"}
    ]
  }'

# Or use region writes for cell-level updates
curl -X POST http://localhost:8080/overlay/abc123/write \
  -H 'Content-Type: application/json' \
  -d '{
    "writes": [
      {"row": 0, "col": 18, "text": "!", "fg": "red", "bold": true}
    ]
  }'

# Clean up when done
curl -X DELETE http://localhost:8080/overlay/abc123
```
