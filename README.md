# wsh: An API for Your Terminal

HTTP, WebSocket, and MCP server APIs for terminal I/O — plus [AI skills](#ai-skills-claude-code-plugin) that teach agents how to use them.

## Vision

**The AI revolution has reached your desktop. Now it has reached your terminal.**

AI agents are crossing the line from assistants to coworkers — managing files, automating browsers, writing and shipping code. But the revolution can only move as fast as the tools allow, and every human interface that hasn't been AI-enabled is lost productivity at scale. The biggest gap is the terminal. It's a fundamental impedance mismatch: the most universal interface in computing, and agents have never been able to simply sit at one and drive it the way a human does.

`wsh` fixes this. It sits transparently between your terminal and your shell, maintains a full terminal state machine, and exposes everything — screen contents, cursor state, input injection, idle detection, real-time events — as both a structured API and via an MCP server. An agent sees what you see. Types what's needed. Waits for the right moment. Reads the screen. Decides what to do next. Your terminal works exactly as before. But now any program a human can operate through a terminal, an agent can operate through `wsh`.

The implications land fast: orchestrator agents that launch fleets of AI coding tools in parallel terminal sessions, feed them tasks, and collect results. End-to-end automation that doesn't choke the moment a program asks a question. Live copilots that watch your session and render contextual help as overlays directly in your workflow -- true generative UI in the terminal. The terminal protocol survived fifty years because it's simple, universal, and composable. `wsh` doesn't replace it — it teaches AI to speak it, and makes every shell session AI-native. See [docs/VISION.md](docs/VISION.md) for the full project vision.

## What This Enables

- **Drive interactive tools**: Agents can operate installers, debuggers, REPLs, TUIs, and AI coding assistants -- anything that expects a human at the keyboard
- **Orchestrate AI in parallel**: Run multiple Claude Code instances (or any terminal-based AI tool) across separate sessions, coordinating a fleet of AI workers
- **Provide live assistance**: Watch a human's terminal session and offer contextual help, rendered as overlays directly in their terminal
- **Audit and monitor**: Observe terminal activity for security, compliance, or operational awareness
- **Automate end-to-end**: Set up entire environments, handling every interactive prompt and error along the way

![wsh demo](demo/demo.gif)

## Quick Start

### Getting Started

```bash
# Start wsh (auto-spawns a server daemon if one isn't running)
wsh

# In another terminal, list sessions
wsh list

# Start a second session with tags
wsh --name dev --tag build --tag frontend

# Attach to an existing session
wsh attach dev

# Manage tags on a running session
wsh tag dev --add production --remove draft

# Kill a session
wsh kill dev
```

Running `wsh` automatically starts a background server daemon (if one isn't already running) and creates a new session. Your terminal enters raw mode, and keyboard input and terminal output pass through transparently. Detach with `Ctrl+\` `Ctrl+\` (double-tap). The server exits automatically when the last session ends.

### Server Mode

For persistent operation (e.g., hosting sessions for AI agents):

```bash
# Start the server daemon (persistent by default)
wsh server

# Create a session via the API (with optional tags)
curl -X POST http://localhost:8080/sessions \
  -H 'Content-Type: application/json' \
  -d '{"name": "dev", "tags": ["build"]}'

# Attach to it from another terminal
wsh attach dev

# List active sessions
wsh list

# Kill a session
wsh kill dev
```

The server exposes an HTTP/WS API on `127.0.0.1:8080` and a Unix domain socket for client commands (`list`, `kill`, `attach`, `detach`). Use `--ephemeral` to have the server exit when its last session ends. Use `wsh persist` to upgrade a running ephemeral server to persistent mode.

### Named Instances

Run multiple independent servers with `-L` (like tmux's `-L`):

```bash
# Start two isolated servers
wsh server -L project-a --bind 127.0.0.1:8080
wsh server -L project-b --bind 127.0.0.1:9090

# Each has its own sessions
wsh -L project-a                  # connects to project-a
wsh list -L project-b             # lists project-b's sessions

# Or set per-project defaults via .envrc
export WSH_SERVER_NAME=myproject
wsh                               # automatically uses "myproject" instance
```

Each instance gets its own socket and lock file under `$XDG_RUNTIME_DIR/wsh/`. The default instance name is `default`.

### Federation (Multi-Server Clusters)

`wsh` supports federation -- a single hub server orchestrating sessions across multiple backend servers. This lets you distribute terminal sessions across machines while managing everything from one API endpoint.

**Configure via TOML** (`~/.config/wsh/federation.toml`):

```toml
# Optional: override the hub's hostname
[server]
hostname = "orchestrator"

# Default auth token for backends
default_token = "shared-secret"

# Backend servers
[[servers]]
address = "10.0.1.10:8080"

[[servers]]
address = "10.0.1.11:8080"
token = "per-server-token"
```

**Or manage at runtime:**

```bash
# Start the hub
wsh server --bind 127.0.0.1:8080

# Register backends via API
curl -X POST http://localhost:8080/servers \
  -H 'Content-Type: application/json' \
  -d '{"address": "10.0.1.10:8080"}'

# List all servers in the cluster
curl http://localhost:8080/servers

# Create a session on a specific backend
curl -X POST 'http://localhost:8080/sessions?server=backend-1' \
  -H 'Content-Type: application/json' \
  -d '{"name": "remote-build"}'

# List sessions across all servers
curl http://localhost:8080/sessions
```

The hub proxies session operations transparently -- all existing session endpoints work the same, with an optional `?server=<hostname>` parameter for targeting specific backends. Session listings aggregate across all healthy servers.

## Your Terminal, in a Browser

Start `wsh`. Open a browser.

```
http://localhost:8080
```

Your terminal is there — live, interactive, fully synced. Type in the browser, see it in your terminal. Type in your terminal, see it in the browser. Pull it up on your phone. The session doesn't care where the keystrokes come from.

This ships inside the `wsh` binary. No separate install, no configuration, no dependencies. It exists because once your terminal has a structured API, a production-quality browser client is a *side effect*. Everything here — session management, multiple view modes, mobile support, themes — is just an API client. The same API that AI agents use to drive your terminal. The web UI is the most visceral proof of what that API makes possible.

**Sidebar + main content layout.** A persistent sidebar shows live mini-previews of all sessions, organized into groups by tag. Drag sessions between groups to reassign tags. A resize handle separates the sidebar from the main content area. Three view modes for the main content:
- **Carousel** — 3D depth effect, navigate between sessions with arrow keys
- **Tiled** — auto-grid layout that adapts to the number of sessions
- **Queue** — idle-driven FIFO, surfaces sessions as they become idle

**Full terminal rendering.** 256-color and true-color ANSI, bold/italic/underline/strikethrough, alternate screen buffer — vim, htop, lazygit, and every other TUI works as expected.

**Keyboard-driven.** Command palette (Ctrl+Shift+K), shortcut cheat sheet (Ctrl+Shift+/), view mode switching (Ctrl+Shift+F/G/Q), session navigation (Ctrl+Shift+1-9), sidebar toggle (Ctrl+Shift+B), and more. All shortcuts use Ctrl+Shift as the modifier — no conflicts with window managers or browser shortcuts.

**Mobile adaptation.** Bottom sheet on phones (<640px), overlay sidebar on tablets (640-1024px), persistent sidebar on desktop (>1024px). Touch gestures, native scrolling, a modifier bar for Ctrl/Esc/arrows.

**Themes.** Glass, Neon, Minimal, Tokyo Night, Catppuccin, Dracula — plus a High Contrast mode for accessibility. Cycle with one click or via the command palette.

### Remote Access

```bash
# Bind to all interfaces (token auto-generated, printed to stderr)
wsh server --bind 0.0.0.0:8080

# Get the token (paste it into the browser when prompted)
wsh token

# Open from any device on the network
# http://<your-ip>:8080
```

For access over the internet, put it behind an SSH tunnel, Tailscale, or a reverse proxy with TLS. `wsh` provides authentication; your network provides encryption.

## The Agent Loop

AI agents interact with `wsh` sessions using a simple, universal pattern:

```
Send input  →  Wait for idle  →  Read screen  →  Decide  →  repeat
```

```bash
# Send a command
curl -X POST http://localhost:8080/sessions/default/input -d 'ls\n'

# Wait for terminal to be idle
curl -s 'http://localhost:8080/sessions/default/idle?timeout_ms=500&max_wait_ms=10000'

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
| `wsh:cluster-orchestration` | Managing sessions across multiple federated wsh servers |

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
| `--tag` | | | Tag for the session (repeatable) |
| `--alt-screen` | | | Use alternate screen buffer |
| `-L`, `--server-name` | `WSH_SERVER_NAME` | `default` | Server instance name (like tmux `-L`) |

### Subcommands

| Subcommand | Description |
|------------|-------------|
| `server` | Start the headless daemon (HTTP/WS + Unix socket) |
| `attach <name>` | Attach to an existing session on the server |
| `list` | List active sessions |
| `kill <name>` | Kill (destroy) a session |
| `tag <name>` | Add or remove tags on a session |
| `detach <name>` | Detach all clients from a session (session stays alive) |
| `token` | Print the server's auth token (retrieved via Unix socket) |
| `persist` | Upgrade a running server to persistent mode |
| `stop` | Stop the running wsh server |
| `mcp` | Start an MCP server over stdio (for AI hosts) |

#### `server` Flags

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--bind` | | `127.0.0.1:8080` | Address to bind the API server |
| `--token` | `WSH_TOKEN` | (auto-generated) | Authentication token |
| `--socket` | | (derived from `-L`) | Path to the Unix domain socket (overrides `-L`) |
| `-L`, `--server-name` | `WSH_SERVER_NAME` | `default` | Server instance name (like tmux `-L`) |
| `--max-sessions` | | (no limit) | Maximum number of concurrent sessions |

#### `attach` Flags

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--scrollback` | | `all` | Scrollback to replay: `all`, `none`, or a number of lines |
| `--socket` | | (derived from `-L`) | Path to the Unix domain socket (overrides `-L`) |
| `-L`, `--server-name` | `WSH_SERVER_NAME` | `default` | Server instance name |
| `--alt-screen` | | | Use alternate screen buffer |

#### `list`, `kill`, `detach`, `token`, `tag`, `stop` Flags

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--socket` | | (derived from `-L`) | Path to the Unix domain socket (overrides `-L`) |
| `-L`, `--server-name` | `WSH_SERVER_NAME` | `default` | Server instance name |

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
| `PATCH` | `/sessions/:name` | Update a session (rename, add/remove tags) |
| `DELETE` | `/sessions/:name` | Kill (destroy) a session |
| `POST` | `/sessions/:name/detach` | Detach all clients from a session |

### Per-Session Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/sessions/:name/input` | Send input to the terminal |
| `GET` | `/sessions/:name/screen` | Current screen state |
| `GET` | `/sessions/:name/scrollback` | Scrollback buffer history |
| `GET` | `/sessions/:name/idle` | Wait for terminal to become idle |
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

### Federation

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/servers` | List all servers in the cluster |
| `POST` | `/servers` | Register a backend server |
| `GET` | `/servers/{hostname}` | Get server status |
| `DELETE` | `/servers/{hostname}` | Deregister a backend server |

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
curl -s 'http://localhost:8080/sessions/default/idle?timeout_ms=500&max_wait_ms=10000' | jq .

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

# Create a session via the API (with optional tags)
curl -X POST http://localhost:8080/sessions \
  -H 'Content-Type: application/json' \
  -d '{"name": "dev", "tags": ["build"]}'

# List sessions (optionally filter by tag)
curl -s http://localhost:8080/sessions | jq .
curl -s 'http://localhost:8080/sessions?tag=build' | jq .

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

### MCP Authentication

The `/mcp` endpoint is subject to the same bearer token requirement as the
rest of the API. When a `--token` is configured, MCP clients must attach the
token as an `Authorization: Bearer <token>` header on every HTTP request.
How to supply a bearer token varies by MCP client — consult your client's
documentation. (The MCP specification defines an OAuth 2.1-based
authorization flow, but `wsh` does not currently implement it; a static
bearer token is used instead.)

If your MCP client does not support bearer tokens, bind the server to
localhost (the default) so that no token is required. A future release may
add the option to restrict `/mcp` to localhost connections regardless of the
server's bind address.

## Architecture

```
┌───────────────────────────────────────────────────────────────────────────┐
│                               wsh                                         │
│                                                                           │
│  ┌───────────┐    ┌───────────┐    ┌──────────┐    ┌────────────────────┐ │
│  │    PTY    │───>│  Broker   │───>│  Parser  │───>│   HTTP/WS Server   │ │
│  │  (shell)  │    │(broadcast)│    │  (avt)   │    │      :8080         │ │
│  │           │<───│           │    │          │    │                    │ │
│  └───────────┘    └───────────┘    └──────────┘    └────────────────────┘ │
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
│  ┌────────────────────┐    ┌──────────────────┐                           │
│  │  Unix Socket       │    │ Activity Tracker │                           │
│  │  (server mode)     │    │ (idle detection) │                           │
│  │  list/kill/attach  │    │                  │                           │
│  └────────────────────┘    └──────────────────┘                           │
└───────────────────────────────────────────────────────────────────────────┘
```

## Project Structure

```
src/
├── main.rs              # Entry point, CLI args, client/server orchestration
├── lib.rs               # Library exports
├── activity.rs          # Activity tracking for idle detection
├── broker.rs            # Broadcast channel for output fanout
├── client.rs            # Unix socket client (for attach/list/kill/detach)
├── protocol.rs          # Unix socket wire protocol (messages, serialization)
├── pty.rs               # PTY management (spawn, read, write, resize)
├── server.rs            # Unix socket server (session management daemon)
├── config.rs            # Federation config (TOML loading, hostname resolution)
├── session.rs           # Session struct, SessionRegistry, session events
├── shutdown.rs          # Graceful shutdown coordination
├── terminal.rs          # Raw mode guard, terminal size, screen mode
├── federation/
│   ├── mod.rs           # Federation module exports
│   ├── auth.rs          # Backend token resolution cascade
│   ├── connection.rs    # Persistent WebSocket connection to backends
│   ├── manager.rs       # FederationManager (registry + connections)
│   ├── registry.rs      # BackendRegistry, health tracking, validation
│   └── sanitize.rs      # Response sanitization for proxied data
├── api/
│   ├── mod.rs           # Router, AppState, route definitions
│   ├── auth.rs          # Bearer token authentication middleware
│   ├── error.rs         # ApiError type with structured JSON responses
│   ├── handlers.rs      # All HTTP/WebSocket handlers
│   ├── proxy.rs         # Federation proxy helpers (forward to backends)
│   ├── web.rs           # Embedded web UI asset serving (rust_embed)
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

web/                             # Browser-based terminal client (Preact + TypeScript)
├── src/
│   ├── app.tsx                  # Main application component
│   ├── api/ws.ts                # WebSocket client and reconnection logic
│   ├── components/              # LayoutShell, Sidebar, MainContent, DepthCarousel,
│   │                            #   AutoGrid, QueueView, CommandPalette, ShortcutSheet,
│   │                            #   ThemePicker, TagEditor, BottomSheet, Terminal, etc.
│   ├── state/
│   │   ├── sessions.ts          # Session reactive state (Preact Signals)
│   │   ├── groups.ts            # Tag-based group computation and sidebar state
│   │   └── terminal.ts          # Terminal rendering utilities
│   └── styles/                  # terminal.css, themes.css (6 themes + high contrast)
├── index.html                   # Entry point
└── vite.config.ts               # Build config (output to web-dist/ → embedded in binary)

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
    ├── generative-ui/SKILL.md     # Dynamic terminal experiences
    └── cluster-orchestration/SKILL.md # Distributed session management

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
├── idle_integration.rs          # Idle detection integration tests
├── server_client_e2e.rs        # Server/client end-to-end tests
├── session_management.rs       # Session management tests
├── lifecycle_stress.rs          # Lifecycle stress tests (detach/reattach/exit)
├── reliability_hardening.rs     # Reliability hardening tests (timeouts, limits, ownership)
├── ws_json_methods.rs          # WebSocket JSON method tests
└── ws_server_integration.rs    # Server-level WebSocket tests
```

## Building

You need a Rust toolchain to build the project:

```bash
cargo build
cargo build --release
```

## Running Tests

```bash
cargo test
cargo test -- --nocapture
cargo test --test api_integration
```

### Lifecycle Stress Tests

Stress tests for client/server lifecycle interactions (detach, reattach, alt screen, overlays, exit). These spawn real `wsh` processes inside PTYs and exercise realistic user interaction sequences. They're `#[ignore]` by default since they're slow and designed for bug hunting.

```bash
# Run all lifecycle stress tests
cargo test --test lifecycle_stress -- --ignored --nocapture

# Run a single scenario
cargo test --test lifecycle_stress scenario_1 -- --ignored --nocapture

# Run just the random walk
cargo test --test lifecycle_stress scenario_6 -- --ignored --nocapture

# Run repeated random walks (scenario 7) with custom iteration count and step range
WSH_STRESS_RUNS=20 WSH_STRESS_STEPS=50..100 cargo test --test lifecycle_stress scenario_7 -- --ignored --nocapture
```

| Env Var | Default | Description |
|---------|---------|-------------|
| `WSH_STRESS_RUNS` | `5` | Number of random walk iterations (scenario 7) |
| `WSH_STRESS_STEPS` | `20..50` | Steps per walk: `N` (exact) or `N..M` (range) |

On failure, each test logs the full action sequence and RNG seed for reproduction.

## License

TBD
