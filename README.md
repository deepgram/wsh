# wsh: An API for Your Terminal

An API that lets AI agents *interact* with terminal programs -- not just run commands, but use them the way a human does. Send keystrokes, read the screen, wait for output, react to prompts. The terminal is the fundamental interface of the modern computer. `wsh` makes it programmable.

`wsh` sits transparently between your terminal emulator and your shell, capturing all I/O, maintaining structured terminal state, and exposing everything via HTTP/WebSocket API. Your terminal works exactly as before -- but now AI agents, automation tools, and other clients can tap into the same session.

See [docs/VISION.md](docs/VISION.md) for the full project vision.

## What This Enables

- **Drive interactive tools**: Agents can operate installers, debuggers, REPLs, TUIs, and AI coding assistants -- anything that expects a human at the keyboard
- **Orchestrate AI in parallel**: Run multiple Claude Code instances (or any terminal-based AI tool) across separate sessions, coordinating a fleet of AI workers
- **Provide live assistance**: Watch a human's terminal session and offer contextual help, rendered as overlays directly in their terminal
- **Audit and monitor**: Observe terminal activity for security, compliance, or operational awareness
- **Automate end-to-end**: Set up entire environments, handling every interactive prompt and error along the way

## Quick Start

### Getting Started

```bash
# Start wsh (auto-spawns a server daemon if one isn't running)
wsh

# In another terminal, list sessions
wsh list

# Start a second session
wsh --name dev

# Attach to an existing session
wsh attach dev

# Kill a session
wsh kill dev
```

Running `wsh` automatically starts a background server daemon (if one isn't already running) and creates a new session. Your terminal enters raw mode, and keyboard input and terminal output pass through transparently. Detach with `Ctrl+\` `Ctrl+\` (double-tap). The server exits automatically when the last session ends.

### Server Mode

For persistent operation (e.g., hosting sessions for AI agents):

```bash
# Start the server daemon (persistent by default)
wsh server

# Create a session via the API
curl -X POST http://localhost:8080/sessions \
  -H 'Content-Type: application/json' \
  -d '{"name": "dev"}'

# Attach to it from another terminal
wsh attach dev

# List active sessions
wsh list

# Kill a session
wsh kill dev
```

The server exposes an HTTP/WS API on `127.0.0.1:8080` and a Unix domain socket for client commands (`list`, `kill`, `attach`, `detach`). Use `--ephemeral` to have the server exit when its last session ends. Use `wsh persist` to upgrade a running ephemeral server to persistent mode.

## The Agent Loop

AI agents interact with `wsh` sessions using a simple, universal pattern:

```
Send input  →  Wait for quiescence  →  Read screen  →  Decide  →  repeat
```

```bash
# Send a command
curl -X POST http://localhost:8080/sessions/default/input -d 'ls\n'

# Wait for terminal to be idle
curl -s 'http://localhost:8080/sessions/default/quiesce?timeout_ms=500&max_wait_ms=10000'

# Read what's on screen
curl -s http://localhost:8080/sessions/default/screen | jq .

# Decide what to do next, then repeat
```

This loop works for any program, any interface, any situation. The agent reads what's on screen and types what's needed -- exactly like a human.

## AI Skills (Claude Code Plugin)

`wsh` ships with **skills** -- structured knowledge documents that teach AI agents how to use the terminal API effectively. When installed as a Claude Code plugin, skills are loaded automatically based on context.

Skills encode operational expertise: the send/wait/read pattern, how to detect errors, how to navigate TUIs, how to manage parallel sessions. They turn raw API access into competent terminal operation.

| Skill | What It Teaches |
|-------|-----------------|
| `wsh:core` | API mechanics and the fundamental send/wait/read/decide loop |
| `wsh:drive-process` | Running CLI commands, handling prompts, detecting errors |
| `wsh:tui` | Operating full-screen apps (vim, htop, lazygit, k9s) |
| `wsh:multi-session` | Creating and managing parallel sessions for concurrent work |
| `wsh:agent-orchestration` | Driving other AI agents through their terminal interfaces |
| `wsh:monitor` | Watching human terminal activity and reacting to events |
| `wsh:visual-feedback` | Using overlays and panels to communicate with users |
| `wsh:input-capture` | Intercepting keyboard input for dialogs and approvals |
| `wsh:generative-ui` | Building dynamic, interactive terminal experiences |

### Installing as a Claude Code Plugin

**From a local checkout (development):**

```bash
claude --plugin-dir /path/to/wsh
```

**Or install persistently:**

```bash
# From a local directory
claude /plugin install /path/to/wsh

# From a git repository
claude /plugin install https://github.com/deepgram/wsh
```

Once installed, the skills are available automatically. Claude Code will load the core skill as background knowledge and invoke specialized skills based on the task at hand.

## CLI Reference

### Top-Level Flags

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--bind` | | `127.0.0.1:8080` | Address to bind the API server |
| `--token` | `WSH_TOKEN` | (auto-generated) | Authentication token |
| `--shell` | | `$SHELL` or `/bin/sh` | Shell to spawn |
| `-c` | | | Command string to execute (like `sh -c`) |
| `-i` | | | Force interactive mode |
| `--name` | | `default` | Name for the session |
| `--alt-screen` | | | Use alternate screen buffer |

### Subcommands

| Subcommand | Description |
|------------|-------------|
| `server` | Start the headless daemon (HTTP/WS + Unix socket) |
| `attach <name>` | Attach to an existing session on the server |
| `list` | List active sessions |
| `kill <name>` | Kill (destroy) a session |
| `detach <name>` | Detach all clients from a session (session stays alive) |
| `persist` | Upgrade a running server to persistent mode |

#### `server` Flags

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--bind` | | `127.0.0.1:8080` | Address to bind the API server |
| `--token` | `WSH_TOKEN` | (auto-generated) | Authentication token |
| `--socket` | | `$XDG_RUNTIME_DIR/wsh.sock` | Path to the Unix domain socket |

#### `attach` Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--scrollback` | `all` | Scrollback to replay: `all`, `none`, or a number of lines |
| `--socket` | (default path) | Path to the Unix domain socket |
| `--alt-screen` | | Use alternate screen buffer |

#### `list`, `kill`, `detach` Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--socket` | (default path) | Path to the Unix domain socket |

#### `persist` Flags

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--bind` | | `127.0.0.1:8080` | Address of the HTTP/WS API server |
| `--token` | `WSH_TOKEN` | | Authentication token |

## API Overview

All session-specific endpoints are nested under `/sessions/:name/`. When running `wsh` with no subcommand, the default session name is `default`.

### Session Management

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/sessions` | List all sessions |
| `POST` | `/sessions` | Create a new session |
| `GET` | `/sessions/:name` | Get session info |
| `PATCH` | `/sessions/:name` | Rename a session |
| `DELETE` | `/sessions/:name` | Kill (destroy) a session |
| `POST` | `/sessions/:name/detach` | Detach all clients from a session |

### Per-Session Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/sessions/:name/input` | Send input to the terminal |
| `GET` | `/sessions/:name/screen` | Current screen state |
| `GET` | `/sessions/:name/scrollback` | Scrollback buffer history |
| `GET` | `/sessions/:name/quiesce` | Wait for terminal quiescence |
| `GET` | `/sessions/:name/ws/raw` | Raw binary WebSocket |
| `GET` | `/sessions/:name/ws/json` | JSON request/response WebSocket |

### Overlays

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/sessions/:name/overlay` | Create a screen overlay |
| `GET` | `/sessions/:name/overlay` | List all overlays |
| `DELETE` | `/sessions/:name/overlay` | Clear all overlays |
| `GET` | `/sessions/:name/overlay/:id` | Get an overlay |
| `PUT` | `/sessions/:name/overlay/:id` | Replace an overlay's spans |
| `PATCH` | `/sessions/:name/overlay/:id` | Update overlay position/z-order |
| `DELETE` | `/sessions/:name/overlay/:id` | Delete an overlay |

### Panels

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/sessions/:name/panel` | Create a panel (top/bottom bar) |
| `GET` | `/sessions/:name/panel` | List all panels |
| `DELETE` | `/sessions/:name/panel` | Clear all panels |
| `GET` | `/sessions/:name/panel/:id` | Get a panel |
| `PUT` | `/sessions/:name/panel/:id` | Replace a panel |
| `PATCH` | `/sessions/:name/panel/:id` | Update panel properties |
| `DELETE` | `/sessions/:name/panel/:id` | Delete a panel |

### Input Capture

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/sessions/:name/input/mode` | Current input routing mode |
| `POST` | `/sessions/:name/input/capture` | Capture input (don't forward to PTY) |
| `POST` | `/sessions/:name/input/release` | Release input (resume forwarding) |

When input is captured, local keyboard input is not forwarded to the PTY. Press Ctrl+\ to toggle capture mode — it switches between passthrough and capture. Ctrl+\ is never forwarded to the PTY.

### Server Management

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/server/persist` | Upgrade server to persistent mode |
| `GET` | `/ws/json` | Server-level multiplexed WebSocket |

### Global

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Health check |
| `GET` | `/openapi.yaml` | OpenAPI 3.1 specification |
| `GET` | `/docs` | API documentation (markdown) |

**Full API documentation:** [docs/api/README.md](docs/api/README.md)

## Examples

```bash
# Check health
curl http://localhost:8080/health
# {"status":"ok"}

# Send a command
curl -X POST http://localhost:8080/sessions/default/input -d 'ls\n'

# Get screen contents
curl -s http://localhost:8080/sessions/default/screen | jq .

# Get scrollback with pagination
curl -s 'http://localhost:8080/sessions/default/scrollback?offset=0&limit=50' | jq .

# Wait for terminal to be idle for 500ms (with 10s deadline)
# Response includes a "generation" counter for efficient re-polling
curl -s 'http://localhost:8080/sessions/default/quiesce?timeout_ms=500&max_wait_ms=10000' | jq .

# Connect to raw WebSocket
websocat ws://localhost:8080/sessions/default/ws/raw

# JSON WebSocket: get screen contents
echo '{"id": 1, "method": "get_screen", "params": {"format": "plain"}}' \
  | websocat ws://localhost:8080/sessions/default/ws/json

# JSON WebSocket: subscribe to events
echo '{"id": 1, "method": "subscribe", "params": {"events": ["lines", "cursor"]}}' \
  | websocat ws://localhost:8080/sessions/default/ws/json

# JSON WebSocket: send input
echo '{"id": 2, "method": "send_input", "params": {"data": "ls\r"}}' \
  | websocat ws://localhost:8080/sessions/default/ws/json

# Create a status overlay
curl -X POST http://localhost:8080/sessions/default/overlay \
  -H 'Content-Type: application/json' \
  -d '{"x": 60, "y": 0, "z": 100, "spans": [{"text": "Agent: OK", "fg": "green"}]}'

# Create a panel (fixed bar at the top of the terminal)
curl -X POST http://localhost:8080/sessions/default/panel \
  -H 'Content-Type: application/json' \
  -d '{"position": "top", "height": 1, "spans": [{"text": " STATUS: running ", "fg": "white", "bg": "blue", "bold": true}]}'
```

### Server Mode Examples

```bash
# Start the server
wsh server &

# Create a session via the API
curl -X POST http://localhost:8080/sessions \
  -H 'Content-Type: application/json' \
  -d '{"name": "dev"}'

# List sessions
curl -s http://localhost:8080/sessions | jq .

# Attach from the terminal
wsh attach dev

# Send input to a session from another process
curl -X POST http://localhost:8080/sessions/dev/input -d 'echo hello\n'

# Detach all clients from a session
curl -X POST http://localhost:8080/sessions/dev/detach

# Upgrade to persistent mode (server survives last session exit)
wsh persist
```

## Authentication

When binding to localhost (default), no authentication is required. When
binding to a non-loopback address, bearer token auth is required:

```bash
# Auto-generated token (printed to stderr on startup)
wsh --bind 0.0.0.0:8080

# User-provided token
wsh --bind 0.0.0.0:8080 --token my-secret

# Or via environment variable
WSH_TOKEN=my-secret wsh --bind 0.0.0.0:8080
```

Authenticate via header or query parameter:

```bash
curl -H "Authorization: Bearer my-secret" http://host:8080/sessions/default/screen
curl 'http://host:8080/sessions/default/screen?token=my-secret'
```

See [docs/api/authentication.md](docs/api/authentication.md) for details.

## Architecture

```
┌───────────────────────────────────────────────────────────────────────────┐
│                               wsh                                         │
│                                                                           │
│  ┌───────────┐    ┌──────────┐    ┌──────────┐    ┌────────────────────┐  │
│  │    PTY    │───>│  Broker  │───>│  Parser  │───>│   HTTP/WS Server   │  │
│  │  (shell)  │    │(broadcast)│    │  (avt)   │    │      :8080         │  │
│  │          │<───│          │    │          │    │                    │  │
│  └───────────┘    └──────────┘    └──────────┘    └────────────────────┘  │
│       ^                                                    │              │
│       │                                                    v              │
│       v                                             ┌────────────┐        │
│  ┌───────────┐                                      │ Overlays   │        │
│  │  stdin    │ (keyboard)                           │ Panels     │        │
│  │  stdout   │ (terminal)                           │ Input      │        │
│  └───────────┘                                      │ Capture    │        │
│                                                     └────────────┘        │
│  ┌───────────────────────────────────────────────────────────────────┐    │
│  │                     Session Registry                              │    │
│  │  Manages named sessions, each with its own PTY/Broker/Parser      │    │
│  └───────────────────────────────────────────────────────────────────┘    │
│                                                                           │
│  ┌──────────────────┐    ┌──────────────────┐                             │
│  │  Unix Socket      │    │ Activity Tracker │                             │
│  │  (server mode)    │    │ (quiescence)     │                             │
│  │  list/kill/attach │    │                  │                             │
│  └──────────────────┘    └──────────────────┘                             │
└───────────────────────────────────────────────────────────────────────────┘
```

## Project Structure

```
src/
├── main.rs              # Entry point, CLI args, client/server orchestration
├── lib.rs               # Library exports
├── activity.rs          # Activity tracking for quiescence detection
├── broker.rs            # Broadcast channel for output fanout
├── client.rs            # Unix socket client (for attach/list/kill/detach)
├── protocol.rs          # Unix socket wire protocol (messages, serialization)
├── pty.rs               # PTY management (spawn, read, write, resize)
├── server.rs            # Unix socket server (session management daemon)
├── session.rs           # Session struct, SessionRegistry, session events
├── shutdown.rs          # Graceful shutdown coordination
├── terminal.rs          # Raw mode guard, terminal size, screen mode
├── api/
│   ├── mod.rs           # Router, AppState, route definitions
│   ├── auth.rs          # Bearer token authentication middleware
│   ├── error.rs         # ApiError type with structured JSON responses
│   ├── handlers.rs      # All HTTP/WebSocket handlers
│   └── ws_methods.rs    # WebSocket JSON-RPC dispatch and param types
├── input/
│   ├── mod.rs           # Input module exports
│   ├── events.rs        # Input event broadcasting
│   ├── keys.rs          # Key parsing (raw bytes -> ParsedKey)
│   └── mode.rs          # Passthrough/Capture mode state
├── overlay/
│   ├── mod.rs           # Overlay module exports
│   ├── render.rs        # ANSI rendering for local terminal
│   ├── store.rs         # Thread-safe overlay storage
│   └── types.rs         # Overlay, OverlaySpan, Color types
├── panel/
│   ├── mod.rs           # Panel module exports, reconfigure_layout
│   ├── coordinator.rs   # Panel layout coordination with PTY resize
│   ├── layout.rs        # Layout calculation (top/bottom panel regions)
│   ├── render.rs        # Panel ANSI rendering for local terminal
│   ├── store.rs         # Thread-safe panel storage
│   └── types.rs         # Panel, Position types
└── parser/
    ├── mod.rs           # Parser actor public API
    ├── events.rs        # Event types for WebSocket streaming
    ├── format.rs        # avt-to-JSON conversion
    ├── state.rs         # Data types (Screen, Cursor, Format, etc.)
    ├── task.rs          # Async parser task
    └── tests.rs         # Parser unit tests

docs/
├── VISION.md            # Project vision and architecture
└── api/
    ├── README.md        # API reference (served at /docs)
    ├── authentication.md
    ├── errors.md
    ├── input-capture.md
    ├── openapi.yaml     # OpenAPI 3.1 spec (served at /openapi.yaml)
    ├── overlays.md
    ├── panels.md
    └── websocket.md

skills/
└── wsh/
    ├── core/SKILL.md              # API mechanics and primitives
    ├── drive-process/SKILL.md     # CLI command interaction
    ├── tui/SKILL.md               # Full-screen TUI operation
    ├── multi-session/SKILL.md     # Parallel session orchestration
    ├── agent-orchestration/SKILL.md # Driving other AI agents
    ├── monitor/SKILL.md           # Watching and reacting
    ├── visual-feedback/SKILL.md   # Overlays and panels
    ├── input-capture/SKILL.md     # Keyboard interception
    └── generative-ui/SKILL.md     # Dynamic terminal experiences

tests/
├── common/
│   └── mod.rs                  # Shared test helpers
├── api_integration.rs          # HTTP API integration tests
├── auth_integration.rs         # Authentication integration tests
├── e2e_concurrent_input.rs     # Concurrent input end-to-end test
├── e2e_http.rs                 # HTTP end-to-end test
├── e2e_input.rs                # Input end-to-end test
├── e2e_websocket_input.rs      # WebSocket input end-to-end test
├── graceful_shutdown.rs        # Graceful shutdown tests
├── input_capture_integration.rs # Input capture integration tests
├── interactive_shell.rs        # Interactive shell tests
├── overlay_integration.rs      # Overlay integration tests
├── panel_integration.rs        # Panel integration tests
├── parser_integration.rs       # Parser integration tests
├── pty_integration.rs          # PTY integration tests
├── quiesce_integration.rs      # Quiescence integration tests
├── server_client_e2e.rs        # Server/client end-to-end tests
├── session_management.rs       # Session management tests
├── ws_json_methods.rs          # WebSocket JSON method tests
└── ws_server_integration.rs    # Server-level WebSocket tests
```

## Building

This project uses Nix for development. All cargo commands must be wrapped:

```bash
nix develop -c sh -c "cargo build"
nix develop -c sh -c "cargo build --release"
nix develop -c sh -c "cargo check"
```

## Running Tests

```bash
nix develop -c sh -c "cargo test"
nix develop -c sh -c "cargo test -- --nocapture"
nix develop -c sh -c "cargo test --test api_integration"
```

## License

TBD
