# Architecture

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
│   ├── ip_access.rs     # CIDR-based blocklist/allowlist for SSRF prevention
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
├── FUTURE.md            # Deferred design decisions and future features
└── api/
    ├── README.md        # API reference (served at /docs)
    ├── alt-screen.md
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
    ├── core-mcp/SKILL.md          # MCP tool reference (auto-loaded for MCP clients)
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
├── federation_api.rs           # Federation API endpoint tests
├── federation_e2e.rs           # Federation end-to-end tests
├── federation_security.rs      # Federation security tests (SSRF, sanitization)
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
