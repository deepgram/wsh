# Web UI Overlay & Panel Rendering — Design Spec

## Goal

Add overlay and panel rendering support to the wsh web UI, so that overlays and panels created by agents (via HTTP, WebSocket, or MCP) are visually rendered in the browser-based terminal client — matching the behavior already present in the thin terminal client (`ws_client.rs`).

## Background

The wsh server stores overlay and panel data, notifies connected clients via `overlay_sync` and `panel_sync` WebSocket events, and leaves rendering to each client. The thin terminal client already renders these using ANSI escape sequences. The web UI currently has **no** overlay or panel support — it doesn't subscribe to overlay events, doesn't handle the sync messages, and has no rendering code.

## Architecture

**Approach: DOM Overlay Layer.** Overlays and panels are rendered as positioned DOM elements layered on top of (overlays) or around (panels) the existing terminal content. This plays to the web UI's existing DOM-based renderer, reuses span styling logic, and lets CSS handle z-ordering and positioning natively.

## Type Definitions

New TypeScript types mirror the Rust types. Added to `web/src/api/types.ts`.

**Important: Overlay colors differ from terminal colors.** The terminal `Color` type uses `indexed` (0-255 ANSI palette) or `rgb`. The overlay `Color` type (from `src/overlay/types.rs`) uses `Named` (string: "black", "red", etc.) or `Rgb`. Serde's `#[serde(untagged)]` serializes Named as a bare string and Rgb as `{r, g, b}`.

```typescript
// Overlay color — different from the terminal Color type.
// Named colors serialize as bare strings, RGB as {r, g, b}.
type OverlayColor = string | { r: number; g: number; b: number };

interface OverlaySpan {
  text: string;
  id?: string;
  fg?: OverlayColor;
  bg?: OverlayColor;
  bold?: boolean;     // omitted when false (serde skip_serializing_if)
  italic?: boolean;
  underline?: boolean;
}

interface RegionWrite {
  row: number;
  col: number;
  text: string;
  fg?: OverlayColor;
  bg?: OverlayColor;
  bold?: boolean;
  italic?: boolean;
  underline?: boolean;
}

interface BackgroundStyle {
  bg: OverlayColor;
}

interface Overlay {
  id: string;
  x: number;          // column position
  y: number;          // row position
  z: number;          // z-index for stacking
  width: number;      // columns
  height: number;     // rows
  background?: BackgroundStyle;
  spans: OverlaySpan[];
  region_writes: RegionWrite[];
  focusable?: boolean;    // omitted when false
  screen_mode?: "normal" | "alt";  // omitted when "normal" (default)
}

interface Panel {
  id: string;
  position: "top" | "bottom";
  height: number;     // rows
  z: number;          // priority for space allocation
  background?: BackgroundStyle;
  spans: OverlaySpan[];
  region_writes: RegionWrite[];
  visible: boolean;
  focusable?: boolean;    // omitted when false
  screen_mode?: "normal" | "alt";  // omitted when "normal" (default)
}
```

Add `"overlay"` to the `EventType` union.

A new `overlayColorToCSS()` function converts `OverlayColor` to CSS color strings. Named colors map to CSS named colors directly (they are valid CSS color names). RGB maps to `rgb(r, g, b)`. This is separate from the existing `colorToCSS()` which handles indexed/RGB terminal colors.

## State Changes

Add overlay and panel arrays to `ScreenState` in `web/src/state/terminal.ts`:

```typescript
interface ScreenState {
  // ... existing fields ...
  overlays: Overlay[];
  panels: Panel[];
}
```

Initialize both as empty arrays in `defaultScreenState()`. Updates arrive as full replacements (not diffs) — each `overlay_sync` message contains the complete overlay list, and each `panel_sync` contains the complete panel list.

**Reconnection timing:** During session setup, the flow is `getScreen` → `setFullScreen` (resets state) → `subscribe` → server sends initial `overlay_sync`/`panel_sync`. Overlay/panel state is intentionally not preserved across `setFullScreen` — fresh state arrives automatically from the server after subscribe.

## WebSocket Message Routing

**Important: `overlay_sync` and `panel_sync` messages use `"type"` as their discriminator, not `"event"`.** They are structured as `{"type": "overlay_sync", "overlays": [...]}` — the same pattern as the existing `"lagged"` notification. They do NOT carry a `session` field.

The current `handleMessage()` in `ws.ts` has three paths: messages with `id` (responses), messages with `connected` (hello), and messages with `event` (events routed to callbacks). The `overlay_sync`/`panel_sync` messages match none of these — they'd be silently dropped.

**Fix:** Add `overlay_sync` and `panel_sync` handling in `handleMessage()` alongside the existing `lagged` check, forwarding them to event callbacks. Since these messages lack a `session` field, they route through the "broadcast to all" path — which is correct because the per-session WS connection context already scopes them.

### Subscription

Update the event subscription in `app.tsx` to include `"overlay"`:

```typescript
subscribe(["lines", "cursor", "mode", "activity", "overlay"])
```

### Event Handling

In `app.tsx`'s event handler:

```typescript
// overlay_sync / panel_sync arrive as {type: ...} not {event: ...}
if ("type" in msg && msg.type === "overlay_sync") {
  updateScreen(session, { overlays: msg.overlays });
  return;
}
if ("type" in msg && msg.type === "panel_sync") {
  updateScreen(session, { panels: msg.panels });
  return;
}
```

## Panel Rendering

Panels carve dedicated rows from the top or bottom of the terminal viewport. The layout becomes:

```
┌─────────────────────────┐
│  Top Panel(s)           │  ← fixed-height region(s)
├─────────────────────────┤
│  Terminal Content        │  ← existing scrollable renderer
│  (with overlay layer)   │
├─────────────────────────┤
│  Bottom Panel(s)         │  ← fixed-height region(s)
└─────────────────────────┘
```

### Layout Computation

The web UI performs client-side layout computation matching the server's `compute_layout()` algorithm (see `src/panel/layout.rs`):

1. **Filter** panels by `visible == true` and matching `screen_mode`. (`compute_layout()` itself does not filter — the caller is responsible.)
2. **Merge** top and bottom panels into a single list and sort by z-index descending (highest priority first). Allocation is global by z-priority, not per-edge — a high-z bottom panel beats a low-z top panel.
3. **Greedily allocate** space: for each panel (in z-order), claim `height` rows from its edge (top or bottom). If insufficient rows remain, the panel is hidden.
4. The remaining rows are the terminal content area. **Panels can consume all rows**, leaving zero terminal rows — the web UI must handle this edge case gracefully (e.g., hide the terminal content area entirely).

This is a pure function: `(panels: Panel[], totalRows: number) => { topPanels: Panel[], bottomPanels: Panel[], terminalRows: number }`.

### Panel Content Rendering

Each panel renders as a block element with:
- Height: `panel.height * charHeight` pixels.
- Background: filled with `panel.background.bg` color if specified.
- Spans: rendered left-to-right using shared span styling (same as terminal lines).
- Region writes: positioned absolutely within the panel at `(col * charWidth, row * charHeight)`.

Panels are rendered in z-index order within their position group (top or bottom).

## Overlay Rendering

Overlays are absolutely-positioned DOM elements inside a layer that covers the terminal content area.

```html
<div class="terminal-content" style="position: relative">
  <!-- existing terminal lines -->

  <!-- overlay layer: covers terminal area, passes through clicks -->
  <div class="overlay-layer">
    <!-- one div per overlay, positioned by character coordinates -->
    <div class="overlay" style="
      left: {x * charWidth}px;
      top: {y * charHeight}px;
      width: {width * charWidth}px;
      height: {height * charHeight}px;
      z-index: {z};
    ">
      <!-- background fill -->
      <!-- spans rendered as styled text -->
      <!-- region_writes positioned absolutely within -->
    </div>
  </div>
</div>
```

### Positioning

Overlay `(x, y)` coordinates are in terminal character cells. Convert to pixels using the existing `charWidth`/`charHeight` measurements from the hidden measurement span.

### Z-Ordering

CSS `z-index` on each overlay div directly maps to the overlay's `z` field. The overlay layer itself sits above terminal content but below any browser-level UI.

### Pointer Events

The overlay layer uses `pointer-events: none` so clicks pass through to the terminal. Individual overlay divs use `pointer-events: auto` so they can be interacted with if needed.

## Screen Mode Filtering

Overlays and panels have different filtering models:

- **Overlays** are **server-side filtered** by screen mode. The server calls `list_by_mode(mode)` before sending `overlay_sync`, so the web UI only receives overlays matching the current mode. No client-side filtering needed.
- **Panels** are sent **unfiltered** — `panel_sync` contains all panels. The web UI must filter by `visible == true` and matching `screen_mode` at render time.

```typescript
const activePanels = panels.filter(p =>
  p.visible && (p.screen_mode ?? "normal") === (alternateActive ? "alt" : "normal")
);
```

**Mode change handling:** When the terminal switches between normal and alt screen, the server does NOT automatically re-send `overlay_sync` for the new mode. The web UI should **clear overlays** when it receives a `mode` event (since the current overlay list is for the old mode), and wait for the next `overlay_sync` to repopulate. Panels don't need this treatment since they arrive unfiltered.

In the mode event handler:

```typescript
case "mode":
  updateScreen(session, { alternateActive: raw.alternate_active, overlays: [] });
  break;
```

## Shared Span Styling

The existing terminal renderer converts `Span` objects to inline CSS styles via `spanStyle()` in `web/src/utils/terminal.ts`. Terminal spans have additional attributes (`faint`, `strikethrough`, `blink`, `inverse`) that overlay spans do not.

Rather than forcing a single polymorphic function, add a parallel `overlaySpanStyle()` function that handles the overlay-specific type (`OverlaySpan` with `OverlayColor`). This avoids coupling the terminal and overlay rendering paths. The overlay function handles: `fg`, `bg` (using `overlayColorToCSS()`), `bold`, `italic`, `underline`.

The `charWidth` and `charHeight` values (computed from the measurement span in `Terminal.tsx`) need to be accessible to the overlay/panel rendering code. Lift these into a ref or signal that child components can read.

## Region Write Rendering

Region writes are cell-level positioned text within an overlay or panel. Each region write renders as an absolutely-positioned span within its parent container:

```html
<span style="
  position: absolute;
  left: {col * charWidth}px;
  top: {row * charHeight}px;
  {styling from fg/bg/bold/italic/underline}
">{text}</span>
```

## Files Modified

| File | Change |
|------|--------|
| `web/src/api/types.ts` | Add Overlay, Panel, OverlaySpan, RegionWrite, BackgroundStyle types; add `"overlay"` to EventType |
| `web/src/state/terminal.ts` | Add `overlays` and `panels` to ScreenState; initialize as `[]` |
| `web/src/api/ws.ts` | Route `overlay_sync` and `panel_sync` messages (by `type` field) to event callbacks |
| `web/src/app.tsx` | Add `"overlay"` to subscribe list; handle overlay_sync/panel_sync; clear overlays on mode change |
| `web/src/utils/terminal.ts` | Add `overlayColorToCSS()` and `overlaySpanStyle()` functions for overlay color/styling |
| `web/src/components/Terminal.tsx` | Add panel layout regions, overlay positioning layer, lift charWidth/charHeight into accessible state |
| `web/src/components/MiniViewPreview.tsx` | Add overlay layer and panel regions inside `mini-term-inner` |
| `web/src/styles/terminal.css` | Add `.overlay-layer`, `.overlay`, `.panel-region`, `.panel` CSS classes |

## Sidebar Mini-Preview Rendering

The sidebar thumbnails (`MiniViewPreview.tsx` / `ThumbnailCell.tsx`) render a full-size terminal inside `mini-term-inner` and use CSS `transform: scale()` to shrink it to fit the 80px thumbnail container. This means **overlays and panels rendered inside `mini-term-inner` scale automatically** — no separate positioning math needed.

### Approach

Reuse the same overlay/panel rendering components from `Terminal.tsx` inside `MiniViewPreview.tsx`:

1. **Overlays**: Add the same `.overlay-layer` div inside `mini-term-inner`, after the terminal lines. Overlays position at `(x * charWidth, y * charHeight)` in the unscaled coordinate space — CSS transform handles the rest.
2. **Panels**: Add panel regions above/below the terminal lines within `mini-term-inner`. Same layout computation as the main terminal.

Since thumbnails already read from `getScreenSignal(session)` and the overlay/panel data lives in `ScreenState`, the data flow requires no changes — thumbnails will reactively pick up overlay/panel updates.

### Simplifications for Thumbnails

- **No pointer events needed** — thumbnails are non-interactive (clicking opens the full session).
- **No charWidth/charHeight lifting needed** — `MiniViewPreview` uses a fixed `BASE_FONT_PX = 12` and `LINE_HEIGHT = 1.2`, so `charWidth` and `charHeight` can be computed locally from these constants.

### Shared Rendering

Extract the overlay/panel rendering into reusable Preact components (e.g., `OverlayLayer`, `PanelRegion`) used by both `Terminal.tsx` and `MiniViewPreview.tsx`. These components accept overlays/panels + charWidth/charHeight as props and render the positioned DOM elements.

## What This Does NOT Cover

- **Input capture in the web UI** — the input capture system (`/input/capture`, `/input/release`) is orthogonal and already works through the existing input API.
- **Panel-induced PTY resize** — when panels carve space, the server handles PTY resize. The web UI receives the already-resized terminal output. The web UI layout computation is purely for visual positioning.
- **Overlay/panel CRUD from the web UI** — this is read-only rendering. Creation and management happen through the API.

## Testing

- Create overlays and panels via HTTP API, verify they render in the web UI.
- Test screen mode filtering: create a normal-mode overlay, switch to alt screen, verify it disappears.
- Test z-ordering: create overlapping overlays with different z values, verify stacking order.
- Test panel layout: create top and bottom panels, verify terminal content shrinks.
- Test overlay positioning: create overlay at known (x, y), verify pixel position matches character grid.
- Test region writes: create overlay with region_writes, verify cell-level positioning.
- Test sidebar thumbnails: create overlay/panel, verify they appear scaled in the mini-preview.
