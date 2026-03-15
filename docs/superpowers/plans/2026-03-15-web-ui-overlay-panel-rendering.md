# Web UI Overlay & Panel Rendering — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render overlays and panels in the web UI (main terminal + sidebar thumbnails) so agents' visual feedback appears in the browser.

**Architecture:** DOM overlay layer approach — overlays are absolutely-positioned divs on top of terminal content, panels carve fixed-height regions above/below the terminal. Shared rendering components are used by both the main terminal and sidebar thumbnails. State flows through existing Preact signals.

**Tech Stack:** Preact + @preact/signals, TypeScript, Vite, CSS

**Spec:** `docs/superpowers/specs/2026-03-15-web-ui-overlay-panel-rendering-design.md`

---

## File Structure

| File | Responsibility | Action |
|------|---------------|--------|
| `web/src/api/types.ts` | Overlay/Panel TypeScript types | Modify |
| `web/src/state/terminal.ts` | Add overlays/panels to ScreenState | Modify |
| `web/src/utils/terminal.ts` | Add `overlayColorToCSS()` and `overlaySpanStyle()` | Modify |
| `web/src/api/ws.ts` | Route overlay_sync/panel_sync messages | Modify |
| `web/src/app.tsx` | Subscribe to overlay events, handle sync messages | Modify |
| `web/src/components/OverlayLayer.tsx` | Shared overlay rendering component | Create |
| `web/src/components/PanelRegion.tsx` | Shared panel rendering component + layout computation | Create |
| `web/src/components/Terminal.tsx` | Integrate OverlayLayer + PanelRegion, lift charWidth/charHeight | Modify |
| `web/src/components/MiniViewPreview.tsx` | Integrate OverlayLayer + PanelRegion | Modify |
| `web/src/styles/terminal.css` | CSS for overlay/panel elements | Modify |

---

## Chunk 1: Types, State, and Wiring

### Task 1: Add TypeScript types for overlays and panels

**Files:**
- Modify: `web/src/api/types.ts`

- [ ] **Step 1: Add overlay/panel types after the existing `SendInputResult` interface (after line 111)**

```typescript
// Overlay color — different from terminal Color.
// Named colors serialize as bare strings (e.g. "red"), RGB as {r, g, b}.
// Serde #[serde(untagged)] on the Rust enum produces this shape.
export type OverlayColor = string | { r: number; g: number; b: number };

export interface OverlaySpan {
  text: string;
  id?: string;
  fg?: OverlayColor;
  bg?: OverlayColor;
  bold?: boolean;
  italic?: boolean;
  underline?: boolean;
}

export interface RegionWrite {
  row: number;
  col: number;
  text: string;
  fg?: OverlayColor;
  bg?: OverlayColor;
  bold?: boolean;
  italic?: boolean;
  underline?: boolean;
}

export interface BackgroundStyle {
  bg: OverlayColor;
}

export interface Overlay {
  id: string;
  x: number;
  y: number;
  z: number;
  width: number;
  height: number;
  background?: BackgroundStyle;
  spans: OverlaySpan[];
  region_writes: RegionWrite[];
  focusable?: boolean;
  screen_mode?: "normal" | "alt";
}

export interface Panel {
  id: string;
  position: "top" | "bottom";
  height: number;
  z: number;
  background?: BackgroundStyle;
  spans: OverlaySpan[];
  region_writes: RegionWrite[];
  visible: boolean;
  focusable?: boolean;
  screen_mode?: "normal" | "alt";
}
```

- [ ] **Step 2: Add `"overlay"` to the EventType union (line 88)**

Change:
```typescript
export type EventType = "lines" | "cursor" | "mode" | "diffs" | "activity";
```
To:
```typescript
export type EventType = "lines" | "cursor" | "mode" | "diffs" | "activity" | "overlay";
```

- [ ] **Step 3: Verify the web project still compiles**

Run: `cd /home/ajsyp/Projects/deepgram/wsh/web && npx tsc --noEmit && npx vite build 2>&1 | tail -5`
Expected: No type errors, build succeeds.

- [ ] **Step 4: Commit**

```bash
git add web/src/api/types.ts
git commit -m "feat(web): add overlay and panel TypeScript types"
```

---

### Task 2: Add overlay/panel state to ScreenState

**Files:**
- Modify: `web/src/state/terminal.ts`

- [ ] **Step 1: Import the new types at the top of the file (line 1)**

Add to existing import:
```typescript
import type { FormattedLine, Cursor, Overlay, Panel } from "../api/types";
```

Check the existing import line first — it currently imports `FormattedLine` and `Cursor`. Add `Overlay` and `Panel` to that import.

- [ ] **Step 2: Add `overlays` and `panels` fields to the `ScreenState` interface (after line 17, the `scrollbackLoading` field)**

```typescript
  overlays: Overlay[];
  panels: Panel[];
```

- [ ] **Step 3: Initialize the new fields in `makeEmptyScreen()` (around line 23)**

Add to the returned object:
```typescript
    overlays: [],
    panels: [],
```

- [ ] **Step 4: Verify build**

Run: `cd /home/ajsyp/Projects/deepgram/wsh/web && npx tsc --noEmit && npx vite build 2>&1 | tail -5`
Expected: No type errors, build succeeds.

- [ ] **Step 5: Commit**

```bash
git add web/src/state/terminal.ts
git commit -m "feat(web): add overlays and panels to ScreenState"
```

---

### Task 3: Add overlay color and span styling utilities

**Files:**
- Modify: `web/src/utils/terminal.ts`

- [ ] **Step 1: Import the new types at the top of the file**

Add:
```typescript
import type { OverlayColor, OverlaySpan } from "../api/types";
```

- [ ] **Step 2: Add `overlayColorToCSS()` function after the existing `colorToCSS()` function (after line 36)**

```typescript
/** Convert overlay color to CSS. Named colors are valid CSS color names. */
export function overlayColorToCSS(c: OverlayColor): string {
  if (typeof c === "string") return c;
  return `rgb(${c.r},${c.g},${c.b})`;
}
```

- [ ] **Step 3: Add `overlaySpanStyle()` function after the existing `spanStyle()` function (after line 62)**

```typescript
/** Convert an OverlaySpan to a CSS style object. */
export function overlaySpanStyle(span: OverlaySpan): Record<string, string> {
  const s: Record<string, string> = {};
  if (span.fg) s.color = overlayColorToCSS(span.fg);
  if (span.bg) s.backgroundColor = overlayColorToCSS(span.bg);
  if (span.bold) s.fontWeight = "bold";
  if (span.italic) s.fontStyle = "italic";
  if (span.underline) s.textDecoration = "underline";
  return s;
}
```

- [ ] **Step 4: Verify build**

Run: `cd /home/ajsyp/Projects/deepgram/wsh/web && npx tsc --noEmit && npx vite build 2>&1 | tail -5`
Expected: No type errors, build succeeds.

- [ ] **Step 5: Commit**

```bash
git add web/src/utils/terminal.ts
git commit -m "feat(web): add overlay color and span styling utilities"
```

---

### Task 4: Route overlay_sync/panel_sync messages in WebSocket client

**Files:**
- Modify: `web/src/api/ws.ts`

- [ ] **Step 1: Add overlay_sync and panel_sync routing in `handleMessage()` (after the "lagged" check at line 335, before the "event" routing at line 338)**

Insert after `return;` on line 335:

```typescript
    // Overlay/panel sync — uses "type" field, not "event". No "session" field.
    // Route through event callbacks via broadcast (no session scoping needed —
    // per-session WS connections scope these messages implicitly).
    if ("type" in msg && (msg.type === "overlay_sync" || msg.type === "panel_sync")) {
      for (const [, callbacks] of this.eventCallbacks) {
        for (const cb of callbacks) {
          try {
            cb(msg);
          } catch (e) {
            console.error("Error in overlay/panel sync handler:", e);
          }
        }
      }
      return;
    }
```

- [ ] **Step 2: Verify build**

Run: `cd /home/ajsyp/Projects/deepgram/wsh/web && npx tsc --noEmit && npx vite build 2>&1 | tail -5`
Expected: No type errors, build succeeds.

- [ ] **Step 3: Commit**

```bash
git add web/src/api/ws.ts
git commit -m "feat(web): route overlay_sync/panel_sync WebSocket messages"
```

---

### Task 5: Subscribe to overlay events and handle sync messages

**Files:**
- Modify: `web/src/app.tsx`

- [ ] **Step 1: Add `"overlay"` to the subscribe events list**

Find the `client.subscribe()` call in `setupSession()` (around line 300). The events array is `["lines", "cursor", "mode", "activity"]`. Change to:

```typescript
["lines", "cursor", "mode", "activity", "overlay"]
```

- [ ] **Step 2: Handle overlay_sync and panel_sync in the event callback**

In the callback passed to `client.subscribe()` (around line 301-305), add overlay/panel handling at the top of the callback, before the existing `handleEvent` call. The callback receives `msg` (the raw WS message). Add:

```typescript
        // overlay_sync / panel_sync arrive as {type: ...} not {event: ...}
        if ("type" in msg && msg.type === "overlay_sync") {
          updateScreen(name, { overlays: msg.overlays as Overlay[] });
          return;
        }
        if ("type" in msg && msg.type === "panel_sync") {
          updateScreen(name, { panels: msg.panels as Panel[] });
          return;
        }
```

Import `Overlay` and `Panel` from `../api/types` at the top of the file.

- [ ] **Step 3: Update all `setFullScreen` calls to include overlays and panels**

After Task 2 adds `overlays` and `panels` as required fields on `ScreenState`, all existing `setFullScreen` call sites will have type errors. There are three locations in `app.tsx`:

1. **`setupSession`** (around line 284-296) — constructs a fresh ScreenState from the initial `getScreen` response. Add `overlays: []` and `panels: []` to the object literal (initial state has no overlays/panels; they arrive via the subscribe).

2. **`handleEvent` `sync`/`diff` case** (around line 435-447) — constructs ScreenState from a sync event. Preserve the current overlay/panel state since overlay_sync/panel_sync update them independently:
   ```typescript
   overlays: getScreen(session).overlays,
   panels: getScreen(session).panels,
   ```

3. **`handleEvent` `reset` case** (around line 478-489) — re-fetches and reconstructs ScreenState. Same approach — preserve current overlay/panel state:
   ```typescript
   overlays: getScreen(session).overlays,
   panels: getScreen(session).panels,
   ```

- [ ] **Step 4: Clear overlays on mode change**

Find the `"mode"` case in `handleEvent()` (around line 471-472). Change:

```typescript
      case "mode":
        updateScreen(session, { alternateActive: raw.alternate_active });
        break;
```

To:

```typescript
      case "mode":
        updateScreen(session, { alternateActive: raw.alternate_active, overlays: [] });
        break;
```

- [ ] **Step 5: Verify build**

Run: `cd /home/ajsyp/Projects/deepgram/wsh/web && npx tsc --noEmit && npx vite build 2>&1 | tail -5`
Expected: No type errors, build succeeds.

- [ ] **Step 6: Commit**

```bash
git add web/src/app.tsx
git commit -m "feat(web): subscribe to overlay events and handle sync messages"
```

---

## Chunk 2: Rendering Components

### Task 6: Add CSS classes for overlays and panels

**Files:**
- Modify: `web/src/styles/terminal.css`

- [ ] **Step 1: Add overlay and panel CSS classes**

Add after the `.terminal-disconnected` block (after line 169):

```css
/* Overlay layer: covers terminal content area, passes clicks through */
.overlay-layer {
  position: absolute;
  inset: 0;
  pointer-events: none;
  overflow: hidden;
}

/* Individual overlay: positioned by character grid coordinates */
.overlay-item {
  position: absolute;
  pointer-events: auto;
  font-family: inherit;
  font-size: inherit;
  line-height: inherit;
  white-space: pre;
  overflow: hidden;
}

/* Region write: cell-level positioned text within an overlay or panel */
.region-write {
  position: absolute;
  white-space: pre;
}

/* Panel region: fixed-height block above or below terminal content */
.panel-region {
  position: relative;
  font-family: inherit;
  font-size: inherit;
  line-height: inherit;
  white-space: pre;
  overflow: hidden;
  flex-shrink: 0;
}
```

- [ ] **Step 2: Verify build**

Run: `cd /home/ajsyp/Projects/deepgram/wsh/web && npx tsc --noEmit && npx vite build 2>&1 | tail -5`
Expected: No type errors, build succeeds.

- [ ] **Step 3: Commit**

```bash
git add web/src/styles/terminal.css
git commit -m "feat(web): add overlay and panel CSS classes"
```

---

### Task 7: Create shared OverlayLayer component

**Files:**
- Create: `web/src/components/OverlayLayer.tsx`

- [ ] **Step 1: Create the OverlayLayer component**

```typescript
import { h } from "preact";
import type { Overlay, OverlaySpan, RegionWrite } from "../api/types";
import { overlaySpanStyle, overlayColorToCSS } from "../utils/terminal";

interface OverlayLayerProps {
  overlays: Overlay[];
  charWidth: number;
  charHeight: number;
}

function renderSpans(spans: OverlaySpan[]): h.JSX.Element[] {
  return spans.map((span, i) => (
    <span key={i} style={overlaySpanStyle(span)}>
      {span.text}
    </span>
  ));
}

function renderRegionWrites(
  writes: RegionWrite[],
  charWidth: number,
  charHeight: number,
): h.JSX.Element[] {
  return writes.map((rw, i) => (
    <span
      key={`rw-${i}`}
      class="region-write"
      style={{
        left: `${rw.col * charWidth}px`,
        top: `${rw.row * charHeight}px`,
        ...overlaySpanStyle(rw),
      }}
    >
      {rw.text}
    </span>
  ));
}

export function OverlayLayer({ overlays, charWidth, charHeight }: OverlayLayerProps) {
  if (overlays.length === 0) return null;

  return (
    <div class="overlay-layer">
      {overlays.map((o) => (
        <div
          key={o.id}
          class="overlay-item"
          style={{
            left: `${o.x * charWidth}px`,
            top: `${o.y * charHeight}px`,
            width: `${o.width * charWidth}px`,
            height: `${o.height * charHeight}px`,
            zIndex: o.z,
            ...(o.background
              ? { backgroundColor: overlayColorToCSS(o.background.bg) }
              : {}),
          }}
        >
          {renderSpans(o.spans)}
          {renderRegionWrites(o.region_writes ?? [], charWidth, charHeight)}
        </div>
      ))}
    </div>
  );
}
```

- [ ] **Step 2: Verify build**

Run: `cd /home/ajsyp/Projects/deepgram/wsh/web && npx tsc --noEmit && npx vite build 2>&1 | tail -5`
Expected: No type errors, build succeeds.

- [ ] **Step 3: Commit**

```bash
git add web/src/components/OverlayLayer.tsx
git commit -m "feat(web): create shared OverlayLayer component"
```

---

### Task 8: Create shared PanelRegion component with layout computation

**Files:**
- Create: `web/src/components/PanelRegion.tsx`

- [ ] **Step 1: Create the PanelRegion component and layout computation**

```typescript
import { h } from "preact";
import type { Panel, OverlaySpan, RegionWrite } from "../api/types";
import { overlaySpanStyle, overlayColorToCSS } from "../utils/terminal";

interface PanelLayout {
  topPanels: Panel[];
  bottomPanels: Panel[];
  hiddenPanelIds: string[];
  terminalRows: number;
}

/**
 * Compute panel layout matching the server's compute_layout() algorithm.
 * Caller must pre-filter by visible and screen_mode before calling.
 */
export function computePanelLayout(
  panels: Panel[],
  totalRows: number,
): PanelLayout {
  // Merge all panels and sort by z descending (highest priority first)
  const sorted = [...panels].sort((a, b) => b.z - a.z);

  let remaining = totalRows;
  const topPanels: Panel[] = [];
  const bottomPanels: Panel[] = [];
  const hiddenPanelIds: string[] = [];

  for (const panel of sorted) {
    if (remaining === 0 || panel.height > remaining) {
      hiddenPanelIds.push(panel.id);
      continue;
    }
    remaining -= panel.height;
    if (panel.position === "top") {
      topPanels.push(panel);
    } else {
      bottomPanels.push(panel);
    }
  }

  // Re-sort within position groups: highest z first (edge toward content)
  topPanels.sort((a, b) => b.z - a.z);
  bottomPanels.sort((a, b) => b.z - a.z);

  return {
    topPanels,
    bottomPanels,
    hiddenPanelIds,
    terminalRows: remaining,
  };
}

interface PanelRegionProps {
  panels: Panel[];
  charWidth: number;
  charHeight: number;
}

function renderSpans(spans: OverlaySpan[]): h.JSX.Element[] {
  return spans.map((span, i) => (
    <span key={i} style={overlaySpanStyle(span)}>
      {span.text}
    </span>
  ));
}

function renderRegionWrites(
  writes: RegionWrite[],
  charWidth: number,
  charHeight: number,
): h.JSX.Element[] {
  return writes.map((rw, i) => (
    <span
      key={`rw-${i}`}
      class="region-write"
      style={{
        left: `${rw.col * charWidth}px`,
        top: `${rw.row * charHeight}px`,
        ...overlaySpanStyle(rw),
      }}
    >
      {rw.text}
    </span>
  ));
}

export function PanelRegion({ panels, charWidth, charHeight }: PanelRegionProps) {
  return (
    <>
      {panels.map((panel) => (
        <div
          key={panel.id}
          class="panel-region"
          style={{
            height: `${panel.height * charHeight}px`,
            ...(panel.background
              ? { backgroundColor: overlayColorToCSS(panel.background.bg) }
              : {}),
          }}
        >
          {renderSpans(panel.spans)}
          {renderRegionWrites(panel.region_writes ?? [], charWidth, charHeight)}
        </div>
      ))}
    </>
  );
}
```

- [ ] **Step 2: Verify build**

Run: `cd /home/ajsyp/Projects/deepgram/wsh/web && npx tsc --noEmit && npx vite build 2>&1 | tail -5`
Expected: No type errors, build succeeds.

- [ ] **Step 3: Commit**

```bash
git add web/src/components/PanelRegion.tsx
git commit -m "feat(web): create shared PanelRegion component with layout computation"
```

---

## Chunk 3: Integration into Terminal and Thumbnails

### Task 9: Integrate overlays and panels into Terminal component

**Files:**
- Modify: `web/src/components/Terminal.tsx`

This is the most complex task. The Terminal component currently renders lines inside a `terminal-container` div. We need to:
1. Lift `charWidth`/`charHeight` into component state
2. Add panel regions above and below the terminal content
3. Add the overlay layer on top of the terminal content
4. Filter panels by screen mode

- [ ] **Step 1: Add imports at the top of Terminal.tsx**

Add `useState` to the hooks import (it currently imports `useRef, useEffect, useCallback` but NOT `useState`):

```typescript
import { useRef, useEffect, useCallback, useState } from "preact/hooks";
```

Add the new component imports:

```typescript
import { OverlayLayer } from "./OverlayLayer";
import { PanelRegion, computePanelLayout } from "./PanelRegion";
```

Check if `getScreenSignal` or `getScreen` is already imported — if so, skip. Look at how the component currently reads its screen state and follow the same pattern.

- [ ] **Step 2: Add charWidth/charHeight state**

Inside the component function, after the existing refs (`measureRef`, `containerRef`), add state for cell dimensions:

```typescript
const [cellSize, setCellSize] = useState<{ w: number; h: number } | null>(null);
```

The existing `computeGridSize` function (around line 188) is a pure `useCallback` that measures the hidden span and returns `{ cols, rows } | null`. **Do not add state setters inside it.** Instead, extend its return type to include cell dimensions:

Change the return statement in `computeGridSize` from:
```typescript
return { cols: Math.max(cols, 1), rows: Math.max(rows, 1) };
```
To:
```typescript
return { cols: Math.max(cols, 1), rows: Math.max(rows, 1), cellWidth: rect.width, cellHeight: rect.height };
```

(`rect` is already in scope — the function measures `rect` from `measureRef` earlier.)

Then, at each **call site** of `computeGridSize` (there are two: the ResizeObserver callback around line 219 and the zoom-change effect around line 240), extract and store the cell size. After `const size = computeGridSize();` (or similar), add:

```typescript
if (size) {
  setCellSize({ w: size.cellWidth, h: size.cellHeight });
}
```

- [ ] **Step 3: Read overlays and panels from screen state**

Find where the component reads `lines`, `cursor`, `alternateActive`, etc. from the screen signal. Alongside those, destructure `overlays` and `panels`:

```typescript
const { lines, cursor, alternateActive, cols, rows, overlays, panels } = screen;
```

- [ ] **Step 4: Compute panel layout and filter**

After destructuring, add panel layout computation:

```typescript
const activePanels = panels.filter(
  (p) => p.visible && (p.screen_mode ?? "normal") === (alternateActive ? "alt" : "normal"),
);
const panelLayout = computePanelLayout(activePanels, rows);
```

- [ ] **Step 5: Modify the JSX to include panels and overlays**

The current JSX structure (around lines 370-415) is roughly:

```jsx
<div class="terminal-wrapper" style={{ fontSize: `${fontSize}px` }}>
  {captureInput && <textarea ... />}
  <div class={containerClass} ref={containerRef}>
    <span ref={measureRef} ... >X</span>
    {allLines.map((line, i) => renderLine(...))}
    {disconnected && <div class="terminal-disconnected">...</div>}
  </div>
</div>
```

Change to:

```jsx
<div class="terminal-wrapper" style={{ fontSize: `${fontSize}px` }}>
  {captureInput && <textarea ... />}
  {cellSize && panelLayout.topPanels.length > 0 && (
    <PanelRegion panels={panelLayout.topPanels} charWidth={cellSize.w} charHeight={cellSize.h} />
  )}
  <div class={containerClass} ref={containerRef}>
    <span ref={measureRef} ... >X</span>
    {allLines.map((line, i) => renderLine(...))}
    {cellSize && overlays.length > 0 && (
      <OverlayLayer overlays={overlays} charWidth={cellSize.w} charHeight={cellSize.h} />
    )}
    {disconnected && <div class="terminal-disconnected">...</div>}
  </div>
  {cellSize && panelLayout.bottomPanels.length > 0 && (
    <PanelRegion panels={panelLayout.bottomPanels} charWidth={cellSize.w} charHeight={cellSize.h} />
  )}
</div>
```

Key points:
- Panel regions go OUTSIDE the `terminal-container` (above and below it).
- Overlay layer goes INSIDE the `terminal-container` (on top of the lines).
- `.terminal-container` already has `position: relative` in CSS (line 114 of terminal.css), so the overlay layer's `position: absolute` works without changes.
- Guard rendering on `cellSize` being available (it's null before first measurement).

- [ ] **Step 6: Verify build**

Run: `cd /home/ajsyp/Projects/deepgram/wsh/web && npx tsc --noEmit && npx vite build 2>&1 | tail -5`
Expected: No type errors, build succeeds.

- [ ] **Step 7: Commit**

```bash
git add web/src/components/Terminal.tsx
git commit -m "feat(web): integrate overlay and panel rendering into Terminal"
```

---

### Task 10: Integrate overlays and panels into MiniViewPreview

**Files:**
- Modify: `web/src/components/MiniViewPreview.tsx`

The thumbnail renders inside `mini-term-inner` with CSS `transform: scale()`. Overlays and panels placed inside this div scale automatically.

- [ ] **Step 1: Add imports**

```typescript
import { OverlayLayer } from "./OverlayLayer";
import { PanelRegion, computePanelLayout } from "./PanelRegion";
```

- [ ] **Step 2: Destructure overlays and panels from screen state**

In `MiniTermContent` (around line 22), where it destructures `const { cols, rows, lines } = screen;`, change to:

```typescript
const { cols, rows, lines, overlays, panels, alternateActive } = screen;
```

- [ ] **Step 3: Compute cell dimensions and panel layout**

After the destructuring, add local constants for cell dimensions. The thumbnail uses a fixed `BASE_FONT_PX = 12` and `LINE_HEIGHT = 1.2`, so we can compute these directly without DOM measurement. The `1ch` CSS unit equals the width of "0" in the current font; for monospace at 12px this is approximately `0.6 * fontSize`:

```typescript
const charHeight = BASE_FONT_PX * LINE_HEIGHT;  // 14.4px
const charWidth = BASE_FONT_PX * 0.6;           // ~7.2px (approximation for monospace)
```

Then compute panel layout:

```typescript
const activePanels = panels.filter(
  (p) => p.visible && (p.screen_mode ?? "normal") === (alternateActive ? "alt" : "normal"),
);
const panelLayout = computePanelLayout(activePanels, rows);
```

Note: The charWidth approximation is sufficient for thumbnails — they're scaled down to ~80px height and the exact pixel offset doesn't need to be perfect. If pixel-perfect alignment is later needed, the ResizeObserver's `naturalWidth / cols` measurement can be stored in a ref.

- [ ] **Step 4: Update the JSX in mini-term-inner**

The current structure (around lines 62-79) is:

```jsx
<div class="mini-term-inner" style={{ transform: `scale(${scale})`, ... }}>
  {lines.map((line, i) => (
    <div key={i} class="mini-term-line">{renderMiniLine(line)}</div>
  ))}
</div>
```

Change to:

```jsx
<div class="mini-term-inner" style={{ transform: `scale(${scale})`, ... }}>
  {panelLayout.topPanels.length > 0 && (
    <PanelRegion panels={panelLayout.topPanels} charWidth={charWidth} charHeight={charHeight} />
  )}
  <div style={{ position: "relative" }}>
    {lines.map((line, i) => (
      <div key={i} class="mini-term-line">{renderMiniLine(line)}</div>
    ))}
    {overlays.length > 0 && (
      <OverlayLayer overlays={overlays} charWidth={charWidth} charHeight={charHeight} />
    )}
  </div>
  {panelLayout.bottomPanels.length > 0 && (
    <PanelRegion panels={panelLayout.bottomPanels} charWidth={charWidth} charHeight={charHeight} />
  )}
</div>
```

- [ ] **Step 5: Verify build**

Run: `cd /home/ajsyp/Projects/deepgram/wsh/web && npx tsc --noEmit && npx vite build 2>&1 | tail -5`
Expected: No type errors, build succeeds.

- [ ] **Step 6: Commit**

```bash
git add web/src/components/MiniViewPreview.tsx
git commit -m "feat(web): integrate overlay and panel rendering into sidebar thumbnails"
```

---

## Chunk 4: Manual Testing and Documentation

### Task 11: Manual end-to-end verification

- [ ] **Step 1: Start the dev server**

```bash
cd /home/ajsyp/Projects/deepgram/wsh/web && npx vite dev &
```

- [ ] **Step 2: Start wsh server**

```bash
nix develop -c sh -c "cargo run -- server --bind 127.0.0.1:9090 --ephemeral --server-name web-ui-test"
```

- [ ] **Step 3: Create a session**

```bash
curl -s http://127.0.0.1:9090/sessions -X POST -H 'Content-Type: application/json' -d '{"name":"test"}' | jq .
```

- [ ] **Step 4: Create an overlay and verify it renders**

```bash
curl -s http://127.0.0.1:9090/sessions/test/overlays -X POST -H 'Content-Type: application/json' -d '{
  "id": "hello",
  "x": 5,
  "y": 3,
  "z": 1,
  "width": 20,
  "height": 3,
  "background": {"bg": {"r": 40, "g": 40, "b": 80}},
  "spans": [{"text": "Hello from overlay!", "fg": "green", "bold": true}]
}'
```

Open the web UI and verify:
- The overlay appears at approximately column 5, row 3
- It has a dark blue background
- Text is green and bold
- It appears in the sidebar thumbnail too

- [ ] **Step 5: Create a panel and verify it renders**

```bash
curl -s http://127.0.0.1:9090/sessions/test/panels -X POST -H 'Content-Type: application/json' -d '{
  "id": "status",
  "position": "bottom",
  "height": 1,
  "z": 1,
  "background": {"bg": {"r": 30, "g": 30, "b": 60}},
  "spans": [{"text": " STATUS: Connected ", "fg": "cyan"}]
}'
```

Verify:
- A 1-row panel appears at the bottom of the terminal
- Terminal content is pushed up by 1 row
- Panel has the dark background with cyan text
- Panel appears in the sidebar thumbnail

- [ ] **Step 6: Test z-ordering with overlapping overlays**

```bash
curl -s http://127.0.0.1:9090/sessions/test/overlays -X POST -H 'Content-Type: application/json' -d '{
  "id": "behind",
  "x": 7,
  "y": 4,
  "z": 0,
  "width": 20,
  "height": 3,
  "background": {"bg": "red"},
  "spans": [{"text": "Behind overlay"}]
}'
```

Verify: The "hello" overlay (z=1) renders on top of the "behind" overlay (z=0).

- [ ] **Step 7: Test region writes**

```bash
curl -s http://127.0.0.1:9090/sessions/test/overlays -X PUT -H 'Content-Type: application/json' -d '{
  "id": "hello",
  "x": 5,
  "y": 3,
  "z": 1,
  "width": 20,
  "height": 3,
  "background": {"bg": {"r": 40, "g": 40, "b": 80}},
  "spans": [{"text": "Hello from overlay!", "fg": "green", "bold": true}],
  "region_writes": [{"row": 2, "col": 0, "text": "Region write!", "fg": "yellow"}]
}'
```

Verify: "Region write!" appears on row 2 of the overlay in yellow.

- [ ] **Step 8: Clean up test server**

```bash
curl -s http://127.0.0.1:9090/sessions/test -X DELETE
kill %1  # vite dev server
```

---

### Task 12: Update skill documentation

The spec's "Files Modified" table is the source of truth. After implementation, verify the skills documentation is up-to-date per `CLAUDE.md` instructions. The core skill should describe the overlay/panel API; the visual-feedback skill should describe the "what" without protocol specifics.

- [ ] **Step 1: Check if `skills/wsh/visual-feedback.md` mentions web UI rendering**

If not, add a brief note that overlays and panels render in the web UI (both main terminal and sidebar thumbnails) when connected via WebSocket with overlay event subscription.

- [ ] **Step 2: Commit any doc updates**

```bash
git add skills/
git commit -m "docs: update skills with web UI overlay/panel rendering info"
```
