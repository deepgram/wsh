# wsh: The Web Shell

A transparent PTY wrapper that exposes terminal I/O via API. Run your shell normally while making it accessible to web clients, agents, and automation tools.

See [docs/VISION.md](docs/VISION.md) for the full project vision.

## Current Status: Phase 2 Complete

Core terminal multiplexing with state tracking:

- PTY spawning with the user's shell
- Bidirectional I/O (stdin passthrough, stdout broadcast)
- Terminal state parsing via `avt` crate (ANSI sequences, cursor, colors)
- HTTP API for input and state queries
- WebSocket streaming (raw bytes and structured JSON events)
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
nix develop -c sh -c "cargo test --test parser_integration"
```

## API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check |
| `/input` | POST | Send keystrokes to PTY |
| `/screen` | GET | Current terminal screen content |
| `/scrollback` | GET | Scrollback buffer history |
| `/ws/raw` | WebSocket | Raw binary terminal I/O |
| `/ws/json` | WebSocket | Structured JSON event stream |

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

### Get Screen Content

Query the current terminal screen:

```bash
# Styled output (with colors, bold, etc.)
curl -s http://127.0.0.1:8080/screen | jq .

# Plain text output
curl -s 'http://127.0.0.1:8080/screen?format=plain' | jq .
```

Response (styled):
```json
{
  "epoch": 0,
  "lines": [
    [{"text": "user@host", "fg": {"indexed": 2}, "bold": true}, {"text": ":~$ "}]
  ],
  "cursor": {"row": 0, "col": 12, "visible": true},
  "cols": 80,
  "rows": 24,
  "alternate_active": false
}
```

Response (plain):
```json
{
  "epoch": 0,
  "lines": ["user@host:~$ "],
  "cursor": {"row": 0, "col": 12, "visible": true},
  "cols": 80,
  "rows": 24,
  "alternate_active": false
}
```

### Get Scrollback History

Query historical output that has scrolled off screen:

```bash
# Get scrollback (default limit: 100 lines)
curl -s http://127.0.0.1:8080/scrollback | jq .

# With pagination
curl -s 'http://127.0.0.1:8080/scrollback?offset=0&limit=50' | jq .

# Plain text format
curl -s 'http://127.0.0.1:8080/scrollback?format=plain' | jq .
```

Response:
```json
{
  "epoch": 0,
  "lines": ["previous output...", "more history..."],
  "total_lines": 150,
  "offset": 0
}
```

### Raw WebSocket Stream

Connect to `/ws/raw` for bidirectional raw terminal I/O:

```bash
# Using websocat (install: cargo install websocat)
websocat ws://127.0.0.1:8080/ws/raw
```

Binary messages sent to the WebSocket are written to the PTY. PTY output is broadcast as binary messages to all connected clients.

### JSON Event Stream

Connect to `/ws/json` for structured terminal events:

```bash
# Subscribe to line and cursor events
echo '{"events": ["lines", "cursor"]}' | websocat ws://127.0.0.1:8080/ws/json
```

**Protocol:**
1. Server sends: `{"connected": true}`
2. Client sends: `{"events": ["lines", "cursor", "mode", "diffs"]}`
3. Server sends: `{"subscribed": ["lines", "cursor"]}`
4. Server streams matching events

**Event types:**
- `lines` - Line content changes
- `cursor` - Cursor position/visibility changes
- `mode` - Mode changes (alternate screen enter/exit)
- `diffs` - Screen diff events

**Example events:**
```json
{"event": "line", "seq": 1, "index": 0, "line": [{"text": "$ ls"}]}
{"event": "cursor", "seq": 2, "row": 0, "col": 4, "visible": true}
{"event": "reset", "seq": 3, "reason": "resize"}
```

## Examples

### Multi-Terminal Demo

**Terminal 1 - Start wsh:**
```bash
nix develop -c sh -c "cargo run"
```

**Terminal 2 - Send commands via API:**
```bash
curl -X POST http://127.0.0.1:8080/input -d 'ls -la'
curl -X POST http://127.0.0.1:8080/input -d $'\n'
```

**Terminal 3 - Watch raw output:**
```bash
websocat ws://127.0.0.1:8080/ws/raw
```

**Terminal 4 - Watch structured events:**
```bash
echo '{"events": ["lines"]}' | websocat ws://127.0.0.1:8080/ws/json
```

All terminals see the same session in real-time.

### Agent Integration Example

```python
import requests
import json

# Send a command
requests.post('http://127.0.0.1:8080/input', data='ls\n')

# Wait a moment for output
import time; time.sleep(0.1)

# Read the screen
resp = requests.get('http://127.0.0.1:8080/screen?format=plain')
screen = resp.json()

print(f"Cursor at row {screen['cursor']['row']}")
for line in screen['lines']:
    print(line)
```

### Event Streaming Example

```python
import asyncio
import websockets
import json

async def monitor():
    async with websockets.connect('ws://127.0.0.1:8080/ws/json') as ws:
        # Wait for connected
        await ws.recv()

        # Subscribe
        await ws.send(json.dumps({"events": ["lines", "cursor"]}))
        await ws.recv()  # subscription confirmation

        # Stream events
        async for msg in ws:
            event = json.loads(msg)
            print(f"Event: {event['event']}, seq: {event['seq']}")

asyncio.run(monitor())
```

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                            wsh                                  │
│                                                                 │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐    ┌───────────┐  │
│  │   PTY    │───▶│  Broker  │───▶│  Parser  │───▶│ HTTP/WS   │  │
│  │ (shell)  │    │(broadcast)│    │  (avt)   │    │ Server    │  │
│  │          │◀───│          │    │          │    │ :8080     │  │
│  └──────────┘    └──────────┘    └──────────┘    └───────────┘  │
│       ▲                                                │        │
│       │                                                ▼        │
│       ▼                                          ┌──────────┐   │
│  ┌──────────┐                                    │ Endpoints│   │
│  │  stdin   │ (keyboard)                         │ /screen  │   │
│  │  stdout  │ (terminal)                         │ /scroll  │   │
│  └──────────┘                                    │ /ws/json │   │
│                                                  └──────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

Data flows:
- **Keyboard → PTY**: Your keystrokes go directly to the shell
- **PTY → Broker → Parser**: Output is broadcast and parsed for terminal state
- **Parser → API**: Screen content, cursor position, scrollback available via HTTP
- **Parser → WebSocket**: Structured events streamed to subscribers
- **API/WebSocket → PTY**: External input is written to the shell

## Project Structure

```
src/
├── main.rs       # Entry point, orchestrates all components
├── lib.rs        # Library exports
├── pty.rs        # PTY management (spawn, read, write, resize)
├── broker.rs     # Broadcast channel for output fanout
├── api.rs        # Axum routes and handlers
├── terminal.rs   # Raw mode guard, terminal size
├── shutdown.rs   # Graceful shutdown coordination
└── parser/       # Terminal state tracking
    ├── mod.rs    # Parser struct and public API
    ├── state.rs  # Data types (Screen, Cursor, Format, etc.)
    ├── events.rs # Event types for streaming
    ├── format.rs # avt to JSON conversion
    └── task.rs   # Async parser task

tests/
├── api_integration.rs      # HTTP endpoint tests
├── parser_integration.rs   # Terminal parsing tests
├── pty_integration.rs      # PTY + Broker tests
└── e2e_*.rs               # End-to-end tests
```

## License

TBD
