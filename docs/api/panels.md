# Panels

Panels are agent-owned screen regions anchored to the top or bottom of the
terminal. Unlike overlays, which draw on top of terminal content, panels cause
the PTY to shrink -- they create dedicated space that terminal output can never
write to.

## Concepts

A panel has:

- **Position** (`position`): `"top"` or `"bottom"` edge of the terminal
- **Height** (`height`): Number of rows the panel occupies
- **Z-order** (`z`): Priority when allocating space (higher = closer to the screen edge)
- **Background** (`background`): Optional fill color for the panel area
- **Spans**: One or more styled text segments (same format as overlay spans), optionally named with `id`
- **Region writes**: Freeform styled text placed at specific (row, col) offsets
- **Visible** (`visible`): Whether the panel is currently rendered
- **Focusable** (`focusable`): Whether the panel can receive input focus
- **Screen mode** (`screen_mode`): Which screen mode the panel belongs to (informational, auto-set at creation)
- **ID**: A unique identifier assigned on creation

Panels resize the PTY. When a panel is created, the terminal's usable area
shrinks by the panel's height. Programs running in the terminal see a smaller
window and reflow their output accordingly.

### Z-Order and Space Allocation

Higher z-index panels are placed closer to the edge of the screen and receive
higher priority when allocating space. If the total height of all panels exceeds
the available terminal height, the lowest z-index panels are hidden (not
rejected) to ensure at least one PTY row remains usable.

Hidden panels remain in the system. Their `visible` field is set to `false` in
API responses. If space becomes available (e.g., a higher-priority panel is
deleted), hidden panels become visible again automatically.

### Screen Mode

Every panel is tagged with a `screen_mode` (`"normal"` or `"alt"`) at creation
time, matching the session's current screen mode. Panels are only returned by
list endpoints when their mode matches the session's current mode. When the
session exits alt screen mode, all alt-mode panels are deleted and the PTY
reclaims their space. See [alt-screen.md](alt-screen.md) for details.

### Newlines in Span Text

Panel spans support newline characters (`\n`) in their text content. A panel
with `height: 2` can use a newline to place content on its second row.

## Create a Panel

```
POST /panel
Content-Type: application/json
```

**Request body:**

```json
{
  "position": "top",
  "height": 2,
  "z": 100,
  "background": {"bg": {"r": 30, "g": 30, "b": 30}},
  "spans": [
    {"id": "label", "text": "Status: ", "bold": true},
    {"id": "value", "text": "OK", "fg": "green"}
  ],
  "focusable": false
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `position` | string | yes | `"top"` or `"bottom"` |
| `height` | integer | yes | Number of rows |
| `z` | integer | no | Z-order (auto-assigned if omitted) |
| `background` | BackgroundStyle | no | Background fill for the panel area |
| `spans` | array | yes | Styled text spans |
| `focusable` | boolean | no | Whether the panel can receive input focus (default: false) |

**Response:** `201 Created`

```json
{"id": "f47ac10b-58cc-4372-a567-0e02b2c3d479"}
```

**Example:**

```bash
curl -X POST http://localhost:8080/panel \
  -H 'Content-Type: application/json' \
  -d '{"position": "top", "height": 2, "z": 100, "background": {"bg": "blue"}, "spans": [{"text": "Status: ", "bold": true}, {"text": "OK", "fg": "green"}]}'
```

## List Panels

```
GET /panel
```

Returns all panels filtered by the session's current screen mode, sorted by
position then z-order descending.

**Response:** `200 OK`

```json
[
  {
    "id": "f47ac10b-58cc-4372-a567-0e02b2c3d479",
    "position": "top",
    "height": 2,
    "z": 100,
    "background": {"bg": {"r": 30, "g": 30, "b": 30}},
    "spans": [
      {"id": "label", "text": "Status: ", "bold": true},
      {"id": "value", "text": "OK", "fg": "green"}
    ],
    "visible": true
  }
]
```

Note: `region_writes` is omitted when empty. `screen_mode` is omitted when
`"normal"` (it only appears in responses for alt-mode elements). `focusable`
is omitted when `false`.

**Example:**

```bash
curl http://localhost:8080/panel
```

## Get a Single Panel

```
GET /panel/:id
```

**Response:** `200 OK` with the panel object.

**Error:** `404` with code `panel_not_found` if the ID doesn't exist.

**Example:**

```bash
curl http://localhost:8080/panel/f47ac10b-58cc-4372-a567-0e02b2c3d479
```

## Replace a Panel

```
PUT /panel/:id
Content-Type: application/json
```

Fully replaces the panel's properties.

**Request body:**

```json
{
  "position": "top",
  "height": 2,
  "z": 100,
  "spans": [
    {"text": "Status: ", "bold": true},
    {"text": "Error", "fg": "red"}
  ]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `position` | string | yes | `"top"` or `"bottom"` |
| `height` | integer | yes | Number of rows |
| `z` | integer | yes | Z-order |
| `spans` | array | yes | Styled text spans |

**Response:** `204 No Content`

**Error:** `404` with code `panel_not_found` if the ID doesn't exist.

**Example:**

```bash
curl -X PUT http://localhost:8080/panel/f47ac10b-58cc-4372-a567-0e02b2c3d479 \
  -H 'Content-Type: application/json' \
  -d '{"position": "top", "height": 2, "z": 100, "spans": [{"text": "Status: ", "bold": true}, {"text": "Error", "fg": "red"}]}'
```

## Partially Update a Panel

```
PATCH /panel/:id
Content-Type: application/json
```

Updates any subset of panel properties without changing the rest. All fields are
optional -- only provided fields are updated.

**Request body:**

```json
{
  "height": 3,
  "background": {"bg": "blue"},
  "spans": [
    {"text": "Line 1\nLine 2\nLine 3"}
  ]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `position` | string | no | New position (`"top"` or `"bottom"`) |
| `height` | integer | no | New height in rows |
| `z` | integer | no | New z-order |
| `background` | BackgroundStyle | no | New background fill |
| `spans` | array | no | New styled text spans |

**Response:** `204 No Content`

**Error:** `404` with code `panel_not_found` if the ID doesn't exist.

**Example:**

```bash
curl -X PATCH http://localhost:8080/panel/f47ac10b-58cc-4372-a567-0e02b2c3d479 \
  -H 'Content-Type: application/json' \
  -d '{"height": 3, "spans": [{"text": "Line 1\nLine 2\nLine 3"}]}'
```

## Partial Span Update by ID

```
POST /panel/:id/spans
Content-Type: application/json
```

Updates only the spans whose `id` matches a span in the request. Spans without
a matching `id` in the panel are ignored. This avoids replacing the entire span
list when only one or two values change.

**Request body:**

```json
{
  "spans": [
    {"id": "value", "text": "Error", "fg": "red"}
  ]
}
```

**Response:** `204 No Content`

**Error:** `404` with code `panel_not_found` if the ID doesn't exist.

**Example:**

```bash
curl -X POST http://localhost:8080/panel/f47ac10b-58cc-4372-a567-0e02b2c3d479/spans \
  -H 'Content-Type: application/json' \
  -d '{"spans": [{"id": "value", "text": "Error", "fg": "red"}]}'
```

## Region Write

```
POST /panel/:id/write
Content-Type: application/json
```

Writes styled text at specific (row, col) positions within the panel. Useful
for cell-level drawing. Each write replaces any existing region write at the
same position.

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
| `row` | integer | yes | Row offset within the panel (0-based) |
| `col` | integer | yes | Column offset within the panel (0-based) |
| `text` | string | yes | Text to write |
| `fg` | OverlayColor | no | Foreground color |
| `bg` | OverlayColor | no | Background color |
| `bold` | boolean | no | Bold (default: false) |
| `italic` | boolean | no | Italic (default: false) |
| `underline` | boolean | no | Underline (default: false) |

**Response:** `204 No Content`

**Error:** `404` with code `panel_not_found` if the ID doesn't exist.

**Example:**

```bash
curl -X POST http://localhost:8080/panel/f47ac10b-58cc-4372-a567-0e02b2c3d479/write \
  -H 'Content-Type: application/json' \
  -d '{"writes": [{"row": 0, "col": 0, "text": "X", "fg": "red"}]}'
```

## Delete a Panel

```
DELETE /panel/:id
```

**Response:** `204 No Content`

**Error:** `404` with code `panel_not_found` if the ID doesn't exist.

If the deleted panel had input focus, focus is automatically cleared.

**Example:**

```bash
curl -X DELETE http://localhost:8080/panel/f47ac10b-58cc-4372-a567-0e02b2c3d479
```

## Clear All Panels

```
DELETE /panel
```

Removes every panel. The PTY reclaims the full terminal height. Focus is
cleared if any panel had focus.

**Response:** `204 No Content`

**Example:**

```bash
curl -X DELETE http://localhost:8080/panel
```

## WebSocket Methods

All panel operations are available over the `/ws/json` WebSocket endpoint using
the request/response protocol:

| Method | Description |
|--------|-------------|
| `create_panel` | Create a panel |
| `list_panels` | List all panels |
| `get_panel` | Get a single panel by ID |
| `update_panel` | Full replace of a panel |
| `patch_panel` | Partial update of a panel |
| `delete_panel` | Delete a panel by ID |
| `clear_panels` | Delete all panels |
| `update_panel_spans` | Partial span update by ID |
| `panel_region_write` | Write at specific (row, col) positions |

**Examples:**

```json
// Create a panel
{"id": 1, "method": "create_panel", "params": {"position": "bottom", "height": 1, "spans": [{"text": "Ready"}]}}
// -> {"id": 1, "method": "create_panel", "result": {"id": "panel-uuid"}}

// List all panels
{"id": 2, "method": "list_panels"}
// -> {"id": 2, "method": "list_panels", "result": [{"id": "panel-uuid", ...}]}

// Get a single panel
{"id": 3, "method": "get_panel", "params": {"id": "panel-uuid"}}
// -> {"id": 3, "method": "get_panel", "result": {"id": "panel-uuid", ...}}

// Full replace
{"id": 4, "method": "update_panel", "params": {"id": "panel-uuid", "position": "bottom", "height": 1, "z": 10, "spans": [{"text": "Updated"}]}}
// -> {"id": 4, "method": "update_panel", "result": {}}

// Partial update
{"id": 5, "method": "patch_panel", "params": {"id": "panel-uuid", "spans": [{"text": "Patched"}]}}
// -> {"id": 5, "method": "patch_panel", "result": {}}

// Partial span update by ID
{"id": 6, "method": "update_panel_spans", "params": {"id": "panel-uuid", "spans": [{"id": "value", "text": "OK"}]}}
// -> {"id": 6, "method": "update_panel_spans", "result": {}}

// Region write
{"id": 7, "method": "panel_region_write", "params": {"id": "panel-uuid", "writes": [{"row": 0, "col": 0, "text": "X"}]}}
// -> {"id": 7, "method": "panel_region_write", "result": {}}

// Delete a panel
{"id": 8, "method": "delete_panel", "params": {"id": "panel-uuid"}}
// -> {"id": 8, "method": "delete_panel", "result": {}}

// Delete all panels
{"id": 9, "method": "clear_panels"}
// -> {"id": 9, "method": "clear_panels", "result": {}}
```

## Panel Spans

Panel spans use the same format as overlay spans. Each span is a styled text
segment:

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
| `text` | string | yes | The text content (may contain `\n`) |
| `fg` | OverlayColor | no | Foreground color |
| `bg` | OverlayColor | no | Background color |
| `bold` | boolean | no | Bold (default: false) |
| `italic` | boolean | no | Italic (default: false) |
| `underline` | boolean | no | Underline (default: false) |

Boolean style fields default to `false` and are omitted from responses when
`false`. The `id` field is omitted when not set.

Colors are either a named string (`"red"`, `"green"`, `"blue"`, `"yellow"`,
`"cyan"`, `"magenta"`, `"black"`, `"white"`) or an RGB object
(`{"r": 255, "g": 128, "b": 0}`). See the [overlays documentation](overlays.md)
for full details on the OverlayColor format.

## Panels vs Overlays

Panels and overlays both render agent-controlled content on the terminal, but
they differ in a fundamental way:

- **Overlays** draw on top of terminal content. The PTY size is unchanged.
  Terminal output can flow underneath an overlay, and the overlay may obscure
  it. Overlays are positioned at arbitrary `(x, y)` coordinates on the terminal
  grid.

- **Panels** reserve space at the edge of the terminal. The PTY shrinks to
  accommodate them. Terminal output never collides with panel content because
  the terminal itself is smaller. Panels are anchored to the `top` or `bottom`
  and span the full width.

Use overlays for floating elements like tooltips, badges, and transient
notifications. Use panels for persistent chrome like status bars, toolbars, and
progress indicators that should never be obscured by terminal output.

## Example: Status Bar

```bash
# Create a two-row status bar at the top of the terminal with a dark background
curl -X POST http://localhost:8080/panel \
  -H 'Content-Type: application/json' \
  -d '{
    "position": "top",
    "height": 2,
    "z": 100,
    "background": {"bg": {"r": 30, "g": 30, "b": 30}},
    "spans": [
      {"id": "agent", "text": " Agent: ", "bg": "blue", "bold": true},
      {"id": "status", "text": "watching ", "bg": "blue"},
      {"id": "icon", "text": "OK", "fg": "green", "bg": "blue"},
      {"text": "\n"},
      {"id": "session", "text": " Session: abc123 ", "bg": "blue"}
    ]
  }'
# {"id":"abc123"}

# Update just the status and icon using partial span update
curl -X POST http://localhost:8080/panel/abc123/spans \
  -H 'Content-Type: application/json' \
  -d '{
    "spans": [
      {"id": "status", "text": "action needed ", "bg": "red"},
      {"id": "icon", "text": "!!", "fg": "white", "bg": "red"}
    ]
  }'

# Clean up when done
curl -X DELETE http://localhost:8080/panel/abc123
```
