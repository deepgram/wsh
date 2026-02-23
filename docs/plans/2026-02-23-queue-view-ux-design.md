# Queue View UX Redesign

## Problem

The queue view has three bugs stemming from a flawed dismiss model:

1. Dismissed sessions always appear in "Running" regardless of actual activity state.
2. Running thumbnails are too dim (0.5 opacity container + 0.4 opacity on handled items).
3. The running queue grows indefinitely with duplicate entries because `enqueueSession()` only deduplicates against `"pending"` entries, not `"handled"` ones.

Beyond the bugs, the dismiss model is wrong: dismissing an idle session shouldn't banish it from the view entirely. Users need to be able to navigate back to any session without resorting to command palette or sidebar.

## Design

### Data Model

The `QueueEntry` status changes from `"pending" | "handled"` to `"pending" | "acknowledged"`:

- **`"pending"`** — session went idle, user hasn't dismissed it yet.
- **`"acknowledged"`** — user dismissed it. Stays in the idle queue, visible but without attention indicators.

State transitions are driven by `sessionStatuses` (the source of truth from the API):

- **Session goes idle** (running->idle in `sessionStatuses`): add to queue as `"pending"` if no entry exists.
- **Session starts running** (idle->running in `sessionStatuses`): remove entry from queue entirely. This ensures it re-enters as `"pending"` on the next idle transition.
- **User dismisses**: mark `"pending"` -> `"acknowledged"` optimistically (no round-trip needed; `sessionStatuses` reconciles on next update).

### Film Roll Layout

Two sections in the top bar:

1. **Idle (N)** — all sessions in the idle queue. Sorted: pending (unacknowledged) first by `idleAt`, then acknowledged by `idleAt`. Label shows pending count when nonzero: `Idle (2 new · 5)`.
2. **Running (N)** — sessions whose `sessionStatuses` value is not `"idle"`. Hidden when empty.

### Thumbnail Visual States

| State | Border | Badge | Opacity |
|-------|--------|-------|---------|
| Pending (unacknowledged) idle | Accent color | Small pulsing dot, top-right corner | Full |
| Acknowledged idle | Default (`--border`) | None | Full |
| Running | Default | None | Full |
| Currently selected (any) | Accent + box-shadow (existing `.active`) | Unchanged | Full |

All thumbnails are full opacity. No muted/dimmed sections.

### Navigation

Left/right arrow keys (`Ctrl+Shift+H/L` or `Ctrl+Shift+Left/Right`) navigate a single flat list across both sections: idle (pending first, then acknowledged, each by `idleAt`), then running. Wraps circularly, matching carousel behavior.

On entering queue view: auto-select the oldest pending idle session. If none, select the first session in navigation order.

When navigating freely and a new session goes idle: the user's selection is not interrupted. The new pending thumbnail (accent border + dot) and updated section label serve as the visual nudge.

### Dismiss Action (Ctrl+Shift+Enter)

Always means "take me to the next session needing my attention":

1. **Current session is pending idle:** mark as `"acknowledged"` (optimistic). Jump to oldest remaining pending session.
2. **Current session is acknowledged idle or running:** don't change its state. Jump to oldest pending session.
3. **No pending sessions remain:** show "All caught up" checkmark. Film roll stays visible for manual navigation.

The shortcut hint (`Ctrl+Shift+Enter to dismiss`) is always visible.

### State Lifecycle

```
Session created
    |
    v
[Running section]  <--------------------------+
    |                                          |
    |  session goes idle                       |  session starts running
    |  (sessionStatuses: running->idle)        |  (sessionStatuses: idle->running)
    v                                          |
[Idle section, pending]                        |
  accent border + dot badge                    |
    |                                          |
    |  user dismisses (Ctrl+Shift+Enter)       |
    v                                          |
[Idle section, acknowledged]                   |
  default border, no dot                       |
    |                                          |
    |  session starts running                  |
    +------------------------------------------+
          (entry removed from queue;
           re-enters as pending on
           next idle transition)
```

### Consistency Model

- **Optimistic** for user-initiated changes: dismiss immediately updates the thumbnail visually.
- **Reactive** for backend-driven changes: the effect watching `sessionStatuses` is the reconciliation layer. If the backend disagrees (e.g., session went running between dismiss and next status push), the effect corrects it.
- The API (`sessionStatuses` from WebSocket subscription) is always the source of truth. The UI never assumes it caused a state transition.

### Edge Cases

- **Session killed/removed:** entry removed from queue. If selected, advance to next session in navigation order.
- **Session goes running without user intervention** (external interaction, long-running process): entry removed from queue on next `sessionStatuses` update, session appears in Running section.
- **Initial load:** `prevStatuses` ref starts empty, so all initially-idle sessions enter as `"pending"` (since `prev !== "idle"` is true when prev is undefined).
