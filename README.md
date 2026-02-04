# wsh: The Web Shell

A transparent PTY wrapper that exposes terminal I/O via API. Run your shell normally while making it accessible to web clients, agents, and automation tools.

See [docs/VISION.md](docs/VISION.md) for the full project vision.

## Current Status: Phase 1 PoC

This is an early proof-of-concept implementing the core data flow:

- PTY spawning with the user's shell
- Bidirectional I/O (stdin passthrough, stdout broadcast)
- HTTP API (`/health`, `POST /input`)
- WebSocket streaming (`/ws/raw`)
- Signal handling for graceful shutdown

## Building

This project uses Nix for development. All cargo commands must be wrapped:

```bash
# Build
nix develop -c sh -c "cargo build"

# Build release
nix develop -c sh -c "cargo build --release"

# Check (fast compile check)
nix develop -c sh -c "cargo check"
```

## Running

```bash
nix develop -c sh -c "cargo run"
```

This starts wsh which:
1. Puts your terminal in raw mode
2. Spawns your `$SHELL` in a PTY
3. Starts an HTTP/WebSocket server on `127.0.0.1:8080`
4. Passes through all keyboard input and terminal output

Use `Ctrl+C` to exit gracefully.

## Running Tests

```bash
# Run all tests
nix develop -c sh -c "cargo test"

# Run with output
nix develop -c sh -c "cargo test -- --nocapture"

# Run specific test file
nix develop -c sh -c "cargo test --test api_integration"
nix develop -c sh -c "cargo test --test pty_integration"
```

## API Endpoints

### Health Check

```bash
curl http://127.0.0.1:8080/health
# {"status":"ok"}
```

### Send Input

Send keystrokes to the PTY:

```bash
# Send a command
curl -X POST http://127.0.0.1:8080/input -d 'echo hello'

# Send Enter key
curl -X POST http://127.0.0.1:8080/input -d $'\n'

# Send Ctrl+C
curl -X POST http://127.0.0.1:8080/input -d $'\x03'
```

### WebSocket Stream

Connect to `/ws/raw` for bidirectional raw terminal I/O:

```bash
# Using websocat (install: cargo install websocat)
websocat ws://127.0.0.1:8080/ws/raw
```

Binary messages sent to the WebSocket are written to the PTY. PTY output is broadcast as binary messages to all connected clients.

## Water Through Pipes

Here's a demonstration of data flowing through the system:

**Terminal 1 - Start wsh:**
```bash
nix develop -c sh -c "cargo run"
```

**Terminal 2 - Send commands via API:**
```bash
# Send 'ls' command
curl -X POST http://127.0.0.1:8080/input -d 'ls'
curl -X POST http://127.0.0.1:8080/input -d $'\n'
```

You'll see the `ls` command execute in Terminal 1, with output appearing both locally and available to any WebSocket clients.

**Terminal 3 - Watch via WebSocket:**
```bash
websocat ws://127.0.0.1:8080/ws/raw
```

Now any output in Terminal 1 streams to Terminal 3, and typing in Terminal 3 sends input to Terminal 1.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                          wsh                                │
│                                                             │
│  ┌──────────┐    ┌──────────┐    ┌───────────────────────┐  │
│  │   PTY    │───▶│  Broker  │───▶│    HTTP/WS Server     │  │
│  │ (shell)  │    │(broadcast)│    │  :3000                │  │
│  │          │◀───│          │◀───│  /health              │  │
│  └──────────┘    └──────────┘    │  /input               │  │
│       ▲                          │  /ws/raw              │  │
│       │                          └───────────────────────┘  │
│       ▼                                                     │
│  ┌──────────┐                                               │
│  │  stdin   │ (your keyboard in raw mode)                   │
│  │  stdout  │ (your terminal)                               │
│  └──────────┘                                               │
└─────────────────────────────────────────────────────────────┘
```

Data flows:
- **Keyboard → PTY**: Your keystrokes go directly to the shell
- **PTY → stdout + Broker**: Shell output goes to your terminal AND broadcasts to subscribers
- **API/WebSocket → PTY**: External input is written to the shell
- **Broker → WebSocket clients**: All connected clients receive shell output

## Project Structure

```
src/
├── main.rs      # Entry point, orchestrates all components
├── lib.rs       # Library exports for testing
├── pty.rs       # PTY management (spawn, read, write, resize)
├── broker.rs    # Broadcast channel for output fanout
├── api.rs       # Axum routes and handlers
└── terminal.rs  # Raw mode guard, terminal size

tests/
├── api_integration.rs   # HTTP and WebSocket endpoint tests
└── pty_integration.rs   # PTY + Broker round-trip tests
```

## License

TBD
