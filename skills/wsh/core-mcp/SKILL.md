---
name: wsh:core-mcp
description: >
  Background knowledge about the wsh terminal API for MCP clients. Loaded
  automatically when wsh is available as an MCP server. Teaches how to
  interact with terminal sessions programmatically using MCP tools —
  sending input, reading screen output, waiting for quiescence, creating
  overlays and panels, managing sessions.
user-invocable: false
---

# wsh: Terminal as a Service (MCP)

You have access to `wsh` via MCP tools that give you direct control over
terminal sessions. You can see exactly what's on screen, send
keystrokes, wait for commands to finish, and create visual elements —
all through MCP tool calls.

Think of it this way: wsh gives you **eyes** (read the screen),
**hands** (send input), **patience** (wait for output to settle),
and **a voice** (overlays and panels to communicate with the human).

## How It Works

wsh manages terminal sessions via a server daemon and exposes
everything as MCP tools. The human sees their normal terminal. You
interact through tool calls to the same session. Everything is
synchronized — input you send appears on their screen, output they
generate appears in your tool responses. All tools take a `session`
parameter to specify which session to operate on (e.g., `"default"`).

## The Fundamental Loop

Almost everything you do with wsh follows this pattern:

1. **Send** — inject input into the terminal
2. **Wait** — let the command run until output settles
3. **Read** — see what's on screen now
4. **Decide** — based on what you see, choose what to do next

This is your heartbeat. Learn it. A `drive-process` interaction is
just this loop repeated until the task is done.

## MCP Tools

These are the building blocks. Every specialized skill builds on these.

### Run a Command (Send + Wait + Read)
The primary tool for the send/wait/read loop. Sends input, waits for
quiescence, then returns the screen contents.

Use `wsh_run_command` with:
- `session` — target session name (e.g., `"default"`)
- `input` — the text to send (include `\n` for Enter)
- `timeout_ms` — quiescence timeout (default 2000)
- `max_wait_ms` — maximum wall-clock wait (default 30000)
- `format` — `"plain"` or `"styled"` (default `"styled"`)

Example: run `ls -la` and read the result:

    wsh_run_command(session="default", input="ls -la\n", format="plain")

Returns the screen contents plus a `generation` counter. If the
terminal doesn't settle within `max_wait_ms`, the screen is still
returned but flagged as an error.

### Send Input
Inject keystrokes into the terminal. Supports UTF-8 text (default)
or base64-encoded binary for control characters.

Use `wsh_send_input` with:
- `session` — target session name
- `input` — the text or data to send
- `encoding` — `"utf8"` (default) or `"base64"`

Examples:
- Send a command: `wsh_send_input(session="default", input="ls -la\n")`
- Send Ctrl+C: `wsh_send_input(session="default", input="Aw==", encoding="base64")` (base64 of `\x03`)
- Send Escape: `wsh_send_input(session="default", input="Gw==", encoding="base64")` (base64 of `\x1b`)
- Send Arrow Up: `wsh_send_input(session="default", input="G1tB", encoding="base64")` (base64 of `\x1b[A`)
- Send Tab: `wsh_send_input(session="default", input="\t")`

Returns `{"status": "sent", "bytes": N}` on success.

### Wait for Quiescence
Block until the terminal has been idle for `timeout_ms` milliseconds.
This is a hint that the program may be idle — it could also just be
working without producing output.

Use `wsh_await_quiesce` with:
- `session` — target session name
- `timeout_ms` — idle duration to wait for (default 2000)
- `max_wait_ms` — maximum wall-clock wait (default 30000)

Returns `{"status": "quiescent", "generation": N}` once idle.
Returns an error result if the terminal doesn't settle within
`max_wait_ms`.

### Read the Screen
Get the current visible screen contents.

Use `wsh_get_screen` with:
- `session` — target session name
- `format` — `"plain"` for simple text or `"styled"` for spans with color/formatting (default `"styled"`)

### Read Scrollback
Get historical output that has scrolled off screen.

Use `wsh_get_scrollback` with:
- `session` — target session name
- `offset` — line offset into scrollback (default 0)
- `limit` — max lines to return (default 100)
- `format` — `"plain"` or `"styled"` (default `"styled"`)

## Visual Elements

### Overlays
Floating text positioned on top of terminal content. They don't
affect the terminal — they're a layer on top.

Use `wsh_overlay` to create, update, or list overlays:

**Create an overlay:**

    wsh_overlay(
      session="default",
      x=0, y=0, width=20, height=1,
      spans=[{"text": "Hello!", "bold": true}]
    )

Returns `{"status": "created", "id": "uuid"}` — use this ID to update or delete.

**Update an overlay** (provide `id`):

    wsh_overlay(
      session="default",
      id="<overlay-id>",
      spans=[{"text": "Updated!", "fg": "green"}]
    )

**List overlays:**

    wsh_overlay(session="default", list=true)

**Opaque overlays:** Add `background` to fill the rectangle with a
solid color, making it a window-like element:

    wsh_overlay(
      session="default",
      x=10, y=5, width=40, height=10,
      background={"bg": "black"},
      spans=[{"text": "Window content"}]
    )

Background accepts named colors (`"blue"`) or RGB
(`{"r": 30, "g": 30, "b": 30}`).

**Focusable:** Add `focusable=true` to allow focus routing during
input capture (see Input Capture below).

Use `wsh_remove_overlay` to remove overlays:
- With `id` — remove a specific overlay
- Without `id` — clear all overlays

Use overlays for: tooltips, status indicators, annotations,
notifications — anything that should appear *on top of* the
terminal without disrupting it. With explicit dimensions: windows,
dialogs, cards.

### Panels
Agent-owned screen regions at the top or bottom of the terminal.
Unlike overlays, panels **shrink the PTY** — they carve out
dedicated space.

Use `wsh_panel` to create, update, or list panels:

**Create a panel:**

    wsh_panel(
      session="default",
      position="bottom", height=3,
      spans=[{"text": "Status: running"}]
    )

**Update a panel** (provide `id`):

    wsh_panel(
      session="default",
      id="<panel-id>",
      spans=[{"text": "Status: done", "fg": "green"}]
    )

**List panels:**

    wsh_panel(session="default", list=true)

**Background:** Add `background` to fill the panel with a solid color:

    wsh_panel(
      session="default",
      position="bottom", height=2,
      background={"bg": "blue"},
      spans=[{"text": "Status: ok"}]
    )

**Focusable:** Add `focusable=true` to allow focus routing during
input capture.

Use `wsh_remove_panel` to remove panels:
- With `id` — remove a specific panel
- Without `id` — clear all panels

Use panels for: persistent status bars, progress displays,
context summaries — anything that deserves its own screen
real estate.

### Input Capture
Intercept keyboard input so it comes to you instead of the shell.

Use `wsh_input_mode` to query or change input mode and focus:
- `mode="capture"` — grab input (keystrokes go to API only)
- `mode="release"` — release back (keystrokes go to PTY)
- `focus="<element-id>"` — direct captured input to a specific focusable overlay or panel
- `unfocus=true` — clear focus
- No mode/focus params — query current state

The human can press Ctrl+\ to toggle capture mode (it switches
between passthrough and capture).

Focus is automatically cleared when input is released or when the
focused element is deleted.

Use input capture for: approval prompts, custom menus, interactive
dialogs between you and the human.

### Alternate Screen Mode
Enter a separate screen mode where you can create a completely
independent set of overlays and panels. Exiting cleans up everything
automatically.

Use `wsh_screen_mode` to query or change screen mode:
- `action="enter_alt"` — enter alternate screen mode
- `action="exit_alt"` — exit alternate screen mode
- No action — query current mode (`"normal"` or `"alt"`)

Overlays and panels are automatically tagged with the screen mode
active at the time of creation. When you exit alt screen, all elements
created in alt mode are deleted and the original screen's elements
are restored.

Use alt screen mode for: temporary full-screen agent UIs, setup
wizards, immersive dashboards — anything that needs a clean canvas
and should leave no trace when done.

## Session Management

wsh always runs as a server daemon managing sessions. Use these tools
to manage session lifecycle:

### List Sessions

    wsh_list_sessions()                          # list all
    wsh_list_sessions(session="build")           # get details for one

### Create Sessions

    wsh_create_session(name="build", command="cargo build")

Optional parameters: `rows`, `cols`, `cwd`, `env`.
Returns `{"name": "build", "rows": 24, "cols": 80}`.

### Manage Sessions

    wsh_manage_session(session="build", action="kill")            # destroy
    wsh_manage_session(session="build", action="rename", new_name="build-v2")  # rename
    wsh_manage_session(session="build", action="detach")          # disconnect clients

### Default Session
When wsh is started with `wsh` (no arguments), it auto-spawns a
server daemon and creates a session named `default`. Use
`session="default"` for all tool calls. If started with `--name`,
the session has that name instead.

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
