# wsh: The Web Shell

A transparent PTY wrapper that exposes terminal I/O via API. Run your shell normally while making it accessible to web clients, agents, and automation tools.

See [docs/VISION.md](docs/VISION.md) for the full project vision.

## Quick Start

```bash
# Build and run (Nix required)
nix develop -c sh -c "cargo run"

# Or with a specific bind address and shell
nix develop -c sh -c "cargo run -- --bind 0.0.0.0:8080 --shell /bin/zsh"
```

This starts wsh which:
1. Puts your terminal in raw mode
2. Spawns your shell in a PTY
3. Starts an HTTP/WebSocket server on `127.0.0.1:8080`
4. Passes through all keyboard input and terminal output

Use `Ctrl+\` to exit.

## API Overview

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Health check |
| `POST` | `/input` | Send input to the terminal |
| `GET` | `/screen` | Current screen state |
| `GET` | `/scrollback` | Scrollback buffer history |
| `GET` | `/ws/raw` | Raw binary WebSocket |
| `GET` | `/ws/json` | JSON event WebSocket |
| `POST` | `/overlay` | Create a screen overlay |
| `GET` | `/overlay` | List all overlays |
| `DELETE` | `/overlay` | Clear all overlays |
| `GET/PUT/PATCH/DELETE` | `/overlay/:id` | Manage a single overlay |
| `GET` | `/input/mode` | Current input routing mode |
| `POST` | `/input/capture` | Capture input (don't forward to PTY) |
| `POST` | `/input/release` | Release input (resume forwarding) |
| `GET` | `/openapi.yaml` | OpenAPI 3.1 specification |
| `GET` | `/docs` | API documentation |

**Full API documentation:** [docs/api/README.md](docs/api/README.md)

## Examples

```bash
# Check health
curl http://localhost:8080/health
# {"status":"ok"}

# Send a command
curl -X POST http://localhost:8080/input -d 'ls\n'

# Get screen contents
curl -s http://localhost:8080/screen | jq .

# Get scrollback with pagination
curl -s 'http://localhost:8080/scrollback?offset=0&limit=50' | jq .

# Connect to raw WebSocket
websocat ws://localhost:8080/ws/raw

# Subscribe to structured events
echo '{"events": ["lines", "cursor"]}' | websocat ws://localhost:8080/ws/json

# Create a status overlay
curl -X POST http://localhost:8080/overlay \
  -H 'Content-Type: application/json' \
  -d '{"x": 60, "y": 0, "z": 100, "spans": [{"text": "Agent: OK", "fg": "green"}]}'
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
curl -H "Authorization: Bearer my-secret" http://host:8080/screen
curl 'http://host:8080/screen?token=my-secret'
```

See [docs/api/authentication.md](docs/api/authentication.md) for details.

## CLI Flags

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--bind` | | `127.0.0.1:8080` | Address to bind the API server |
| `--token` | `WSH_TOKEN` | (auto-generated) | Authentication token |
| `--shell` | | `$SHELL` or `/bin/sh` | Shell to spawn |

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

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                            wsh                                  │
│                                                                 │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐    ┌───────────┐ │
│  │   PTY    │───>│  Broker  │───>│  Parser  │───>│ HTTP/WS   │ │
│  │ (shell)  │    │(broadcast)│    │  (avt)   │    │ Server    │ │
│  │          │<───│          │    │          │    │ :8080     │ │
│  └──────────┘    └──────────┘    └──────────┘    └───────────┘ │
│       ^                                               │        │
│       │                                               v        │
│       v                                         ┌──────────┐   │
│  ┌──────────┐                                   │ Overlays │   │
│  │  stdin   │ (keyboard)                        │ Input    │   │
│  │  stdout  │ (terminal)                        │ Capture  │   │
│  └──────────┘                                   └──────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

## Project Structure

```
src/
├── main.rs           # Entry point, CLI args, orchestration
├── lib.rs            # Library exports
├── pty.rs            # PTY management (spawn, read, write, resize)
├── broker.rs         # Broadcast channel for output fanout
├── terminal.rs       # Raw mode guard, terminal size
├── shutdown.rs       # Graceful shutdown coordination
├── api/
│   ├── mod.rs        # Router, AppState, route definitions
│   ├── handlers.rs   # All HTTP/WebSocket handlers
│   ├── error.rs      # ApiError type with structured JSON responses
│   └── auth.rs       # Bearer token authentication middleware
├── parser/
│   ├── mod.rs        # Parser actor public API
│   ├── state.rs      # Data types (Screen, Cursor, Format, etc.)
│   ├── events.rs     # Event types for WebSocket streaming
│   ├── format.rs     # avt-to-JSON conversion
│   └── task.rs       # Async parser task
├── overlay/
│   ├── mod.rs        # Overlay module exports
│   ├── types.rs      # Overlay, OverlaySpan, Color types
│   ├── store.rs      # Thread-safe overlay storage
│   └── render.rs     # ANSI rendering for local terminal
└── input/
    ├── mod.rs        # Input module exports
    ├── mode.rs       # Passthrough/Capture mode state
    ├── events.rs     # Input event broadcasting
    └── keys.rs       # Key parsing (raw bytes -> ParsedKey)

docs/
├── VISION.md         # Project vision and architecture
└── api/
    ├── README.md     # API reference (served at /docs)
    ├── authentication.md
    ├── websocket.md
    ├── errors.md
    ├── overlays.md
    ├── input-capture.md
    └── openapi.yaml  # OpenAPI 3.1 spec (served at /openapi.yaml)

tests/
├── api_integration.rs
├── auth_integration.rs
├── overlay_integration.rs
├── input_capture_integration.rs
├── parser_integration.rs
├── pty_integration.rs
├── e2e_*.rs
├── graceful_shutdown.rs
└── interactive_shell.rs
```

## License

TBD
