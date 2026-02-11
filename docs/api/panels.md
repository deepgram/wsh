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
- **Spans**: One or more styled text segments (same format as overlay spans)
- **Visible** (`visible`): Whether the panel is currently rendered
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
  "spans": [
    {"text": "Status: ", "bold": true},
    {"text": "OK", "fg": "green"}
  ]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `position` | string | yes | `"top"` or `"bottom"` |
| `height` | integer | yes | Number of rows |
| `z` | integer | no | Z-order (auto-assigned if omitted) |
| `spans` | array | yes | Styled text spans |

**Response:** `201 Created`

```json
{"id": "f47ac10b-58cc-4372-a567-0e02b2c3d479"}
```

## List Panels

```
GET /panel
```

Returns all panels sorted by position then z-order descending.

**Response:** `200 OK`

```json
[
  {
    "id": "f47ac10b-58cc-4372-a567-0e02b2c3d479",
    "position": "top",
    "height": 2,
    "z": 100,
    "spans": [
      {"text": "Status: ", "bold": true},
      {"text": "OK", "fg": "green"}
    ],
    "visible": true
  }
]
```

## Get a Single Panel

```
GET /panel/:id
```

**Response:** `200 OK` with the panel object.

**Error:** `404` with code `panel_not_found` if the ID doesn't exist.

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
| `spans` | array | no | New styled text spans |

**Response:** `204 No Content`

**Error:** `404` with code `panel_not_found` if the ID doesn't exist.

## Delete a Panel

```
DELETE /panel/:id
```

**Response:** `204 No Content`

**Error:** `404` with code `panel_not_found` if the ID doesn't exist.

## Clear All Panels

```
DELETE /panel
```

Removes every panel. The PTY reclaims the full terminal height.

**Response:** `204 No Content`

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

## Panel Spans

Panel spans use the same format as overlay spans. Each span is a styled text
segment:

```json
{
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
| `text` | string | yes | The text content (may contain `\n`) |
| `fg` | OverlayColor | no | Foreground color |
| `bg` | OverlayColor | no | Background color |
| `bold` | boolean | no | Bold (default: false) |
| `italic` | boolean | no | Italic (default: false) |
| `underline` | boolean | no | Underline (default: false) |

Boolean style fields default to `false` and are omitted from responses when
`false`.

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
# Create a two-row status bar at the top of the terminal
curl -X POST http://localhost:8080/panel \
  -H 'Content-Type: application/json' \
  -d '{
    "position": "top",
    "height": 2,
    "z": 100,
    "spans": [
      {"text": " Agent: ", "bg": "blue", "bold": true},
      {"text": "watching ", "bg": "blue"},
      {"text": "OK", "fg": "green", "bg": "blue"},
      {"text": "\n"},
      {"text": " Session: abc123 ", "bg": "blue"}
    ]
  }'
# {"id":"abc123"}

# Update the status to show an alert
curl -X PATCH http://localhost:8080/panel/abc123 \
  -H 'Content-Type: application/json' \
  -d '{
    "spans": [
      {"text": " Agent: ", "bg": "red", "bold": true},
      {"text": "action needed ", "bg": "red"},
      {"text": "\n"},
      {"text": " Approve pending command? ", "bg": "red"}
    ]
  }'

# Clean up when done
curl -X DELETE http://localhost:8080/panel/abc123
```
