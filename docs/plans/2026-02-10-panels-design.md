# Panels: Agent-Owned Screen Regions

## Overview

Panels are agent-owned screen regions anchored to the top or bottom edge of
the terminal. Unlike overlays (which draw on top of PTY content), panels
**carve out dedicated rows** that shrink the PTY viewport. Programs running
in the PTY see a smaller terminal and never write into panel space.

Overlays = floating layers on top of content.
Panels = dedicated regions that shrink the PTY viewport.

## Data Model

```rust
Panel {
    id: String,              // UUID v4, server-generated
    position: Position,      // Top | Bottom
    height: u16,             // rows consumed
    z_index: i32,            // higher = closer to edge; auto-assigned if omitted
    spans: Vec<OverlaySpan>, // reuse existing styled span type; newlines for multi-row
    visible: bool,           // read-only, computed by layout engine
}
```

### Z-Index Stacking

Higher z-index = closer to the screen edge (higher priority).

For bottom panels, higher z-index means closer to the bottom edge. For top
panels, higher z-index means closer to the top edge. Panels with lower
z-index sit closer to the PTY content area.

This is consistent with overlays where higher z means "wins" / more
prominent.

## Screen Layout

Example: 24-row terminal, 2-row top panel, 1-row bottom panel:

```
Row 1:  ┐ top panel (z=1)
Row 2:  ┘
Row 3:  ┐
  ...   │ PTY content (scroll region: rows 3–23)
Row 23: ┘
Row 24:   bottom panel (z=1)
```

## Rendering

### Scroll Region (DECSTBM)

`wsh` uses `DECSTBM` (Set Top and Bottom Margins) to confine PTY output to
the non-panel rows. This is the same technique tmux uses for its status bar.

- When the first panel is created, set `DECSTBM` to confine scrolling to
  the PTY region.
- When panels are added/removed/resized, recalculate and reapply `DECSTBM`.
- When the last panel is deleted, reset the scroll region to the full
  terminal.

### Panel Rendering Cycle

1. Save cursor position
2. Move cursor to the panel's rows (outside scroll region)
3. Render styled spans with ANSI SGR codes
4. Clear unfilled rows within the panel (spaces)
5. Restore cursor position

### Interaction with Overlays

Panels do not participate in the overlay erase/render cycle. Because panels
are outside the scroll region, PTY output cannot disturb them. Panel content
is only re-rendered when the agent explicitly updates it or when panels are
reconfigured.

Overlays use absolute coordinates and could visually overlap with panel
content. This is the agent's problem -- `wsh` does not prevent it.

## PTY Resizing

Panel create, delete, or height change triggers an immediate PTY resize.
The shell and running programs receive `SIGWINCH`.

PTY rows = terminal rows - sum of all visible panel heights.
Column count is unaffected.

### Outer Terminal Resize

When the real terminal resizes (`SIGWINCH` from the outer terminal), `wsh`
recalculates: new PTY rows = new terminal rows - total visible panel height.
Update scroll region, resize PTY, re-render all panels at new positions.

## Layout Algorithm & Panel Visibility

Panels are never rejected. If there is not enough room for all panels plus
a minimum of 1 PTY row, the lowest-priority panels are hidden.

Algorithm:

1. Sort panels by position, then z_index descending (highest priority first).
2. Greedily allocate rows to panels in priority order.
3. Once only 1 row remains for the PTY, stop. Remaining panels are hidden.
4. Hidden panels still exist in the store and respond to API queries, but
   are not rendered and do not contribute to PTY sizing.
5. If space opens up (panel deleted, terminal grows), hidden panels become
   visible again automatically.

The `visible` field on the panel response tells agents whether their panel
is currently being rendered.

## API Surface

### HTTP REST

| Method   | Path           | Body                                      | Effect                                          |
|----------|----------------|-------------------------------------------|-------------------------------------------------|
| `POST`   | `/panels`      | `{position, height, z_index?, spans?}`    | Create panel, resize PTY                        |
| `GET`    | `/panels`      | --                                        | List all panels (sorted by position, z_index)   |
| `GET`    | `/panels/:id`  | --                                        | Get single panel                                |
| `PUT`    | `/panels/:id`  | `{position, height, z_index, spans}`      | Full replace; resize PTY if height changed      |
| `PATCH`  | `/panels/:id`  | any subset of fields                      | Partial update; resize PTY only if height changed |
| `DELETE` | `/panels/:id`  | --                                        | Delete panel, resize PTY                        |
| `DELETE` | `/panels`      | --                                        | Clear all panels, single PTY resize             |

### WebSocket JSON Methods

Same request/response format as overlays:

- `create_panel` -- params: `{position, height, z_index?, spans?}` -> returns panel
- `list_panels` -- params: `{}` -> returns sorted array
- `get_panel` -- params: `{id}` -> returns panel
- `update_panel` -- params: `{id, position, height, z_index, spans}` -> returns panel
- `patch_panel` -- params: `{id, ...fields}` -> returns panel
- `delete_panel` -- params: `{id}` -> returns `{}`
- `clear_panels` -- params: `{}` -> returns `{}`

### Resize Logic

Every mutation that changes total panel height triggers: recalculate
layout -> apply `DECSTBM` -> resize PTY -> re-render all panels.
Span-only updates skip the resize and just re-render the affected panel.

## Implementation Architecture

### New Module: `src/panel/`

Mirrors the overlay module structure:

- `types.rs` -- `Panel`, `Position` enum (`Top`, `Bottom`), serialization
- `store.rs` -- `PanelStore` with `Arc<RwLock<HashMap<PanelId, Panel>>>`,
  auto-incrementing z_index, CRUD operations
- `render.rs` -- ANSI rendering for panel content (cursor positioning,
  styled spans, clear unfilled rows)
- `layout.rs` -- Given all panels + terminal size, compute: visible/hidden
  panels, top panel rows, bottom panel rows, PTY row range, scroll region
  bounds
- `mod.rs` -- Re-exports

### Changes to Existing Modules

- `src/main.rs` -- Wire up `PanelStore`, apply scroll region on panel
  changes, handle outer terminal resize (recalculate layout + resize PTY +
  re-render panels)
- `src/api/handlers.rs` -- Add HTTP handlers for panel CRUD, add WebSocket
  dispatch methods
- `src/api/ws_methods.rs` -- Add panel methods to `dispatch()`
- `src/pty.rs` -- No changes (already supports `resize()`)
- `src/parser/mod.rs` -- No changes (already supports `resize()`)

### Key Function: `reconfigure_layout()`

Called on any panel mutation that changes total height, and on outer
terminal resize:

1. Query all panels from the store
2. Compute layout (visible/hidden panels, top rows, PTY region, bottom rows)
3. Set `DECSTBM` for the PTY region
4. Resize the PTY and parser
5. Re-render all visible panels at their computed positions

### Shared Infrastructure

`OverlaySpan`, `Color`, and the SGR rendering helpers are reused directly
from the overlay module. No duplication.

## Edge Cases

- **Panel with empty spans:** Valid. Renders as blank rows. Agent may create
  a panel and populate it later.
- **All rows consumed by panels:** Lowest z-index panels are hidden until
  at least 1 PTY row is available.
- **Terminal shrinks below panel height:** Same visibility algorithm applies.
  Panels are hidden as needed to maintain minimum 1 PTY row.
- **Overlays in panel rows:** Allowed. Agent's responsibility to coordinate.

## Documentation Updates

The following documents must be updated as part of implementation:

- **`docs/api/README.md`** -- Add panel endpoints to the "Endpoints at a
  Glance" table, add a Panels section with a link to the panels doc, update
  the WebSocket description to mention panel methods.
- **`docs/api/panels.md`** (new) -- Full panel system documentation,
  mirroring the structure of `docs/api/overlays.md`. Covers HTTP endpoints,
  request/response examples, the panel data model, z-index stacking,
  visibility behavior, and the relationship to PTY sizing.
- **`docs/api/websocket.md`** -- Add panel WebSocket methods
  (`create_panel`, `list_panels`, `get_panel`, `update_panel`,
  `patch_panel`, `delete_panel`, `clear_panels`) to the method reference.
- **`docs/api/openapi.yaml`** -- Add panel schemas (`Panel`,
  `CreatePanelRequest`, `PatchPanelRequest`, etc.) and all panel endpoint
  paths with request/response definitions.

## Testing Strategy

- **Unit tests:** `PanelStore` CRUD, layout calculation (given N panels at
  various positions/z-indices, verify correct row assignments and
  visibility), minimum PTY height enforcement, panel hiding/unhiding
- **Integration tests:** Panel create -> verify PTY resize, panel delete ->
  verify PTY resize, scroll region correctness, HTTP and WebSocket API
  round-trips
- **End-to-end:** Create panel via API, verify terminal output positions
  correctly within scroll region, update panel spans and verify rendering
