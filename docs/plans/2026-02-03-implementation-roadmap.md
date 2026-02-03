# wsh Implementation Roadmap

This document describes the implementation plan for wsh, from proof-of-concept through production-ready features.

---

## Proof-of-Concept Goal

Establish the core data flow: PTY ↔ wsh ↔ API clients.

A successful PoC means:
- `curl -X POST -d 'ls' localhost:8080/input` sends keystrokes to the shell
- `websocat ws://localhost:8080/ws/raw` streams terminal output in real-time
- Local terminal works normally (transparent passthrough)

---

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                        wsh                              │
│                                                         │
│   ┌──────────┐     ┌──────────┐     ┌──────────────┐   │
│   │  stdin   │────▶│          │────▶│   PTY        │   │
│   │  stdout  │◀────│  Core    │◀────│   (shell)    │   │
│   └──────────┘     │  Loop    │     └──────────────┘   │
│                    │          │                         │
│                    │          │◀────┐                   │
│                    └──────────┘     │                   │
│                         │           │                   │
│                         ▼           │                   │
│                    ┌──────────┐     │                   │
│                    │  Axum    │     │                   │
│                    │  Server  │─────┘                   │
│                    └──────────┘                         │
│                      │     │                            │
└──────────────────────│─────│────────────────────────────┘
                       │     │
              WebSocket│     │HTTP POST
              /ws/raw  │     │/input
                       ▼     ▼
                    API Clients
```

**Data flow:**
- Local stdin → PTY (you type normally)
- PTY output → local stdout AND broadcast to WebSocket clients
- HTTP POST /input → PTY (curl can inject keystrokes)
- WebSocket messages → PTY (web clients can send input)

All paths converge at the PTY. Output fans out to all consumers.

---

## Technology Choices

| Component | Choice | Rationale |
|-----------|--------|-----------|
| Async runtime | Tokio | Industry standard, excellent ecosystem |
| Web framework | Axum | Best WebSocket ergonomics, tower middleware |
| PTY library | portable-pty | Cross-platform, maintained by Wezterm author |
| Terminal parser | vte | Battle-tested by Alacritty (Phase 2) |

---

## API Design

### Proof-of-Concept Endpoints

**`GET /ws/raw` - Raw WebSocket connection**
- Upgrades to WebSocket
- Server pushes PTY output as binary frames (raw bytes, unprocessed)
- Client sends binary/text frames → forwarded to PTY as input
- Multiple clients can connect; all receive the same output
- This is what a web-based terminal emulator would use

**`POST /input` - Send keystrokes**
- Request body sent directly to PTY
- Returns 204 No Content on success
- Example: `curl -X POST -d 'ls -la' localhost:8080/input`
- Example: `curl -X POST -d $'\x03' localhost:8080/input` (Ctrl+C)

**`GET /health` - Liveness check**
- Returns `{"status": "ok"}`

### Future Endpoints (not in PoC)

**`GET /ws/json` - Structured WebSocket connection**
- Server pushes parsed events as JSON
- Client sends JSON commands
- For agents and programmatic consumers

**`GET /screen` - Current screen state**

**`GET /scrollback` - History buffer**

**`GET /status` - Terminal mode flags, cursor position**

---

## Module Structure

**`main.rs` - Entry point**
- Parse CLI args (`--bind`, defaulting to `127.0.0.1:8080`)
- Initialize PTY with user's shell (`$SHELL` or `/bin/sh`)
- Spawn the core loop and Axum server
- Handle graceful shutdown (SIGINT/SIGTERM)

**`pty.rs` - PTY management**
- Wrapper around `portable-pty`
- Spawns shell, holds master handle
- Provides async read/write methods
- Handles resize (SIGWINCH from local terminal → PTY)

**`broker.rs` - Output fanout**
- Receives bytes from PTY reader
- Broadcasts to: local stdout, all connected WebSocket clients
- Manages client subscription list (add/remove on connect/disconnect)
- Uses `tokio::sync::broadcast` channel for fanout

**`api.rs` - Axum routes**
- `GET /ws/raw` - upgrades to WebSocket, subscribes to broker, forwards input to PTY
- `POST /input` - writes request body to PTY
- `GET /health` - returns status JSON

---

## Concurrency Model

Three async tasks run concurrently, communicating via channels:

**Task 1: PTY Reader**
- Loops reading from PTY master
- Sends each chunk to the broadcast channel
- Also writes to local stdout

**Task 2: Local Input**
- Reads from local stdin (async)
- Sends to PTY writer task via channel

**Task 3: Axum Server**
- Accepts HTTP/WebSocket connections
- Each WebSocket connection spawns two sub-tasks:
  - Reader: receives client messages → sends to PTY writer channel
  - Writer: subscribes to broadcast channel → sends to client

**PTY Writer Task**
- Owns the PTY write handle exclusively
- Receives from mpsc channel (all input sources send here)
- Serializes writes without mutex

```
stdin ──────────────────────────────────┐
                                        ▼
                                   ┌─────────┐
broadcast_tx ◀── PTY Reader ◀───── │   PTY   │
     │                             └─────────┘
     │                                  ▲
     ├──▶ stdout                        │
     │                                  │
     └──▶ WS Client 1 Writer            │
     └──▶ WS Client 2 Writer            │
                                        │
         ┌──────────────────────────────┤
         │        PTY Writer Task       │
         │              ▲               │
         └──────────────│───────────────┘
                        │
WS Client 1 Reader ─────┤
WS Client 2 Reader ─────┤
POST /input ────────────┤
stdin ──────────────────┘
```

---

## Error Handling

**PTY exits (shell terminates)**
- PTY reader gets EOF
- Broadcast close to all WebSocket clients
- Shut down gracefully, exit wsh with shell's exit code

**WebSocket client disconnects**
- Remove from broadcast subscriber list
- No impact on other clients or local terminal

**Local terminal disconnects (stdin EOF)**
- Keep running - API clients may still be connected
- Log the event, continue serving

**PTY write fails**
- Return 400 on POST /input (shell is gone, client's problem now)
- Close WebSocket with appropriate close code
- Expected during shutdown, not a server error

**Broadcast channel falls behind (slow client)**
- Log a warning with client identifier when messages are dropped
- Include drop count in logs (be noisy about this)
- Consider sending "you missed N bytes" notification to client
- Future: configurable buffer size, backpressure, or disconnect policy

**Local terminal resize (SIGWINCH)**
- Capture signal, read new terminal size
- Call `pty.resize()` to propagate to shell

---

## Local Terminal Raw Mode

For transparent passthrough, the local terminal needs raw mode.

**On startup:**
- Save current termios settings
- Set stdin to raw mode (no echo, no line buffering, no signal handling)
- This lets wsh capture Ctrl+C, Ctrl+Z, etc. and forward to PTY

**On shutdown:**
- Restore original termios settings
- Must handle both clean exit and panic/signal

**Implementation:**
- Use `crossterm` or `termios` crate for raw mode
- Wrap in a guard struct that restores on drop
- Register signal handlers to ensure cleanup

---

## Phased Roadmap

### Phase 1: Proof-of-Concept

- PTY spawn with portable-pty
- Local stdin/stdout passthrough in raw mode
- Axum server on 127.0.0.1:8080
- `/ws/raw`, `POST /input`, `GET /health`
- Graceful shutdown, signal handling
- No parsing, no state, no auth

### Phase 2: Terminal Parsing & State

- Integrate vte parser
- Build terminal state machine (cursor, screen buffer, scrollback)
- Add `/ws/json` with structured events
- Add `GET /screen`, `GET /scrollback` endpoints
- This unlocks meaningful agent integration

### Phase 3: API Hardening & Documentation

- OpenAPI/JSON Schema for all endpoints
- Authentication for non-localhost binding
- Configurable buffer sizes, timeouts
- Comprehensive error responses
- CLI flags: `--bind`, `--token`, `--shell`

### Phase 4: Web UI

- Mobile-first browser interface
- Normal mode (reflowing HTML) vs alternate screen mode (grid)
- Modifier bar for Esc, Ctrl, arrows
- Native scrolling and text selection

### Phase 5: Headless Mode & Agent Hooks

- `--headless` flag for automation (no local stdin/stdout)
- MCP-style interface
- Semantic events (command complete, prompt detected)

---

## Default Configuration

| Setting | Default | Notes |
|---------|---------|-------|
| Bind address | 127.0.0.1:8080 | Localhost only, no auth required |
| Shell | `$SHELL` or `/bin/sh` | User's default shell |
| Broadcast buffer | TBD | Tokio broadcast channel capacity |
