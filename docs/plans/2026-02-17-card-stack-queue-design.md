# Card-Stack Triage Queue

A second web UI for wsh that surfaces sessions needing human attention as a card stack. Users clear items one by one -- approving, rejecting, typing responses, or acknowledging.

## Interaction Model

One card visible at a time, full viewport. Behind it, subsequent cards peek out (staggered 4px offsets, up to 3-4 visible edges). Dismiss the current card to reveal the next. A status pill shows the count of waiting items.

When the queue is empty: centered "All clear" message. New cards animate in from the bottom as events arrive via WebSocket push.

## Card Anatomy

Each card has three zones:

**Header** -- Session name, project name, role, timestamp. One line.

**Live terminal** -- Full interactive terminal for the session (same `<Terminal>` component from the main UI). Shows real-time output, accepts typed input.

**Action bar** -- Always visible, three parts:
1. Event description text (what happened, what's being asked)
2. Context-appropriate buttons:
   - Approval events: "Approve" / "Reject"
   - Error events: "Resolved"
   - Attention events: "Acknowledged"
3. Text input field with send button (for free-form responses)

Buttons and text input are always present. Tap a button for the common action, type for specifics.

## Card Types

Derived from the orchestrator's event model:

| Trigger | Card type | Default buttons | Terminal input sent |
|---------|-----------|-----------------|---------------------|
| `EventKind.APPROVAL` | Approval | Approve, Reject | Configurable per event (e.g., `y\n` / `n\n`) |
| `EventKind.ERROR` | Error | Resolved | None (user types fix directly in terminal) |
| `human_attention_needed=True` | Attention | Acknowledged | None |

## Architecture

- **Route**: `/queue` in the existing Vite app (same build, shared components)
- **Terminal rendering**: Reuses `<Terminal>`, `<InputBar>`, `WshClient` from main UI
- **Queue data**: New orchestrator HTTP+WebSocket server (Python, separate port)
- **Terminal data**: Existing wsh WebSocket (same `WshClient`)

The web UI connects to two backends:
1. wsh (`localhost:8080`) -- terminal state, input, session management
2. orchestrator (`localhost:9090`) -- event queue, resolutions

## Orchestrator Server

New `orchestrator/server.py` -- lightweight asyncio HTTP+WebSocket server.

### HTTP Endpoints

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `GET /queue` | GET | Events needing attention (approval, error, human_attention_needed) |
| `POST /queue/:id/resolve` | POST | Mark event as resolved. Body: `{action, text?}` |
| `GET /projects` | GET | Active projects with session counts |
| `GET /projects/:id/sessions` | GET | Sessions for a project |

### WebSocket

`/ws` -- pushes events as they arrive:
- `{type: "queue_add", entry: {...}}` -- new item needs attention
- `{type: "queue_remove", id: "..."}` -- item resolved

### Store Changes

`ContextStore` gets `resolve_entry(id, action, text)` which:
1. Marks the original entry as resolved
2. Appends a new `ContextEntry` recording the human's decision
3. Returns the resolved entry for WebSocket broadcast

## Resolution Flow

When the user clears a card:

1. POST to `orchestrator /queue/:id/resolve` with the action
2. If the action implies terminal input (Approve/Reject with configured text, or free-form text), also send that input to the wsh session via `WshClient.sendInput()`
3. Orchestrator stores the resolution, broadcasts `queue_remove` over WebSocket
4. Card animates out, next card scales up

## Implementation Phases

### Phase 1: Orchestrator server
- `orchestrator/server.py` -- asyncio HTTP+WS server
- Store methods for queue queries and resolution
- Entry point: `python -m orchestrator serve --port 9090`

### Phase 2: Queue page routing
- `/queue` route in the Vite app
- `QueueView.tsx` top-level component
- Shared components, themes, styles

### Phase 3: Card component
- `QueueCard.tsx` -- header + live Terminal + action bar
- Card type detection from event kind
- Button handlers + text input

### Phase 4: Card stack and transitions
- `CardStack.tsx` -- manages card ordering and display
- CSS stagger effect, dismiss animation (slide up, 200ms ease-out)
- Status pill, empty state

### Phase 5: Orchestrator WebSocket client
- `web/src/api/orchestrator.ts` -- connects to orchestrator WS
- Real-time card add/remove
- Resolve POST calls

### Phase 6: End-to-end wiring
- Dual resolve (orchestrator + wsh input)
- Navigation between queue and main UI
- Connection status indicators for both backends
