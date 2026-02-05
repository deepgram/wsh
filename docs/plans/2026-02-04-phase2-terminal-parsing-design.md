# Phase 2: Terminal Parsing & State - Design Document

**Date:** 2026-02-04
**Status:** Draft
**Depends on:** Phase 1 (complete)

---

## Overview

Phase 2 adds terminal state tracking to wsh, enabling structured API endpoints for agents and the foundation for the web UI. The core addition is a Parser module that maintains terminal state using the `avt` crate.

### Goals

- Integrate `avt` for terminal emulation
- Maintain terminal state (screen, scrollback, cursor)
- Add `/screen` and `/scrollback` HTTP endpoints
- Add `/ws/json` WebSocket endpoint with structured events
- Support both plain text and styled output formats

### Non-Goals

- Web UI (Phase 4)
- Authentication (Phase 3)
- Multiple sessions (future "server mode")

---

## Architecture

### Parser as Separate Task (Vanilla B)

The Parser runs as an independent async task that:
1. Subscribes to the raw byte broker (existing)
2. Owns the `avt::Vt` instance exclusively
3. Handles queries via internal channels
4. Broadcasts events to `/ws/json` subscribers

```
┌─────────────────────────────────────────────────────────────┐
│                                                             │
│  PTY Reader ──▶ Broker (raw bytes) ──┬──▶ stdout            │
│                                      ├──▶ /ws/raw clients   │
│                                      │                      │
│                                      ▼                      │
│                               Parser Task                   │
│                               ┌─────────────┐               │
│                               │ subscribes  │               │
│                               │ to raw      │               │
│                               │ bytes       │               │
│                               │             │               │
│                               │ avt::Vt     │               │
│                               │ (owned)     │               │
│                               └─────────────┘               │
│                                 │         │                 │
│                    queries ◀────┘         └────▶ events     │
│                       │                           │         │
│              ┌────────┴────────┐         ┌───────┴───────┐  │
│              ▼                 ▼         ▼               ▼  │
│         /screen          /scrollback  /ws/json       /ws/json│
│                                       client 1       client 2│
└─────────────────────────────────────────────────────────────┘
```

### Why This Architecture

- **No locks**: Parser owns Vt exclusively, no mutex contention
- **Clean separation**: Parser is isolated, testable independently
- **Simple consumer API**: `parser.query(...)` hides channel plumbing
- **Additive**: Existing `/ws/raw` and stdout passthrough unchanged

---

## Module Structure

```
src/
├── main.rs          # modified - create and wire up Parser
├── pty.rs           # unchanged
├── broker.rs        # unchanged
├── api.rs           # modified - add new endpoints
├── terminal.rs      # unchanged
├── shutdown.rs      # unchanged
├── lib.rs           # modified - export parser module
│
└── parser/          # NEW
    ├── mod.rs       # Parser struct, public API
    ├── state.rs     # TerminalState, Screen, Scrollback types
    ├── events.rs    # Event enum, subscription handling
    └── format.rs    # Plain/styled formatting
```

---

## Data Types

### Terminal State

```rust
pub struct TerminalState {
    /// Increments on discontinuities (reset, clear, alternate screen toggle)
    pub epoch: u64,

    /// Current screen (visible area)
    pub screen: Screen,

    /// Scrollback history
    pub scrollback: Arc<Scrollback>,

    /// Current cursor state
    pub cursor: Cursor,

    /// Whether alternate screen is active
    pub alternate_active: bool,
}

pub struct Screen {
    pub lines: Vec<Line>,
    pub cols: usize,
    pub rows: usize,
}

pub struct Scrollback {
    pub lines: Vec<Line>,
    pub limit: usize,
}

pub struct Line {
    pub spans: Vec<Span>,
}

pub struct Span {
    pub text: String,
    pub style: Style,
}

pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub visible: bool,
}

pub enum Color {
    Named(NamedColor),
    Indexed(u8),
    Rgb(u8, u8, u8),
}
```

### Query/Response Types

```rust
pub enum Query {
    Screen { format: Format },
    Scrollback { format: Format, offset: usize, limit: usize },
    Cursor,
    State,
    Resize { cols: usize, rows: usize },
}

pub enum Format {
    Plain,
    Styled,
}

pub enum QueryResponse {
    Screen(ScreenResponse),
    Scrollback(ScrollbackResponse),
    Cursor(CursorResponse),
    State(StateResponse),
    Ok,
}

pub struct ScreenResponse {
    pub epoch: u64,
    pub lines: Vec<FormattedLine>,
    pub cursor: Cursor,
    pub cols: usize,
    pub rows: usize,
}

pub struct ScrollbackResponse {
    pub epoch: u64,
    pub lines: Vec<FormattedLine>,
    pub total_lines: usize,
    pub offset: usize,
}

pub enum FormattedLine {
    Plain(String),
    Styled(Vec<Span>),
}
```

### Event Types

```rust
#[derive(Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Line {
        seq: u64,
        index: usize,
        line: FormattedLine,
    },
    Char {
        seq: u64,
        row: usize,
        col: usize,
        char: char,
        style: Style,
    },
    Cursor {
        seq: u64,
        row: usize,
        col: usize,
        visible: bool,
    },
    Mode {
        seq: u64,
        alternate_active: bool,
    },
    Reset {
        seq: u64,
        reason: ResetReason,
    },
    Sync {
        seq: u64,
        screen: ScreenResponse,
        scrollback_lines: usize,
    },
    Diff {
        seq: u64,
        changed_lines: Vec<usize>,
        screen: ScreenResponse,
    },
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetReason {
    ClearScreen,
    ClearScrollback,
    HardReset,
    AlternateScreenEnter,
    AlternateScreenExit,
    Resize,
}
```

### Subscription Protocol

```rust
#[derive(Deserialize)]
pub struct Subscribe {
    pub events: Vec<EventType>,
    #[serde(default = "default_interval")]
    pub interval_ms: u64,
    #[serde(default)]
    pub format: Format,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    Lines,
    Chars,
    Cursor,
    Mode,
    Diffs,
}
```

---

## Parser Public API

```rust
pub struct Parser {
    query_tx: mpsc::Sender<(Query, oneshot::Sender<QueryResponse>)>,
    event_tx: broadcast::Sender<Event>,
}

impl Parser {
    /// Spawn parser task, subscribing to raw byte broker
    pub fn spawn(raw_broker: &Broker, scrollback_limit: usize) -> Self;

    /// Query current state (hides channel creation)
    pub async fn query(&self, query: Query) -> Result<QueryResponse, ParserError>;

    /// Notify parser of terminal resize
    pub async fn resize(&self, cols: usize, rows: usize) -> Result<(), ParserError>;

    /// Subscribe to events (returns async Stream)
    pub fn subscribe(&self) -> impl Stream<Item = Event>;
}
```

---

## API Endpoints

### Existing (unchanged)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Health check |
| POST | `/input` | Send bytes to PTY |
| GET | `/ws/raw` | Raw byte WebSocket |

### New

| Method | Path | Description |
|--------|------|-------------|
| GET | `/screen` | Current screen state |
| GET | `/scrollback` | Scrollback history |
| GET | `/ws/json` | Structured event WebSocket |

### Query Parameters

**`/screen`**
- `format`: `plain` or `styled` (default: `styled`)

**`/scrollback`**
- `format`: `plain` or `styled` (default: `styled`)
- `offset`: Starting line (default: `0`)
- `limit`: Max lines to return (default: `100`)

### `/ws/json` Protocol

```
Client                              Server
  |                                    |
  |-------- connect to /ws/json ------>|
  |                                    |
  |<------- {"connected": true} -------|
  |                                    |
  |-- {"subscribe": ["lines", "mode"]} |
  |                                    |
  |<-- {"subscribed": ["lines","mode"]}|
  |                                    |
  |<-- {"event":"line", "seq":1, ...} -|
  |<-- {"event":"cursor", "seq":2, ...}|
  |                                    |
```

Clients can change subscription mid-session by sending a new `subscribe` message.

---

## Error Handling

```rust
#[derive(Debug, thiserror::Error)]
pub enum ParserError {
    #[error("parser task died unexpectedly")]
    TaskDied,

    #[error("query channel full")]
    ChannelFull,

    #[error("invalid query parameters: {0}")]
    InvalidQuery(String),
}
```

HTTP errors return JSON: `{"error": "message"}`

WebSocket errors are sent as messages: `{"error": "message", "code": "error_code"}`

---

## Integration

### main.rs Changes

```rust
// Create parser after broker
let broker = Broker::new();
let parser = Parser::spawn(&broker, 10_000);

// Pass to AppState
let state = AppState {
    input_tx,
    output_rx: broker.sender(),
    shutdown,
    parser,
};

// Set initial size
let (rows, cols) = terminal_size();
parser.resize(cols, rows).await?;

// In SIGWINCH handler
pty.resize(rows, cols)?;
parser.resize(cols, rows).await?;
```

### Dependencies

```toml
[dependencies]
avt = "0.15"
thiserror = "1.0"
tokio-stream = "0.1"
```

---

## Discontinuity Handling

Terminal programs can reset/clear state. The Parser tracks this via:

1. **Epoch counter**: Increments on any discontinuity
2. **Reset event**: Tells clients to discard accumulated state
3. **Sync event**: Provides full state for re-synchronization

Discontinuity triggers:
- Clear screen (`\e[2J`)
- Clear scrollback (`\e[3J`)
- Hard reset (`\ec`)
- Alternate screen enter/exit
- Terminal resize

---

## Testing Strategy

1. **Unit tests**: Parser in isolation with mock byte streams
2. **Integration tests**: Parser + Broker with real PTY output
3. **API tests**: HTTP endpoints return correct format
4. **WebSocket tests**: Subscription and event delivery

---

## Open Questions

None currently.

---

## References

- [avt crate](https://github.com/asciinema/avt) - Terminal emulation library
- [vte crate](https://docs.rs/vte/latest/vte/) - Lower-level parser (avt uses internally)
- Phase 1 implementation in `src/`
