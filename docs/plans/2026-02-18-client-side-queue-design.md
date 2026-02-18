# Client-Side Triage Queue

A web UI at `#/queue` that watches wsh sessions and surfaces ones needing
human attention as a card stack. Entirely client-side -- no separate backend.

## Detection

The `QueueDetector` class subscribes to all sessions via `WshClient` and
runs three detection strategies:

**Prompt detection** -- Session is quiescent and the last non-empty screen
line matches interactive prompt patterns: `?`, `[y/n]`, `(yes/no)`,
`password:`, `Enter to continue`, `(Y/n)`.

**Error detection** -- Screen contains error indicators: red foreground
spans (ANSI color 1/9), lines matching `error:`, `FAILED`, `panic`,
`Traceback`. Debounced to avoid firing per-line in a stack trace.

**Idle timeout** -- Session quiescent for 5+ seconds after activity.
Configurable threshold. Catch-all for missed patterns.

Each strategy emits a `QueueEntry` with session name, trigger type, and
relevant screen text.

## Card Model

Each card has:
- `id`: session name + generation counter
- `sessionName`: the wsh session
- `trigger`: `"prompt"` | `"error"` | `"idle"`
- `triggerText`: the line(s) that triggered detection
- `timestamp`: when detected

Cards show:
1. **Header** -- session name, trigger type badge, timestamp
2. **Live terminal** -- full interactive `Terminal` + `InputBar`
3. **Action bar** -- trigger text, buttons (Respond/Skip for prompts,
   Resolved for errors, Check/Dismiss for idle), text input + send

## Dismiss Behavior

Dismiss removes the card and bumps a generation counter for that session.
If the same session triggers again later (new prompt, new error), a new
card appears. No cooldown, no permanent mute.

## Layout

Full-viewport card stack at `#/queue`. Top card interactive, subsequent
cards peek behind (4px stagger, up to 4 visible edges). Status pill shows
count. Empty state: "All clear." Dismiss animates card up, next scales in.

## Architecture

Four new files in the existing Vite app:

| File | Purpose |
|------|---------|
| `web/src/queue/detector.ts` | QueueDetector class, heuristics, signal |
| `web/src/components/QueueView.tsx` | Top-level view, WshClient setup |
| `web/src/components/QueueCard.tsx` | Card with terminal + action bar |
| `web/src/components/CardStack.tsx` | Stack rendering, CSS offsets |

Plus routing in `main.tsx` and a link in `StatusBar.tsx`.

No new backend. No proxy rules. Uses existing `WshClient`, auth, and
session APIs. The `QueueDetector` replaces the orchestrator by using
heuristics on live terminal output.

## Build Order

1. `detector.ts` -- core detection logic + signal
2. `QueueCard.tsx` -- card component
3. `CardStack.tsx` -- stack layout
4. `QueueView.tsx` -- page wiring
5. `main.tsx` routing + `StatusBar` link
6. CSS for queue components
