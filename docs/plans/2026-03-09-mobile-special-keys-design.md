# Mobile Special Keys Design

**Date:** 2026-03-09
**Status:** Approved

## Problem

The web UI's mobile experience lacks access to special keys that mobile software keyboards don't provide: Ctrl combinations, Tab, Escape, arrow keys, function keys, and navigation keys. This makes basic shell usage (Ctrl+C to cancel, Tab to complete, arrow keys for history) impossible without a physical keyboard.

## Approach

Hybrid: always-visible modifier bar with horizontally scrollable buttons, plus two-finger swipe gestures on the terminal area for arrow key navigation.

## Component Architecture

Two new components, two modified:

### New: `ModifierBar`

A horizontally scrollable row of touch-friendly buttons, rendered between Terminal and InputBar on mobile.

**Button layout** (left to right):

```
visible without scroll:
[ Tab ] [ Esc ] [ Ctrl ] [ Alt ] [ ← ] [ → ] [ ↑ ] [ ↓ ]

scroll to reveal:
[ Home ] [ End ] [ PgUp ] [ PgDn ] [ F1 ] [ F2 ] ... [ F12 ]
```

The first 8 buttons fit on most phones (~375px width). Everything beyond requires horizontal scroll.

**Button sizing:**
- Height: 34px (touch-friendly, matches InputBar's 36px)
- Min-width: 40px for single-char labels, auto-width for longer labels (e.g., "PgUp")
- Padding: 0 10px
- Gap: 4px between buttons
- Font: 12px, system-ui/monospace for key labels

**Regular buttons (Tab, Esc, arrows, Home, End, PgUp, PgDn, F1-F12):**
- Tap sends the corresponding ANSI escape sequence immediately via `client.sendInput()`
- Arrow keys support press-and-hold repeat: `setInterval` on `touchstart`, clear on `touchend`, 500ms initial delay, 100ms repeat rate

**Toggle modifiers (Ctrl, Alt):**
- Tap to activate (visually highlighted), next keypress in InputBar combines with the modifier, then auto-deactivates
- Tap again while active to cancel without sending
- Only one modifier active at a time (tapping Alt while Ctrl is active switches to Alt)

Props: `session: string`, `client: WshClient` (same as InputBar).

### New: `useTerminalGestures` hook

Attaches touch listeners to the terminal container for two-finger swipe arrow key navigation.

**Detection algorithm:**
1. On `touchstart`: if `e.touches.length === 2`, record midpoint as gesture origin
2. On `touchmove`: compute new midpoint and delta from origin
3. On `touchend`: if delta exceeds 30px threshold, fire arrow key; otherwise discard

**Direction lock:** Once the primary axis is determined (horizontal vs vertical, based on first 15px of movement), lock to that axis to prevent diagonal confusion.

**Conflict avoidance:**
- One-finger touch: normal scroll (unchanged)
- Two-finger pinch (touches moving toward/away from each other): ignored, allows browser zoom
- The hook checks both touches are moving in roughly the same direction before treating as a swipe
- One gesture per touch sequence (no repeated arrows from a single swipe)

**Mapping:**
- Two-finger swipe left → `\x1b[D` (Left)
- Two-finger swipe right → `\x1b[C` (Right)
- Two-finger swipe up → `\x1b[A` (Up)
- Two-finger swipe down → `\x1b[B` (Down)

**Attachment:** Called inside `Terminal.tsx` when `captureInput` is false (mobile mode). Attaches to the `.terminal-container` element.

### Modified: `InputBar`

- Reads `ctrlActive` and `altActive` signals from shared state
- When a keypress arrives and a modifier is active, combines them (e.g., Ctrl + "c" → `\x03`) and deactivates the modifier
- Shows a visual "Ctrl+" or "Alt+" prefix badge on the input field when a modifier is active
- Calls `scheduleSyncFromTerminal()` when ModifierBar sends Tab (to sync tab completion)

### Modified: `SessionPane`

Renders ModifierBar between Terminal and InputBar when `isMobile` is true:

```
SessionPane
├── Title Bar
├── Terminal (+ gesture hook)
├── ModifierBar (new)
└── InputBar (enhanced)
```

## State Management

Modifier state as Preact signals in `state/modifiers.ts`:

```typescript
ctrlActive: Signal<boolean>  // default: false
altActive:  Signal<boolean>  // default: false
```

Signals (not component state) because two sibling components coordinate: ModifierBar writes, InputBar reads.

### Ctrl+C flow example

1. User taps Ctrl in ModifierBar → `ctrlActive.value = true`
2. User types "c" in InputBar
3. InputBar checks `ctrlActive.value` → true
4. Computes `\x03`, sends via `client.sendInput()`
5. Sets `ctrlActive.value = false`

### Regular button flow (e.g., Tab)

1. User taps Tab in ModifierBar
2. ModifierBar sends `"\t"` directly via `client.sendInput()`
3. Calls `scheduleSyncFromTerminal()` on InputBar to sync completion results

### Two-finger swipe flow

1. Hook detects swipe-right on terminal
2. Sends `"\x1b[C"` via `client.sendInput()`
3. No interaction with modifier state (gestures always send plain arrows)

## Styling

**ModifierBar container:**
- `display: flex`, `overflow-x: auto`, hidden scrollbar
- Background: `var(--chrome)`, `border-top: 1px solid var(--border)`
- Padding: `4px 8px`

**Buttons:**
- Background: `rgba(255, 255, 255, 0.06)`, border: `1px solid rgba(255, 255, 255, 0.12)`
- Border-radius: `6px`, color: `var(--fg)`
- `:active` — darker background for press feedback
- `.active` (Ctrl/Alt toggle) — `border-color: var(--cursor-color)`, `background: rgba(0, 212, 255, 0.15)`

**Scroll fade hints:** `::before`/`::after` gradient pseudo-elements (same pattern as `.carousel-strip-wrap`).

**Responsive:**
- Below 375px: buttons shrink to min-width 36px, font 11px
- Mobile-only — hidden on desktop via existing `isMobile` check
- Respects `prefers-reduced-motion`

**Theme compatibility:** Uses only CSS variables (`--chrome`, `--border`, `--fg`, `--cursor-color`) — works across all 6 themes.

**Vertical space budget:**
- ModifierBar: ~42px (34px buttons + 8px padding)
- InputBar: ~52px (36px input + 16px padding)
- Combined: ~94px — leaves majority of screen for terminal content
