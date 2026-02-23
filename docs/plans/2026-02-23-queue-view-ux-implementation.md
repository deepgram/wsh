# Queue View UX Redesign Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Redesign queue view to have two sections (Idle sorted by acknowledgment, Running by actual status), left/right keyboard navigation across all sessions, and a dismiss action that acknowledges idle sessions without hiding them.

**Architecture:** The idle queue keeps entries as `"pending" | "acknowledged"` instead of removing them on dismiss. A status-watching effect handles both enqueue (running→idle) and dequeue (idle→running) transitions. The film roll becomes a navigable flat list with keyboard left/right support.

**Tech Stack:** Preact, Preact Signals, CSS

---

### Task 1: Update data model — QueueEntry type and state functions

**Files:**
- Modify: `web/src/state/groups.ts:12-16` (QueueEntry type)
- Modify: `web/src/state/groups.ts:133-147` (enqueueSession, dismissQueueEntry)

**Step 1: Change QueueEntry status type**

In `web/src/state/groups.ts`, change the status union from `"pending" | "handled"` to `"pending" | "acknowledged"`:

```typescript
export interface QueueEntry {
  session: string;
  idleAt: number;
  status: "pending" | "acknowledged";
}
```

**Step 2: Fix enqueueSession duplicate check**

Change the duplicate check to prevent any duplicate entry (not just pending), since acknowledged entries should be re-activated to pending on a new idle transition. Replace the entire function:

```typescript
export function enqueueSession(tag: string, session: string): void {
  const queues = { ...idleQueues.value };
  const queue = [...(queues[tag] || [])];
  const idx = queue.findIndex((e) => e.session === session);
  if (idx !== -1) {
    // Re-activate existing entry as pending with fresh timestamp
    queue[idx] = { ...queue[idx], status: "pending", idleAt: Date.now() };
  } else {
    queue.push({ session, idleAt: Date.now(), status: "pending" });
  }
  queues[tag] = queue;
  idleQueues.value = queues;
}
```

**Step 3: Change dismissQueueEntry to mark acknowledged instead of removing**

```typescript
export function dismissQueueEntry(tag: string, session: string): void {
  const queues = { ...idleQueues.value };
  const queue = (queues[tag] || []).map((e) =>
    e.session === session && e.status === "pending"
      ? { ...e, status: "acknowledged" as const }
      : e
  );
  queues[tag] = queue;
  idleQueues.value = queues;
}
```

**Step 4: Add removeQueueEntry for running transitions**

Add a new export after `dismissQueueEntry`:

```typescript
export function removeQueueEntry(tag: string, session: string): void {
  const queues = { ...idleQueues.value };
  const queue = (queues[tag] || []).filter((e) => e.session !== session);
  queues[tag] = queue;
  idleQueues.value = queues;
}
```

**Step 5: Commit**

```bash
git add web/src/state/groups.ts
git commit -m "refactor: update queue entry model to pending/acknowledged with removal on running"
```

---

### Task 2: Update QueueView status-watching effect

**Files:**
- Modify: `web/src/components/QueueView.tsx:3` (imports)
- Modify: `web/src/components/QueueView.tsx:41-59` (status-watching effect)

**Step 1: Add removeQueueEntry to imports**

```typescript
import { idleQueues, enqueueSession, dismissQueueEntry, removeQueueEntry, sessionStatuses } from "../state/groups";
```

**Step 2: Update the effect to handle both idle and running transitions**

Replace lines 41-59 with:

```typescript
  // Watch sessionStatuses for transitions:
  // running→idle: enqueue as pending
  // idle→running: remove from queue (re-enters as pending on next idle)
  const prevStatuses = useRef<Map<string, string>>(new Map());
  useEffect(() => {
    const statuses = sessionStatuses.value;
    for (const s of sessions) {
      const current = statuses.get(s);
      const prev = prevStatuses.current.get(s);
      if (current === "idle" && prev !== "idle") {
        enqueueSession(groupTag, s);
      } else if (current !== "idle" && prev === "idle") {
        removeQueueEntry(groupTag, s);
      }
    }
    // Update previous statuses
    const updated = new Map<string, string>();
    for (const s of sessions) {
      const st = statuses.get(s);
      if (st) updated.set(s, st);
    }
    prevStatuses.current = updated;
  }, [sessions, groupTag, sessionStatuses.value]);
```

**Step 3: Commit**

```bash
git add web/src/components/QueueView.tsx
git commit -m "feat: watch for running transitions to remove queue entries"
```

---

### Task 3: Rewrite QueueView categorization and navigation

**Files:**
- Modify: `web/src/components/QueueView.tsx` (full component rewrite)

**Step 1: Rewrite the component**

Replace the entire `QueueView` function body. Key changes:
- Compute `idle` list (pending sorted first by idleAt, then acknowledged by idleAt)
- Compute `running` list from actual `sessionStatuses`
- Build a flat `navList` across both sections for arrow key navigation
- Replace `manualSelection` string with index-based navigation
- Add left/right keyboard handler (Ctrl+Shift+H/L or Left/Right)
- Dismiss always jumps to oldest pending

```tsx
import { useCallback, useEffect, useMemo, useRef, useState } from "preact/hooks";
import type { WshClient } from "../api/ws";
import { idleQueues, enqueueSession, dismissQueueEntry, removeQueueEntry, sessionStatuses } from "../state/groups";
import { focusedSession } from "../state/sessions";
import { SessionPane } from "./SessionPane";
import { MiniTermContent } from "./MiniViewPreview";

interface QueueViewProps {
  sessions: string[];
  groupTag: string;
  client: WshClient;
}

export function QueueView({ sessions, groupTag, client }: QueueViewProps) {
  const queue = idleQueues.value[groupTag] || [];
  const statuses = sessionStatuses.value;

  // Idle section: pending first (by idleAt), then acknowledged (by idleAt)
  const pending = queue
    .filter((e) => e.status === "pending")
    .sort((a, b) => a.idleAt - b.idleAt);
  const acknowledged = queue
    .filter((e) => e.status === "acknowledged")
    .sort((a, b) => a.idleAt - b.idleAt);
  const idle = [...pending, ...acknowledged];

  // Running section: sessions whose actual status is not idle
  const idleNames = new Set(queue.map((e) => e.session));
  const running = sessions.filter(
    (s) => !idleNames.has(s) && statuses.get(s) !== "idle"
  );

  // Flat navigation list: idle then running
  const navList = useMemo(
    () => [...idle.map((e) => e.session), ...running],
    [idle, running]
  );

  // Selection state
  const [selectedSession, setSelectedSession] = useState<string | null>(null);

  // Resolve current session: manual selection if valid, else oldest pending, else first in nav
  const oldestPending = pending[0]?.session || null;
  const currentSession =
    selectedSession && navList.includes(selectedSession)
      ? selectedSession
      : oldestPending || navList[0] || null;

  // Focus the current session for other components
  useEffect(() => {
    if (currentSession) {
      focusedSession.value = currentSession;
    }
  }, [currentSession]);

  // Watch sessionStatuses for transitions
  const prevStatuses = useRef<Map<string, string>>(new Map());
  useEffect(() => {
    const statuses = sessionStatuses.value;
    for (const s of sessions) {
      const current = statuses.get(s);
      const prev = prevStatuses.current.get(s);
      if (current === "idle" && prev !== "idle") {
        enqueueSession(groupTag, s);
      } else if (current !== "idle" && prev === "idle") {
        removeQueueEntry(groupTag, s);
      }
    }
    const updated = new Map<string, string>();
    for (const s of sessions) {
      const st = statuses.get(s);
      if (st) updated.set(s, st);
    }
    prevStatuses.current = updated;
  }, [sessions, groupTag, sessionStatuses.value]);

  // Dismiss: acknowledge current if pending, then jump to next pending
  const handleDismiss = useCallback(() => {
    if (currentSession) {
      const isPending = pending.some((e) => e.session === currentSession);
      if (isPending) {
        dismissQueueEntry(groupTag, currentSession);
      }
    }
    // Jump to oldest pending (after the one we just dismissed)
    // The signal update is synchronous, so re-read the queue
    const updatedQueue = idleQueues.value[groupTag] || [];
    const nextPending = updatedQueue.find((e) => e.status === "pending" && e.session !== currentSession);
    setSelectedSession(nextPending?.session || null);
  }, [groupTag, currentSession, pending]);

  // Left/right navigation: Ctrl+Shift+H/L or Left/Right
  const navigate = useCallback((direction: -1 | 1) => {
    if (navList.length === 0) return;
    const currentIndex = currentSession ? navList.indexOf(currentSession) : -1;
    const newIndex = currentIndex === -1
      ? 0
      : (currentIndex + direction + navList.length) % navList.length;
    setSelectedSession(navList[newIndex]);
  }, [navList, currentSession]);

  // Keyboard handler
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (!e.ctrlKey || !e.shiftKey || e.altKey || e.metaKey) return;

      if (e.key === "ArrowLeft" || e.key === "h" || e.key === "H") {
        e.preventDefault();
        navigate(-1);
      } else if (e.key === "ArrowRight" || e.key === "l" || e.key === "L") {
        e.preventDefault();
        navigate(1);
      } else if (e.key === "Enter") {
        e.preventDefault();
        handleDismiss();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [navigate, handleDismiss]);

  // Idle section label with pending count
  const idleLabel = pending.length > 0
    ? `Idle (${pending.length} new · ${idle.length})`
    : `Idle (${idle.length})`;

  return (
    <div class="queue-view">
      {/* Top bar */}
      <div class="queue-top-bar">
        <div class="queue-pending">
          <div class="queue-section-header">
            <span class="queue-section-label">{idleLabel}</span>
            <kbd class="queue-shortcut-hint">Ctrl+Shift+Enter to dismiss</kbd>
          </div>
          <div class="queue-thumbnails">
            {idle.map((e) => (
              <div
                key={e.session}
                class={`queue-thumb${e.session === currentSession ? " active" : ""}${e.status === "pending" ? " pending" : ""}`}
                onClick={() => setSelectedSession(e.session)}
              >
                {e.status === "pending" && <span class="queue-pending-dot" />}
                <MiniTermContent session={e.session} />
              </div>
            ))}
          </div>
        </div>
        {running.length > 0 && (
          <div class="queue-handled">
            <span class="queue-section-label">Running ({running.length})</span>
            <div class="queue-thumbnails">
              {running.map((s) => (
                <div
                  key={s}
                  class={`queue-thumb${s === currentSession ? " active" : ""}`}
                  onClick={() => setSelectedSession(s)}
                >
                  <MiniTermContent session={s} />
                </div>
              ))}
            </div>
          </div>
        )}
      </div>

      {/* Center content */}
      {currentSession ? (
        <div class="queue-center">
          <SessionPane session={currentSession} client={client} />
        </div>
      ) : (
        <div class="queue-empty">
          <div class="queue-empty-icon">&#10003;</div>
          <div class="queue-empty-text">All caught up</div>
        </div>
      )}
    </div>
  );
}
```

**Step 2: Verify TypeScript compiles**

Run: `nix develop -c sh -c "cd web && bun run tsc --noEmit"`
Expected: no errors

**Step 3: Commit**

```bash
git add web/src/components/QueueView.tsx
git commit -m "feat: rewrite queue view with idle/running sections and keyboard navigation"
```

---

### Task 4: Add CSS for pending thumbnail styling

**Files:**
- Modify: `web/src/styles/terminal.css:1923-1943` (queue-thumb styles)

**Step 1: Add pending border and dot badge styles**

After the existing `.queue-thumb.active` rule (around line 1943), add:

```css
.queue-thumb.pending {
  border-color: var(--accent, #666);
}

.queue-thumb.pending.active {
  box-shadow: 0 0 0 1px var(--accent, #666);
}

.queue-pending-dot {
  position: absolute;
  top: 3px;
  right: 3px;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--accent, #f90);
  z-index: 1;
  animation: pulse-glow 1.5s ease-in-out infinite;
}
```

**Step 2: Make queue-thumb position relative for the dot**

Add `position: relative;` to the existing `.queue-thumb` rule so the absolutely-positioned dot is contained:

```css
.queue-thumb {
  position: relative;
  width: 80px;
  height: 40px;
  /* ... rest unchanged */
}
```

**Step 3: Verify the build**

Run: `nix develop -c sh -c "cargo check"`
Expected: compiles (web assets are embedded)

**Step 4: Commit**

```bash
git add web/src/styles/terminal.css
git commit -m "feat: add pending dot badge and accent border for unacknowledged queue thumbnails"
```

---

### Task 5: Update ShortcutSheet for queue navigation

**Files:**
- Modify: `web/src/components/ShortcutSheet.tsx:16-20` (Navigation shortcuts)

**Step 1: Update the navigation shortcuts**

The existing entry `"Carousel rotate / Grid navigate"` for Left/Right should also mention queue. Change line 17:

```typescript
{ keys: "Ctrl+Shift+Left/Right or H/L", description: "Navigate sessions (all views)" },
```

**Step 2: Update the dismiss description**

Change line 36 from `"Dismiss queue item"` to:

```typescript
{ keys: "Ctrl+Shift+Enter", description: "Dismiss & next idle session (queue)" },
```

**Step 3: Commit**

```bash
git add web/src/components/ShortcutSheet.tsx
git commit -m "docs: update shortcut descriptions for queue navigation"
```

---

### Task 6: Manual testing and edge case verification

**Step 1: Build and run**

Run: `nix develop -c sh -c "cargo build && cargo run -- server --bind 127.0.0.1:8080 --ephemeral"`

**Step 2: Test matrix**

Open the web UI and verify:

1. **Idle section shows pending first:** Create sessions, let them go idle. Pending thumbnails should have accent border + pulsing dot. Acknowledged ones should have default border, no dot.
2. **Dismiss flow:** Select a pending session, press Ctrl+Shift+Enter. It should lose its dot/border and sort to the end of idle section. View should jump to next pending session.
3. **Running section accuracy:** Sessions that are actually running appear in Running section. Sessions that go idle move to Idle section as pending.
4. **No duplicates:** Dismiss a session, let it go running then idle again. It should re-enter as pending without duplicates.
5. **Keyboard navigation:** Ctrl+Shift+Left/Right moves through all sessions across both sections. Wraps at boundaries.
6. **Dismiss from non-pending:** Navigate to a running or acknowledged session, press Ctrl+Shift+Enter. Should jump to oldest pending without changing current session's state.
7. **All caught up:** Dismiss all pending sessions. Shows checkmark. Film roll stays visible for manual navigation.
8. **New pending while browsing:** While viewing a running session, another session goes idle. Idle section updates with new pending thumbnail (dot + border) but selection stays on current session.

**Step 3: Final commit if any fixups needed**

```bash
git add -A
git commit -m "fix: queue view edge case fixes"
```
