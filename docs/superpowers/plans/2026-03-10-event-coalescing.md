# Event Coalescing Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent WebSocket subscriber lag under high-throughput terminal sessions by coalescing events in forwarding tasks and bumping broadcast buffer capacities.

**Architecture:** Forwarding tasks switch from blocking `mpsc::send().await` to non-blocking `try_send()`. When the downstream mpsc is full, a dirty flag is set and a periodic timer queries the parser for a full `Sync` snapshot. The per-session WS path gets a new bounded mpsc between the handler loop and the WS sink, giving it the same `try_send` backpressure signal.

**Tech Stack:** Rust (tokio, axum), TypeScript (web client)

**Spec:** `docs/superpowers/specs/2026-03-10-event-coalescing-design.md`

---

## Chunk 1: Buffer Bump + Server-Level Coalescing

### Task 1: Bump broadcast buffer capacities

**Files:**
- Modify: `src/broker.rs:4` — `BROADCAST_CAPACITY`
- Modify: `src/parser/mod.rs:63` — parser event broadcast channel

- [ ] **Step 1: Change `BROADCAST_CAPACITY` from 64 to 1024**

In `src/broker.rs`, line 4:

```rust
pub const BROADCAST_CAPACITY: usize = 1024;
```

- [ ] **Step 2: Change parser event broadcast capacity from 256 to 1024**

In `src/parser/mod.rs`, line 63:

```rust
let (event_tx, _) = broadcast::channel(1024);
```

- [ ] **Step 3: Run existing tests to verify no regressions**

Run: `nix develop -c sh -c "cargo test"`
Expected: All tests pass. The capacity change is invisible to test assertions.

- [ ] **Step 4: Commit**

```bash
git add src/broker.rs src/parser/mod.rs
git commit -m "perf: bump broadcast channel capacities from 64/256 to 1024

Gives coalescing logic more headroom before Lagged errors fire.
Events are small (Bytes / enum variants), so 1024 slots is negligible memory."
```

---

### Task 2: Server-level WS forwarding task coalescing

The forwarding task is spawned inside `handle_server_ws_request()` at `src/api/handlers.rs:1925-1949`. Currently it reads from `parser.subscribe()` and does `tx.send(event).await`, blocking when the mpsc (capacity 256) is full. Replace with `try_send` + dirty flag + timer.

**Files:**
- Modify: `src/api/handlers.rs:1920-1949` — forwarding task
- Modify: `src/api/handlers.rs:2088-2097` — `SubHandle` insert (pass `interval_ms`)

- [ ] **Step 1: Write test for coalescing behavior**

Create a new test file `tests/event_coalescing.rs`. This test creates a test session with a large parser channel (to allow real burst without blocking the test task), hooks into the server-level WS, subscribes, then blasts parser events faster than the WS can drain. It asserts that the client receives Sync events (coalesced) rather than lagged notifications.

```rust
//! Integration tests for event coalescing under high-throughput output.
mod common;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// Helper: start an axum server on a random port, return the address.
async fn start_server(app: axum::Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// Helper: receive a JSON message from a WS stream with timeout.
async fn recv_json(
    rx: &mut futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) -> serde_json::Value {
    let timeout = Duration::from_secs(5);
    match tokio::time::timeout(timeout, rx.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected text message, got {:?}", other),
    }
}

/// Create a test state with a large parser channel to allow burst sending
/// without the test task blocking on the bounded parser_tx channel.
fn create_burst_test_state() -> (wsh::api::AppState, mpsc::Receiver<Bytes>, tokio::sync::broadcast::Sender<Bytes>, mpsc::Sender<Bytes>) {
    // Use a large parser channel (8192) so we can send thousands of lines
    // without the test task blocking — the default 256 is too small.
    let ts = common::create_test_session("test");
    let output_tx = ts.broker.sender();

    // Create a new parser with a large channel instead of the default 256
    let (large_parser_tx, large_parser_rx) = mpsc::channel(8192);
    let parser = wsh::parser::Parser::spawn(large_parser_rx, 80, 24, 1000);

    // Build session with the large-channel parser
    let session = wsh::session::Session {
        parser,
        ..ts.session
    };

    let registry = wsh::session::SessionRegistry::new();
    registry.insert(Some("test".into()), session).unwrap();
    let state = wsh::api::AppState {
        sessions: registry,
        shutdown: wsh::shutdown::ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(wsh::api::ServerConfig::new(false)),
        server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        ticket_store: std::sync::Arc::new(wsh::api::ticket::TicketStore::new()),
        backends: wsh::federation::registry::BackendRegistry::new(),
        federation: std::sync::Arc::new(tokio::sync::Mutex::new(wsh::federation::manager::FederationManager::new())),
        ip_access: None,
        hostname: "test".to_string(),
        federation_config_path: None,
        local_token: None,
        default_backend_token: None,
        server_id: "test-server-id".to_string(),
    };
    (state, ts.input_rx, output_tx, large_parser_tx)
}

/// Blast events through the parser and verify that the server-level WS
/// coalesces into Sync events rather than producing lagged notifications.
#[tokio::test]
async fn test_server_ws_coalescing_under_burst() {
    let (state, _input_rx, _output_tx, parser_tx) = create_burst_test_state();
    let app = wsh::api::router(state, wsh::api::RouterConfig::default());
    let addr = start_server(app).await;

    // Connect to the server-level WS
    let (ws, _) = connect_async(format!("ws://{}/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    let _ = recv_json(&mut rx).await; // connected

    // Subscribe with a short interval to trigger coalescing quickly
    tx.send(Message::Text(
        serde_json::json!({
            "method": "subscribe",
            "session": "test",
            "params": {
                "events": ["lines", "diffs"],
                "format": "plain",
                "interval_ms": 50
            }
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();

    // Consume subscribe response + initial sync
    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["method"], "subscribe");
    let sync = recv_json(&mut rx).await;
    assert_eq!(sync["event"], "sync");

    // Blast 5000 lines through the parser — way more than the mpsc can handle.
    // The large parser channel (8192) prevents the test task from blocking.
    for i in 0..5000 {
        let line = format!("line {}\r\n", i);
        let _ = parser_tx.send(Bytes::from(line)).await;
    }

    // Collect events for 2 seconds
    let mut sync_count = 0u32;
    let mut lagged_count = 0u32;
    let mut event_count = 0u32;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), rx.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let json: serde_json::Value = serde_json::from_str(&text).unwrap();
                event_count += 1;
                if json.get("event") == Some(&serde_json::json!("sync")) {
                    sync_count += 1;
                }
                if json.get("type") == Some(&serde_json::json!("lagged")) {
                    lagged_count += 1;
                }
            }
            _ => break,
        }
    }

    // Coalescing should produce Sync snapshots. The non-blocking drain
    // prevents broadcast overflow, so lagged_count should be zero.
    assert!(
        sync_count >= 1,
        "expected at least 1 coalesced sync event, got {sync_count} (total events: {event_count})"
    );
    assert!(
        lagged_count == 0,
        "expected no lagged notifications with coalescing, got {lagged_count}"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `nix develop -c sh -c "cargo test test_server_ws_coalescing_under_burst -- --nocapture"`
Expected: FAIL — either lagged_count > 0 or the test hangs because the forwarding task blocks.

- [ ] **Step 3: Implement coalescing in the forwarding task**

In `src/api/handlers.rs`, replace the forwarding task (lines 1920-1949) with the coalescing version. The changes are:

1. Capture additional values before the `tokio::spawn`:

Replace lines 1920-1924 (the captures before `tokio::spawn`):

```rust
                let mut events = Box::pin(session.parser.subscribe());
                let tx = sub_tx.clone();
                let shared_name = std::sync::Arc::new(parking_lot::Mutex::new(session_name.clone()));
                let task_name = shared_name.clone();
                let cancelled = session.cancelled.clone();
                let task_parser = session.parser.clone();
                let task_format = params.format;
                let task_interval = std::time::Duration::from_millis(params.interval_ms.max(1));
```

2. Replace the `tokio::spawn` body (lines 1925-1949):

```rust
                let task = tokio::spawn(async move {
                    let mut dirty = false;
                    let mut coalesce_timer = tokio::time::interval(task_interval);
                    coalesce_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    // Don't fire immediately — only after dirty is set
                    coalesce_timer.tick().await;

                    loop {
                        tokio::select! {
                            event = events.next() => {
                                match event {
                                    Some(e) => {
                                        if dirty {
                                            // Draining broadcast to stay current; will sync on timer
                                            continue;
                                        }
                                        let current_name = task_name.lock().clone();
                                        match tx.try_send(TaggedSessionEvent {
                                            session: current_name,
                                            event: e,
                                        }) {
                                            Ok(()) => {}
                                            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                                dirty = true;
                                            }
                                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                                        }
                                    }
                                    None => break,
                                }
                            }

                            _ = coalesce_timer.tick(), if dirty => {
                                // Query parser for current screen snapshot
                                if let Ok(Ok(crate::parser::state::QueryResponse::Screen(screen))) =
                                    tokio::time::timeout(
                                        std::time::Duration::from_secs(10),
                                        task_parser.query(crate::parser::state::Query::Screen {
                                            format: task_format,
                                        }),
                                    ).await
                                {
                                    let scrollback_lines = screen.total_lines;
                                    let sync_event = crate::parser::SubscriptionEvent::Event(
                                        crate::parser::events::Event::Sync {
                                            seq: 0,
                                            screen,
                                            scrollback_lines,
                                        },
                                    );
                                    let current_name = task_name.lock().clone();
                                    match tx.try_send(TaggedSessionEvent {
                                        session: current_name,
                                        event: sync_event,
                                    }) {
                                        Ok(()) => {
                                            dirty = false;
                                            coalesce_timer.reset();
                                        }
                                        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                            // Still backed up, retry next tick
                                        }
                                        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                                    }
                                }
                            }

                            _ = cancelled.cancelled() => break,
                        }
                    }
                });
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `nix develop -c sh -c "cargo test test_server_ws_coalescing_under_burst -- --nocapture"`
Expected: PASS — sync_count >= 1, lagged_count == 0.

- [ ] **Step 5: Run full test suite**

Run: `nix develop -c sh -c "cargo test"`
Expected: All existing tests pass. The coalescing is transparent — under light load, events still forward individually.

- [ ] **Step 6: Commit**

```bash
git add src/api/handlers.rs tests/event_coalescing.rs
git commit -m "feat: coalesce events in server-level WS forwarding task

Replace blocking mpsc::send().await with try_send() + dirty flag +
periodic timer. Under normal throughput, events forward individually.
Under pressure, the task drains the broadcast to stay current and
sends periodic Sync snapshots at interval_ms intervals."
```

---

### Task 3: Fix Format::default() in Lagged recovery paths

Both the per-session and server-level WS handlers use `Format::default()` when building a Sync after a Lagged error. This ignores the subscriber's requested format (plain vs styled).

**Files:**
- Modify: `src/api/handlers.rs:460` — per-session Lagged recovery
- Modify: `src/api/handlers.rs:1285` — server-level Lagged recovery

- [ ] **Step 1: Fix per-session Lagged recovery format**

The per-session handler stores the subscriber's format in `sub_format` (line 641), but this variable is scoped inside the `if req.method == "subscribe"` block. We need a persistent format variable.

Add a new local variable after `subscribed_types` (around line 379):

```rust
    let mut subscribed_types: Vec<crate::parser::events::EventType> = Vec::new();
    let mut subscribe_format = crate::parser::state::Format::default();
```

In the subscribe handler (line 641), after `let sub_format = params.format;`, add:

```rust
                                    subscribe_format = sub_format;
```

In the Lagged recovery (line 460), replace `Format::default()` with the subscriber's format:

```rust
                            session.parser.query(crate::parser::state::Query::Screen {
                                format: subscribe_format,
                            }),
```

- [ ] **Step 2: Fix server-level Lagged recovery format**

The server-level handler stores `SubHandle` per session. Add a `format` field to `SubHandle`:

In the `SubHandle` struct (around line 950), add:

```rust
    format: crate::parser::state::Format,
```

In the `sub_handles.insert()` call (around line 2088), add:

```rust
                    SubHandle {
                        subscribed_types: subscribed_types.clone(),
                        task,
                        activity_task,
                        _client_guard: session.connect(),
                        shared_name,
                        idle_timeout_ms: params.idle_timeout_ms,
                        format: params.format,
                    },
```

In the Lagged recovery (line 1285), look up the subscriber's format from `sub_handles`:

```rust
                        // Look up subscriber format
                        let sub_format = sub_handles
                            .get(&tagged.session)
                            .map(|h| h.format)
                            .unwrap_or_default();
```

Then use it in the query:

```rust
                                session.parser.query(crate::parser::state::Query::Screen {
                                    format: sub_format,
                                }),
```

- [ ] **Step 3: Run tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/api/handlers.rs
git commit -m "fix: use subscriber's format in Lagged recovery paths

Both per-session and server-level WS handlers were using
Format::default() when building Sync after Lagged errors,
ignoring the subscriber's requested format (plain vs styled)."
```

---

## Chunk 2: Per-Session WS Coalescing + Web Client

### Task 4: Per-session WS drain task + coalescing

The per-session WS handler (`handle_ws_json`) sends directly to `ws_tx` via the `ws_send!` macro. To detect backpressure, introduce a bounded mpsc between the handler loop and the WS sink.

**Files:**
- Modify: `src/api/handlers.rs:356-414` — `handle_ws_json` setup
- Modify: `src/api/handlers.rs:416-477` — parser event handling in select!

- [ ] **Step 1: Write test for per-session WS coalescing**

Add to `tests/event_coalescing.rs`:

```rust
/// Blast events through the parser and verify that the per-session WS
/// coalesces into Sync events rather than producing lagged notifications.
#[tokio::test]
async fn test_per_session_ws_coalescing_under_burst() {
    let (state, _input_rx, _output_tx, parser_tx) = create_burst_test_state();
    let app = wsh::api::router(state, wsh::api::RouterConfig::default());
    let addr = start_server(app).await;

    // Connect to the per-session WS
    let (ws, _) = connect_async(format!("ws://{}/sessions/test/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    let _ = recv_json(&mut rx).await; // connected

    // Subscribe with short interval
    tx.send(Message::Text(
        serde_json::json!({
            "method": "subscribe",
            "params": {
                "events": ["lines", "diffs"],
                "format": "plain",
                "interval_ms": 50
            }
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["method"], "subscribe");
    let sync = recv_json(&mut rx).await;
    assert_eq!(sync["event"], "sync");

    // Blast 5000 lines
    for i in 0..5000 {
        let line = format!("line {}\r\n", i);
        let _ = parser_tx.send(Bytes::from(line)).await;
    }

    // Collect events for 2 seconds
    let mut sync_count = 0u32;
    let mut lagged_count = 0u32;
    let mut event_count = 0u32;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), rx.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let json: serde_json::Value = serde_json::from_str(&text).unwrap();
                event_count += 1;
                if json.get("event") == Some(&serde_json::json!("sync")) {
                    sync_count += 1;
                }
                if json.get("type") == Some(&serde_json::json!("lagged")) {
                    lagged_count += 1;
                }
            }
            _ => break,
        }
    }

    assert!(
        sync_count >= 1,
        "expected at least 1 coalesced sync event, got {sync_count} (total events: {event_count})"
    );
    assert!(
        lagged_count == 0,
        "expected no lagged notifications with coalescing, got {lagged_count}"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop -c sh -c "cargo test test_per_session_ws_coalescing_under_burst -- --nocapture"`
Expected: FAIL — lagged_count > 0 since the per-session path has no coalescing yet.

- [ ] **Step 3: Add bounded mpsc + drain task to handle_ws_json**

In `handle_ws_json()`, after the WebSocket split (line 366), replace the direct `ws_tx` usage with an mpsc channel + drain task.

**IMPORTANT:** The drain task takes ownership of `ws_tx` (moved into the `async move` block). This means the close frame (line 885) and all other `ws_send!` calls in the handler must go through the mpsc channel, not `ws_tx` directly.

After `let (mut ws_tx, mut ws_rx) = socket.split();` (line 366), add:

```rust
    // Bounded mpsc for backpressure-aware sending. The handler loop writes
    // to ws_mpsc_tx; a drain task reads from ws_mpsc_rx and writes to ws_tx.
    // The drain task owns ws_tx — when ws_mpsc_tx is dropped, the drain task
    // will flush remaining messages and send the close frame.
    let (ws_mpsc_tx, mut ws_mpsc_rx) = tokio::sync::mpsc::channel::<Message>(256);
    let drain_task = tokio::spawn(async move {
        while let Some(msg) = ws_mpsc_rx.recv().await {
            match tokio::time::timeout(WS_SEND_TIMEOUT, ws_tx.send(msg)).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => break,
                Err(_) => {
                    tracing::debug!("ws_json drain task: send timed out, closing");
                    break;
                }
            }
        }
        // Send close frame before exiting (with timeout to avoid blocking on dead connections)
        let close_frame = axum::extract::ws::CloseFrame {
            code: axum::extract::ws::close_code::NORMAL,
            reason: "session ended".into(),
        };
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ws_tx.send(Message::Close(Some(close_frame))),
        ).await;
    });
```

Now replace the `ws_send!` macro definition (lines 401-413). The new macro sends through the mpsc with a timeout matching `WS_SEND_TIMEOUT` to prevent the handler from blocking indefinitely if the mpsc fills up:

```rust
    /// Send a WebSocket message via the drain channel with timeout.
    /// Breaks out of the enclosing loop on closed channel or timeout.
    macro_rules! ws_send {
        ($tx:expr, $msg:expr) => {
            match tokio::time::timeout(WS_SEND_TIMEOUT, $tx.send($msg)).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => break,
                Err(_) => {
                    tracing::debug!("ws_json mpsc send timed out, closing");
                    break;
                }
            }
        };
    }
```

Replace all `ws_send!(ws_tx, ...)` with `ws_send!(ws_mpsc_tx, ...)` in the handler. Search-and-replace: `ws_send!(ws_tx,` → `ws_send!(ws_mpsc_tx,`.

Replace the close frame + cleanup code at the end of the function (lines 878-891). Since the drain task now handles the close frame, the handler just needs to drop `ws_mpsc_tx` and await the drain task:

```rust
    // Drop the mpsc sender to signal the drain task to flush and close.
    drop(ws_mpsc_tx);
    // Wait for drain task to finish sending close frame (with timeout).
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), drain_task).await;

    // Clean up activity subscription task
    if let Some(handle) = activity_sub_handle {
        handle.abort();
    }
```

- [ ] **Step 4: Add coalescing to the parser event branch**

Add persistent coalescing state after `subscribe_format` (the new variable from Task 3):

```rust
    let mut coalesce_dirty = false;
    let mut coalesce_interval_ms: u64 = 100; // updated on subscribe
    let mut coalesce_timer = tokio::time::interval(std::time::Duration::from_millis(100));
    coalesce_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    coalesce_timer.tick().await; // consume initial tick
```

In the subscribe handler (around line 639), after `params.interval_ms = params.interval_ms.min(MAX_WAIT_CEILING_MS);`, add:

```rust
                                    coalesce_interval_ms = params.interval_ms.max(1);
                                    coalesce_timer = tokio::time::interval(
                                        std::time::Duration::from_millis(coalesce_interval_ms),
                                    );
                                    coalesce_timer.set_missed_tick_behavior(
                                        tokio::time::MissedTickBehavior::Skip,
                                    );
                                    coalesce_timer.tick().await;
                                    coalesce_dirty = false;
```

Replace the parser event branch in the select loop (lines 419-477). Instead of sending directly via `ws_send!` for parser events, use `try_send` on `ws_mpsc_tx`:

```rust
            sub_event = events.next() => {
                match sub_event {
                    Some(crate::parser::SubscriptionEvent::Event(event)) if !subscribed_types.is_empty() => {
                        if coalesce_dirty {
                            // Draining broadcast to stay current; will sync on timer
                            continue;
                        }
                        let should_send = match &event {
                            crate::parser::events::Event::Line { .. } => {
                                subscribed_types.contains(&EventType::Lines)
                            }
                            crate::parser::events::Event::Cursor { .. } => {
                                subscribed_types.contains(&EventType::Cursor)
                            }
                            crate::parser::events::Event::Mode { .. } => {
                                subscribed_types.contains(&EventType::Mode)
                            }
                            crate::parser::events::Event::Diff { .. } => {
                                subscribed_types.contains(&EventType::Diffs)
                            }
                            crate::parser::events::Event::Reset { .. }
                            | crate::parser::events::Event::Sync { .. } => true,
                            crate::parser::events::Event::Idle { .. }
                            | crate::parser::events::Event::Running { .. } => {
                                subscribed_types.contains(&EventType::Activity)
                            }
                        };

                        if should_send {
                            if let Ok(json) = serde_json::to_string(&event) {
                                match ws_mpsc_tx.try_send(Message::Text(json.into())) {
                                    Ok(()) => {}
                                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                        coalesce_dirty = true;
                                    }
                                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                                }
                            }
                        }
                    }
                    Some(crate::parser::SubscriptionEvent::Lagged(n)) => {
                        tracing::warn!(skipped = n, "parser event subscriber lagged");
                        let lag_msg = serde_json::json!({"type": "lagged", "skipped": n});
                        if let Ok(json) = serde_json::to_string(&lag_msg) {
                            ws_send!(ws_mpsc_tx, Message::Text(json.into()));
                        }
                        // After lag, push a full sync so the client can recover.
                        if let Ok(Ok(crate::parser::state::QueryResponse::Screen(screen))) = tokio::time::timeout(
                            std::time::Duration::from_secs(10),
                            session.parser.query(crate::parser::state::Query::Screen {
                                format: subscribe_format,
                            }),
                        ).await {
                            let scrollback_lines = screen.total_lines;
                            let sync_event = crate::parser::events::Event::Sync {
                                seq: 0,
                                screen,
                                scrollback_lines,
                            };
                            if let Ok(json) = serde_json::to_string(&sync_event) {
                                ws_send!(ws_mpsc_tx, Message::Text(json.into()));
                            }
                        }
                    }
                    None => break,
                    _ => {} // No subscription active, discard
                }
            }
```

Add a new coalesce timer branch to the select loop (alongside the existing ping_interval and activity branches):

```rust
            _ = coalesce_timer.tick(), if coalesce_dirty => {
                if let Ok(Ok(crate::parser::state::QueryResponse::Screen(screen))) =
                    tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        session.parser.query(crate::parser::state::Query::Screen {
                            format: subscribe_format,
                        }),
                    ).await
                {
                    let scrollback_lines = screen.total_lines;
                    let sync_event = crate::parser::events::Event::Sync {
                        seq: 0,
                        screen,
                        scrollback_lines,
                    };
                    if let Ok(json) = serde_json::to_string(&sync_event) {
                        match ws_mpsc_tx.try_send(Message::Text(json.into())) {
                            Ok(()) => {
                                coalesce_dirty = false;
                                coalesce_timer.reset();
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                // Still backed up, retry next tick
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                        }
                    }
                }
            }
```

- [ ] **Step 5: Run tests**

Run: `nix develop -c sh -c "cargo test test_per_session_ws_coalescing_under_burst -- --nocapture"`
Expected: PASS

Run: `nix develop -c sh -c "cargo test"`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/api/handlers.rs tests/event_coalescing.rs
git commit -m "feat: coalesce events in per-session WS handler

Introduce bounded mpsc between handler loop and WS sink for
backpressure detection. Parser events use try_send() + dirty flag +
periodic timer, matching the server-level WS coalescing pattern.
Input events, activity changes, and other low-frequency messages
continue sending through the mpsc normally."
```

---

### Task 5: Web client interval_ms

**Files:**
- Modify: `web/src/api/ws.ts:516`

- [ ] **Step 1: Add interval_ms to subscribe params**

In `web/src/api/ws.ts`, line 516, change:

```typescript
    const params: Record<string, unknown> = { events, format: "styled" };
```

to:

```typescript
    const params: Record<string, unknown> = { events, format: "styled", interval_ms: 16 };
```

- [ ] **Step 2: Commit**

```bash
git add web/src/api/ws.ts
git commit -m "feat(web): set interval_ms: 16 for ~60fps coalesced updates

Tells the server to coalesce events at 16ms intervals when the
WebSocket can't keep up with high-throughput terminal sessions."
```

---

## Chunk 3: Documentation

### Task 6: Update API documentation

**Files:**
- Modify: `docs/api/websocket.md:172` — `interval_ms` description
- Modify: `docs/api/websocket.md` — add coalescing section after "Idle Sync Subscription"
- Modify: `docs/api/README.md` — update subscribe example if present

- [ ] **Step 1: Update interval_ms description in websocket.md**

In `docs/api/websocket.md`, line 172, replace:

```
| `interval_ms` | integer | `100` | Minimum interval between events (ms) |
```

with:

```
| `interval_ms` | integer | `100` | Event coalescing interval (ms). When the server can't deliver events fast enough, it switches to sending periodic `sync` snapshots at this interval instead of individual events. Lower values give smoother updates; `16` is recommended for UI clients (~60fps). |
```

- [ ] **Step 2: Add Event Coalescing section**

In `docs/api/websocket.md`, after the "Idle Sync Subscription" section (after line 401), add:

```markdown
### Event Coalescing

When a terminal session generates output faster than the WebSocket can deliver
events, the server automatically switches from sending individual events to
sending periodic `sync` snapshots. This prevents the event buffer from
overflowing and ensures the client always converges to the correct state.

**How it works:**

1. Under normal throughput, events are delivered individually with no added latency.
2. When the internal buffer fills up, the server stops forwarding individual events
   and sets a "dirty" flag.
3. Every `interval_ms` milliseconds, if the flag is set, the server queries the
   terminal for a full screen snapshot and sends it as a `sync` event.
4. Once the buffer drains, individual event delivery resumes automatically.

**Recommended `interval_ms` values:**

| Use case | Value | Notes |
|----------|-------|-------|
| UI rendering | `16` | ~60fps, smooth visual updates |
| Agent monitoring | `100` (default) | Good balance of responsiveness and efficiency |
| Logging / audit | `500`–`1000` | Lower overhead, periodic snapshots sufficient |

Clients should already handle `sync` events (they're sent on initial subscribe
and after idle timeouts). No client-side changes are needed to benefit from
coalescing — it happens transparently on the server.
```

- [ ] **Step 3: Commit**

```bash
git add docs/api/websocket.md
git commit -m "docs: document event coalescing and interval_ms semantics"
```

---

### Task 7: Update skills

**Files:**
- Modify: `skills/wsh/core/SKILL.md` — "Real-Time Events" section (~line 126-149)
- Modify: `skills/wsh/monitor/SKILL.md` — "Event Subscription" section (~line 32-43)

- [ ] **Step 1: Update core skill**

In `skills/wsh/core/SKILL.md`, replace lines 126-149 (from "### Real-Time Events" through "controlled by `idle_timeout_ms`).") with the following. **Preserve lines 150-160** (session-switching example and WS method list) — they remain unchanged.

```markdown
### Real-Time Events (WebSocket)
For monitoring and input capture, you need real-time event
streaming. Connect to the JSON WebSocket:

    websocat ws://localhost:8080/sessions/default/ws/json

After connecting, subscribe to the events you care about:

    {"id": 1, "method": "subscribe", "params": {
      "events": ["lines", "input"],
      "format": "plain",
      "idle_timeout_ms": 1000
    }}

Available event types:
- `lines` — new lines of output
- `cursor` — cursor movement
- `mode` — alternate screen toggled
- `diffs` — batched screen changes
- `input` — keyboard input (essential for input capture)

The server pushes events as they happen. It also sends
periodic `sync` snapshots when the terminal goes idle
(controlled by `idle_timeout_ms`).

Under high output, the server coalesces events automatically:
instead of individual updates, you get periodic `sync`
snapshots at `interval_ms` intervals (default 100ms). This
is transparent — handle `sync` events the same way you
handle the initial sync after subscribing.
```

- [ ] **Step 2: Update monitor skill**

In `skills/wsh/monitor/SKILL.md`, replace the "Event Subscription (Real-Time)" section (lines 32-43) with:

```markdown
### Event Subscription (Real-Time)
Subscribe to real-time events via the WebSocket (see the
core skill for connection mechanics). Subscribe to the
events you care about — `lines` for output, `input` for
keystrokes — and the server pushes them as they happen.

You also get periodic `sync` snapshots when the terminal
goes quiet, giving you a natural checkpoint to analyze
the current state.

Under heavy output (e.g. build logs, test runners), the
server may coalesce events — delivering periodic `sync`
snapshots instead of individual line updates. Your code
should handle `sync` events to stay in sync regardless
of output volume.

For most monitoring tasks, **start with polling**. Move to
event subscription when you need immediate reaction time.
```

- [ ] **Step 3: Run a build to ensure skill `include_str!` still works**

Run: `nix develop -c sh -c "cargo build"`
Expected: Builds successfully. Skills are embedded at compile time via `include_str!`.

- [ ] **Step 4: Commit**

```bash
git add skills/wsh/core/SKILL.md skills/wsh/monitor/SKILL.md
git commit -m "docs: update skills with event coalescing behavior

Core skill: add note about automatic coalescing under high output.
Monitor skill: add guidance on handling sync events during heavy output."
```

---

### Task 8: Final verification

- [ ] **Step 1: Run full test suite**

Run: `nix develop -c sh -c "cargo test"`
Expected: All tests pass.

- [ ] **Step 2: Run clippy**

Run: `nix develop -c sh -c "cargo clippy -- -D warnings"`
Expected: No warnings.

- [ ] **Step 3: Build web frontend**

Run: `cd /home/ajsyp/Projects/deepgram/wsh/web && npm run build`
Expected: Builds successfully.
