# Web UI Overlay & Panel Rendering — Design Spec

## Goal

Add overlay and panel rendering support to the wsh web UI, so that overlays and panels created by agents (via HTTP, WebSocket, or MCP) are visually rendered in the browser-based terminal client — matching the behavior already present in the thin terminal client (`ws_client.rs`).

## Background

The wsh server stores overlay and panel data, notifies connected clients via `overlay_sync` and `panel_sync` WebSocket events, and leaves rendering to each client. The thin terminal client already renders these using ANSI escape sequences. The web UI currently has **no** overlay or panel support — it doesn't subscribe to overlay events, doesn't handle the sync messages, and has no rendering code.

## Architecture

**Approach: DOM Overlay Layer.** Overlays and panels are rendered as positioned DOM elements layered on top of (overlays) or around (panels) the existing terminal content. This plays to the web UI's existing DOM-based renderer, reuses span styling logic, and lets CSS handle z-ordering and positioning natively.

## Type Definitions

New TypeScript types mirror the Rust types. Added to `web/src/api/types.ts`.

```typescript
// Color is already defined in types.ts as the existing Span color type.
// OverlaySpan extends the concept with optional id.

interface OverlaySpan {
  text: string;
  id?: string;
  fg?: Color;
  bg?: Color;
  bold: boolean;
  italic: boolean;
  underline: boolean;
}

interface RegionWrite {
  row: number;
  col: number;
  text: string;
  fg?: Color;
  bg?: Color;
  bold: boolean;
  italic: boolean;
  underline: boolean;
}

interface BackgroundStyle {
  bg: Color;
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
  focusable: boolean;
  screen_mode: "normal" | "alt";
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
  focusable: boolean;
  screen_mode: "normal" | "alt";
}
```

Add `"overlay"` to the `EventType` union.

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

## WebSocket Subscription

Update the event subscription in `app.tsx` to include `"overlay"`:

```typescript
subscribe(["lines", "cursor", "mode", "activity", "overlay"])
```

Handle new message types in the event handler:

```typescript
case "overlay_sync":
  updateScreen(session, { overlays: msg.overlays });
  break;
case "panel_sync":
  updateScreen(session, { panels: msg.panels });
  break;
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

The web UI performs client-side layout computation matching the server's `compute_layout()` algorithm:

1. Filter panels by `visible == true` and matching `screen_mode`.
2. Sort by z-index descending (highest priority first).
3. Greedily allocate space: each panel claims `height` rows from its edge (top or bottom). If insufficient rows remain, the panel is hidden.
4. The remaining rows are the terminal content area.

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

Both overlays and panels have a `screen_mode` field (`"normal"` or `"alt"`). Only items whose mode matches the current terminal state are rendered:

```typescript
const activeOverlays = overlays.filter(o =>
  o.screen_mode === (alternateActive ? "alt" : "normal")
);
```

This filtering happens at render time, not in state updates — state always holds the full list.

## Shared Span Styling

The existing terminal renderer converts `Span` objects to inline CSS styles. Overlay/panel spans use the same type shape (`OverlaySpan` is a superset of `Span`). Extract the span-to-style logic into a shared utility function so terminal lines, overlays, and panels all use the same rendering path.

The shared function handles: `fg`, `bg` (both indexed and RGB colors), `bold`, `italic`, `underline`.

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
| `web/src/api/ws.ts` | Handle `overlay_sync` and `panel_sync` message types |
| `web/src/app.tsx` | Add `"overlay"` to subscribe list; route new events to state |
| `web/src/components/Terminal.tsx` | Add panel layout regions, overlay positioning layer, screen mode filtering, shared span styling |
| `web/src/styles/terminal.css` | Add `.overlay-layer`, `.overlay`, `.panel-region`, `.panel` CSS classes |

## What This Does NOT Cover

- **Input capture in the web UI** — the input capture system (`/input/capture`, `/input/release`) is orthogonal and already works through the existing input API.
- **Panel-induced PTY resize** — when panels carve space, the server handles PTY resize. The web UI receives the already-resized terminal output. The web UI layout computation is purely for visual positioning.
- **Overlay/panel CRUD from the web UI** — this is read-only rendering. Creation and management happen through the API.
- **Web UI mini-previews (sidebar thumbnails)** — overlays/panels in session preview thumbnails are out of scope for now.

## Testing

- Create overlays and panels via HTTP API, verify they render in the web UI.
- Test screen mode filtering: create a normal-mode overlay, switch to alt screen, verify it disappears.
- Test z-ordering: create overlapping overlays with different z values, verify stacking order.
- Test panel layout: create top and bottom panels, verify terminal content shrinks.
- Test overlay positioning: create overlay at known (x, y), verify pixel position matches character grid.
- Test region writes: create overlay with region_writes, verify cell-level positioning.
