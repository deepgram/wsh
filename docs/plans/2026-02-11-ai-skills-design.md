# AI Skills Design

## Overview

wsh exposes terminal I/O as an API. AI skills teach agents how to use that API effectively. This document defines a set of Claude Code skills that enable AI agents to drive terminal processes, observe human activity, communicate visually, and build dynamic terminal experiences.

## Architecture: Two-Tier Skill System

Skills are organized into two tiers:

**Core skill** (`wsh:core`) — Always visible. Establishes the mental model, teaches the API primitives with concrete invocation examples, and routes to specialized skills. This is the only invocation-specific skill; if the interaction method changes (e.g., from curl to MCP tools), only this skill needs updating.

**Specialized skills** (8 total) — Loaded on demand when Claude detects a relevant task. Written in terms of concepts defined by the core skill, not raw API calls. Each covers a distinct interaction posture.

```
┌─────────────────────────────────────────────┐
│  Specialized Skills                         │
│  "send input, wait, read screen"            │  ← concepts
├─────────────────────────────────────────────┤
│  Core Skill                                 │
│  "POST /input sends input"                  │  ← mechanics
│  "GET /quiesce waits for idle"              │
│  "GET /screen reads the display"            │
│  "here are the curl commands"               │
└─────────────────────────────────────────────┘
```

### Skill Roster

| # | Skill | Posture | One-liner |
|---|-------|---------|-----------|
| 0 | `wsh:core` | Foundation | "You have a terminal API — here's what's possible" |
| 1 | `wsh:drive-process` | AI acts | Drive CLI programs through command-and-response |
| 2 | `wsh:tui` | AI acts | Operate full-screen TUI applications |
| 3 | `wsh:multi-session` | AI acts | Orchestrate multiple parallel terminal sessions |
| 4 | `wsh:agent-orchestration` | AI acts | Drive other AI agents via their terminal interfaces |
| 5 | `wsh:monitor` | AI observes | Watch and react to human terminal activity |
| 6 | `wsh:visual-feedback` | AI speaks | Communicate with humans via overlays and panels |
| 7 | `wsh:input-capture` | AI listens | Capture keyboard input for dialogs and approvals |
| 8 | `wsh:generative-ui` | AI creates | Build dynamic interactive terminal experiences |

### Invocation Method

The core skill teaches Claude to interact with wsh via HTTP requests using `curl` from the Bash tool. This requires zero additional software. An MCP server wrapping the wsh API is a natural follow-on that would make execution cleaner and less error-prone, but the specialized skills are written invocation-agnostically so they wouldn't need to change.

---

## Skill 0: wsh:core

The foundation skill. Always visible in the system prompt. Establishes the mental model, teaches the API primitives, and routes to specialized skills.

### Section 1: Mental Model

```markdown
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
```

### Section 2: API Primitives

```markdown
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

Returns the current screen snapshot once idle. Returns 408 if the
terminal doesn't settle within 30 seconds (configurable via
`max_wait_ms`).

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
```

### Section 3: Visual Elements

```markdown
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
```

### Section 4: Server Mode and Sessions

```markdown
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
```

### Section 5: Skill Routing

```markdown
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
```

---

## Skill 1: wsh:drive-process

The flagship skill. Teaches the AI how to drive CLI programs through command-and-response interaction.

### Section 1: The Interaction Loop

```markdown
# wsh:drive-process — Driving CLI Programs

You're operating a terminal programmatically. You send input, wait
for output to settle, read the screen, and decide what to do next.
This skill teaches you the patterns and pitfalls.

## The Loop

Every interaction follows the same shape:

1. **Send input** — a command, a response to a prompt, a keystroke
2. **Wait for quiescence** — output settles, suggesting the program
   may be idle. Choose your timeout based on what you expect:
   - Fast commands (ls, cat, echo): 500-1000ms
   - Build/install commands: 3000-5000ms
   - Network operations: 2000-3000ms
   Quiescence is a *hint*, not a guarantee. The program may still
   be working — it just hasn't produced output recently.
3. **Read the screen** — see what happened
4. **Decide** — did the command succeed? Is there a prompt waiting
   for input? Did something go wrong? Act accordingly.

## Sending a Command

Always include a newline to "press Enter":

    send input: npm install\n

Without the trailing `\n`, you've typed the text but haven't
submitted it. Sometimes that's what you want (e.g., building up
a command before sending), but usually you want the newline.

## Reading the Result

After waiting for quiescence, read the screen. Prefer `plain`
format when you just need text content. Use `styled` when
formatting matters (e.g., distinguishing error output highlighted
in red).

If the output is long, it may have scrolled off screen. Use
scrollback to get the full history.
```

### Section 2: Interactive Prompts and Control Characters

```markdown
## Handling Interactive Prompts

Many programs ask questions and wait for a response. After reading
the screen, look for patterns like:

- `[Y/n]` or `[y/N]` — yes/no confirmation
- `Password:` or `Enter passphrase:` — credential prompts
- `>` or `?` — interactive selection (fzf, inquirer, etc.)
- `(yes/no)` — full-word confirmation (e.g., SSH host verification)
- `Press any key to continue`

Respond naturally — send the appropriate input:

    send input: y\n
    send input: yes\n

For password prompts, note that the terminal will not echo your
input back. The screen will look unchanged after you type. Wait
for quiescence after sending — the program will advance.

## Control Characters

These are your emergency exits and special actions:

    $'\x03'         # Ctrl+C  — interrupt / cancel
    $'\x04'         # Ctrl+D  — EOF / exit shell
    $'\x1a'         # Ctrl+Z  — suspend process
    $'\x0c'         # Ctrl+L  — clear screen
    $'\x01'         # Ctrl+A  — beginning of line
    $'\x05'         # Ctrl+E  — end of line
    $'\x15'         # Ctrl+U  — clear line
    $'\x1b'         # Escape

If a command hangs or you need to bail out, Ctrl+C is your first
resort. If the process doesn't respond to Ctrl+C, Ctrl+D or
Ctrl+Z may work. Read the screen after each attempt to see if
it had effect.
```

### Section 3: Error Detection and Long-Running Commands

```markdown
## Detecting Success and Failure

After reading the screen, look for signals:

**Success indicators:**
- A fresh shell prompt (`$`, `#`, `❯`) on the last line
- Explicit success messages ("done", "completed", "ok")
- Exit code 0 if visible

**Failure indicators:**
- Words like "error", "failed", "fatal", "denied", "not found"
- Stack traces or tracebacks
- A shell prompt after unexpectedly short output
- Non-zero exit codes

When in doubt, check the exit code explicitly:

    send input: echo $?\n

A `0` means the previous command succeeded. Anything else is
a failure.

## Long-Running Commands

Some commands run for minutes or longer — builds, downloads,
test suites. Waiting for quiescence will return when output
pauses, but the command may not be done.

Strategies:

**Poll in a loop.** Wait for quiescence, read the screen, check
if a shell prompt has returned. If not, wait again:

    wait for quiescence (timeout: 5000ms)
    read screen
    # No prompt yet? Wait again.

**Use scrollback for full output.** Long commands produce output
that scrolls off screen. After the command finishes, read
scrollback to get everything:

    read scrollback (offset: 0, limit: 500)

**Don't set unreasonably long quiescence timeouts.** A
`timeout_ms=30000` means you'll wait 30 seconds of silence
before getting a response. Prefer shorter timeouts with
repeated polls — it lets you observe intermediate progress
and react if something goes wrong.
```

### Section 4: Common Workflow Patterns

```markdown
## Common Patterns

### Chained Commands
When you need to run several commands in sequence, you have two
options. Run them as separate send/wait/read cycles when you need
to inspect output between steps:

    # Step 1
    send: cd /project
    wait, read — verify directory exists

    # Step 2
    send: npm install
    wait, read — check for errors

    # Step 3
    send: npm test
    wait, read — check results

Or chain with `&&` when intermediate output doesn't matter:

    send: cd /project && npm install && npm test
    wait, read — check final result

Prefer separate cycles. They give you the chance to detect
problems early and adjust.

### Piped Commands
Pipes work naturally. Send the full pipeline:

    send: grep -r "TODO" src/ | wc -l

### Background Processes
If you start a background process (`&`), it won't block the shell
prompt. But its output may interleave with future commands.
Consider redirecting output:

    send: ./long-task.sh > /tmp/task.log 2>&1 &

Then check on it later:

    send: cat /tmp/task.log

### Pagers
Commands like `git log`, `man`, or `less` enter a pager that
waits for keyboard navigation. If you just need the content,
bypass the pager:

    send: git log --no-pager
    send: PAGER=cat man ls

If you're already stuck in a pager, press `q` to exit:

    send: q

### Heredocs and Multi-Line Input
To write multi-line content, use heredocs:

    send: cat > /tmp/config.yaml << 'EOF'\n
    send: key: value\n
    send: other: thing\n
    send: EOF\n
```

### Section 5: Pitfalls and Guardrails

```markdown
## Pitfalls

### Don't skip the wait
It's tempting to send input immediately after the previous
command. Don't. If the shell hasn't finished processing, your
input may land in the wrong place — or be swallowed entirely.
Always wait for quiescence before sending the next input.

### Don't assume the screen is everything
The screen shows only the last N lines (typically 24 rows). A
command that produced 500 lines of output will have 476 lines
in scrollback. If you need full output, read scrollback.

### Watch for prompts you didn't expect
Installers, package managers, and system tools love to ask
surprise questions. If you read the screen and see no shell
prompt but also no obvious output-in-progress, look for a
prompt waiting for your response.

### Destructive commands
You are operating a real terminal on a real machine. `rm`,
`DROP TABLE`, `git push --force` — these do real damage.
Before running destructive commands:
- Confirm with the human via overlay, panel, or input capture
- Double-check paths and arguments
- Prefer dry-run flags when available (--dry-run, --whatif, -n)

### Knowing when to give up
If a command is stuck and not responding to Ctrl+C, don't
hammer it with more input. Strategies in order:
1. Send Ctrl+C (`$'\x03'`)
2. Wait a moment, try Ctrl+C again
3. Send Ctrl+Z (`$'\x1a'`) to suspend, then `kill %1`
4. Tell the human what's happening and ask for help

### Shell state persists
You're in a real shell session. Environment variables you set,
directories you `cd` into, background jobs you spawn — they
all persist. Be mindful of the state you leave behind.
```

---

## Skill 2: wsh:tui

Teaches the AI how to operate full-screen terminal applications that use the alternate screen buffer.

### Section 1: The Alternate Screen

```markdown
# wsh:tui — Operating Full-Screen Terminal Applications

Some programs take over the entire terminal — vim, htop, lazygit,
k9s, midnight commander. They use the terminal's "alternate screen
buffer," a fixed grid where the program controls every character
position. This is a fundamentally different interaction model from
command-and-response.

## Detecting Alternate Screen Mode

When a TUI is active, the screen response includes:

    "alternate_active": true

This tells you you're in grid mode. The screen is no longer a
log of output — it's a 2D canvas the program redraws at will.
Scrollback is irrelevant while alternate screen is active;
the program owns the entire display.

When the TUI exits, `alternate_active` flips back to `false`
and the normal scrollback view resumes exactly where it left
off. None of the TUI's screen content leaks into scrollback.

## Reading a 2D Grid

In a TUI, screen position matters. A line isn't just text —
it's a row in a spatial layout. Use `styled` format here;
formatting carries critical information:

- **Bold or highlighted text** often marks the selected item
- **Color differences** distinguish panes, headers, status bars
- **Inverse/reverse video** typically indicates cursor position
  or selection
- **Dim or faint text** marks inactive elements

Read the full screen and interpret it spatially. The first few
lines are often a header or menu bar. The last line or two are
often a status bar or command input. The middle is content.
```

### Section 2: Navigation and Input

```markdown
## Navigation

TUI programs don't use typed commands — they use keystrokes.
Every key does something different depending on context. You
need to know the program's keybindings.

### Universal Navigation Keys

    $'\x1b[A'       # Arrow Up
    $'\x1b[B'       # Arrow Down
    $'\x1b[C'       # Arrow Right
    $'\x1b[D'       # Arrow Left
    $'\x1b[5~'      # Page Up
    $'\x1b[6~'      # Page Down
    $'\x1b[H'       # Home
    $'\x1b[F'       # End
    $'\t'           # Tab (often cycles panes or fields)
    $'\n'           # Enter (confirm / open)
    $'\x1b'         # Escape (cancel / back)

### Vim-Style Navigation
Many TUIs adopt vim conventions:

    h, j, k, l      # left, down, up, right
    g, G             # top, bottom
    /                # search
    n, N             # next/previous match
    q                # quit

### Sending Keystrokes
Send one keystroke at a time. Wait briefly between keystrokes
to let the TUI redraw — TUIs repaint the screen after each
input, and you need the updated screen to know where you are.

    send: j          # move down
    wait (500ms)
    read screen      # see what's selected now
    send: j          # move down again
    wait (500ms)
    read screen      # verify position

This is slower than blasting keys, but reliable. You're
navigating blind if you don't read between keystrokes.
```

### Section 3: Orienting in a TUI

```markdown
## Understanding TUI Layouts

When you first enter a TUI, read the full screen and build a
mental map. Most TUIs follow common layout patterns:

### Typical Structure

    ┌──────────────────────────────────┐
    │ Menu bar / Title bar             │  ← rows 0-1
    ├──────────────────────────────────┤
    │                                  │
    │ Main content area                │  ← middle rows
    │ (list, editor, dashboard)        │
    │                                  │
    ├──────────────────────────────────┤
    │ Status bar / Help / Command line │  ← last 1-2 rows
    └──────────────────────────────────┘

### Finding Your Bearings
- **Status bar** (usually bottom): shows mode, filename,
  position, hints. Read this first — it often tells you
  everything you need to know about current state.
- **Help hints**: many TUIs show keybinding hints at the
  bottom or top. Look for text like `q:quit  j/k:navigate
  ?:help` or `^X Exit  ^O Save`.
- **The selected item**: look for inverse video, bold, or
  color-highlighted text in the content area. That's your
  cursor position.
- **Pane borders**: look for `│`, `─`, `┌`, `└` characters.
  These indicate split panes. Only one pane is active at
  a time — Tab or Ctrl+W typically switches between them.

### Modals and Dialogs
TUIs often pop up confirmation dialogs or input fields over
the main content. These appear as a differently-styled block
in the middle of the screen. Look for:
- A bordered box that wasn't there before
- Text like "Are you sure?" or "Enter filename"
- Highlighted buttons like `[ OK ]  [ Cancel ]`

When a modal is active, navigation keys operate on the modal,
not the content behind it.
```

### Section 4: Common TUI Applications

```markdown
## Common Applications

You don't need to memorize every TUI's keybindings. But
knowing the basics for frequently encountered programs helps.

### Text Editors
**vim/neovim:** Starts in Normal mode. `i` to insert text,
`Esc` to return to Normal. `:w` save, `:q` quit, `:wq` both.
If lost, press `Esc Esc` then `:q!` to quit without saving.

**nano:** Simpler. Just type to edit. Keybindings shown at
bottom. `^` means Ctrl. `^X` exits, `^O` saves.

### Git TUIs
**lazygit:** Pane-based. Tab switches panes (files, branches,
commits). `j/k` navigates, Enter opens, `space` stages, `c`
commits, `p` pushes, `q` quits.

### System Monitors
**htop/top:** Shows processes. `j/k` or arrows to navigate,
`k` to kill a process, `q` to quit. `F` keys for actions
(shown at bottom).

### Kubernetes
**k9s:** Resource browser. `:` opens command mode for
resource type (`:pods`, `:deployments`). `j/k` navigates,
`Enter` drills in, `Esc` goes back, `d` describe, `l` logs.

### General Strategy for Unfamiliar TUIs
1. Read the screen — look for help hints at top or bottom
2. Try `?` or `h` — most TUIs open a help screen
3. Try `F1` (`$'\x1bOP'`) — some use function keys for help
4. Read the help, then press `q` or `Esc` to close it
5. If completely stuck: `q`, `Esc`, `:q`, `Ctrl+C`,
   `Ctrl+Q` — try these in order to exit
```

### Section 5: Exiting and Pitfalls

```markdown
## Exiting a TUI

Getting out is as important as getting in. When you're done
with a TUI, you need to return to the normal shell prompt.

### Exit Strategies (in order of preference)
1. Use the program's quit command (`q`, `:q`, `Ctrl+X`)
2. Check the status bar for exit hints
3. Press `Esc` to back out of any modal or sub-mode first
4. If the program won't quit cleanly, `Ctrl+C`
5. Last resort: `Ctrl+Z` to suspend, then `kill %1`

### Confirming You're Out
After sending a quit command, wait for quiescence, then check:

    "alternate_active": false

If this is `false`, you're back in normal mode with your shell
prompt. If it's still `true`, the TUI is still running —
your quit command may not have worked, or the program asked
for confirmation before exiting.

## Pitfalls

### Don't type commands into a TUI
A TUI is not a shell. If you send `ls -la\n` into vim, you'll
get those characters inserted into the document. Always know
what mode you're in before sending input.

### Don't blast keystrokes
TUIs redraw the screen after each input. If you send 10 `j`
keystrokes without reading in between, you won't know where
you landed. Navigate one step at a time.

### Watch for mode changes
Many TUIs have multiple modes (vim's Normal/Insert/Visual,
lazygit's panes). The same key does different things in
different modes. Read the screen after each action to confirm
you're in the mode you expect.

### Alternate screen within alternate screen
If you launch a TUI from within a TUI (e.g., vim from within
a file manager), `alternate_active` is still just `true`. You
need to track the nesting yourself by remembering what you
launched and how many layers deep you are.
```

---

## Skill 3: wsh:multi-session

Teaches the AI how to orchestrate multiple parallel terminal sessions using wsh's server mode.

### Section 1: When and Why

```markdown
# wsh:multi-session — Parallel Terminal Sessions

Sometimes one terminal isn't enough. You need to run a build
while tailing logs. Run tests across three environments
simultaneously. Drive multiple processes that each need
independent input and output. Multi-session gives you this.

## When to Use Multiple Sessions

**Use multi-session when:**
- Tasks are independent and can run in parallel
- You need isolated environments (different directories,
  different env vars, different shells)
- A long-running process needs monitoring while you work
  in another session
- You're coordinating multiple tools that each need their
  own terminal

**Don't use multi-session when:**
- A single shell with `&&` or `&` would suffice
- The tasks are strictly sequential
- You only need to run one thing at a time

## Server Mode

Multi-session requires wsh to be running in server mode:

    wsh server --bind 127.0.0.1:8080

In server mode, there is no implicit session — you create
them explicitly. The API base URL is the same, but you
interact with sessions through the `/sessions/` prefix.

If wsh is running in standalone mode (single session), you
cannot create additional sessions. Check which mode you're
in by listing sessions — if it succeeds, you're in server
mode. If it returns a 404, you're in standalone mode with
a single session.
```

### Section 2: Session Lifecycle

```markdown
## Creating Sessions

Give each session a descriptive name that reflects its purpose:

    create session "build"
    create session "test" with command: npm test --watch
    create session "logs" with command: tail -f /var/log/app.log

You can specify:
- `name` — identifier (auto-generated if omitted)
- `command` — run a specific command instead of a shell
- `rows`, `cols` — terminal dimensions
- `cwd` — working directory
- `env` — environment variables (object of key-value pairs)

A session with a `command` will exit when that command
finishes. A session without one starts an interactive shell
that persists until you kill it.

## Listing and Inspecting

    list sessions
    get session "build"

## Ending Sessions

Prefer a graceful exit when the session is running an
interactive program:

    # Exit a shell
    send input to "build": exit\n

    # Quit a TUI
    send input to "monitor": q

The session will close automatically when its process exits.

If the process is stuck or you don't care about graceful
shutdown, force-kill it:

    kill session "build"

This terminates the process immediately. Clean up after
yourself — don't leave orphaned sessions running.

## Renaming Sessions

If a session's purpose changes:

    rename session "build" to "build-v2"
```

### Section 3: Coordination Patterns

```markdown
## Working Across Sessions

The power of multi-session is parallelism. Here are the
common coordination patterns.

### Fan-Out: Run in Parallel, Gather Results

Spawn several sessions, kick off work in each, then poll
them for completion:

    # Create sessions and start work
    create session "test-unit", send: npm run test:unit
    create session "test-e2e", send: npm run test:e2e
    create session "lint", send: npm run lint

    # Poll each for completion
    for each session:
        wait for quiescence
        read screen
        check for shell prompt (done) or still running

    # Gather results
    read scrollback from each session
    report combined results

This is the most common pattern. The key insight: you don't
have to wait for one to finish before checking another.
Poll them round-robin:

    quiesce test-unit (short timeout, 1000ms)
    quiesce test-e2e (short timeout, 1000ms)
    quiesce lint (short timeout, 1000ms)
    # repeat until all show shell prompts

### Watcher: Long-Running Process + Working Session

One session runs something persistent (a dev server, log
tail, file watcher). Other sessions do active work.
Periodically check the watcher for relevant output:

    create session "server", send: npm run dev
    create session "work"

    # Do work in the work session
    send to "work": curl localhost:3000/api/health

    # Check server session for errors if something fails
    read screen from "server"

### Pipeline: Sequential Handoff

One session's output informs the next session's input.
This isn't true parallelism — it's staged work:

    create session "build"
    send to "build": cargo build 2>&1 | tee /tmp/build.log
    wait for build to finish

    create session "deploy"
    send to "deploy": ./deploy.sh
    # only if build succeeded
```

### Section 4: Pitfalls and Discipline

```markdown
## Pitfalls

### Session Sprawl
It's easy to create sessions and forget about them. Every
session is a running process consuming resources. Adopt a
discipline:
- Create sessions with a clear purpose
- Destroy or exit sessions as soon as their purpose is served
- Before creating new sessions, list existing ones to see
  if you can reuse one
- If you're doing a fan-out, clean up all sessions when
  the fan-out is complete

### Naming Discipline
Names are how you keep track of what's what. Use descriptive,
consistent names:

    Good: "test-unit", "test-e2e", "build-frontend"
    Bad:  "session1", "s2", "tmp"

If you're creating sessions in a loop, use a predictable
naming scheme so you can iterate over them later:

    test-0, test-1, test-2
    build-api, build-web, build-docs

### Don't Multiplex What Doesn't Need It
If you just need to run three commands in sequence, one
session with `&&` is simpler than three sessions. Multi-session
adds overhead — session creation, polling, cleanup. Only use
it when you genuinely need parallelism or isolation.

### Session Exit Detection
A session running a specific command (not a shell) will exit
when that command finishes. The session disappears from the
sessions list. If you're polling and a session vanishes, the
process finished — read its output before it's gone, or
redirect output to a file you can read from another session.

### Context Isolation Cuts Both Ways
Each session is independent — different working directory,
different environment, different shell history. If you `cd`
in one session, the others are unaffected. This is useful
for isolation but means you can't share state between
sessions through shell variables. Use files, environment
variables at creation time, or the filesystem as shared state.
```

---

## Skill 4: wsh:agent-orchestration

Teaches the AI how to drive other AI agents through their terminal interfaces.

### Section 1: AI Driving AI

```markdown
# wsh:agent-orchestration — Driving AI Agents

You can use wsh to launch and drive other AI agents — Claude
Code, Aider, Codex, or any AI tool with a terminal interface.
This is not science fiction. You spawn the agent in a wsh
session, feed it tasks, handle its approval prompts, and
review its output. You become a manager of AI workers.

## Why?

- **Parallelism.** You can run 5 Claude Code sessions
  simultaneously, each working on a different task.
- **Delegation.** Break a large project into subtasks and
  assign each to an agent session.
- **Specialization.** Different agents have different
  strengths. Orchestrate the right tool for each job.
- **Automation.** Unattended workflows — an agent that
  spawns agents, reviews their work, and merges the results.

## Launching an Agent

Create a session with the agent as its command:

    create session "agent-auth" with command: claude --print "Implement user auth in src/auth.rs"

Or start a shell and launch the agent interactively:

    create session "agent-auth"
    send: claude
    wait for quiescence
    read screen — verify Claude Code has started
    send: Implement user authentication in src/auth.rs\n

The interactive approach gives you more control — you can
set up the environment first, then launch the agent.
```

### Section 2: Understanding Agent Interaction Patterns

```markdown
## Agent Interaction Patterns

AI agents aren't like regular CLI programs. They produce
long bursts of output, pause to think, ask for approval,
and sometimes wait indefinitely for human input. You need
to recognize these states.

### The Agent Lifecycle
Most AI terminal agents follow this pattern:

    1. Startup — banner, loading, initialization
    2. Thinking — reading files, planning (may be quiet
       for seconds or minutes)
    3. Working — producing output, writing code, running
       commands
    4. Approval — asking permission before a tool use or
       destructive action
    5. Waiting — idle, expecting the next task
    6. Repeat from 2

### Detecting Agent State

**Startup:** Look for the agent's banner or welcome message.
Each agent has a recognizable one. Wait for it before
sending input.

**Thinking:** The terminal may be silent for extended
periods. This is normal — don't assume it's stuck. Use
longer quiescence timeouts (10-30 seconds) and poll
patiently. Look for spinner characters or progress
indicators.

**Approval prompts:** These are critical. Look for patterns
like:
- "Allow?" "Approve?" "Proceed?" "Y/n"
- "Do you want to run this command?"
- A tool call description followed by a prompt
- Highlighted or bold text asking for confirmation

**Waiting for input:** After completing a task, the agent
shows a prompt for the next instruction. Look for an
input indicator — a cursor, a `>`, or an explicit
"What would you like to do?" message.

### Quiescence Is Tricky with Agents
Agents think. Thinking produces no output. A quiescence
timeout of 2 seconds will trigger constantly during
thinking phases. Use longer timeouts (10-30 seconds) and
always read the screen to distinguish "thinking" from
"waiting for input."
```

### Section 3: Feeding Tasks and Handling Approvals

```markdown
## Feeding Tasks

Keep instructions clear and self-contained. The agent can't
ask you clarifying questions — or rather, it can, but you
need to detect and answer them programmatically.

### Good Task Instructions
Be specific. Include context the agent needs:

    send: Add input validation to the POST /users endpoint. \
    Reject requests where email is missing or malformed. \
    Return 400 with a JSON error body.\n

Not:

    send: fix the users endpoint\n

### Scoping Tasks
Prefer small, well-defined tasks over large ambiguous ones.
An agent working on "implement the entire auth system" will
make many decisions you might disagree with. An agent working
on "add bcrypt password hashing to the User model" has less
room to go sideways.

## Handling Approvals

Many agents ask for permission before taking actions. You
have three strategies:

### Auto-Approve Everything
If you trust the agent and the task is low-risk, configure
it to skip approvals:

    send: claude --dangerously-skip-permissions\n

Only do this for isolated, low-stakes work. Never for
production systems.

### Selective Approval
Read the approval prompt, decide whether to approve:

    read screen
    # See: "Run command: npm install express?"
    # Looks safe.
    send: y\n

    read screen
    # See: "Run command: rm -rf /tmp/build"
    # Inspect further before approving.

### Reject and Redirect
If the agent proposes something wrong, reject and explain:

    send: n\n
    wait, read
    send: Don't delete that directory. Use a fresh \
    build directory at /tmp/build-new instead.\n

## Reviewing Output

After the agent finishes a task, review what it did:

    read screen — see the final status
    read scrollback — see the full conversation

Look for: files changed, commands run, errors encountered,
tests passed or failed. The scrollback is your audit trail.
```

### Section 4: Multi-Agent Coordination

```markdown
## Multi-Agent Coordination

The real power is running multiple agents in parallel. Each
gets its own session, its own task, its own workspace.

### The Delegation Pattern

    1. Plan — break the project into independent subtasks
    2. Spawn — create a session for each subtask
    3. Launch — start an agent in each session with its task
    4. Monitor — poll sessions, handle approvals
    5. Review — read each agent's output when it finishes
    6. Integrate — merge the results

### Monitoring Multiple Agents

Poll round-robin with short quiescence timeouts. You're
looking for approval prompts that need your attention:

    for each agent session:
        quiesce (timeout_ms=3000)
        read screen
        if approval prompt detected:
            evaluate and respond
        if task complete:
            review output, clean up session
        if still working:
            move to next session

Agents that are thinking or working need no intervention.
Focus your attention on agents that are blocked waiting
for approval or input.

### Workspace Isolation

Give each agent its own workspace to avoid conflicts.
Agents editing the same files simultaneously will corrupt
each other's work:

    # Use git worktrees for code tasks
    send to "setup": git worktree add /tmp/agent-auth feature/auth
    send to "setup": git worktree add /tmp/agent-api feature/api

    create session "agent-auth", cwd: /tmp/agent-auth
    create session "agent-api", cwd: /tmp/agent-api

Or use separate branches, separate directories, or
separate repositories. The key principle: agents that
might touch the same files must not run in parallel
without isolation.

### Passing Results Between Agents

Agents can't talk to each other directly. Use the
filesystem as the communication channel:

    # Agent 1 produces an artifact
    agent-1 writes to /tmp/api-spec.json

    # You read it and feed it to Agent 2
    read /tmp/api-spec.json
    send to agent-2: Implement the client based on the
    API spec at /tmp/api-spec.json\n
```

### Section 5: Pitfalls

```markdown
## Pitfalls

### Don't Over-Parallelize
More agents isn't always better. Each agent consumes
resources — API rate limits, CPU, memory. And each agent
you run is one more you need to monitor. Start with 2-3
agents and scale up once you're comfortable with the
monitoring rhythm.

### Watch for Agents Talking to Each Other Accidentally
If two agents are running in the same git repo without
workspace isolation, one agent's changes (file writes,
branch switches, dependency installs) will affect the
other. This causes bizarre failures that are hard to
diagnose. Always isolate.

### Infinite Loops
An agent can get stuck — retrying a failing command,
asking itself a question, or endlessly editing a file.
If you notice an agent producing repetitive output
without progress, intervene:

    send: Stop. The current approach isn't working. \
    Try a different strategy.\n

If it persists, Ctrl+C and give it a fresh start with
clearer instructions.

### Don't Forget Cleanup
Agent sessions with worktrees, temp files, and running
processes leave debris. When the orchestration is done:
- Exit or kill all agent sessions
- Remove worktrees: `git worktree remove /tmp/agent-auth`
- Clean up temp files
- Verify the main repo is in a clean state

### Know When Not to Orchestrate
If the task requires deep understanding of a complex
system, one focused agent with good context will
outperform three agents with shallow context.
Orchestration works best when tasks are genuinely
independent and well-specified. If tasks are tightly
coupled, work them sequentially in a single session.
```

---

## Skill 5: wsh:monitor

Teaches the AI how to watch and react to human terminal activity.

### Section 1: The Observer Posture

```markdown
# wsh:monitor — Watching and Reacting

In this mode, you're not driving the terminal — the human is.
You're watching what happens and providing value by reacting:
flagging errors, offering help, catching mistakes, maintaining
context. You're a copilot, not the pilot.

## Two Approaches

### Polling (Simple)
Periodically read the screen and react to what you see.
Good enough for most use cases:

    read screen
    analyze what changed
    respond if needed (overlay, panel, conversation)
    wait
    repeat

Polling is simple and straightforward. The downside is latency —
you're checking on an interval, so you might miss transient
output or react a few seconds late.

### Event Subscription (Real-Time)
Subscribe to real-time events via the WebSocket (see the
core skill for connection mechanics). Subscribe to the
events you care about — `lines` for output, `input` for
keystrokes — and the server pushes them as they happen.

You also get periodic `sync` snapshots when the terminal
goes quiet, giving you a natural checkpoint to analyze
the current state.

For most monitoring tasks, **start with polling**. Move to
event subscription when you need immediate reaction time.
```

### Section 2: What to Watch For

```markdown
## Pattern Detection

Monitoring is only useful if you know what to look for.
Here are the categories of patterns worth detecting.

### Errors and Failures
Read the screen and scan for:
- Compiler errors — "error[E", "SyntaxError", "TypeError"
- Command failures — "command not found", "No such file"
- Permission issues — "Permission denied", "EACCES"
- Network failures — "connection refused", "timeout"
- Stack traces — indented lines starting with "at" or "in"

When detected: show a panel or overlay with a brief
explanation and suggested fix. Don't interrupt the human's
flow — they may have already noticed.

### Dangerous Commands
Watch input events for risky patterns:
- `rm -rf` with broad paths
- `git push --force` to main/master
- `DROP TABLE`, `DELETE FROM` without WHERE
- `chmod 777`
- Credentials or tokens being pasted into commands

When detected: use input capture to intercept before
execution. Show an overlay asking for confirmation.
Release input if approved, discard if rejected.

### Opportunities to Help
Not everything is about preventing mistakes. Watch for
moments where help would be welcome:
- A command was run three times with slightly different
  flags — the human might be guessing
- A long error message just scrolled by — summarize it
- The human typed a command that has a better alternative
- A build succeeded after repeated failures — celebrate

### State Tracking
Maintain a mental model of what the human is doing:
- What directory are they in?
- What project are they working on?
- What was the last command they ran?
- Are they in a flow state or exploring?

This context makes your reactions more relevant. An `rm`
in a temp directory is different from an `rm` in the
project root.
```

### Section 3: Responding Appropriately

```markdown
## How to Respond

The hardest part of monitoring isn't detection — it's
calibrating your response. Too noisy and the human ignores
you. Too quiet and you're useless.

### Response Channels

**Overlays** — lightweight, transient. Best for:
- Brief warnings ("this will delete 47 files")
- Quick tips ("try --dry-run first")
- Acknowledgments ("build passed")

Position them near the relevant content. Remove them
after a few seconds or when the screen changes.

**Panels** — persistent, always visible. Best for:
- Running context summaries ("working in: /project,
  branch: feature/auth, last command: cargo test")
- Session dashboards during long workflows
- Error explanations that need to stay visible while
  the human fixes the issue

Keep panels compact. One or two lines. Update in place
rather than creating new ones.

**Input capture** — disruptive, use sparingly. Best for:
- Blocking genuinely dangerous commands
- Approval gates where the human explicitly asked for
  your oversight

Never capture input for something the human can easily
undo. Reserve it for irreversible actions.

**Conversation** — the chat with the human. Best for:
- Detailed explanations that don't fit in an overlay
- Suggestions that need discussion
- Questions that require a thoughtful answer

### Visual Structure

wsh renders spans as-is — no built-in borders, padding,
or separators. Build visual structure from text characters:

- **Borders:** use box-drawing characters (`┌─┐│└─┘`)
  for framed overlays and panels
- **Padding:** add spaces for breathing room
- **Separators:** use `│` between inline elements
- **Full-width rules:** use `━` repeated to `cols` width
  to separate panels from terminal content

See the wsh:visual-feedback skill for detailed guidance
on constructing visual elements.

### Calibration Principles

**Be quiet by default.** Only react when you have
something genuinely useful to say. The human chose to
work in a terminal — they know what they're doing most
of the time.

**Severity drives channel.** Informational → overlay.
Important → panel. Critical → input capture. Complex →
conversation.

**Don't repeat yourself.** If you flagged an error and
the human re-runs the same command, they saw your
warning and chose to proceed. Don't flag it again.

**Dissolve gracefully.** Remove overlays when they're
stale. Update panels rather than accumulating them.
Leave no visual debris.
```

### Section 4: Recipes

```markdown
## Monitoring Recipes

### Contextual Help
Watch what command the human is typing or has just started.
Detect the program and provide relevant, timely guidance:

    read screen
    # See: "$ parted /dev/sda"
    # The human is partitioning a disk.

    create overlay near the bottom of screen:
      "┌─ Parted Quick Ref ────────────────┐"
      "│ Common schemes:                   │"
      "│  GPT + EFI: mkpart ESP fat32      │"
      "│    1MiB 513MiB                    │"
      "│  Root: mkpart primary ext4        │"
      "│    513MiB 100%                    │"
      "│ Type 'help' for all commands      │"
      "└───────────────────────────────────┘"

This works for any tool. Detect the command, surface the
most useful information:
- `git rebase` → show the rebase commands (pick, squash,
  fixup, drop) and common flags
- `docker build` → show relevant Dockerfile tips
- `kubectl` → show the resource types and common flags
- `ffmpeg` → show common codec and format options
- `iptables` → show chain names and common patterns

**Timing matters.** Show help when the human starts
the command, not after they've already finished. Update
or remove the overlay when they move on to something else.

**Be concise.** A help overlay is a cheat sheet, not a
man page. Three to five lines of the most useful
information. If the human needs more, they'll ask.

### Error Summarizer
Long error messages scroll by and are hard to parse.
Detect them and provide a summary:

    read screen or scrollback
    # See: 47 lines of Rust compiler errors

    create panel at bottom:
      "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
      " 3 errors: missing lifetime in     "
      " auth.rs:42, type mismatch in      "
      " db.rs:108, unused import main.rs:3"

### Security Watchdog
Monitor for sensitive data in terminal output:
- API keys, tokens, passwords echoed to screen
- AWS credentials in environment variables
- Private keys displayed via cat

When detected, overlay a warning. The data is already
on screen — you can't un-show it — but you can alert the
human to rotate the credential.

### Session Journaling
Maintain a running summary panel of what's happened:

    panel at top, 2 lines:
      "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
      " Session: 14 cmds │ 2 errs │ 38 min"
      " Last: cargo test (PASS) in /wsh   "

Update after each command completes. This gives the human
(and you) a persistent sense of where things stand.
```

### Section 5: Pitfalls

```markdown
## Pitfalls

### Don't Be a Backseat Driver
The human is in control. If they run a command you'd do
differently, that's their choice. Only intervene when
something is genuinely dangerous or when they appear stuck.
"You should use --verbose" is annoying. "That rm will
delete your git repo" is helpful.

### Don't Obscure the Terminal
Overlays and panels consume screen space. On a small
terminal, a 3-line panel and two overlays can cover a
significant portion of the visible content. Be aware of
the terminal dimensions (available in the screen response)
and scale your visual elements accordingly. On a 24-row
terminal, a 1-line panel is plenty.

### Polling Frequency
If you're polling, don't hammer the API. Every request
costs a round-trip. Reasonable intervals:
- Active monitoring (security): every 1-2 seconds
- Contextual help: every 2-3 seconds
- Session journaling: every 5-10 seconds

Match the frequency to the urgency. Most monitoring
doesn't need sub-second reaction time.

### Don't Monitor What Wasn't Asked For
If the human asked you to watch for errors, don't also
start providing unsolicited style tips. Scope your
monitoring to what was requested. You can suggest
expanding scope, but don't do it silently.

### Privacy
The human may type passwords, access personal accounts,
or work on confidential material. If you're monitoring,
you see everything. Don't log, repeat, or comment on
anything that looks private unless it's directly relevant
to the monitoring task you were asked to perform.

### Know When to Stop
Monitoring is not a permanent state. When the human is
done with the task that warranted monitoring, tear down
your panels and overlays and stop polling. Ask if you
should continue rather than assuming.
```

---

## Skill 6: wsh:visual-feedback

Teaches the AI how to communicate with humans through overlays and panels.

### Section 1: Overlays vs Panels

```markdown
# wsh:visual-feedback — Communicating Visually

You can place text on the terminal screen to communicate
with the human. You have two tools: overlays and panels.
Choosing the right one matters.

## Overlays

Floating text positioned at specific screen coordinates.
They sit on top of terminal content without affecting it.

**Characteristics:**
- Positioned at (x, y) — you choose exactly where
- Can overlap terminal content and each other
- Don't affect the PTY or its layout
- Best for transient, contextual information

**Use for:** tooltips, warnings, quick tips, annotations,
notifications, contextual help near relevant output.

## Panels

Dedicated screen regions at the top or bottom edge. They
**shrink the PTY** — the terminal gets fewer rows, and
programs adapt to the reduced space.

**Characteristics:**
- Fixed to top or bottom
- Have a height in rows
- Carve out permanent space
- Best for persistent information

**Use for:** status bars, progress displays, context
summaries, dashboards, error explanations.

## Choosing Between Them

Ask two questions:

1. **Is it tied to a screen position?** If the information
   relates to a specific line or region of output, use an
   overlay positioned near it. If it's ambient context,
   use a panel.

2. **How long should it live?** If it's relevant for a few
   seconds or until the screen changes, use an overlay.
   If it should persist across screen updates, use a panel.
```

### Section 2: Designing Overlays

```markdown
## Designing Effective Overlays

### Positioning
Screen coordinates are (x, y) where (0, 0) is the top-left
corner. x is the column, y is the row.

Place overlays near the content they relate to:
- Error annotation → same row as the error, offset to the
  right so it doesn't obscure the error text
- Command hint → just below or above the command line
- Notification → top-right corner, out of the way

Read the screen dimensions from the screen response
(`cols`, `rows`) to avoid placing overlays off-screen.

### Sizing
Keep overlay text short. One to three lines, under half
the screen width. If you need more space, use a panel or
the conversation instead.

### Styling
Spans support formatting attributes. Use them for
visual hierarchy:

    {"spans": [
      {"text": "Warning: ", "bold": true, "fg": {"indexed": 3}},
      {"text": "this deletes 47 files"}
    ]}

- **Bold** for labels, keywords, emphasis
- **Color** for severity (red = danger, yellow = warning,
  green = success, cyan = informational)
- **Faint/dim** for secondary information

Use the indexed color palette (0-7 for standard, 8-15 for
bright) for broad terminal compatibility. Use RGB colors
only when you're confident the terminal supports them.

### Z-Order
When overlays overlap, higher `z` values render on top.
Default is 0. Use z-order to layer related elements:

    background context:  z=0
    primary information:  z=1
    urgent alert:         z=2

### Lifecycle
Overlays don't disappear on their own. You must manage them:
- Store the ID returned on creation
- Update content when information changes
- Delete individual overlays when no longer relevant
- Clear all overlays when cleaning up entirely
```

### Section 3: Designing Panels and Visual Structure

```markdown
## Designing Effective Panels

### Layout
Panels are rows of styled text at the top or bottom of the
terminal. A 2-line panel at the bottom makes a natural
status bar. A 3-line panel at the top works for context
summaries.

    create panel (bottom, height: 2):
      spans: [
        {text: " Status: ", bold},
        {text: "building", fg: yellow},
        {text: "  |  "},
        {text: "Branch: ", bold},
        {text: "feature/auth"}
      ]

### Updating In Place
Panels persist. When information changes, update the
existing panel rather than deleting and recreating.
Use the panel's ID (returned on creation) to update
its content in place.

This prevents flicker and keeps the panel stable.

### Height Budget
Every row of panel height is a row stolen from the
terminal. Be miserly:
- 1 line for simple status
- 2 lines for status + detail
- 3 lines maximum in most cases

On a 24-row terminal, a 3-line panel costs 12% of the
visible area. Respect the human's screen space.

## Visual Structure: Borders and Padding

wsh renders your spans as-is. There are no built-in
borders, padding, margins, or separators. If you want
visual structure, you build it from text characters.

### Borders
Use box-drawing characters for framed overlays:

    {"spans": [
      {"text": "┌─ Warning ────────────┐\n"},
      {"text": "│ This will delete the │\n"},
      {"text": "│ entire build cache.  │\n"},
      {"text": "└──────────────────────┘"}
    ]}

### Padding
Add spaces for breathing room:

    {"text": "  Status: running  "}
              ^^              ^^  padding

### Separators
Use `│` or `|` between inline elements:

    {"spans": [
      {"text": " build: "},
      {"text": "ok", "fg": {"indexed": 2}},
      {"text": " │ tests: "},
      {"text": "3 failed", "fg": {"indexed": 1}},
      {"text": " │ lint: "},
      {"text": "clean", "fg": {"indexed": 2}},
      {"text": " "}
    ]}

### Panel Separators
A panel has no visible border separating it from the
terminal content. Add your own with a full-width line:

    {"text": "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n"}

Read the terminal's `cols` value to size separators
to the full width.
```

### Section 4: Composition Patterns

```markdown
## Composition Patterns

Individual overlays and panels are building blocks.
Combining them creates richer experiences.

### Status Bar + Contextual Annotations
A persistent panel tracks overall state. Overlays
provide momentary context:

    panel (bottom, 1 line):
      " ● monitoring  │  12 commands  │  2 errors "

    overlay (near error on row 15):
      "┌─ Suggestion ──────────────────┐"
      "│ Try: cargo build --release    │"
      "└───────────────────────────────┘"

The panel stays. The overlay appears when relevant
and disappears when the screen moves on.

### Layered Overlays
Stack overlays with z-order for progressive detail.
A compact label at z=0, expanded detail at z=1
shown only when the human triggers it (via input
capture or a follow-up question):

    z=0: " ⚠ 3 warnings "
    z=1: "┌─ Warnings ──────────────────┐"
         "│ auth.rs:42  unused import   │"
         "│ db.rs:17    deprecated call │"
         "│ main.rs:3   dead code       │"
         "└────────────────────────────-┘"

### Multi-Panel Dashboard
Use top and bottom panels together. Top for context,
bottom for status:

    panel (top, 2 lines):
      "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
      " Project: wsh  Branch: master "

    panel (bottom, 1 line):
      " ● 3 sessions  │  build: ok  │  tests: running "

Be conservative. Two panels means the terminal loses
3+ rows. Only use both when the information justifies it.

### Temporary Overlays
For notifications that should appear briefly, create
the overlay and then delete it after a delay. You'll
need to manage the timing yourself:

    create overlay → get id
    (continue working)
    after a few interaction cycles, delete the overlay

There's no built-in timer. The overlay lives until you
remove it. Tie cleanup to your next interaction with
the terminal — when you next read the screen, check
if any overlays are stale and remove them.
```

### Section 5: Pitfalls

```markdown
## Pitfalls

### Visual Clutter
Every overlay and panel competes for attention. If
everything is highlighted, nothing is. Apply a strict
budget:
- At most 1-2 panels active
- At most 2-3 overlays visible simultaneously
- If you need to show a new overlay and you're at your
  limit, remove the least relevant one first

### Stale Elements
Overlays and panels don't expire. If you create an
overlay saying "build started" and never remove it,
it's still there 20 minutes later when the human has
moved on to something else entirely. Track what you've
created and clean up proactively.

Keep a mental inventory of your active visual elements.
Before creating a new one, ask: are any existing ones
stale? Remove them first.

### Overlapping Content
An overlay at (0, 5) that's 40 characters wide will
cover whatever is on row 5. If that row contains
important terminal output, you've hidden it. Strategies:
- Position overlays at the right edge of the screen,
  past where output typically ends
- Use short overlays that don't span the full width
- Prefer panels for information that shouldn't compete
  with terminal content

### Color Assumptions
Not all terminals use the same color scheme. Indexed
color 1 is "red" but the exact shade depends on the
terminal's theme. More importantly, the terminal might
have a light or dark background. Don't rely on color
alone to convey meaning — pair it with text labels
or symbols:

    Good:  {"text": "✗ FAIL", "fg": {"indexed": 1}}
    Bad:   {"text": "●", "fg": {"indexed": 1}}

The word "FAIL" communicates even without color. A
red dot is meaningless on a terminal where red is
hard to see.

### Coordinate Drift
Screen content scrolls. An overlay pinned to row 10
made sense when the error was on row 10, but after
the human runs another command, row 10 is different
content. Contextual overlays should be removed when
the screen changes significantly. Use the `epoch`
field in screen responses — if the epoch has changed
since you placed the overlay, re-evaluate whether it's
still positioned correctly.
```

---

## Skill 7: wsh:input-capture

Teaches the AI how to intercept keyboard input for dialogs and approvals.

### Section 1: How Input Capture Works

```markdown
# wsh:input-capture — Intercepting Keyboard Input

Input capture lets you temporarily take over the keyboard.
While active, keystrokes from the human go to you instead
of the shell. The terminal is frozen — nothing the human
types reaches the PTY. You decide what to do with each
keystroke.

## The Mechanism

    capture input       # grab the keyboard
    # Keystrokes now go to subscribers, not the PTY
    # Do your thing — build a menu, ask a question, etc.
    release input       # give it back

While captured, the human can always press Ctrl+\ to
force-release. This is a safety valve — never disable it,
never tell the human to avoid it. It's their escape hatch.

## Reading Captured Input

Captured keystrokes arrive via WebSocket event subscription
(see the core skill for connection mechanics). Subscribe to
`input` events. Each event includes:
- `raw` — the byte sequence
- `parsed` — structured key information (key name,
  modifiers like ctrl, alt, shift)

Use `parsed` when you want to understand what key was
pressed. Use `raw` when you need to forward the exact
bytes somewhere.

## Check the Current Mode

    get input mode → "passthrough" or "capture"

Always check before capturing. If input is already
captured (by another agent or process), don't capture
again without understanding why.
```

### Section 2: Approval Workflows

```markdown
## Approval Workflows

The most common use of input capture: ask the human a
yes-or-no question and wait for their answer.

### The Pattern

    1. Show the question (overlay or panel)
    2. Capture input
    3. Wait for a keystroke
    4. Interpret the keystroke
    5. Release input
    6. Remove the visual prompt
    7. Act on the answer

### Example: Confirm a Dangerous Command

    # Show the prompt
    create overlay:
      "┌─ Confirm ──────────────────────┐"
      "│ Delete 47 files from /build ?  │"
      "│         [Y]es    [N]o          │"
      "└────────────────────────────────┘"

    # Capture input
    capture input

    # Read keystroke via WebSocket
    receive input event
    if key == "y" or key == "Y":
        proceed with deletion
    else:
        cancel

    # Release and clean up
    release input
    delete overlay

### Always Provide a Way Out
Every prompt must accept a "no" or "cancel" keystroke.
Never build a prompt where the only option is "yes."
Show the available keys clearly in the prompt so the
human isn't guessing.
```

### Section 3: Menus and Text Input

```markdown
## Selection Menus

Let the human choose from a list of options using
arrow keys and Enter.

### The Pattern

    # Show the menu with one item highlighted
    create overlay:
      "┌─ Select environment ──────┐"
      "│   development             │"
      "│ ▸ staging                 │"
      "│   production              │"
      "└───────────────────────────┘"

    # Capture input
    capture input

    # Handle navigation
    receive input events in a loop:
        Arrow Up / k   → move highlight up
        Arrow Down / j → move highlight down
        Enter          → confirm selection
        Escape / q     → cancel

    # After each navigation keystroke, update the overlay
    # to reflect the new highlight position

    # Release and clean up
    release input
    delete overlay

Track the selected index yourself. On each arrow key,
update the index, rebuild the spans with the highlight
on the new item, and update the overlay.

## Text Input

Capture free-form text from the human — a filename,
a commit message, a search query.

### The Pattern

    create overlay:
      "┌─ Session name ────────────┐"
      "│ > _                       │"
      "└───────────────────────────┘"

    capture input

    buffer = ""
    receive input events in a loop:
        printable character → append to buffer
        Backspace          → remove last character
        Enter              → confirm
        Escape             → cancel

    # After each keystroke, update the overlay to show
    # the current buffer:
    "│ > my-session_               │"

    release input
    delete overlay

You're building a tiny text editor. Handle at least:
character input, backspace, enter to confirm, escape
to cancel. Don't try to build a full readline — keep
it simple.

## Multi-Step Dialogs

Chain prompts together for workflows that need several
pieces of information:

    Step 1: Select environment  (menu)
    Step 2: Enter version tag   (text input)
    Step 3: Confirm deployment  (yes/no)

Keep input captured across all steps. Show a progress
indicator so the human knows where they are:

    "Step 2 of 3 — Enter version tag"

If the human presses Escape at any step, cancel the
entire flow and release input. Don't trap them in a
multi-step dialog they can't exit.
```

### Section 4: Pitfalls

```markdown
## Pitfalls

### Minimize Capture Duration
Every moment input is captured, the human cannot use
their terminal. This is disruptive. Capture as late as
possible, release as early as possible:

    Bad:  capture → build UI → show prompt → wait
    Good: build UI → show prompt → capture → wait

Prepare everything before you grab the keyboard. The
human should never see a captured terminal with nothing
on screen explaining why.

### Always Show What's Happening
A captured terminal with no visual explanation is
terrifying. The human types and nothing happens. They
don't know if the terminal is frozen, crashed, or
waiting. Before or simultaneously with capturing input,
always display an overlay or panel explaining what
you're asking and what keys to press.

### Handle Unexpected Input
The human may press keys you didn't anticipate. Don't
crash or behave erratically. Ignore keys you don't
handle:

    if key in expected_keys:
        handle it
    else:
        ignore, do nothing

Don't beep, flash, or scold. Just do nothing for
unrecognized keys.

### Don't Nest Captures
Input is either captured or it isn't — there's no
nesting. If you capture while already captured, you're
still in the same capture session. Design your flows
to be flat: capture once, do your multi-step dialog,
release once.

### Remember Ctrl+\
The human can force-release at any time with Ctrl+\.
Your code must handle this gracefully. If you're
mid-dialog and input is suddenly released:
- Your WebSocket will stop receiving input events
- Your overlay is still showing a stale prompt
- Clean up: remove the overlay, abandon the flow
- Don't re-capture without the human's consent

Check the input mode if you're unsure whether you
still have capture.

### Don't Capture for Information You Could Ask Differently
Input capture is the right tool for real-time keystroke
interaction — menus, approvals, text input that needs
character-by-character handling. If you just need an
answer to a question and latency doesn't matter,
consider using the conversation instead. It's less
disruptive and gives the human more room to think.
```

---

## Skill 8: wsh:generative-ui

Teaches the AI how to build dynamic, interactive terminal experiences on the fly.

### Section 1: The Terminal as Canvas

```markdown
# wsh:generative-ui — Building Dynamic Experiences

The terminal isn't just a place to run existing programs.
With wsh, you can create interactive experiences on the
fly — interfaces that didn't exist before this moment,
tailored to this specific task, this specific user,
this specific context.

## What You Have to Work With

Three layers of capability, from simple to complex:

**Layer 1: Overlays and Panels**
The wsh primitives. Position styled text anywhere on
screen, carve out panel regions, capture input. No
external programs needed. Good for: annotations,
status displays, menus, prompts, simple dashboards.

**Layer 2: Composed Experiences**
Combine overlays + panels + input capture into
cohesive interactive applications. A selection menu
with a preview panel. A dashboard with live-updating
sections. A wizard that walks through multiple steps.
Still no external programs — just wsh primitives
orchestrated together.

**Layer 3: Generated Programs**
Write and run a small program that produces terminal
output. A Python script that renders a table. A bash
script that draws a chart. A small TUI application
built with a framework. The generated program runs
in the terminal; you read its output and interact
with it through wsh.

## Choosing a Layer

Start with the simplest layer that works:
- Need to show some text? Layer 1.
- Need interaction with multiple elements? Layer 2.
- Need complex rendering, live data, or rich layout
  that's awkward to build from spans? Layer 3.

Most generative UI lives in layers 1 and 2. Layer 3
is for when you need something the primitives can't
do well.
```

### Section 2: Design Patterns

```markdown
## Design Patterns

### Live Dashboard
Combine panels with periodic updates to create a
real-time display. Each update replaces the panel
content:

    panel (top, 3 lines), update every few seconds:
      "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
      " CPU: ████░░░░ 52%  │  MEM: 3.2/8G "
      " Pods: 12 ready     │  Errs: 0     "

Read the data source (run a command, read a file,
call an API), format it into spans, update the
panel. The human sees a live-updating display without
any dedicated monitoring tool installed.

### Interactive Browser
Let the human explore structured data — files,
commits, log entries, API responses. Combine an
overlay list with input capture:

    overlay:
      "┌─ Recent Commits ──────────────────┐"
      "│ ▸ 81883ad docs: rewrite README    │"
      "│   8acf8d7 feat: clear screen on   │"
      "│   55719e8 feat: add session detach │"
      "│                                   │"
      "│   ↑↓ navigate  Enter: view diff   │"
      "│   q: close                        │"
      "└───────────────────────────────────┘"

On Enter, fetch the diff for the selected commit,
replace the overlay content with the diff view,
and add a "Back" option. You're building a mini
application from overlays and keystrokes.

### Wizard
Walk the human through a multi-step process,
building up configuration or input along the way.
Use a panel for progress and an overlay for the
current step:

    panel (bottom, 1 line):
      " Step 2/4: Configure database ──────"

    overlay (center):
      "┌─ Database Type ───────────────┐"
      "│   SQLite                      │"
      "│ ▸ PostgreSQL                  │"
      "│   MySQL                       │"
      "└───────────────────────────────┘"

Each step collects input, updates the progress
panel, and advances. At the end, summarize all
choices and ask for confirmation before acting.

### Contextual Preview
Show a preview that updates based on what the
human is doing. Combine monitoring with visual
feedback:

    # Human is editing a Markdown file
    # Panel shows a simplified rendered preview

    panel (bottom, 5 lines):
      "━━ Preview ━━━━━━━━━━━━━━━━━━━━━━"
      " # My Document                   "
      " This is the **first** paragraph "
      " - item one                      "
      " - item two                      "

Read the file periodically, render a simplified
version, update the panel. The human gets live
feedback without switching tools.
```

### Section 3: Generated Programs

```markdown
## When to Generate a Program

Sometimes overlays and panels aren't enough. You need
scrolling, complex layout, rich interactivity, or live
data streams. In these cases, write a small program,
run it in the terminal, and interact with it via wsh.

### Good Candidates for Generation
- **Data tables** with sorting and filtering — hard to
  build from overlays, trivial with a script
- **Charts and graphs** — ASCII bar charts, sparklines,
  histograms
- **Log viewers** with search and highlighting
- **File browsers** with directory traversal
- **Forms** with multiple field types (text, checkbox,
  dropdown)

### Keep It Simple
Generated programs should be disposable — small,
single-purpose, written in seconds. Don't build
a framework. Write the minimum that solves the
immediate need.

Good choices for generation:
- **Bash + standard tools** — printf, column, tput.
  Available everywhere, no dependencies.
- **Python** — rich standard library, good string
  formatting, widely installed.
- **Tools like `gum`, `fzf`, `dialog`** — if installed,
  these provide polished interactive elements with
  minimal code.

### Example: Quick Data Table

Write a small script, run it, read the result:

    # Generate and run
    write /tmp/report.py:
        import json, sys
        data = json.load(open("/tmp/metrics.json"))
        print(f"{'Service':<20} {'Status':<10} {'Latency':>8}")
        print("─" * 40)
        for s in data:
            print(f"{s['name']:<20} {s['status']:<10} {s['ms']:>6}ms")

    send: python3 /tmp/report.py\n
    wait, read screen

### Example: Interactive Selection with fzf

If `fzf` is available, use it instead of building
your own menu:

    send: ls src/**/*.rs | fzf --preview 'head -20 {}'\n
    # fzf enters alternate screen
    # interact via wsh:tui patterns
    # result is printed to stdout after selection

### Clean Up After Yourself
Delete generated scripts when done. Don't leave
/tmp littered with one-off programs:

    delete /tmp/report.py
```

### Section 4: Composition Philosophy and Pitfalls

```markdown
## Composition Philosophy

The best generative UI combines layers fluidly. A
dashboard panel at the bottom. An overlay menu when
the human needs to make a choice. A generated script
when you need rich output. Each layer serves its
purpose, then gets out of the way.

### Design Principles

**Fit the terminal aesthetic.** The human chose to work
in a terminal. Respect that. Use box-drawing characters,
monospace alignment, and ASCII art — not emoji-heavy
decoration. Clean, functional, information-dense.

**Build incrementally.** Start with the simplest version
that's useful. A one-line panel is better than nothing.
Add complexity only when the human needs it or asks
for it.

**Make it disposable.** Every UI element you create
should be easy to dismiss and leave no trace. The
human should never have to clean up after your
interface. When they're done, everything disappears.

**Adapt to the terminal size.** Read `cols` and `rows`
from the screen response. A dashboard designed for
120 columns is useless on a 80-column terminal. Scale
your layouts:
- < 80 cols: minimal, single-column, short labels
- 80-120 cols: standard layout
- > 120 cols: multi-column, more detail

**Respond to context.** A generative UI should feel
relevant to what's happening right now. A deployment
dashboard during deployment. A test results browser
after a test run. A log viewer when errors appear.
Build it when it's needed, tear it down when it's not.

## Pitfalls

### Don't Build What Already Exists
Before generating a custom file browser, check if
`ranger` or `lf` is installed. Before building a git
log viewer, try `lazygit` or `tig`. Generated UI
is for gaps — when no existing tool fits the need.
Use wsh:tui to drive existing tools when they're
available.

### Don't Over-Engineer
A generated script that took 30 seconds to write
and solves the problem is better than an elegant
TUI application that takes 10 minutes. The human
is waiting. Bias toward quick and functional.

### Don't Mix Layers Carelessly
If you have a generated program running in alternate
screen AND overlays displayed, the overlays will
sit on top of the TUI. This might be useful
(annotations on a running program) or confusing
(two layers of UI competing for attention). Be
deliberate about what's visible when.

### Test on Small Terminals
If you don't know the terminal size, design for 80x24
— the classic default. Everything you build should
be usable at this size, even if it looks better on
a larger terminal.
```

---

## Implementation Plan

### File Structure

Skills will live in the wsh repository under `skills/`:

```
skills/
├── wsh/
│   ├── core/
│   │   └── SKILL.md
│   ├── drive-process/
│   │   └── SKILL.md
│   ├── tui/
│   │   └── SKILL.md
│   ├── multi-session/
│   │   └── SKILL.md
│   ├── agent-orchestration/
│   │   └── SKILL.md
│   ├── monitor/
│   │   └── SKILL.md
│   ├── visual-feedback/
│   │   └── SKILL.md
│   ├── input-capture/
│   │   └── SKILL.md
│   └── generative-ui/
│       └── SKILL.md
```

### SKILL.md Frontmatter

Each skill file needs appropriate frontmatter:

- **`wsh:core`**: `user-invocable: false` — Claude-only, loaded as background knowledge. Should detect wsh availability.
- **Specialized skills**: Standard invocation. Clear `description` fields with trigger phrases so Claude discovers them automatically.

### Implementation Steps

1. Create the directory structure under `skills/`
2. Write `wsh:core/SKILL.md` with full frontmatter and all 5 sections
3. Write each specialized skill's `SKILL.md` with frontmatter and sections
4. Add cross-reference from `wsh:monitor` to `wsh:visual-feedback` for visual structure guidance
5. Test skill loading and invocation in Claude Code
