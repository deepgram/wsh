# Web UI Polish Design

Date: 2026-02-20

## Context

After the web UI redesign, several usability issues were identified through manual testing. This design addresses 8 interconnected issues spanning sidebar previews, carousel rendering, hotkeys, focus management, queue interaction, grid navigation, tagging discoverability, and view mode indicators.

## Issues Addressed

1. Sidebar mini-previews show only first ~7 lines, clipped, showing stale top-of-terminal content
2. Tagging is hard to discover (hover-only `+` button)
3. Carousel rendering broken: off-center with 1 session, overlapping ghosts with 2 sessions
4. Grid mode has no keyboard navigation
5. Queue mode has no way to manually select specific sessions
6. Hotkeys use Super modifier which conflicts with Sway WM and browser shortcuts
7. Focus management: typing doesn't auto-target the terminal; shortcut sheet requires double-Escape
8. View mode not indicated in sidebar

## Design

### 1. Sidebar Mini-Previews as View Mode Replicas (Issues #1 + #8)

**Replace** the current `MiniTerminal` (first 8 lines of terminal text) with a **miniature replica of the group's active view mode**.

Each group's sidebar preview renders a tiny version of what the main content area would show if that group were selected:

- **Carousel group**: Flat mini-carousel layout. Center session rendered with real terminal text at tiny scale, flanking sessions as thinner faded panels.
- **Tiled group**: Mini grid matching the NxM layout from `computeGridLayout()`. Each cell contains real terminal text at tiny scale.
- **Queue group**: Mini queue representation. Show pending count, a highlighted "current" cell with real content, and muted active cells.

**Implementation:**
- New component `MiniViewPreview` replaces `MiniTerminal` in `Sidebar.tsx`
- Accepts `group` prop (contains sessions list and tag)
- Reads `viewModePerGroup` to determine which mini layout to render
- Each mini layout is a simplified, non-interactive version of its full counterpart
- Terminal text is rendered at 3-4px font size, no styled spans, bottom-aligned (show recent activity)
- All mini sessions show the **bottom** N lines of the screen buffer, not the top

**Removed:** `MiniTerminal` component (folded into `MiniViewPreview`)

### 2. Tagging Discoverability (Issue #2)

Current tag editing via hover-only `+` button is hard to find with a single session in the "untagged" group.

**Changes:**
- Right-click (contextmenu event) on a sidebar preview cell opens the tag editor
- Command palette gains a "Tag session..." action that lists sessions, then opens an inline tag input
- The `+` button remains for hover discovery but is no longer the only entry point

### 3. Carousel Rendering Fixes (Issue #3)

**1 session:** Fix centering. Remove `left: 15%` on `.carousel-center` when there are no side slides. The single session should occupy the full carousel track, centered.

**2 sessions:** Show center + one side only. When `sessions.length === 2`, render center and next (or prev), not both. The duplicate session appearing on both sides creates the ghost overlap.

**3+ sessions:** Current 3D depth behavior is correct. Ensure side slides are keyed by session name so React/Preact doesn't reuse stale DOM when rotating.

**CSS fix for 1-session centering:**
```css
/* When single session, carousel-center fills the track */
.carousel-slide.carousel-center:only-child {
  width: 100%;
  left: 0;
}
```

**Component fix:** In `DepthCarousel.tsx`, add a `sessions.length === 2` branch that renders only center + one adjacent slide.

### 4. Grid Mode Keyboard Navigation (Issue #4)

**New bindings (Ctrl+Shift+Arrows):**
- Ctrl+Shift+ArrowRight/Left: Move focus to adjacent cell in same row
- Ctrl+Shift+ArrowDown/Up: Move focus to cell in adjacent row (same column index, clamped)

**Implementation:**
- Add a `useEffect` keydown handler in `AutoGrid.tsx`
- Compute current focused cell's row/col from `orderedSessions` and `layout`
- Arrow navigation wraps within the grid (or clamps at edges)
- Focused cell gets the terminal input focus

**Existing behavior preserved:**
- Click: Select/focus a cell
- Double-click: Switch to carousel mode
- Drag-to-swap: Reorder cells

### 5. Queue Mode Manual Selection (Issue #5)

**Behavior:**
- Clicking any thumbnail (pending or active) in the queue top bar brings that session to the foreground
- This is a "manual override" tracked in local state (`manualSelection: string | null`)
- When a manual selection is active, the queue center shows that session instead of the first pending

**Dismiss logic:**
- Ctrl+Shift+Enter (dismiss): If the displayed session is the manual selection, move it to "active/handled" and clear `manualSelection`. Next pending takes over.
- If the displayed session was the auto-queued first-pending, dismiss works as before.

**Navigation without dismissing:**
- Clicking another session while one is manually selected: previous manual selection returns to its original queue position (pending or active). New clicked session becomes the manual selection.
- The queue continues processing in the background (quiescence watchers keep running).

**No visual indicator** of manual override. The interaction should feel seamless.

### 6. Hotkey Overhaul (Issue #6)

**Drop Super modifier entirely.** Primary modifier becomes **Ctrl+Shift**.

| Binding | Action | Component |
|---------|--------|-----------|
| Ctrl+Shift+K | Command palette | app.tsx |
| Ctrl+Shift+/ | Shortcut help | app.tsx |
| Ctrl+Shift+B | Toggle sidebar | app.tsx |
| Ctrl+Shift+O | New session | app.tsx |
| Ctrl+Shift+W | Kill focused session | app.tsx |
| Ctrl+Shift+1-9 | Jump to Nth session | app.tsx |
| Ctrl+Shift+Tab | Next sidebar group | app.tsx |
| Ctrl+Shift+F | Carousel mode | MainContent.tsx |
| Ctrl+Shift+G | Tiled mode | MainContent.tsx |
| Ctrl+Shift+Q | Queue mode | MainContent.tsx |
| Ctrl+Shift+Arrows | Carousel rotate / Grid navigate | DepthCarousel.tsx / AutoGrid.tsx |
| Ctrl+Shift+Enter | Dismiss queue item | QueueView.tsx |

**Avoided combos** (browser conflicts):
- Ctrl+Shift+N (Chrome incognito)
- Ctrl+Shift+T (reopen tab)
- Ctrl+Shift+I/J (devtools/console)
- Ctrl+Shift+P (Firefox private browsing)

**Why Ctrl+Shift+O for new session** (not N): Ctrl+Shift+N opens incognito in Chrome. "O" for "open" avoids the conflict.

**Removed:** All `e.metaKey` (Super) checks. The `e.ctrlKey && e.shiftKey` path becomes the sole binding.

Update `ShortcutSheet.tsx` CATEGORIES to reflect new bindings.

### 7. Focus Management Overhaul (Issue #7)

#### Desktop: Direct Terminal Input

**Eliminate the InputBar on desktop.** The terminal itself becomes the keyboard input target.

**Architecture:**
- Add a hidden `<textarea>` inside `Terminal.tsx`, positioned absolutely over the terminal content
- The textarea is invisible (opacity 0, size 1x1) but focusable
- `SessionPane` auto-focuses the hidden textarea when the session is selected
- All `keydown` events on the textarea are intercepted, converted to escape sequences (reusing `keyToSequence` logic from InputBar), and sent to the PTY
- `input`/`compositionend` events handle IME composition and paste
- The textarea's value is cleared after each input event

**Focus flow:**
1. Session becomes focused (carousel rotate, grid click, queue display) → auto-focus hidden textarea
2. User clicks terminal content → focus hidden textarea
3. User types → keystrokes go to PTY
4. User presses Ctrl+Shift+<key> → intercepted as UI shortcut, not sent to PTY
5. User clicks outside terminal (sidebar, palette) → terminal loses focus, that's fine

**SessionPane changes:**
- Remove `InputBar` import and rendering on desktop (`pointer: fine`)
- Keep `InputBar` on mobile (`pointer: coarse`) for virtual keyboard trigger

#### Mobile: Keep InputBar

- `SessionPane` conditionally renders `InputBar` only when `pointer: coarse`
- InputBar behavior unchanged from current implementation
- Mobile hotkeys use the same Ctrl+Shift modifier (works with physical keyboards on tablets)

#### Shortcut Sheet Focus Fix

- Remove `autoFocus` from the filter `<input>` in `ShortcutSheet.tsx`
- Escape handler on the backdrop closes the dialog immediately (no focus interference)
- Filter input focuses only on click or when user starts typing (delegated keydown on the dialog)

### Component Changes Summary

| File | Changes |
|------|---------|
| `MiniTerminal.tsx` | **Delete** (replaced by MiniViewPreview) |
| `MiniViewPreview.tsx` | **New** — renders mini carousel/grid/queue per group |
| `Sidebar.tsx` | Replace MiniTerminal with MiniViewPreview; add contextmenu handler for tag editing |
| `DepthCarousel.tsx` | Fix 1-session centering, 2-session rendering; update hotkey modifier |
| `AutoGrid.tsx` | Add Ctrl+Shift+Arrow keyboard navigation |
| `QueueView.tsx` | Add manual selection state, click-to-override on thumbnails, update dismiss logic |
| `Terminal.tsx` | Add hidden textarea for desktop input capture |
| `SessionPane.tsx` | Conditional InputBar (mobile only); auto-focus terminal on session focus |
| `InputBar.tsx` | Keep for mobile; remove desktop usage |
| `MainContent.tsx` | Update hotkey modifier from Super to Ctrl+Shift |
| `ShortcutSheet.tsx` | Update keybinding list; remove autoFocus on filter input |
| `app.tsx` | Update all hotkey handlers from Super to Ctrl+Shift |
| `terminal.css` | Fix carousel-center centering; add hidden textarea styles |

### Migration Notes

- No API changes required (all changes are client-side)
- No new dependencies
- The `keyToSequence` function moves from `InputBar.tsx` to a shared utility (used by both InputBar on mobile and Terminal.tsx hidden textarea on desktop)
