# Event Coalescing for WebSocket Subscriptions

## Problem

High-throughput terminal sessions (e.g. Claude Code) generate parser events
faster than WebSocket subscribers can consume them. The parser event broadcast
channel (capacity 256) overflows, producing `Lagged` errors. The recovery path
sends a full `Sync` snapshot after each lag, which is expensive to serialize and
send, causing further lag — a cascade of lag -> sync -> lag -> sync.

The forwarding task between the parser broadcast and the WS handler blocks on
`mpsc::send().await` when the WS can't flush fast enough. While blocked, the
broadcast receiver falls behind and eventually hits `Lagged`.

## Solution

Two complementary changes:

1. **Bump broadcast buffer capacities** — cheap headroom.
2. **Event coalescing in forwarding tasks** — drain the broadcast without
   blocking; when downstream can't keep up, accumulate a dirty flag and
   periodically flush a single `Sync` snapshot instead of individual events.

## Design

### Broadcast Capacity Bump

| Channel | Current | New |
|---------|---------|-----|
| `BROADCAST_CAPACITY` (broker, raw PTY output) | 64 | 1024 |
| Parser event broadcast (`src/parser/mod.rs`) | 256 | 1024 |

Events are small (`Bytes` / enum variants). 1024 slots is negligible memory.

### Coalescing: Server-Level WS Forwarding Task

The per-session forwarding task (spawned in `handle_server_ws_json` at
`src/api/handlers.rs:1925`) currently does:

```
loop { event = broadcast.next() -> mpsc.send(event).await }
```

Replacing with a three-branch `select!`:

1. **Parser event arrives** — drain it immediately. Use `tx.try_send()` to
   forward. If `TrySendError::Full`, set `dirty = true` and drop the event
   (the periodic sync will recover state). If `Closed`, break.

2. **Timer tick** (every `interval_ms`, guarded by `if dirty`) — query the
   parser for a `Screen` snapshot, send a `Sync` event through the mpsc via
   `try_send`. On success, clear `dirty` and `timer.reset()`. On `Full`,
   leave `dirty` set and retry next tick.

3. **Cancellation** — exit.

The `interval_ms` comes from `SubscribeParams` (default 100ms, already parsed
and clamped). It needs to be passed into the spawned task alongside the existing
`parser` clone and `format`.

Key properties:
- The broadcast receiver **never blocks**, so it stays current and avoids `Lagged`.
- Under normal throughput, events forward individually with no added latency.
- Under pressure, clients get periodic `Sync` snapshots at a bounded rate.
- `MissedTickBehavior::Skip` ensures the timer doesn't pile up during sustained
  pressure.

### Coalescing: Per-Session WS Path

The per-session WS handler (`handle_ws_json`) reads parser events inline in its
`select!` loop. It doesn't have the forwarding task / mpsc indirection — events
go directly to `ws_tx.send()`.

To get backpressure detection (equivalent to `try_send` on the mpsc), introduce
a **bounded mpsc between the handler loop and the WS sink**:

- Spawn a small drain task that reads from the mpsc and writes to `ws_tx`.
- The handler loop writes to the mpsc with `try_send`.
- Same dirty flag + timer pattern as the server-level path.

This makes both paths structurally identical and gives the per-session path the
same `try_send` backpressure signal.

### Web Client

- Add `interval_ms: 16` to the subscribe params in `web/src/api/ws.ts:516`
  for ~60fps coalesced updates.
- No other client changes needed — the client already handles `Sync` events.
- The existing `lagged` log-and-discard handler remains as a fallback.

### Existing `Lagged` Handling

All existing `Lagged` match arms in both WS paths remain. They serve as a
fallback for edge cases where the broadcast overflows despite the larger buffer
(e.g., extremely long pauses in the forwarding task). With coalescing active,
these should fire rarely or never.

## Files Changed

| File | Change |
|------|--------|
| `src/broker.rs` | `BROADCAST_CAPACITY`: 64 -> 1024 |
| `src/parser/mod.rs` | Parser event broadcast: 256 -> 1024 |
| `src/api/handlers.rs` | Server WS forwarding task: `try_send` + dirty flag + timer |
| `src/api/handlers.rs` | Per-session WS handler: bounded mpsc to WS sink + coalescing |
| `web/src/api/ws.ts` | Subscribe call: add `interval_ms: 16` |

## Testing

1. **Coalescing unit test** — small mpsc (capacity 2-3), blast parser events,
   assert: broadcast never `Lagged`, consumer receives mix of individual events
   and `Sync` events, syncs arrive at roughly `interval_ms` intervals.

2. **Per-session WS burst test** — connect WS with `interval_ms: 50`, generate
   burst PTY output (`seq 1 10000`), verify: no `lagged` notifications, final
   screen state correct via `Sync`.

3. **Server-level WS burst test** — same through multiplexed server WS.

## Documentation Updates

| File | Update |
|------|--------|
| `docs/api/websocket.md` | Document `interval_ms` coalescing semantics: what it does, how `Sync` replaces granular events under pressure, recommended values, interaction with `idle_timeout_ms` |
| `docs/api/README.md` | Update subscribe example to show `interval_ms` |
| `skills/wsh/core/SKILL.md` | Update "Real-Time Events" section: high-throughput sessions may deliver periodic `Sync` snapshots instead of individual events |
| `skills/wsh/monitor/SKILL.md` | Update event subscription section: latency implications of coalescing for monitoring patterns |

## Non-Changes

- `skills/wsh/core-mcp/SKILL.md` — MCP is request/response, no streaming.
- `skills/wsh/input-capture/SKILL.md` — input events are discrete, unaffected.
- `src/mcp/tools.rs` — MCP tool parameters unchanged.
- No protocol changes. No new event types. No breaking changes.
- Clients that don't send `interval_ms` get the default (100ms).
