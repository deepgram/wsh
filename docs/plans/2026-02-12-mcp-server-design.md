# MCP Server for wsh

## Overview

Add a Model Context Protocol (MCP) server to `wsh`, exposing terminal interaction
capabilities as native MCP tools, resources, and prompts. This makes `wsh` a
first-class citizen in the MCP ecosystem — any MCP-compatible AI agent (Claude,
OpenAI, etc.) can discover and use terminal sessions without learning the HTTP API.

The MCP server is a **new transport layer** inside `wsh`. No new state, no new
logic — just a new way to invoke the same `Session` methods that the HTTP/WS
handlers call. The HTTP/WS API is completely unchanged.

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                    wsh server                        │
│                                                      │
│  ┌──────────┐   ┌──────────────┐   ┌──────────────┐ │
│  │ Sessions │◀──│ HTTP/WS API  │◀──│ AI agents    │ │
│  │ (PTY,    │   └──────────────┘   │ Web UI       │ │
│  │  parser, │                      │ curl         │ │
│  │  overlays│   ┌──────────────┐   └──────────────┘ │
│  │  panels, │◀──│ MCP Server   │◀──┐               │
│  │  etc.)   │   │ (Streamable  │   │               │
│  └──────────┘   │  HTTP)       │   │               │
│       ▲         └──────────────┘   │               │
│       │                            │               │
└───────│────────────────────────────│───────────────┘
        │                            │
        │         ┌──────────────┐   │
        └─────────│ wsh mcp      │───┘
                  │ (stdio       │
                  │  bridge)     │◀── Claude Desktop,
                  └──────────────┘    AI hosts
```

Two entry points, same internals:

- **Streamable HTTP** — embedded in the existing `wsh server` process, served
  on the same port at `/mcp`. Uses SSE for server-to-client streaming. Shares
  the same `AppState`.
- **stdio bridge** (`wsh mcp`) — a thin subprocess that speaks MCP over
  stdin/stdout. Connects to a running `wsh server` (auto-spawning an ephemeral
  one if needed, reusing existing spawn logic from standalone `wsh`). Translates
  MCP JSON-RPC to internal calls over the Unix socket or localhost HTTP. Exits
  when the MCP host closes stdin.

MCP is inherently multiplexed — one connection, session specified per-request.
This matches the server-level `/ws/json` pattern where each request includes a
`session` field for routing. The MCP dispatch uses the same internal path:
session lookup per-request, not per-connection.

## Dependency

The [`rmcp` crate](https://github.com/modelcontextprotocol/rust-sdk) (v0.15.0),
the official Rust MCP SDK. Provides JSON-RPC framing, `ServerHandler` trait,
proc macros for tool schema generation, and stdio/HTTP transport primitives.

## Tools (14)

### Design Principles

Informed by [Anthropic's tool design guidance](https://www.anthropic.com/engineering/writing-tools-for-agents)
and [MCP design patterns](https://www.klavis.ai/blog/less-is-more-mcp-design-patterns-for-ai-agents):

- **~14 tools** — in the sweet spot alongside filesystem (13) and postgres (14)
  MCP servers. Enough for full coverage, few enough for reliable model selection.
- **1:1 tool-per-concept** rather than multiplexed action-param tools, except
  where a natural upsert pattern applies (overlays, panels).
- **Flat, descriptive parameters** with sensible defaults. No nested config
  objects.
- **`isError: true`** in tool results for operational failures (quiesce timeout,
  command error). Protocol-level JSON-RPC errors for bad params or server faults.
- **`wsh_` prefix** for namespacing in multi-server environments.

### Session Lifecycle (3 tools)

**`wsh_create_session`** — Create a new terminal session with an optional name,
command, size, and working directory.

```
Params:
  name?: string          — session name (auto-generated if omitted)
  command?: string       — command to run (default: $SHELL)
  rows?: number          — terminal rows (default: 24)
  cols?: number          — terminal columns (default: 80)
  cwd?: string           — working directory
  env?: object           — additional environment variables

Returns: { session, pid, rows, cols }
```

**`wsh_list_sessions`** — List all active sessions, or get details for a
specific one.

```
Params:
  session?: string       — if provided, return details for this session only

Returns: [{ name, pid, command, rows, cols, running }] or single object
```

**`wsh_manage_session`** — Perform a lifecycle action on an existing session:
kill it, rename it, or detach all clients.

```
Params:
  session: string        — target session name
  action: "kill" | "rename" | "detach"
  new_name?: string      — required when action is "rename"

Returns: { success: true }
```

### Terminal I/O (5 tools)

**`wsh_send_input`** — Send keystrokes or text to a session. Supports raw text
and base64-encoded binary input.

```
Params:
  session: string
  input: string          — the text or keystrokes to send
  encoding?: "utf8" | "base64"  — default: "utf8"

Returns: { success: true }
```

**`wsh_get_screen`** — Read the current screen contents of a session.

```
Params:
  session: string
  format?: "styled" | "plain"  — default: "styled"

Returns: { lines, cursor, rows, cols, mode }
```

**`wsh_get_scrollback`** — Read the scrollback buffer history of a session.

```
Params:
  session: string
  offset?: number        — default: 0
  limit?: number         — default: 100
  format?: "styled" | "plain"  — default: "styled"

Returns: { lines, total }
```

**`wsh_await_quiesce`** — Wait for a session's terminal to go idle (no output
for the timeout period).

```
Params:
  session: string
  timeout_ms?: number    — silence threshold, default: 2000
  max_wait_ms?: number   — overall deadline, default: 30000

Returns: { generation }
```

**`wsh_run_command`** — High-level: send input, wait for the terminal to go
idle, then read the screen. This is the primary tool for running commands and
reading their output. Encodes the entire send/wait/read loop in a single call.

```
Params:
  session: string
  input: string          — the command or text to send
  timeout_ms?: number    — silence threshold, default: 2000
  max_wait_ms?: number   — overall deadline, default: 30000
  format?: "styled" | "plain"  — default: "styled"

Returns: { screen: { lines, cursor, rows, cols, mode }, generation }
```

### Visual Feedback (4 tools)

**`wsh_overlay`** — Create a new floating overlay, update an existing one by ID,
or list all overlays. Uses a natural upsert pattern: omit `id` to create, provide
`id` to update.

```
Params:
  session: string
  id?: string            — omit to create, provide to update
  x?: number             — column position
  y?: number             — row position
  z?: number             — z-order
  width?: number
  height?: number
  background?: string    — background color
  spans?: array          — styled text spans
  list?: boolean         — if true, return all overlays (ignores other params)

Returns: { id, ... } or [{ id, ... }] when list=true
```

**`wsh_remove_overlay`** — Delete a specific overlay by ID, or clear all
overlays in the session.

```
Params:
  session: string
  id?: string            — omit to clear all

Returns: { success: true }
```

**`wsh_panel`** — Create a new anchored panel, update an existing one by ID,
or list all panels. Panels attach to the top or bottom of the terminal and
shrink the PTY area. Same upsert pattern as overlays.

```
Params:
  session: string
  id?: string            — omit to create, provide to update
  position?: "top" | "bottom"
  height?: number
  z?: number
  background?: string
  spans?: array
  list?: boolean         — if true, return all panels

Returns: { id, ... } or [{ id, ... }] when list=true
```

**`wsh_remove_panel`** — Delete a specific panel by ID, or clear all panels.

```
Params:
  session: string
  id?: string            — omit to clear all

Returns: { success: true }
```

### Input & Screen Control (2 tools)

**`wsh_input_mode`** — Get or set the input capture mode. In capture mode,
keyboard input is intercepted and delivered as events instead of being sent to
the PTY. Optionally set focus to a specific overlay or panel.

```
Params:
  session: string
  mode?: "capture" | "release"  — omit to get current mode
  focus?: string         — element ID to focus (overlay or panel)
  unfocus?: boolean      — clear focus

Returns: { mode, focused_element? }
```

**`wsh_screen_mode`** — Get or control the alternate screen mode. TUI
applications (vim, htop) use alternate screen; knowing which mode is active
determines what overlays/panels are visible.

```
Params:
  session: string
  action?: "enter_alt" | "exit_alt"  — omit to get current mode

Returns: { mode: "normal" | "alt" }
```

## Resources (3)

Resources expose read-only state that MCP hosts can pull into the model's
context without a tool call. Complementary to the tools — same data, different
access pattern.

| URI | Description | Returns |
|-----|-------------|---------|
| `wsh://sessions` | List of all active sessions | `[{ name, pid, command, rows, cols, running }]` |
| `wsh://sessions/{name}/screen` | Current screen contents | `{ lines, cursor, rows, cols, mode }` |
| `wsh://sessions/{name}/scrollback` | Scrollback buffer (last 100 lines) | `{ lines, total }` |

`wsh://sessions` is a dynamic resource that updates as sessions are
created/destroyed. The session-scoped resources are resource templates — the
host substitutes the session name.

## Prompts (9)

Each existing skill document becomes an MCP prompt. The agent (or user) can
request a skill by name, and its content is injected into the conversation as
context.

| Prompt Name | Source File | Description |
|-------------|-------------|-------------|
| `wsh:core` | `skills/wsh/core-mcp.md` | API primitives and send/wait/read/decide loop (MCP-adapted) |
| `wsh:drive-process` | `skills/wsh/drive-process.md` | Running CLI commands, handling prompts |
| `wsh:tui` | `skills/wsh/tui.md` | Operating full-screen TUI applications |
| `wsh:multi-session` | `skills/wsh/multi-session.md` | Parallel session orchestration |
| `wsh:agent-orchestration` | `skills/wsh/agent-orchestration.md` | Driving other AI agents |
| `wsh:monitor` | `skills/wsh/monitor.md` | Watching terminal activity |
| `wsh:visual-feedback` | `skills/wsh/visual-feedback.md` | Overlays and panels |
| `wsh:input-capture` | `skills/wsh/input-capture.md` | Capturing keyboard input |
| `wsh:generative-ui` | `skills/wsh/generative-ui.md` | Dynamic interactive experiences |

The core skill gets a separate `core-mcp.md` variant. The existing `core.md`
teaches curl-based HTTP invocation. `core-mcp.md` teaches the same concepts
(send/wait/read loop, quiescence, etc.) but references MCP tool names. The
specialized skills are already protocol-agnostic and work as-is.

## Error Handling

### Protocol-level errors (JSON-RPC)

Returned when the agent made a bad request:

| ApiError variant | JSON-RPC code | When |
|---|---|---|
| `SessionNotFound` | `-32602` (Invalid params) | Session name doesn't exist |
| `SessionAlreadyExists` | `-32602` (Invalid params) | Duplicate name on create |
| `InvalidInput` | `-32602` (Invalid params) | Bad param values, missing required |
| `OverlayNotFound` / `PanelNotFound` | `-32602` (Invalid params) | ID doesn't exist |
| `Internal` | `-32603` (Internal error) | PTY failures, unexpected state |

### Operational errors (tool result with `isError: true`)

Returned when the operation ran but produced a failure the model should reason
about:

- Quiesce timeout exceeded `max_wait_ms`
- Session process exited during operation
- Input rejected (e.g., session not running)

The error `message` field carries the same human-readable strings the HTTP API
returns.

## Module Structure

```
src/
  mcp/
    mod.rs          — module root, MCP server setup, capability negotiation
    tools.rs        — tool definitions, parameter schemas, dispatch
    resources.rs    — resource definitions and handlers
    prompts.rs      — prompt definitions (loads skill markdown files)
    transport.rs    — Streamable HTTP + stdio transport adapters
  api/
    ...             — existing HTTP/WS code, unchanged
  main.rs           — adds `wsh mcp` subcommand
  lib.rs            — adds `mod mcp`
skills/
  wsh/
    core-mcp.md     — MCP-adapted core skill (new)
    ...             — existing skills, unchanged
```

The MCP handlers call the same internal `Session` / `AppState` methods that
the HTTP handlers call. No duplication of business logic.

## Testing

### Unit Tests

Located in `src/mcp/tools.rs`, `src/mcp/resources.rs`, `src/mcp/prompts.rs`.
Test the MCP layer in isolation — no server, no PTY, no network.

**Tool parameter parsing & validation (per tool):**
- Valid params → correct internal call mapped
- Missing required params → JSON-RPC `-32602` error
- Invalid param values (bad enum variants, negative numbers) → error
- `wsh_overlay` / `wsh_panel` upsert: create-path (no id) vs update-path
  (with id) vs list-path (`list: true`) dispatched correctly
- `wsh_manage_session` action dispatch: kill/rename/detach map to correct
  internal calls
- `wsh_input_mode` combined behavior: mode-only, mode+focus, unfocus, get-only

**Response shaping:**
- Tool results serialize with correct field names and types
- `isError: true` for operational failures vs protocol errors for bad params
- Screen data correct in both `styled` and `plain` formats

**Error mapping:**
- Every `ApiError` variant maps to correct JSON-RPC error code
- Error messages preserved through mapping

**Resource handlers:**
- `wsh://sessions` returns well-formed session list
- `wsh://sessions/{name}/screen` returns screen data
- `wsh://sessions/{name}/scrollback` returns scrollback with defaults
- Unknown session in URI → error

**Prompt handlers:**
- All 9 prompts listed with correct names and descriptions
- Each prompt returns non-empty markdown content
- `wsh:core` returns MCP-adapted variant, not curl variant
- Unknown prompt name → error

### Integration Tests — Streamable HTTP (`tests/mcp_http.rs`)

Stand up a real `wsh server` with MCP endpoint, exercise full
tool → session → PTY path.

**Session lifecycle:**
- Create → list shows it → manage(kill) → gone
- Create with custom name, command, cwd, size → all params take effect
- Create with duplicate name → error
- Manage(rename) → list reflects new name
- Manage(detach) → verify behavior
- List with `session` param returns single session detail

**Core agent loop (most critical):**
- `wsh_run_command("echo hello")` → response contains "hello" on screen
- `wsh_run_command` with slow command → quiesce works, screen read after
  output completes
- `wsh_run_command` exceeding `max_wait_ms` → `isError: true`, not protocol
  error
- Sequential `wsh_run_command` calls → each returns correct screen state

**Granular terminal I/O:**
- `send_input` → `await_quiesce` → `get_screen` — same result as `run_command`
- `send_input` with base64 encoding
- `get_scrollback` with offset/limit pagination
- `get_scrollback` after enough output to fill buffer — verify total count
- `await_quiesce` with generation — second call blocks until new activity
- `get_screen` in plain vs styled format

**Overlays:**
- Create → list → update spans → read back → remove → list empty
- Create with all position/size params → stored correctly
- Update position (x, y, z) via upsert with id
- Remove specific overlay by id (others remain)
- Clear all overlays (no id param)
- Overlay on nonexistent session → error
- Update nonexistent overlay id → error

**Panels:**
- Same test matrix as overlays, adapted for panel params (position, height)
- Verify panel anchoring (top vs bottom)

**Input control:**
- `input_mode()` → returns passthrough
- `input_mode(capture)` → mode changes → confirmed on re-read
- `input_mode(release)` → back to passthrough
- `input_mode(capture, focus: overlay_id)` → focus set
- `input_mode(unfocus: true)` → focus cleared
- Focus on nonexistent element → error

**Screen mode:**
- `screen_mode()` → returns "normal"
- `screen_mode(enter_alt)` → mode changes
- `screen_mode(exit_alt)` → back to normal
- Overlays created in alt mode not visible after exit

**Resources over MCP:**
- Read `wsh://sessions` → matches `list_sessions` output
- Read `wsh://sessions/{name}/screen` → matches `get_screen` output
- Read `wsh://sessions/{name}/scrollback` → matches `get_scrollback` output
- Resource read for nonexistent session → error

**Prompts over MCP:**
- List prompts → all 9 returned with correct names
- Get each prompt → non-empty markdown content
- `wsh:core` contains MCP tool names, not curl

**Multi-session:**
- Create 3 sessions → `run_command` on each → screens independent
- Kill one → others unaffected
- List shows remaining sessions only

**Edge cases:**
- Unknown tool name → method not found error
- Concurrent tool calls to same session → no corruption
- Tool calls after session process exits → appropriate error
- Very large screen output → well-formed response
- Unicode input and output

### Integration Tests — stdio Bridge (`tests/mcp_stdio.rs`)

Spawn `wsh mcp` as a child process, speak MCP JSON-RPC over stdin/stdout.

**Auto-spawn:**
- `wsh mcp` with no server running → server auto-spawns → tools work
- `wsh mcp` with server already running → connects to existing server

**Full tool exercise:**
- Core agent loop over stdio: create session, run command, read screen, kill
- Verify same results as HTTP transport

**Lifecycle:**
- Close stdin → `wsh mcp` exits cleanly
- Kill `wsh mcp` → ephemeral server shuts down (if only client)

**Error behavior:**
- Malformed JSON-RPC on stdin → error response, no crash
- Invalid method → error response

### Compatibility Tests (`tests/api_unchanged.rs`)

- Representative subset of existing HTTP API tests with MCP server enabled
- Verify no regressions: same endpoints, same responses, same behavior
- MCP `/mcp` route does not interfere with existing routes

## Implementation Phases

### Phase 1: Foundation
- Add `rmcp` dependency to `Cargo.toml`
- Create `src/mcp/mod.rs` module structure
- Implement `ServerHandler` trait with capability negotiation (advertise
  tools, resources, prompts)
- Wire Streamable HTTP transport at `/mcp` on existing Axum server
- Implement one tool: `wsh_list_sessions` — proves full path end-to-end
- Unit tests for handler setup and capability negotiation
- Integration test: connect to `/mcp`, call `wsh_list_sessions`, get response

### Phase 2: Session & Terminal I/O Tools
- Implement all 8 session + terminal tools
- Internal dispatch routes each tool to existing `Session` / `AppState`
  methods — same functions HTTP handlers call
- Session routing follows server-level WebSocket pattern: lookup by name
  per-request, not per-connection
- Unit tests for every tool's param parsing, validation, error mapping
- Integration tests for core agent loop and all terminal I/O scenarios

### Phase 3: Visual Feedback & Control Tools
- Implement remaining 6 tools: `wsh_overlay`, `wsh_remove_overlay`,
  `wsh_panel`, `wsh_remove_panel`, `wsh_input_mode`, `wsh_screen_mode`
- Upsert logic for overlay/panel tools
- Unit and integration tests for all CRUD operations, input capture,
  focus, screen mode

### Phase 4: Resources & Prompts
- Implement 3 resource handlers
- Implement prompt handler that loads skill markdown files
- Create `core-mcp.md` — adapted core skill referencing MCP tool names
- Unit and integration tests for resources and prompts

### Phase 5: stdio Bridge
- Add `wsh mcp` subcommand to CLI
- Reuse existing server auto-spawn logic from standalone `wsh`
- Bridge: MCP JSON-RPC on stdin/stdout ↔ internal server calls
- Integration tests: spawn as child process, exercise full tool set
- Test auto-spawn and clean shutdown

### Phase 6: Documentation & Skills
- Finalize `core-mcp.md` with final tool names and usage patterns
- Update README with MCP setup instructions (Claude Desktop config, etc.)
- Update `docs/VISION.md` roadmap to reflect MCP as shipped
- OpenAPI spec unchanged (MCP is a separate protocol)
