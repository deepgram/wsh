# Web UI Polish Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix 8 interconnected web UI usability issues: sidebar previews, carousel rendering, hotkeys, focus management, queue interaction, grid navigation, tagging discoverability, and view mode indicators.

**Architecture:** All changes are client-side in `web/src/`. No Rust/API changes. The `keyToSequence` function is extracted to a shared utility. The InputBar is replaced on desktop by a hidden textarea in Terminal.tsx. Sidebar mini-previews become miniature replicas of each group's view mode. All hotkeys switch from Super to Ctrl+Shift.

**Tech Stack:** Preact, @preact/signals, TypeScript, CSS

---

### Task 1: Extract `keyToSequence` to shared utility

**Files:**
- Create: `web/src/utils/keymap.ts`
- Modify: `web/src/components/InputBar.tsx`

**Step 1: Create `web/src/utils/keymap.ts`**

Move the `keyToSequence` function and `lineToPlainText` helper from InputBar.tsx into a new shared utility file. These will be used by both InputBar (mobile) and Terminal.tsx (desktop).

```typescript
import type { FormattedLine } from "../api/types";

/**
 * Map a KeyboardEvent to the terminal escape sequence it represents.
 * Returns null if the event is not a recognized terminal key.
 */
export function keyToSequence(e: KeyboardEvent): string | null {
  // Ctrl combos
  if (e.ctrlKey && !e.altKey && !e.metaKey) {
    const key = e.key.toLowerCase();
    if (key.length === 1 && key >= "a" && key <= "z") {
      return String.fromCharCode(key.charCodeAt(0) - 96);
    }
    if (key === "[") return "\x1b";
    if (key === "\\") return "\x1c";
    if (key === "]") return "\x1d";
    return null;
  }

  // Alt combos — send ESC prefix
  if (e.altKey && !e.ctrlKey && !e.metaKey) {
    if (e.key.length === 1) {
      return "\x1b" + e.key;
    }
  }

  switch (e.key) {
    case "Enter": return "\r";
    case "Backspace": return "\x7f";
    case "Tab": return "\t";
    case "Escape": return "\x1b";
    case "ArrowUp": return "\x1b[A";
    case "ArrowDown": return "\x1b[B";
    case "ArrowRight": return "\x1b[C";
    case "ArrowLeft": return "\x1b[D";
    case "Home": return "\x1b[H";
    case "End": return "\x1b[F";
    case "PageUp": return "\x1b[5~";
    case "PageDown": return "\x1b[6~";
    case "Insert": return "\x1b[2~";
    case "Delete": return "\x1b[3~";
    case "F1": return "\x1bOP";
    case "F2": return "\x1bOQ";
    case "F3": return "\x1bOR";
    case "F4": return "\x1bOS";
    case "F5": return "\x1b[15~";
    case "F6": return "\x1b[17~";
    case "F7": return "\x1b[18~";
    case "F8": return "\x1b[19~";
    case "F9": return "\x1b[20~";
    case "F10": return "\x1b[21~";
    case "F11": return "\x1b[23~";
    case "F12": return "\x1b[24~";
    default: return null;
  }
}

export function lineToPlainText(line: FormattedLine): string {
  if (typeof line === "string") return line;
  return line.map((span) => span.text).join("");
}
```

**Step 2: Update `InputBar.tsx` to import from shared utility**

Replace the local `keyToSequence` function and `lineToPlainText` with imports:

```typescript
import { keyToSequence, lineToPlainText } from "../utils/keymap";
```

Delete the local `keyToSequence` function (lines 13-88) and `lineToPlainText` (lines 91-94) from InputBar.tsx.

**Step 3: Verify the build compiles**

Run: `cd /home/ajsyp/Projects/deepgram/wsh && nix develop -c sh -c "cd web && npx tsc --noEmit"`
Expected: No errors

**Step 4: Commit**

```bash
git add web/src/utils/keymap.ts web/src/components/InputBar.tsx
git commit -m "refactor(web): extract keyToSequence to shared utility"
```

---

### Task 2: Hotkey overhaul — Switch from Super to Ctrl+Shift

**Files:**
- Modify: `web/src/app.tsx` (lines 133-200)
- Modify: `web/src/components/MainContent.tsx` (lines 58-78)
- Modify: `web/src/components/DepthCarousel.tsx` (lines 32-48)
- Modify: `web/src/components/QueueView.tsx` (lines 82-91)
- Modify: `web/src/components/ShortcutSheet.tsx` (lines 13-49)

**Step 1: Update `app.tsx` global hotkeys**

In the `useEffect` keydown handler (line 133), replace the modifier detection:

Old (lines 135-137):
```typescript
const superKey = e.metaKey;
const fallback = e.ctrlKey && e.shiftKey;
if (!superKey && !fallback) return;
```

New:
```typescript
if (!e.ctrlKey || !e.shiftKey) return;
// Ignore if Alt or Meta also held (avoid triple-modifier conflicts)
if (e.altKey || e.metaKey) return;
```

Also change `key === "n" || key === "N"` to `key === "o" || key === "O"` (Ctrl+Shift+O for new session, avoids Chrome incognito conflict with Ctrl+Shift+N).

Change `key === "?"` to `key === "/"` (Ctrl+Shift+/ for shortcut help — no shift needed for `/`).

Note: When Shift is held, `e.key` for `/` is `?` on US keyboards. So we check for both: `key === "/" || key === "?"`.

**Step 2: Update `MainContent.tsx` view mode hotkeys**

In the `useEffect` handler (line 59), replace modifier detection:

Old (lines 60-62):
```typescript
const superKey = e.metaKey;
const fallback = e.ctrlKey && e.shiftKey;
if (!superKey && !fallback) return;
```

New:
```typescript
if (!e.ctrlKey || !e.shiftKey) return;
if (e.altKey || e.metaKey) return;
```

**Step 3: Update `DepthCarousel.tsx` navigation hotkeys**

In the `useEffect` handler (line 33), replace modifier detection:

Old (lines 34-36):
```typescript
const superKey = e.metaKey;
const fallback = e.ctrlKey && e.shiftKey;
if (!superKey && !fallback) return;
```

New:
```typescript
if (!e.ctrlKey || !e.shiftKey) return;
if (e.altKey || e.metaKey) return;
```

**Step 4: Update `QueueView.tsx` dismiss hotkey**

In the `useEffect` handler (line 83), replace modifier detection:

Old (line 84):
```typescript
if ((e.metaKey || (e.ctrlKey && e.shiftKey)) && e.key === "Enter") {
```

New:
```typescript
if (e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey && e.key === "Enter") {
```

**Step 5: Update `ShortcutSheet.tsx` displayed bindings**

Replace the `CATEGORIES` array (lines 13-49) with updated bindings:

```typescript
const CATEGORIES: ShortcutCategory[] = [
  {
    label: "Navigation",
    shortcuts: [
      { keys: "Ctrl+Shift+Left/Right", description: "Carousel rotate / Grid navigate" },
      { keys: "Ctrl+Shift+Up/Down", description: "Grid navigate rows" },
      { keys: "Ctrl+Shift+1-9", description: "Jump to Nth session" },
      { keys: "Ctrl+Shift+Tab", description: "Next sidebar group" },
    ],
  },
  {
    label: "View Modes",
    shortcuts: [
      { keys: "Ctrl+Shift+F", description: "Carousel mode" },
      { keys: "Ctrl+Shift+G", description: "Tiled mode" },
      { keys: "Ctrl+Shift+Q", description: "Queue mode" },
    ],
  },
  {
    label: "Session Management",
    shortcuts: [
      { keys: "Ctrl+Shift+O", description: "New session" },
      { keys: "Ctrl+Shift+W", description: "Kill focused session" },
      { keys: "Ctrl+Shift+Enter", description: "Dismiss queue item" },
    ],
  },
  {
    label: "UI",
    shortcuts: [
      { keys: "Ctrl+Shift+B", description: "Toggle sidebar" },
      { keys: "Ctrl+Shift+K", description: "Command palette" },
      { keys: "Ctrl+Shift+/", description: "This help" },
    ],
  },
];
```

**Step 6: Verify build**

Run: `cd /home/ajsyp/Projects/deepgram/wsh && nix develop -c sh -c "cd web && npx tsc --noEmit"`
Expected: No errors

**Step 7: Commit**

```bash
git add web/src/app.tsx web/src/components/MainContent.tsx web/src/components/DepthCarousel.tsx web/src/components/QueueView.tsx web/src/components/ShortcutSheet.tsx
git commit -m "feat(web): switch all hotkeys from Super to Ctrl+Shift"
```

---

### Task 3: Focus management — Desktop direct terminal input

**Files:**
- Modify: `web/src/components/Terminal.tsx`
- Modify: `web/src/components/SessionPane.tsx`
- Modify: `web/src/styles/terminal.css`

**Step 1: Add hidden textarea to `Terminal.tsx`**

Add a new prop `onInput` and a hidden textarea to the Terminal component. The textarea captures keyboard input and forwards it via the callback.

Add after the existing imports in Terminal.tsx:
```typescript
import { keyToSequence } from "../utils/keymap";
import { focusedSession } from "../state/sessions";
```

Add a new prop to the interface:
```typescript
interface TerminalProps {
  session: string;
  client?: WshClient;
  /** If true, embed a hidden textarea for direct keyboard input (desktop mode). */
  captureInput?: boolean;
}
```

Inside the `Terminal` component function, add a ref for the textarea and an effect to auto-focus:

```typescript
const textareaRef = useRef<HTMLTextAreaElement>(null);

// Auto-focus textarea when this session is focused (desktop input capture)
const isFocused = session === focusedSession.value;
useEffect(() => {
  if (captureInput && isFocused && textareaRef.current) {
    textareaRef.current.focus();
  }
}, [captureInput, isFocused]);

// Handle keyboard input from hidden textarea
const handleTextareaKeyDown = useCallback((e: KeyboardEvent) => {
  if (!client) return;

  // Let Ctrl+Shift combos bubble up for UI shortcuts
  if (e.ctrlKey && e.shiftKey) return;

  const seq = keyToSequence(e);
  if (seq !== null) {
    e.preventDefault();
    client.sendInput(session, seq).catch(() => {});
  }
}, [client, session]);

// Handle text input (printable characters, IME, paste) from hidden textarea
const handleTextareaInput = useCallback(() => {
  if (!client) return;
  const ta = textareaRef.current;
  if (!ta) return;
  const value = ta.value;
  if (value) {
    client.sendInput(session, value).catch(() => {});
    ta.value = "";
  }
}, [client, session]);

// Click on terminal container focuses the hidden textarea
const handleContainerClick = useCallback(() => {
  if (captureInput && textareaRef.current) {
    textareaRef.current.focus();
  }
}, [captureInput]);
```

Add `onClick={handleContainerClick}` to the terminal container div.

Add the hidden textarea inside the container div, right after the measurement span:

```tsx
{captureInput && (
  <textarea
    ref={textareaRef}
    class="terminal-hidden-input"
    onKeyDown={handleTextareaKeyDown}
    onInput={handleTextareaInput}
    autocomplete="off"
    autocapitalize="off"
    autocorrect="off"
    spellcheck={false}
    aria-label={`Terminal input for ${session}`}
  />
)}
```

**Step 2: Add CSS for hidden textarea**

Add to `terminal.css` after the `.terminal-container` rules (around line 103):

```css
.terminal-hidden-input {
  position: absolute;
  top: 0;
  left: 0;
  width: 1px;
  height: 1px;
  opacity: 0;
  border: none;
  outline: none;
  padding: 0;
  margin: 0;
  resize: none;
  overflow: hidden;
  z-index: -1;
  font-size: 16px; /* Prevent iOS zoom on focus */
}
```

**Step 3: Update `SessionPane.tsx` — conditional InputBar**

```typescript
import { useState, useEffect } from "preact/hooks";
import { Terminal } from "./Terminal";
import { InputBar } from "./InputBar";
import type { WshClient } from "../api/ws";

interface SessionPaneProps {
  session: string;
  client: WshClient;
}

export function SessionPane({ session, client }: SessionPaneProps) {
  const [isMobile, setIsMobile] = useState(false);

  useEffect(() => {
    const mq = window.matchMedia("(pointer: coarse)");
    setIsMobile(mq.matches);
    const handler = (e: MediaQueryListEvent) => setIsMobile(e.matches);
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, []);

  return (
    <div class="session-pane">
      <Terminal session={session} client={client} captureInput={!isMobile} />
      {isMobile && <InputBar session={session} client={client} />}
    </div>
  );
}
```

**Step 4: Verify build**

Run: `cd /home/ajsyp/Projects/deepgram/wsh && nix develop -c sh -c "cd web && npx tsc --noEmit"`
Expected: No errors

**Step 5: Commit**

```bash
git add web/src/components/Terminal.tsx web/src/components/SessionPane.tsx web/src/styles/terminal.css
git commit -m "feat(web): direct terminal input on desktop, keep InputBar on mobile"
```

---

### Task 4: Fix ShortcutSheet focus (double-Escape issue)

**Files:**
- Modify: `web/src/components/ShortcutSheet.tsx`

**Step 1: Remove autoFocus from filter input**

In `ShortcutSheet.tsx`, remove `autoFocus` from the filter input (line 96). The filter input should only focus when the user clicks it or starts typing.

Old (line 89-96):
```tsx
<input
  class="shortcut-filter"
  type="text"
  placeholder="Filter shortcuts..."
  value={filter}
  onInput={(e) => setFilter((e.target as HTMLInputElement).value)}
  autoFocus
/>
```

New:
```tsx
<input
  class="shortcut-filter"
  type="text"
  placeholder="Filter shortcuts..."
  value={filter}
  onInput={(e) => setFilter((e.target as HTMLInputElement).value)}
/>
```

**Step 2: Add delegated keydown to forward typing into filter**

Add a keydown handler on the shortcut-sheet div that delegates printable characters to the filter input:

```typescript
const handleSheetKeyDown = useCallback((e: KeyboardEvent) => {
  // If a printable character is typed and the filter isn't focused, focus it
  if (e.key.length === 1 && !e.ctrlKey && !e.altKey && !e.metaKey) {
    const filterInput = containerRef.current?.querySelector(".shortcut-filter") as HTMLInputElement | null;
    if (filterInput && document.activeElement !== filterInput) {
      filterInput.focus();
      // The character will be captured by the input naturally
    }
  }
}, []);
```

Add `onKeyDown={handleSheetKeyDown}` to the `.shortcut-sheet` div.

**Step 3: Verify build**

Run: `cd /home/ajsyp/Projects/deepgram/wsh && nix develop -c sh -c "cd web && npx tsc --noEmit"`
Expected: No errors

**Step 4: Commit**

```bash
git add web/src/components/ShortcutSheet.tsx
git commit -m "fix(web): shortcut sheet no longer steals focus on open"
```

---

### Task 5: Carousel rendering fixes

**Files:**
- Modify: `web/src/components/DepthCarousel.tsx`
- Modify: `web/src/styles/terminal.css`

**Step 1: Fix 1-session centering**

In `DepthCarousel.tsx`, the single-session branch (lines 53-63) already exists but the CSS `.carousel-center` has `width: 70%` and `left: 15%`. Add CSS override for when center is the only child:

Add to `terminal.css` after the `.carousel-center` block (around line 1457):
```css
/* Single session: fill the entire track */
.carousel-track > .carousel-slide:only-child {
  width: 100%;
  left: 0;
}
```

**Step 2: Fix 2-session carousel**

In `DepthCarousel.tsx`, add a new branch for exactly 2 sessions. Between the `sessions.length === 1` early return (line 63) and the mobile check (line 66), add:

```typescript
// Two sessions — show center + one side (not both, which would show the same session twice)
if (sessions.length === 2 && !isMobile) {
  const otherIndex = currentIndex === 0 ? 1 : 0;
  const isOtherNext = otherIndex > currentIndex || (currentIndex === 1 && otherIndex === 0);
  return (
    <div class="depth-carousel">
      <div class="carousel-track">
        {!isOtherNext && (
          <div class="carousel-slide carousel-prev" onClick={() => navigate(-1)} key={sessions[otherIndex]}>
            <SessionPane session={sessions[otherIndex]} client={client} />
          </div>
        )}
        <div class="carousel-slide carousel-center" key={sessions[currentIndex]}>
          <SessionPane session={sessions[currentIndex]} client={client} />
          <div class="carousel-focus-ring" />
        </div>
        {isOtherNext && (
          <div class="carousel-slide carousel-next" onClick={() => navigate(1)} key={sessions[otherIndex]}>
            <SessionPane session={sessions[otherIndex]} client={client} />
          </div>
        )}
      </div>
    </div>
  );
}
```

**Step 3: Add keys to 3+ session slides**

In the desktop 3D depth section (lines 89-108), ensure slides have `key` props based on session name to prevent stale DOM reuse:

Change:
```tsx
<div class="carousel-slide carousel-prev" onClick={() => navigate(-1)}>
```
To:
```tsx
<div class="carousel-slide carousel-prev" onClick={() => navigate(-1)} key={sessions[prevIndex]}>
```

Same for `carousel-next`: add `key={sessions[nextIndex]}`.

And for center: add `key={sessions[currentIndex]}`.

**Step 4: Verify build**

Run: `cd /home/ajsyp/Projects/deepgram/wsh && nix develop -c sh -c "cd web && npx tsc --noEmit"`
Expected: No errors

**Step 5: Commit**

```bash
git add web/src/components/DepthCarousel.tsx web/src/styles/terminal.css
git commit -m "fix(web): carousel centering for 1 session, no ghost for 2 sessions"
```

---

### Task 6: Grid mode keyboard navigation

**Files:**
- Modify: `web/src/components/AutoGrid.tsx`

**Step 1: Add Ctrl+Shift+Arrow keyboard navigation**

Add a `useEffect` in `AutoGrid.tsx` for arrow key navigation. Place it after the existing hooks:

```typescript
// Keyboard navigation: Ctrl+Shift+Arrows to move focus between cells
useEffect(() => {
  const handler = (e: KeyboardEvent) => {
    if (!e.ctrlKey || !e.shiftKey || e.altKey || e.metaKey) return;
    if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(e.key)) return;

    e.preventDefault();

    const currentFocused = focusedSession.value;
    const currentIdx = orderedSessions.indexOf(currentFocused ?? "");
    if (currentIdx < 0 && orderedSessions.length > 0) {
      // Nothing focused — focus first cell
      focusedSession.value = orderedSessions[0];
      return;
    }

    // Compute grid position from layout
    const cols = layout.length > 0 ? layout[0].count : 1;
    const row = Math.floor(currentIdx / cols);
    const col = currentIdx % cols;

    let newRow = row;
    let newCol = col;

    switch (e.key) {
      case "ArrowLeft":
        newCol = Math.max(0, col - 1);
        break;
      case "ArrowRight":
        newCol = Math.min(cols - 1, col + 1);
        break;
      case "ArrowUp":
        newRow = Math.max(0, row - 1);
        break;
      case "ArrowDown":
        newRow = Math.min(layout.length - 1, row + 1);
        break;
    }

    // Clamp column to row's actual cell count
    const rowCellCount = layout[newRow]?.count ?? cols;
    newCol = Math.min(newCol, rowCellCount - 1);

    const newIdx = newRow * cols + newCol;
    if (newIdx >= 0 && newIdx < orderedSessions.length) {
      focusedSession.value = orderedSessions[newIdx];
    }
  };
  window.addEventListener("keydown", handler);
  return () => window.removeEventListener("keydown", handler);
}, [orderedSessions, layout]);
```

**Step 2: Verify build**

Run: `cd /home/ajsyp/Projects/deepgram/wsh && nix develop -c sh -c "cd web && npx tsc --noEmit"`
Expected: No errors

**Step 3: Commit**

```bash
git add web/src/components/AutoGrid.tsx
git commit -m "feat(web): Ctrl+Shift+Arrow keyboard navigation in grid mode"
```

---

### Task 7: Queue mode manual selection

**Files:**
- Modify: `web/src/components/QueueView.tsx`

**Step 1: Add manual selection state**

Add a `manualSelection` state to QueueView:

```typescript
const [manualSelection, setManualSelection] = useState<string | null>(null);
```

**Step 2: Update current session logic**

Change the `currentSession` derivation to account for manual override:

Old (lines 24-25):
```typescript
const currentEntry = pending[0] || null;
const currentSession = currentEntry?.session || null;
```

New:
```typescript
// Manual selection overrides queue order
const autoSession = pending[0]?.session || null;
const currentSession = manualSelection && sessions.includes(manualSelection)
  ? manualSelection
  : autoSession;
```

**Step 3: Add click handlers to all thumbnails**

The pending thumbnails already have `onClick` (line 104). Update them and add handlers to active and handled thumbnails:

For pending thumbnails, change the onClick:
```typescript
onClick={() => setManualSelection(e.session)}
```

For active thumbnails (line 114-117), add onClick:
```typescript
<div key={s} class={`queue-thumb ${s === currentSession ? "active" : ""}`}
  onClick={() => setManualSelection(s)}>
```

For handled thumbnails (line 119-122), add onClick:
```typescript
<div key={e.session} class={`queue-thumb handled ${e.session === currentSession ? "active" : ""}`}
  onClick={() => setManualSelection(e.session)}>
```

**Step 4: Update dismiss logic**

Update `handleDismiss` to clear manual selection:

```typescript
const handleDismiss = useCallback(() => {
  if (!currentSession) return;

  // If this was a manual selection of a session not in pending, just clear the override
  const isPending = pending.some((e) => e.session === currentSession);
  if (isPending) {
    dismissQueueEntry(groupTag, currentSession);
  }

  // Clear manual selection — return to auto queue
  setManualSelection(null);
}, [groupTag, currentSession, pending]);
```

**Step 5: Clear manual selection when clicking another session**

The `setManualSelection(newSession)` calls already handle this — setting a new value replaces the old one, and the previously-selected session naturally returns to its queue position.

**Step 6: Verify build**

Run: `cd /home/ajsyp/Projects/deepgram/wsh && nix develop -c sh -c "cd web && npx tsc --noEmit"`
Expected: No errors

**Step 7: Commit**

```bash
git add web/src/components/QueueView.tsx
git commit -m "feat(web): click any queue thumbnail to override queue order"
```

---

### Task 8: Tagging discoverability — right-click context menu

**Files:**
- Modify: `web/src/components/Sidebar.tsx`
- Modify: `web/src/components/CommandPalette.tsx`

**Step 1: Add contextmenu handler to sidebar preview cells**

In `Sidebar.tsx`, add an `onContextMenu` handler to the `.sidebar-preview-cell` div (line 122-126):

```tsx
<div
  key={s}
  class="sidebar-preview-cell"
  draggable
  onDragStart={(e: DragEvent) => startSessionDrag(s, e)}
  onDragEnd={endDrag}
  onContextMenu={(e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setEditingSession(s);
  }}
>
```

**Step 2: Add "Tag session..." action to CommandPalette**

In `CommandPalette.tsx`, add a new action in the items builder (after the "Toggle Sidebar" action, around line 96):

```typescript
// Tag session actions — one per session
for (const name of sessions.value) {
  result.push({
    type: "action",
    label: `Tag: ${name}`,
    description: "Add or remove tags for this session",
    action: () => {
      // Focus the session and open tag editor in sidebar
      focusedSession.value = name;
      // We'll need to signal the sidebar to open the tag editor.
      // For now, just close the palette and the user can right-click.
      onClose();
    },
  });
}
```

Note: A full inline tag editor in the command palette would require more state coordination. For this iteration, the palette action focuses the session and closes; the user then uses right-click or the `+` button. A future enhancement could embed a tag input directly in the palette.

**Step 3: Verify build**

Run: `cd /home/ajsyp/Projects/deepgram/wsh && nix develop -c sh -c "cd web && npx tsc --noEmit"`
Expected: No errors

**Step 4: Commit**

```bash
git add web/src/components/Sidebar.tsx web/src/components/CommandPalette.tsx
git commit -m "feat(web): right-click to tag sessions, tag actions in command palette"
```

---

### Task 9: Sidebar mini-preview as view mode replica

**Files:**
- Create: `web/src/components/MiniViewPreview.tsx`
- Modify: `web/src/components/Sidebar.tsx`
- Modify: `web/src/styles/terminal.css`
- Delete: `web/src/components/MiniTerminal.tsx` (after migration)

This is the largest task. The `MiniViewPreview` component renders a tiny replica of the group's active view mode.

**Step 1: Create `MiniViewPreview.tsx`**

```tsx
import { getScreenSignal } from "../state/terminal";
import { getViewModeForGroup, quiescenceQueues } from "../state/groups";
import type { Group } from "../state/groups";
import type { FormattedLine } from "../api/types";

interface MiniViewPreviewProps {
  group: Group;
}

/** Render the bottom N lines of a session's screen as plain text. */
function MiniTermContent({ session, maxLines }: { session: string; maxLines: number }) {
  const screen = getScreenSignal(session).value;
  // Show bottom lines (most recent activity), not top
  const lines = screen.lines;
  const start = Math.max(0, lines.length - maxLines);
  const visibleLines = lines.slice(start);

  return (
    <div class="mini-term-content">
      {visibleLines.map((line: FormattedLine, i: number) => (
        <div key={i} class="mini-term-line">
          {typeof line === "string" ? line : line.map((s) => s.text).join("")}
        </div>
      ))}
    </div>
  );
}

function MiniCarousel({ sessions }: { sessions: string[] }) {
  if (sessions.length === 0) return null;

  if (sessions.length === 1) {
    return (
      <div class="mini-carousel">
        <div class="mini-carousel-center">
          <MiniTermContent session={sessions[0]} maxLines={6} />
        </div>
      </div>
    );
  }

  // Show center + side peeks
  return (
    <div class="mini-carousel">
      <div class="mini-carousel-side mini-carousel-prev">
        <MiniTermContent session={sessions[sessions.length - 1]} maxLines={4} />
      </div>
      <div class="mini-carousel-center">
        <MiniTermContent session={sessions[0]} maxLines={6} />
      </div>
      {sessions.length > 2 && (
        <div class="mini-carousel-side mini-carousel-next">
          <MiniTermContent session={sessions[1]} maxLines={4} />
        </div>
      )}
    </div>
  );
}

function MiniGrid({ sessions }: { sessions: string[] }) {
  if (sessions.length === 0) return null;
  const cols = Math.ceil(Math.sqrt(sessions.length));

  return (
    <div class="mini-grid" style={{ gridTemplateColumns: `repeat(${cols}, 1fr)` }}>
      {sessions.slice(0, 9).map((s) => (
        <div key={s} class="mini-grid-cell">
          <MiniTermContent session={s} maxLines={3} />
        </div>
      ))}
    </div>
  );
}

function MiniQueue({ sessions, groupTag }: { sessions: string[]; groupTag: string }) {
  const queue = quiescenceQueues.value[groupTag] || [];
  const pending = queue.filter((e) => e.status === "pending");
  const currentSession = pending[0]?.session || sessions[0] || null;

  return (
    <div class="mini-queue">
      {currentSession && (
        <div class="mini-queue-current">
          <MiniTermContent session={currentSession} maxLines={5} />
        </div>
      )}
      {pending.length > 1 && (
        <div class="mini-queue-badge">{pending.length} pending</div>
      )}
    </div>
  );
}

export function MiniViewPreview({ group }: MiniViewPreviewProps) {
  const mode = getViewModeForGroup(group.tag);
  const sessions = group.sessions;

  if (sessions.length === 0) return null;

  switch (mode) {
    case "carousel":
      return <MiniCarousel sessions={sessions} />;
    case "tiled":
      return <MiniGrid sessions={sessions} />;
    case "queue":
      return <MiniQueue sessions={sessions} groupTag={group.tag} />;
    default:
      return <MiniCarousel sessions={sessions} />;
  }
}
```

**Step 2: Add CSS for mini view previews**

Add to `terminal.css` after the `.mini-term-line` styles (around line 1166):

```css
/* Mini View Preview — layout replicas in sidebar */

.mini-term-content {
  font-family: var(--font-mono, "JetBrains Mono", "Fira Code", monospace);
  font-size: 3.5px;
  line-height: 1.2;
  padding: 1px;
  color: var(--fg, #ccc);
  overflow: hidden;
  white-space: pre;
}

/* Mini Carousel */
.mini-carousel {
  display: flex;
  align-items: stretch;
  gap: 1px;
  min-height: 28px;
  overflow: hidden;
}

.mini-carousel-center {
  flex: 3;
  background: rgba(0, 0, 0, 0.3);
  border-radius: 2px;
  overflow: hidden;
  border: 1px solid rgba(255, 255, 255, 0.1);
}

.mini-carousel-side {
  flex: 1;
  background: rgba(0, 0, 0, 0.2);
  border-radius: 2px;
  overflow: hidden;
  opacity: 0.5;
}

/* Mini Grid */
.mini-grid {
  display: grid;
  gap: 1px;
  min-height: 28px;
}

.mini-grid-cell {
  background: rgba(0, 0, 0, 0.3);
  border-radius: 1px;
  overflow: hidden;
  min-height: 12px;
}

/* Mini Queue */
.mini-queue {
  position: relative;
  min-height: 28px;
}

.mini-queue-current {
  background: rgba(0, 0, 0, 0.3);
  border-radius: 2px;
  overflow: hidden;
  border: 1px solid rgba(255, 255, 255, 0.1);
}

.mini-queue-badge {
  position: absolute;
  top: 1px;
  right: 1px;
  font-size: 6px;
  background: var(--accent, #f90);
  color: var(--bg, #000);
  border-radius: 3px;
  padding: 0 2px;
  font-weight: 600;
  line-height: 1.4;
}
```

**Step 3: Update `Sidebar.tsx` to use `MiniViewPreview`**

Replace the import:
```typescript
// OLD
import { MiniTerminal } from "./MiniTerminal";
// NEW
import { MiniViewPreview } from "./MiniViewPreview";
```

Replace the preview grid in the sidebar group (lines 117-146). Instead of showing up to 4 individual session previews in a 2x2 grid, show a single `MiniViewPreview` for the whole group:

Old:
```tsx
{g.sessions.length > 0 && (
  <div class="sidebar-preview-grid">
    {g.sessions.slice(0, 4).map((s) => (
      <div
        key={s}
        class="sidebar-preview-cell"
        draggable
        onDragStart={(e: DragEvent) => startSessionDrag(s, e)}
        onDragEnd={endDrag}
      >
        <MiniTerminal session={s} />
        <StatusDot status={statuses.get(s)} />
        <button ... />
        {editingSession === s && <TagEditor ... />}
      </div>
    ))}
  </div>
)}
```

New:
```tsx
{g.sessions.length > 0 && (
  <div class="sidebar-preview-area">
    <MiniViewPreview group={g} />
    {/* Session list with drag/tag support */}
    <div class="sidebar-session-list">
      {g.sessions.slice(0, 6).map((s) => (
        <div
          key={s}
          class="sidebar-session-item"
          draggable
          onDragStart={(e: DragEvent) => startSessionDrag(s, e)}
          onDragEnd={endDrag}
          onContextMenu={(e: MouseEvent) => {
            e.preventDefault();
            e.stopPropagation();
            setEditingSession(s);
          }}
        >
          <StatusDot status={statuses.get(s)} />
          <span class="sidebar-session-name">{s}</span>
          <button
            class="tag-edit-btn"
            onClick={(e: MouseEvent) => { e.stopPropagation(); setEditingSession(s); }}
            title="Edit tags"
          >
            +
          </button>
          {editingSession === s && (
            <TagEditor session={s} client={client} onClose={() => setEditingSession(null)} />
          )}
        </div>
      ))}
    </div>
  </div>
)}
```

**Step 4: Add CSS for sidebar session list**

Add to `terminal.css`:

```css
.sidebar-preview-area {
  margin-top: 6px;
  margin-bottom: 4px;
}

.sidebar-session-list {
  margin-top: 4px;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.sidebar-session-item {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 1px 4px;
  border-radius: 2px;
  cursor: grab;
  position: relative;
  font-size: 9px;
  color: var(--fg, #ccc);
  opacity: 0.6;
  transition: opacity 0.15s, background 0.15s;
}

.sidebar-session-item:hover {
  opacity: 1;
  background: rgba(255, 255, 255, 0.04);
}

.sidebar-session-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
```

**Step 5: Remove `MiniTerminal.tsx`**

Delete the file: `web/src/components/MiniTerminal.tsx`

Also update `QueueView.tsx` to use `MiniViewPreview` or inline term content for its thumbnails. For queue thumbnails, replace:
```typescript
import { MiniTerminal } from "./MiniTerminal";
```
With a simple inline rendering or import `MiniTermContent` if we export it from `MiniViewPreview.tsx`. The simpler approach: export `MiniTermContent` from `MiniViewPreview.tsx` and use it in QueueView thumbnails:

In `MiniViewPreview.tsx`, export `MiniTermContent`.

In `QueueView.tsx`, replace:
```typescript
import { MiniTerminal } from "./MiniTerminal";
```
With:
```typescript
import { MiniTermContent } from "./MiniViewPreview";
```

And replace all `<MiniTerminal session={...} />` with `<MiniTermContent session={...} maxLines={4} />`.

**Step 6: Verify build**

Run: `cd /home/ajsyp/Projects/deepgram/wsh && nix develop -c sh -c "cd web && npx tsc --noEmit"`
Expected: No errors

**Step 7: Commit**

```bash
git add web/src/components/MiniViewPreview.tsx web/src/components/Sidebar.tsx web/src/components/QueueView.tsx web/src/styles/terminal.css
git rm web/src/components/MiniTerminal.tsx
git commit -m "feat(web): sidebar shows mini replica of group view mode"
```

---

### Task 10: Manual testing and visual polish

**Files:**
- Potentially any of the above files for adjustments

**Step 1: Start the dev server**

Run: `cd /home/ajsyp/Projects/deepgram/wsh && nix develop -c sh -c "cd web && npx vite --host"`

In another terminal:
Run: `cd /home/ajsyp/Projects/deepgram/wsh && nix develop -c sh -c "cargo run -- server --bind 127.0.0.1:8080"`

**Step 2: Test each fix manually**

Test checklist:
- [ ] Sidebar previews show mini-carousel/grid/queue based on group view mode
- [ ] Right-click on session in sidebar opens tag editor
- [ ] Carousel: 1 session is centered properly
- [ ] Carousel: 2 sessions show center + one side (no ghost)
- [ ] Carousel: 3+ sessions show 3D depth correctly
- [ ] Grid: Ctrl+Shift+Arrows navigate between cells
- [ ] Queue: Click any thumbnail to override display
- [ ] Queue: Dismiss returns to auto-queue
- [ ] All hotkeys use Ctrl+Shift (not Super)
- [ ] Shortcut sheet: Escape closes immediately (no double-press)
- [ ] Desktop: typing goes directly to terminal (no InputBar visible)
- [ ] Mobile (resize browser narrow): InputBar appears
- [ ] Ctrl+Shift+K opens command palette
- [ ] Ctrl+Shift+/ opens shortcut help
- [ ] Ctrl+Shift+O creates new session

**Step 3: Fix any visual issues found**

Adjust CSS sizing, spacing, or opacity as needed based on visual testing.

**Step 4: Commit any polish fixes**

```bash
git add -u
git commit -m "fix(web): visual polish from manual testing"
```

---

### Task 11: Update documentation

**Files:**
- Modify: `skills/wsh/core.md` (if hotkey info exists)
- Modify: Any other docs referencing old hotkey bindings

**Step 1: Search for references to old hotkeys**

Search all markdown and skill files for "Super+" references and update to "Ctrl+Shift+".

**Step 2: Update README or docs if they reference the web UI shortcuts**

**Step 3: Commit**

```bash
git add -u
git commit -m "docs: update hotkey references to Ctrl+Shift"
```
