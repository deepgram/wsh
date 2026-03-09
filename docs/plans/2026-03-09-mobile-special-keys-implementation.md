# Mobile Special Keys Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a modifier bar and two-finger swipe gestures so mobile users can access Ctrl, Tab, Esc, arrows, function keys, and navigation keys.

**Architecture:** New `ModifierBar` component with scrollable buttons sits between Terminal and InputBar on mobile. Ctrl/Alt are sticky toggle modifiers coordinated via Preact signals. A `useTerminalGestures` hook on the terminal container detects two-finger swipes for arrow keys.

**Tech Stack:** Preact, @preact/signals, CSS (existing theme variables)

**Design doc:** `docs/plans/2026-03-09-mobile-special-keys-design.md`

---

### Task 1: Modifier State Signals

**Files:**
- Create: `web/src/state/modifiers.ts`

**Step 1: Create the state module**

```typescript
import { signal } from "@preact/signals";

export const ctrlActive = signal(false);
export const altActive = signal(false);

/** Activate ctrl, deactivating alt. */
export function toggleCtrl(): void {
  if (ctrlActive.value) {
    ctrlActive.value = false;
  } else {
    altActive.value = false;
    ctrlActive.value = true;
  }
}

/** Activate alt, deactivating ctrl. */
export function toggleAlt(): void {
  if (altActive.value) {
    altActive.value = false;
  } else {
    ctrlActive.value = false;
    altActive.value = true;
  }
}

/** Clear all modifiers. Called after a modified keypress is sent. */
export function clearModifiers(): void {
  ctrlActive.value = false;
  altActive.value = false;
}
```

**Step 2: Verify build**

Run: `cd web && npx tsc --noEmit`
Expected: no errors

**Step 3: Commit**

```bash
git add web/src/state/modifiers.ts
git commit -m "feat(web): add modifier state signals for mobile special keys"
```

---

### Task 2: ModifierBar Component

**Files:**
- Create: `web/src/components/ModifierBar.tsx`

**Step 1: Create the component**

```tsx
import { useRef, useEffect, useCallback } from "preact/hooks";
import { ctrlActive, altActive, toggleCtrl, toggleAlt } from "../state/modifiers";
import { connectionState } from "../state/sessions";
import type { WshClient } from "../api/ws";

interface ModifierBarProps {
  session: string;
  client: WshClient;
  onTabSent?: () => void;
}

interface KeyDef {
  label: string;
  /** ANSI sequence to send on tap. Null = toggle modifier. */
  seq: string | null;
  /** If true, this is a toggle modifier button (Ctrl/Alt). */
  modifier?: "ctrl" | "alt";
  /** If true, support press-and-hold repeat. */
  repeatable?: boolean;
}

const KEYS: KeyDef[] = [
  { label: "Tab", seq: "\t" },
  { label: "Esc", seq: "\x1b" },
  { label: "Ctrl", seq: null, modifier: "ctrl" },
  { label: "Alt", seq: null, modifier: "alt" },
  { label: "\u2190", seq: "\x1b[D", repeatable: true },
  { label: "\u2192", seq: "\x1b[C", repeatable: true },
  { label: "\u2191", seq: "\x1b[A", repeatable: true },
  { label: "\u2193", seq: "\x1b[B", repeatable: true },
  { label: "Home", seq: "\x1b[H" },
  { label: "End", seq: "\x1b[F" },
  { label: "PgUp", seq: "\x1b[5~" },
  { label: "PgDn", seq: "\x1b[6~" },
  { label: "F1", seq: "\x1bOP" },
  { label: "F2", seq: "\x1bOQ" },
  { label: "F3", seq: "\x1bOR" },
  { label: "F4", seq: "\x1bOS" },
  { label: "F5", seq: "\x1b[15~" },
  { label: "F6", seq: "\x1b[17~" },
  { label: "F7", seq: "\x1b[18~" },
  { label: "F8", seq: "\x1b[19~" },
  { label: "F9", seq: "\x1b[20~" },
  { label: "F10", seq: "\x1b[21~" },
  { label: "F11", seq: "\x1b[23~" },
  { label: "F12", seq: "\x1b[24~" },
];

/** Initial delay before key repeat starts (ms). */
const REPEAT_DELAY = 500;
/** Interval between repeats (ms). */
const REPEAT_INTERVAL = 100;

export function ModifierBar({ session, client, onTabSent }: ModifierBarProps) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const repeatTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const repeatIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const connected = connectionState.value === "connected";

  // Clean up repeat timers on unmount
  useEffect(() => {
    return () => {
      if (repeatTimerRef.current) clearTimeout(repeatTimerRef.current);
      if (repeatIntervalRef.current) clearInterval(repeatIntervalRef.current);
    };
  }, []);

  // Scroll-fade overflow detection
  const [overflowLeft, setOverflowLeft] = useState(false);
  const [overflowRight, setOverflowRight] = useState(false);

  const updateOverflow = useCallback(() => {
    const el = wrapRef.current;
    if (!el) return;
    setOverflowLeft(el.scrollLeft > 0);
    setOverflowRight(el.scrollLeft + el.clientWidth < el.scrollWidth - 1);
  }, []);

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    updateOverflow();
    el.addEventListener("scroll", updateOverflow, { passive: true });
    return () => el.removeEventListener("scroll", updateOverflow);
  }, [updateOverflow]);

  const send = useCallback(
    (data: string) => {
      if (!connected) return;
      client.sendInput(session, data).catch((e) => {
        console.error(`ModifierBar: failed to send input:`, e);
      });
    },
    [connected, client, session],
  );

  const handleTap = useCallback(
    (key: KeyDef) => {
      if (key.modifier === "ctrl") {
        toggleCtrl();
        return;
      }
      if (key.modifier === "alt") {
        toggleAlt();
        return;
      }
      if (key.seq) {
        send(key.seq);
        if (key.seq === "\t" && onTabSent) onTabSent();
      }
    },
    [send, onTabSent],
  );

  const startRepeat = useCallback(
    (key: KeyDef) => {
      if (!key.repeatable || !key.seq) return;
      const seq = key.seq;
      repeatTimerRef.current = setTimeout(() => {
        repeatIntervalRef.current = setInterval(() => send(seq), REPEAT_INTERVAL);
      }, REPEAT_DELAY);
    },
    [send],
  );

  const stopRepeat = useCallback(() => {
    if (repeatTimerRef.current) {
      clearTimeout(repeatTimerRef.current);
      repeatTimerRef.current = null;
    }
    if (repeatIntervalRef.current) {
      clearInterval(repeatIntervalRef.current);
      repeatIntervalRef.current = null;
    }
  }, []);

  const wrapClass =
    "modifier-bar-wrap" +
    (overflowLeft ? " overflow-left" : "") +
    (overflowRight ? " overflow-right" : "");

  return (
    <div class={wrapClass}>
      <div class="modifier-bar" ref={wrapRef}>
        {KEYS.map((key) => {
          const isActive =
            (key.modifier === "ctrl" && ctrlActive.value) ||
            (key.modifier === "alt" && altActive.value);
          return (
            <button
              key={key.label}
              class={`modifier-key${isActive ? " active" : ""}`}
              disabled={!connected}
              onClick={() => handleTap(key)}
              onTouchStart={key.repeatable ? () => startRepeat(key) : undefined}
              onTouchEnd={key.repeatable ? stopRepeat : undefined}
              onTouchCancel={key.repeatable ? stopRepeat : undefined}
            >
              {key.label}
            </button>
          );
        })}
      </div>
    </div>
  );
}
```

**Important:** Add the missing `useState` import — the import line should be:
```tsx
import { useRef, useEffect, useCallback, useState } from "preact/hooks";
```

**Step 2: Verify build**

Run: `cd web && npx tsc --noEmit`
Expected: no errors

**Step 3: Commit**

```bash
git add web/src/components/ModifierBar.tsx
git commit -m "feat(web): add ModifierBar component with scrollable special keys"
```

---

### Task 3: ModifierBar CSS

**Files:**
- Modify: `web/src/styles/terminal.css` (add after the Input Bar section, ~line 198)

**Step 1: Add CSS rules**

Insert after the `.input-bar input:focus` rule (line 198):

```css
/* ==========================================================================
   Modifier Bar
   ========================================================================== */

.modifier-bar-wrap {
  position: relative;
  flex-shrink: 0;
  background: var(--chrome);
  border-top: 1px solid var(--border);
}

/* Scroll-fade indicators */
.modifier-bar-wrap.overflow-left::before,
.modifier-bar-wrap.overflow-right::after {
  content: "";
  position: absolute;
  top: 0;
  bottom: 0;
  width: 20px;
  z-index: 2;
  pointer-events: none;
}

.modifier-bar-wrap.overflow-left::before {
  left: 0;
  background: linear-gradient(to right, var(--chrome), transparent);
}

.modifier-bar-wrap.overflow-right::after {
  right: 0;
  background: linear-gradient(to left, var(--chrome), transparent);
}

.modifier-bar {
  display: flex;
  gap: 4px;
  padding: 4px 8px;
  overflow-x: auto;
  scrollbar-width: none;
  -webkit-overflow-scrolling: touch;
}

.modifier-bar::-webkit-scrollbar {
  display: none;
}

.modifier-key {
  flex-shrink: 0;
  height: 34px;
  min-width: 40px;
  padding: 0 10px;
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 6px;
  background: rgba(255, 255, 255, 0.06);
  color: var(--fg);
  font-size: 12px;
  font-family: system-ui, -apple-system, sans-serif;
  cursor: pointer;
  user-select: none;
  -webkit-user-select: none;
  transition: background 0.1s, border-color 0.1s;
}

.modifier-key:active {
  background: rgba(255, 255, 255, 0.02);
}

.modifier-key.active {
  border-color: var(--cursor-color);
  background: rgba(0, 212, 255, 0.15);
  color: var(--cursor-color);
}

.modifier-key:disabled {
  opacity: 0.3;
  cursor: default;
}

@media (max-width: 374px) {
  .modifier-key {
    min-width: 36px;
    padding: 0 8px;
    font-size: 11px;
  }
}
```

**Step 2: Verify build — open in browser on mobile or responsive mode, confirm styling**

Run: `cd web && npx tsc --noEmit`
Expected: no errors

**Step 3: Commit**

```bash
git add web/src/styles/terminal.css
git commit -m "feat(web): add modifier bar CSS with scroll fade and theme support"
```

---

### Task 4: Wire ModifierBar into SessionPane

**Files:**
- Modify: `web/src/components/SessionPane.tsx`

**Step 1: Add import and render ModifierBar**

Add import at top of `SessionPane.tsx`:
```tsx
import { ModifierBar } from "./ModifierBar";
```

Replace line 125 (`{isMobile && <InputBar session={session} client={client} />}`):
```tsx
      {isMobile && <ModifierBar session={session} client={client} />}
      {isMobile && <InputBar session={session} client={client} />}
```

The full render section (lines 123-126) becomes:
```tsx
      <Terminal session={session} client={client} captureInput={!isMobile} />
      {isMobile && <ModifierBar session={session} client={client} />}
      {isMobile && <InputBar session={session} client={client} />}
```

**Step 2: Verify build**

Run: `cd web && npx tsc --noEmit`
Expected: no errors

**Step 3: Manual test — open on mobile or responsive mode, verify the bar appears between terminal and input**

**Step 4: Commit**

```bash
git add web/src/components/SessionPane.tsx
git commit -m "feat(web): render ModifierBar on mobile between terminal and input"
```

---

### Task 5: Integrate Modifier State into InputBar

**Files:**
- Modify: `web/src/components/InputBar.tsx`

**Step 1: Add modifier imports**

Add at top of `InputBar.tsx`:
```tsx
import { ctrlActive, altActive, clearModifiers } from "../state/modifiers";
```

**Step 2: Modify `handleInput` to apply active modifiers**

The `handleInput` function (lines 124-152) currently handles printable character diffs. When a modifier is active and the user types a single character, we need to intercept it and send the modified sequence instead.

Replace the `handleInput` function with:

```tsx
  const handleInput = () => {
    const input = inputRef.current;
    if (!input) return;

    const current = input.value;
    const prev = prevValueRef.current;

    if (current === prev) return;

    // If a modifier is active, intercept the newly typed character(s)
    if (ctrlActive.value || altActive.value) {
      const added = current.slice(prev.length);
      if (added.length === 1) {
        if (ctrlActive.value) {
          const lower = added.toLowerCase();
          if (lower >= "a" && lower <= "z") {
            send(String.fromCharCode(lower.charCodeAt(0) - 96));
          }
        } else if (altActive.value) {
          send("\x1b" + added);
        }
        clearModifiers();
        // Revert the input field — the modified char shouldn't appear
        input.value = prev;
        prevValueRef.current = prev;
        return;
      }
    }

    // Normal diff-based input (no modifier active)
    // Find common prefix
    let common = 0;
    while (common < prev.length && common < current.length && prev[common] === current[common]) {
      common++;
    }

    // Characters removed from prev after the common prefix
    const removed = prev.length - common;
    // Characters added in current after the common prefix
    const added = current.slice(common);

    if (removed > 0) {
      send("\x7f".repeat(removed));
    }
    if (added) {
      send(added);
    }

    prevValueRef.current = current;
  };
```

**Step 3: Add modifier badge to the input placeholder**

In the return JSX, update the placeholder to reflect active modifier:

Replace the `placeholder` attribute:
```tsx
        placeholder={
          !connected
            ? "Disconnected"
            : ctrlActive.value
              ? "Ctrl + ..."
              : altActive.value
                ? "Alt + ..."
                : "Type here..."
        }
```

**Step 4: Add visual indicator — update input class when modifier is active**

Replace the `<input` element's `class` (or add a wrapping class) — simplest is to add a conditional class to the `.input-bar` div:

Replace `<div class="input-bar">` with:
```tsx
    <div class={`input-bar${ctrlActive.value || altActive.value ? " modifier-active" : ""}`}>
```

And add this CSS rule to `terminal.css` (inside the Input Bar section):
```css
.input-bar.modifier-active input {
  border-color: var(--cursor-color);
}
```

**Step 5: Verify build**

Run: `cd web && npx tsc --noEmit`
Expected: no errors

**Step 6: Manual test**

1. Open on mobile/responsive mode
2. Tap Ctrl in modifier bar — should highlight, input placeholder shows "Ctrl + ...", input border glows
3. Type "c" — should send Ctrl+C (`\x03`), modifier deactivates, placeholder returns to normal
4. Tap Alt, type "f" — should send Alt+f (`\x1bf`)
5. Normal typing without modifier — unchanged behavior

**Step 7: Commit**

```bash
git add web/src/components/InputBar.tsx web/src/styles/terminal.css
git commit -m "feat(web): integrate modifier state into InputBar for Ctrl/Alt combos"
```

---

### Task 6: Tab Completion Sync from ModifierBar

**Files:**
- Modify: `web/src/components/InputBar.tsx`
- Modify: `web/src/components/SessionPane.tsx`

The design calls for ModifierBar's Tab button to trigger `scheduleSyncFromTerminal()` on InputBar. The cleanest approach: expose `scheduleSyncFromTerminal` as an imperative handle via a ref.

**Step 1: Export a ref type and use `useImperativeHandle` in InputBar**

Add import at top of `InputBar.tsx`:
```tsx
import { useRef, useEffect, useImperativeHandle } from "preact/hooks";
import { forwardRef } from "preact/compat";
```

Add an exported interface:
```tsx
export interface InputBarHandle {
  scheduleSyncFromTerminal: () => void;
}
```

Change the component signature to use `forwardRef`:
```tsx
export const InputBar = forwardRef<InputBarHandle, InputBarProps>(
  function InputBar({ session, client }, ref) {
```

Add the imperative handle after `scheduleSyncFromTerminal` is defined:
```tsx
    useImperativeHandle(ref, () => ({
      scheduleSyncFromTerminal,
    }));
```

Close with an extra `})` instead of just `}`.

**Step 2: Wire the ref in SessionPane**

In `SessionPane.tsx`, add:
```tsx
import { useRef } from "preact/hooks";
import type { InputBarHandle } from "./InputBar";
```

Inside the component, add:
```tsx
  const inputBarRef = useRef<InputBarHandle>(null);
```

Update the render:
```tsx
      {isMobile && (
        <ModifierBar
          session={session}
          client={client}
          onTabSent={() => inputBarRef.current?.scheduleSyncFromTerminal()}
        />
      )}
      {isMobile && <InputBar ref={inputBarRef} session={session} client={client} />}
```

**Step 3: Verify build**

Run: `cd web && npx tsc --noEmit`
Expected: no errors

**Step 4: Manual test — tap Tab in modifier bar, verify input field syncs with terminal completion**

**Step 5: Commit**

```bash
git add web/src/components/InputBar.tsx web/src/components/SessionPane.tsx
git commit -m "feat(web): wire tab completion sync from ModifierBar to InputBar"
```

---

### Task 7: Two-Finger Swipe Gesture Hook

**Files:**
- Create: `web/src/hooks/useTerminalGestures.ts`

**Step 1: Create the hook**

```typescript
import { useEffect, type RefObject } from "preact/hooks";

/** Minimum px distance before a swipe fires. */
const SWIPE_THRESHOLD = 30;
/** px of movement before axis is locked. */
const AXIS_LOCK_THRESHOLD = 15;

interface GestureOptions {
  /** Ref to the element to attach touch listeners to. */
  containerRef: RefObject<HTMLElement>;
  /** Called with the ANSI sequence for the detected swipe direction. */
  onSwipe: (seq: string) => void;
  /** Whether the hook is enabled (false on desktop). */
  enabled: boolean;
}

export function useTerminalGestures({
  containerRef,
  onSwipe,
  enabled,
}: GestureOptions): void {
  useEffect(() => {
    if (!enabled) return;
    const el = containerRef.current;
    if (!el) return;

    let tracking = false;
    let originX = 0;
    let originY = 0;
    let lockedAxis: "h" | "v" | null = null;
    let fired = false;

    // Track per-touch movement to reject pinch gestures
    let touch0Start: { x: number; y: number } | null = null;
    let touch1Start: { x: number; y: number } | null = null;

    function midpoint(t: TouchList): { x: number; y: number } {
      return {
        x: (t[0].clientX + t[1].clientX) / 2,
        y: (t[0].clientY + t[1].clientY) / 2,
      };
    }

    function isPinch(t: TouchList): boolean {
      if (!touch0Start || !touch1Start) return false;
      const d0x = t[0].clientX - touch0Start.x;
      const d0y = t[0].clientY - touch0Start.y;
      const d1x = t[1].clientX - touch1Start.x;
      const d1y = t[1].clientY - touch1Start.y;
      // Dot product of the two movement vectors; negative = moving apart/together
      const dot = d0x * d1x + d0y * d1y;
      const mag0 = Math.sqrt(d0x * d0x + d0y * d0y);
      const mag1 = Math.sqrt(d1x * d1x + d1y * d1y);
      if (mag0 < 5 || mag1 < 5) return false;
      // cos(angle) < 0.3 means they're moving in quite different directions
      return dot / (mag0 * mag1) < 0.3;
    }

    const onTouchStart = (e: TouchEvent) => {
      if (e.touches.length === 2) {
        const mid = midpoint(e.touches);
        originX = mid.x;
        originY = mid.y;
        touch0Start = { x: e.touches[0].clientX, y: e.touches[0].clientY };
        touch1Start = { x: e.touches[1].clientX, y: e.touches[1].clientY };
        tracking = true;
        lockedAxis = null;
        fired = false;
      }
    };

    const onTouchMove = (e: TouchEvent) => {
      if (!tracking || e.touches.length !== 2) {
        tracking = false;
        return;
      }
      if (fired) return;
      if (isPinch(e.touches)) {
        tracking = false;
        return;
      }

      const mid = midpoint(e.touches);
      const dx = mid.x - originX;
      const dy = mid.y - originY;
      const absDx = Math.abs(dx);
      const absDy = Math.abs(dy);

      // Lock axis once we pass the threshold
      if (!lockedAxis && (absDx > AXIS_LOCK_THRESHOLD || absDy > AXIS_LOCK_THRESHOLD)) {
        lockedAxis = absDx > absDy ? "h" : "v";
      }

      if (!lockedAxis) return;

      if (lockedAxis === "h" && absDx > SWIPE_THRESHOLD) {
        onSwipe(dx < 0 ? "\x1b[D" : "\x1b[C");
        fired = true;
      } else if (lockedAxis === "v" && absDy > SWIPE_THRESHOLD) {
        onSwipe(dy < 0 ? "\x1b[A" : "\x1b[B");
        fired = true;
      }
    };

    const onTouchEnd = () => {
      tracking = false;
      lockedAxis = null;
      touch0Start = null;
      touch1Start = null;
    };

    el.addEventListener("touchstart", onTouchStart, { passive: true });
    el.addEventListener("touchmove", onTouchMove, { passive: true });
    el.addEventListener("touchend", onTouchEnd);
    el.addEventListener("touchcancel", onTouchEnd);

    return () => {
      el.removeEventListener("touchstart", onTouchStart);
      el.removeEventListener("touchmove", onTouchMove);
      el.removeEventListener("touchend", onTouchEnd);
      el.removeEventListener("touchcancel", onTouchEnd);
    };
  }, [containerRef, onSwipe, enabled]);
}
```

**Step 2: Verify build**

Run: `cd web && npx tsc --noEmit`
Expected: no errors

**Step 3: Commit**

```bash
git add web/src/hooks/useTerminalGestures.ts
git commit -m "feat(web): add useTerminalGestures hook for two-finger swipe arrows"
```

---

### Task 8: Wire Gesture Hook into Terminal

**Files:**
- Modify: `web/src/components/Terminal.tsx`

**Step 1: Add import and hook call**

Add import at top of `Terminal.tsx`:
```tsx
import { useTerminalGestures } from "../hooks/useTerminalGestures";
```

Inside the `Terminal` component, after the `containerRef` declaration and before the ResizeObserver effect, add:

```tsx
  // Two-finger swipe gestures for arrow keys on mobile
  const handleSwipe = useCallback(
    (seq: string) => {
      if (client) {
        client.sendInput(session, seq).catch(() => {});
      }
    },
    [client, session],
  );

  useTerminalGestures({
    containerRef,
    onSwipe: handleSwipe,
    enabled: !captureInput && !!client,
  });
```

Note: `!captureInput` means the hook is only active on mobile (where `captureInput` is `false`).

**Step 2: Verify build**

Run: `cd web && npx tsc --noEmit`
Expected: no errors

**Step 3: Manual test on mobile or responsive mode with touch emulation**

1. Two-finger swipe right on terminal area → should send Right arrow
2. Two-finger swipe up → should send Up arrow (shell history)
3. One-finger scroll → should remain normal scroll
4. Pinch gesture → should not fire any arrow key

**Step 4: Commit**

```bash
git add web/src/components/Terminal.tsx
git commit -m "feat(web): wire two-finger swipe gestures into Terminal for mobile"
```

---

### Task 9: Focus-Visible and Reduced Motion

**Files:**
- Modify: `web/src/styles/terminal.css`

**Step 1: Add focus-visible rule**

In the `Focus-Visible Indicators` section (~line 2515), add `.modifier-key:focus-visible` to the existing comma-separated selector list:

```css
.modifier-key:focus-visible,
```

Add this line before `error-boundary button:focus-visible` in the selector list.

**Step 2: The `prefers-reduced-motion` section already disables all transitions globally via `* { transition-duration: 0.01s !important; }`, so no additional work needed.**

**Step 3: Commit**

```bash
git add web/src/styles/terminal.css
git commit -m "feat(web): add focus-visible and a11y for modifier bar buttons"
```

---

### Task 10: End-to-End Manual Testing & Polish

**No new files. This is a verification task.**

**Step 1: Test on mobile device or Chrome DevTools responsive mode (iPhone SE, iPhone 14, Pixel 7)**

Test matrix:
- [ ] ModifierBar visible on mobile, hidden on desktop
- [ ] Tab sends tab completion, input field syncs
- [ ] Esc sends escape, clears input
- [ ] Ctrl toggle → type "c" → sends Ctrl+C (verify running command gets interrupted)
- [ ] Ctrl toggle → type "d" → sends Ctrl+D (EOF)
- [ ] Alt toggle → type "f" → sends Alt+f (word forward in bash)
- [ ] Arrow buttons send arrow keys (history, cursor movement)
- [ ] Arrow button press-and-hold repeats
- [ ] Two-finger swipe on terminal sends arrows
- [ ] One-finger scroll on terminal still scrolls normally
- [ ] Pinch on terminal does NOT send arrows
- [ ] Home/End/PgUp/PgDn reachable via scroll
- [ ] F1-F12 reachable via scroll
- [ ] Scroll fade hints visible when content overflows
- [ ] All 7 themes render correctly (glass, neon, minimal, tokyo-night, catppuccin, dracula, high-contrast)
- [ ] Narrow phone (<375px) — buttons shrink but remain usable
- [ ] Reduced motion — no transitions on buttons

**Step 2: Fix any issues found**

**Step 3: Final commit if fixes were needed**

```bash
git add -A
git commit -m "fix(web): polish mobile modifier bar from manual testing"
```
