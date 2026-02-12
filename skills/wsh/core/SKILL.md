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

wsh wraps a shell in a PTY and exposes everything over an HTTP API
at `http://localhost:8080`. The human sees their normal terminal. You
see a programmatic interface to the same session. Everything is
synchronized — input you send appears on their screen, output they
generate appears in your API calls.

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

    curl -s -X POST http://localhost:8080/input -d 'ls -la'
    curl -s -X POST http://localhost:8080/input -d $'ls -la\n'
    curl -s -X POST http://localhost:8080/input -d $'\x03'        # Ctrl+C
    curl -s -X POST http://localhost:8080/input -d $'\x1b'        # Escape
    curl -s -X POST http://localhost:8080/input -d $'\x1b[A'      # Arrow Up
    curl -s -X POST http://localhost:8080/input -d $'\t'          # Tab

Returns 204 (no content) on success.

### Wait for Quiescence
Block until the terminal has been idle for `timeout_ms` milliseconds.
This is a hint that the program may be idle — it could also just be
working without producing output.

    curl -s http://localhost:8080/quiesce?timeout_ms=2000

Returns the current screen snapshot plus a `generation` counter once
idle. Returns 408 if the terminal doesn't settle within 30 seconds
(configurable via `max_wait_ms`).

When polling repeatedly, pass back the `generation` from the previous
response as `last_generation` to avoid busy-loop storms:

    curl -s 'http://localhost:8080/quiesce?timeout_ms=2000&last_generation=42'

Or use `fresh=true` to always observe real silence (simpler, but
always waits at least `timeout_ms`):

    curl -s 'http://localhost:8080/quiesce?timeout_ms=2000&fresh=true'

### Read the Screen
Get the current visible screen contents.

    curl -s http://localhost:8080/screen?format=plain
    curl -s http://localhost:8080/screen?format=styled

`plain` returns simple text lines. `styled` returns spans with
color and formatting attributes.

### Read Scrollback
Get historical output that has scrolled off screen.

    curl -s http://localhost:8080/scrollback?format=plain&offset=0&limit=100

Use `offset` and `limit` to page through history.

### Health Check
Verify wsh is running.

    curl -s http://localhost:8080/health

### Real-Time Events (WebSocket)
For monitoring and input capture, you need real-time event
streaming. Connect to the JSON WebSocket:

    websocat ws://localhost:8080/ws/json

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

For per-session WebSocket in server mode:

    websocat ws://localhost:8080/sessions/:name/ws/json

You can also send requests over the WebSocket instead of
HTTP — `get_screen`, `send_input`, `capture_input`,
`release_input`, etc. Same capabilities, persistent
connection.

## Visual Elements

### Overlays
Floating text positioned on top of terminal content. They don't
affect the terminal — they're a layer on top.

    # Create an overlay at position (0, 0)
    curl -s -X POST http://localhost:8080/overlay \
      -H "Content-Type: application/json" \
      -d '{"x": 0, "y": 0, "spans": [{"text": "Hello!", "bold": true}]}'

    # Returns {"id": "uuid"} — use this to update or delete it
    curl -s -X DELETE http://localhost:8080/overlay/{id}
    curl -s -X DELETE http://localhost:8080/overlay          # clear all

Use overlays for: tooltips, status indicators, annotations,
notifications — anything that should appear *on top of* the
terminal without disrupting it.

### Panels
Agent-owned screen regions at the top or bottom of the terminal.
Unlike overlays, panels **shrink the PTY** — they carve out
dedicated space.

    curl -s -X POST http://localhost:8080/panel \
      -H "Content-Type: application/json" \
      -d '{"position": "bottom", "height": 3, "spans": [{"text": "Status: running"}]}'

Use panels for: persistent status bars, progress displays,
context summaries — anything that deserves its own screen
real estate.

### Input Capture
Intercept keyboard input so it comes to you instead of the shell.

    curl -s -X POST http://localhost:8080/input/capture    # grab input
    curl -s -X POST http://localhost:8080/input/release    # release back

While captured, keystrokes are available via WebSocket subscription
instead of going to the PTY. The human can always press Ctrl+\ to
force-release.

Use input capture for: approval prompts, custom menus, interactive
dialogs between you and the human.

## Server Mode

wsh can run as a headless daemon managing multiple sessions. This
unlocks parallel workflows — run several processes simultaneously,
each in its own terminal session.

### Checking for Server Mode
If wsh is running in server mode, the sessions endpoint is available:

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

### Session Lifecycle

    curl -s http://localhost:8080/sessions              # list all
    curl -s http://localhost:8080/sessions/build         # get info
    curl -s -X PATCH http://localhost:8080/sessions/build \
      -H "Content-Type: application/json" \
      -d '{"name": "build-v2"}'                         # rename
    curl -s -X DELETE http://localhost:8080/sessions/build  # kill

### Standalone Mode
When wsh is running in standalone mode (the default), there is a
single implicit session. Use the unprefixed endpoints — no session
name needed.

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
capture, and potentially generated programs to create bespoke
interfaces on the fly.
