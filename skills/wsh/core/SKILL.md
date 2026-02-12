---
name: wsh:core
description: >
  Background knowledge about the wsh terminal API. Loaded automatically
  when wsh is available. Teaches how to interact with terminal sessions
  programmatically — sending input, reading screen output, waiting for
  quiescence, creating overlays and panels, managing sessions.
user-invocable: false
---

# wsh: Terminal as a Service

You have access to `wsh`, an API that gives you direct control over
terminal sessions. You can see exactly what's on screen, send
keystrokes, wait for commands to finish, and create visual elements —
all programmatically.

Think of it this way: wsh gives you **eyes** (read the screen),
**hands** (send input), **patience** (wait for output to settle),
and **a voice** (overlays and panels to communicate with the human).

## How It Works

wsh manages terminal sessions via a server daemon and exposes
everything over an HTTP API at `http://localhost:8080`. The human
sees their normal terminal. You see a programmatic interface to
the same session. Everything is synchronized — input you send
appears on their screen, output they generate appears in your
API calls. All endpoints are scoped to a session via
`/sessions/:name/` prefix (e.g., `/sessions/default/input`).

## The Fundamental Loop

Almost everything you do with wsh follows this pattern:

1. **Send** — inject input into the terminal
2. **Wait** — let the command run until output settles
3. **Read** — see what's on screen now
4. **Decide** — based on what you see, choose what to do next

This is your heartbeat. Learn it. A `drive-process` interaction is
just this loop repeated until the task is done.

## API Primitives

These are the building blocks. Every specialized skill builds on these.

### Send Input
Inject keystrokes into the terminal. Supports raw bytes — use
bash `$'...'` quoting for control characters.

    curl -s -X POST http://localhost:8080/sessions/default/input -d 'ls -la'
    curl -s -X POST http://localhost:8080/sessions/default/input -d $'ls -la\n'
    curl -s -X POST http://localhost:8080/sessions/default/input -d $'\x03'        # Ctrl+C
    curl -s -X POST http://localhost:8080/sessions/default/input -d $'\x1b'        # Escape
    curl -s -X POST http://localhost:8080/sessions/default/input -d $'\x1b[A'      # Arrow Up
    curl -s -X POST http://localhost:8080/sessions/default/input -d $'\t'          # Tab

Returns 204 (no content) on success.

### Wait for Quiescence
Block until the terminal has been idle for `timeout_ms` milliseconds.
This is a hint that the program may be idle — it could also just be
working without producing output.

    curl -s http://localhost:8080/sessions/default/quiesce?timeout_ms=2000

Returns the current screen snapshot plus a `generation` counter once
idle. Returns 408 if the terminal doesn't settle within 30 seconds
(configurable via `max_wait_ms`).

When polling repeatedly, pass back the `generation` from the previous
response as `last_generation` to avoid busy-loop storms:

    curl -s 'http://localhost:8080/sessions/default/quiesce?timeout_ms=2000&last_generation=42'

Or use `fresh=true` to always observe real silence (simpler, but
always waits at least `timeout_ms`):

    curl -s 'http://localhost:8080/sessions/default/quiesce?timeout_ms=2000&fresh=true'

### Read the Screen
Get the current visible screen contents.

    curl -s http://localhost:8080/sessions/default/screen?format=plain
    curl -s http://localhost:8080/sessions/default/screen?format=styled

`plain` returns simple text lines. `styled` returns spans with
color and formatting attributes.

### Read Scrollback
Get historical output that has scrolled off screen.

    curl -s http://localhost:8080/sessions/default/scrollback?format=plain&offset=0&limit=100

Use `offset` and `limit` to page through history.

### Health Check
Verify wsh is running.

    curl -s http://localhost:8080/health

### Real-Time Events (WebSocket)
For monitoring and input capture, you need real-time event
streaming. Connect to the JSON WebSocket:

    websocat ws://localhost:8080/sessions/default/ws/json

After connecting, subscribe to the events you care about:

    {"id": 1, "method": "subscribe", "params": {
      "events": ["lines", "input"],
      "format": "plain",
      "quiesce_ms": 1000
    }}

Available event types:
- `lines` — new lines of output
- `cursor` — cursor movement
- `mode` — alternate screen toggled
- `diffs` — batched screen changes
- `input` — keyboard input (essential for input capture)

The server pushes events as they happen. It also sends
periodic `sync` snapshots when the terminal goes quiet
(controlled by `quiesce_ms`).

For a different session, replace `default` with the session name:

    websocat ws://localhost:8080/sessions/build/ws/json

You can also send requests over the WebSocket instead of
HTTP — `get_screen`, `send_input`, `capture_input`,
`release_input`, `focus`, `unfocus`, `get_focus`,
`get_screen_mode`, `enter_alt_screen`, `exit_alt_screen`,
etc. Same capabilities, persistent connection.

## Visual Elements

### Overlays
Floating text positioned on top of terminal content. They don't
affect the terminal — they're a layer on top.

    # Create an overlay at position (0, 0) with explicit size
    curl -s -X POST http://localhost:8080/sessions/default/overlay \
      -H "Content-Type: application/json" \
      -d '{"x": 0, "y": 0, "width": 20, "height": 1,
           "spans": [{"text": "Hello!", "bold": true}]}'

    # Returns {"id": "uuid"} — use this to update or delete it
    curl -s -X DELETE http://localhost:8080/sessions/default/overlay/{id}
    curl -s -X DELETE http://localhost:8080/sessions/default/overlay          # clear all

**Opaque overlays:** Add `background` to fill the rectangle with a
solid color, making it a window-like element:

    curl -s -X POST http://localhost:8080/sessions/default/overlay \
      -H "Content-Type: application/json" \
      -d '{"x": 10, "y": 5, "width": 40, "height": 10,
           "background": {"bg": "black"},
           "spans": [{"text": "Window content"}]}'

Background accepts named colors (`"bg": "blue"`) or RGB
(`"bg": {"r": 30, "g": 30, "b": 30}`).

**Named spans:** Give spans an `id` for targeted updates:

    curl -s -X POST http://localhost:8080/sessions/default/overlay \
      -H "Content-Type: application/json" \
      -d '{"x": 0, "y": 0, "width": 30, "height": 1,
           "spans": [
            {"id": "label", "text": "Status: ", "bold": true},
            {"id": "value", "text": "running", "fg": "green"}
          ]}'

    # Update named spans by id (POST with array of span updates)
    curl -s -X POST http://localhost:8080/sessions/default/overlay/{id}/spans \
      -H "Content-Type: application/json" \
      -d '{"spans": [{"id": "value", "text": "stopped", "fg": "red"}]}'

**Region writes:** Place styled text at specific (row, col) offsets:

    curl -s -X POST http://localhost:8080/sessions/default/overlay/{id}/write \
      -H "Content-Type: application/json" \
      -d '{"writes": [{"row": 2, "col": 5, "text": "Hello", "bold": true}]}'

**Focusable:** Add `focusable: true` to allow focus routing during
input capture (see Input Capture below).

Use overlays for: tooltips, status indicators, annotations,
notifications — anything that should appear *on top of* the
terminal without disrupting it. With explicit dimensions: windows,
dialogs, cards.

### Panels
Agent-owned screen regions at the top or bottom of the terminal.
Unlike overlays, panels **shrink the PTY** — they carve out
dedicated space.

    curl -s -X POST http://localhost:8080/sessions/default/panel \
      -H "Content-Type: application/json" \
      -d '{"position": "bottom", "height": 3, "spans": [{"text": "Status: running"}]}'

**Background:** Add `background` to fill the panel with a solid color:

    curl -s -X POST http://localhost:8080/sessions/default/panel \
      -H "Content-Type: application/json" \
      -d '{"position": "bottom", "height": 2,
           "background": {"bg": "blue"},
           "spans": [{"text": "Status: ok"}]}'

**Named spans:** Same as overlays — give spans an `id` for targeted
updates via POST with an array of span updates:

    curl -s -X POST http://localhost:8080/sessions/default/panel/{id}/spans \
      -H "Content-Type: application/json" \
      -d '{"spans": [{"id": "status", "text": "3 errors", "fg": "red"}]}'

**Region writes:** Place text at specific (row, col) offsets:

    curl -s -X POST http://localhost:8080/sessions/default/panel/{id}/write \
      -H "Content-Type: application/json" \
      -d '{"writes": [{"row": 0, "col": 10, "text": "updated", "bold": true}]}'

**Focusable:** Add `focusable: true` to allow focus routing during
input capture.

Use panels for: persistent status bars, progress displays,
context summaries — anything that deserves its own screen
real estate.

### Input Capture
Intercept keyboard input so it comes to you instead of the shell.

    curl -s -X POST http://localhost:8080/sessions/default/input/capture    # grab input
    curl -s -X POST http://localhost:8080/sessions/default/input/release    # release back

While captured, keystrokes are available via WebSocket subscription
instead of going to the PTY. The human can press Ctrl+\ to toggle
capture mode (it switches between passthrough and capture).

**Focus routing:** Direct captured input to a specific focusable
overlay or panel. At most one element has focus at a time.

    curl -s -X POST http://localhost:8080/sessions/default/input/focus \
      -H "Content-Type: application/json" \
      -d '{"id": "overlay-uuid"}'

    curl -s http://localhost:8080/sessions/default/input/focus               # get current focus
    curl -s -X POST http://localhost:8080/sessions/default/input/unfocus     # clear focus

Focus is automatically cleared when input is released or when the
focused element is deleted.

Use input capture for: approval prompts, custom menus, interactive
dialogs between you and the human.

### Alternate Screen Mode
Enter a separate screen mode where you can create a completely
independent set of overlays and panels. Exiting cleans up everything
automatically.

    curl -s http://localhost:8080/sessions/default/screen_mode                  # get current mode
    curl -s -X POST http://localhost:8080/sessions/default/screen_mode/enter_alt  # enter alt screen
    curl -s -X POST http://localhost:8080/sessions/default/screen_mode/exit_alt   # exit alt screen

Overlays and panels are automatically tagged with the screen mode
active at the time of creation. List endpoints return only elements
belonging to the current mode. When you exit alt screen, all elements
created in alt mode are deleted and the original screen's elements
are restored.

Use alt screen mode for: temporary full-screen agent UIs, setup
wizards, immersive dashboards — anything that needs a clean canvas
and should leave no trace when done.

## Session Management

wsh always runs as a server daemon managing sessions. The sessions
endpoint is always available:

    curl -s http://localhost:8080/sessions

### Creating Sessions

    curl -s -X POST http://localhost:8080/sessions \
      -H "Content-Type: application/json" \
      -d '{"name": "build", "command": "cargo build"}'

Returns `{"name": "build"}` on success.

### Interacting with a Specific Session
All the primitives work per-session by adding `/sessions/:name/`
as a prefix:

    curl -s -X POST http://localhost:8080/sessions/build/input -d $'cargo test\n'
    curl -s http://localhost:8080/sessions/build/quiesce?timeout_ms=2000
    curl -s http://localhost:8080/sessions/build/screen?format=plain

Overlays, panels, and input capture are also per-session.

### Wait for Quiescence on Any Session
You can race quiescence across all sessions:

    curl -s 'http://localhost:8080/sessions/default/quiesce?timeout_ms=2000&format=plain'

Returns the first session to become quiescent, including its name:

    {"session": "build", "screen": {...}, "scrollback_lines": 42, "generation": 7}

To avoid re-returning the same session, pass `last_session` and
`last_generation` from the previous response:

    curl -s 'http://localhost:8080/sessions/default/quiesce?timeout_ms=2000&last_session=build&last_generation=7'

Returns 404 (`no_sessions`) if no sessions exist. Returns 408 if no
session settles within `max_wait_ms`.

### Session Lifecycle

    curl -s http://localhost:8080/sessions              # list all
    curl -s http://localhost:8080/sessions/build         # get info
    curl -s -X PATCH http://localhost:8080/sessions/build \
      -H "Content-Type: application/json" \
      -d '{"name": "build-v2"}'                         # rename
    curl -s -X DELETE http://localhost:8080/sessions/build  # kill

### Default Session
When wsh is started with `wsh` (no arguments), it auto-spawns a
server daemon and creates a session named `default`. Use
`/sessions/default/` prefix for all endpoints. If started with
`--name`, the session has that name instead.

## Specialized Skills

When your task matches one of these patterns, invoke the
corresponding skill for detailed guidance.

**wsh:drive-process** — You need to run a CLI command and interact
with it. Sending input, reading output, handling prompts, navigating
sequential command-and-response workflows.

**wsh:tui** — You need to operate a full-screen terminal application
like vim, htop, lazygit, or k9s. Reading a 2D grid, sending
navigation keys, understanding menus and panes.

**wsh:multi-session** — You need to run multiple things in parallel.
Spawning sessions, monitoring them, collecting results across
sessions.

**wsh:agent-orchestration** — You need to drive another AI agent
(Claude Code, Aider, etc.) through its terminal interface. Feeding
tasks, handling approval prompts, reviewing agent output.

**wsh:monitor** — You need to watch what a human is doing and react.
Subscribing to terminal events, detecting patterns, providing
contextual assistance or auditing.

**wsh:visual-feedback** — You need to communicate with the human
visually. Building overlay notifications, status panels, progress
displays, contextual annotations.

**wsh:input-capture** — You need to take over keyboard input
temporarily. Building approval workflows, custom menus, interactive
dialogs.

**wsh:generative-ui** — You need to build a dynamic interactive
experience in the terminal. Combining overlays, panels, input
capture, direct drawing, and alternate screen mode to create
bespoke interfaces on the fly.
