# Server Model Documentation Design

**Date:** 2026-03-23
**Output:** `docs/SERVER_MODEL.md`
**Audience:** AI coding agents and developers implementing similar client/server terminal architectures in other projects.

## Purpose

Document the `wsh` server process lifecycle and communication architecture with enough detail — both design patterns and concrete implementation techniques — that another agent or developer can reproduce this architecture in an unrelated project.

## Document Structure

### Section 1: Overview (~200 words)

Establish the core mental model:

- Client/server architecture where the daemon owns all state (PTY sessions, terminal state, overlays, panels) and clients are disposable thin connections.
- Why: sessions survive client disconnects, multiple clients share sessions, server can run headless for API-only access.
- Two modes: **dedicated** (`wsh server`) for persistent multi-session operation, **ephemeral** (auto-spawned) for transparent single-user experience.
- Communication is unified: HTTP-over-UDS is always available, TCP is opt-in. The same API serves both transports with transport-aware middleware for security differences.
- The doc covers both design patterns and concrete implementation techniques.

### Section 2: Server Lifecycle

The meatiest section. Covers the full life of a server process.

**2.1 Startup Sequence** — Ordered steps:
1. Validate configuration (base prefix format, TLS cert/key pairing)
2. Resolve security settings: token generation/validation, rate limiting defaults
3. Initialize shared state (session registry, shutdown coordinator, server config)
4. Acquire instance lock (`flock(LOCK_EX | LOCK_NB)`) — fail fast if another instance holds it
5. Bind UDS listener (remove stale socket, bind, chmod 0600)
6. Optionally bind TCP listener (with auth/CORS/rate-limit middleware)
7. If ephemeral: spawn the ephemeral monitor task
8. Enter main serving loop

**2.2 Dedicated Mode** (`wsh server`):
- Persistent by default — stays alive with zero sessions
- Explicit startup, explicit shutdown (Ctrl+C, SIGTERM, or `wsh stop`)
- Runtime persistence toggle via `wsh persist on/off` (atomic bool, immediate effect in ephemeral monitor)

**2.3 Ephemeral Mode** (`wsh server --ephemeral`):
- Auto-spawned by clients; exits when no longer needed
- **Interest** = sessions exist OR active persistent connections (WebSocket, MCP WS) are held
- Two-phase monitoring:
  - **Phase 1 (orphan guard):** 30-second idle timeout. Shut down if no interest materializes. Prevents orphaned daemons when spawning client crashes before creating a session.
  - **Phase 2 (normal monitoring):** Wait for interest to drain to zero. Wakes on session events AND connection count changes (via `interest_changed` notify). On a `Destroyed` event, checks interest immediately (no grace). On event lag (rapid churn), applies a 2-second grace period before rechecking (handles destroy-then-create sequences).
- Checks persistence toggle every iteration — `wsh persist on` converts ephemeral to persistent at runtime.

**2.4 Graceful Shutdown** — Ordered teardown:
1. Stop accepting new connections (cancel HTTP listeners)
2. Remove UDS socket file immediately (prevents new connection attempts)
3. Signal existing WebSocket handlers to close (ShutdownCoordinator)
4. Wait up to 5s total for connections to drain (includes a 100ms post-drain grace period; the 100ms is inside the 5s timeout, not additive)
5. Shut down federation backend connections
6. Drain sessions: SIGHUP child processes, schedule SIGKILL after 2s
7. Await server tasks with 5s timeout
8. Wait for SIGKILL escalation if sessions were drained

Each subsection includes design rationale (why this order, why these timeouts, what races are prevented).

### Section 3: Auto-Spawn Protocol

How clients transparently get a server when none is running.

**3.1 Detection:** Client creates a `UdsHttpClient` targeting the HTTP socket (`<name>.http.sock`) and sends `GET /health`. Socket file missing or connection refused → no server.

**3.2 Race Prevention** — Spawn lock protocol:
1. Acquire spawn lock (`<name>.spawn.lock`): first try 50 non-blocking attempts at 100ms intervals (5s bounded), then fall back to a single blocking `flock` call (unbounded worst case, but in practice the lock is held only briefly)
2. Re-check health after lock (another client may have spawned while we waited)
3. If still no server, spawn daemon
4. If server appeared during lock wait, use it

Why separate from instance lock: instance lock is held by the server for its lifetime. Clients blocking on it would wait for server exit. Spawn lock coordinates clients with each other, not with the server.

**3.3 Daemon Spawning** — Concrete technique:
- Fork current executable with `server --ephemeral --socket <path> --server-name <name>`
- Detach: null stdin/stdout/stderr, `process_group(0)` for new process group
- Reaper thread to `waitpid()` the child (prevent zombie)
- Server survives parent exit and SIGHUP

**3.4 Connection Wait:**
- Poll `GET /health` over UDS every 50ms, 5s timeout

**3.5 Full Client Flow** (default `wsh` command):
1. Resolve socket path
2. Health check → auto-spawn if needed
3. `POST /sessions` to create session
4. Open WebSocket to `/sessions/{name}/ws/json` over UDS
5. Protocol handshake: read `{"connected": true}` initial message, then send `subscribe` with event types (e.g., `["output", "overlay"]`)
6. Bidirectional streaming: local TTY ↔ WebSocket ↔ PTY
7. On disconnect: clean up terminal state

### Section 4: Communication Architecture

**4.1 Dual-Transport Design:**
- UDS (always): `core_router()` — no auth, no rate limiting, no CORS, no base prefix. Filesystem permissions (0600) are the security boundary.
- TCP (opt-in): `router()` — same API routes wrapped with auth, rate limiting, CORS, base prefix.
- Both share same `AppState` and handler functions. Transport middleware injects connection metadata so handlers can distinguish origin.

**4.2 HTTP-over-UDS** — The key unifying decision:
- Standard HTTP/1.1 served over Unix socket (hyper accepts from `UnixListener` instead of `TcpListener`)
- Benefits: one API implementation, standard HTTP tooling works (`curl --unix-socket`), WebSocket upgrade works identically
- Socket path uses `.http.sock` extension to distinguish it as an HTTP-speaking socket (a legacy `<name>.sock` path exists in the codebase for the former binary protocol but is no longer created by the server)

**4.3 TCP Binding & Authentication:**
- Secondary IPv6 loopback: when binding `127.0.0.1`, attempts to also bind `[::1]` as a separate listener (best-effort; failure is non-fatal, only a debug log)
- TLS: manual accept loop with tokio-rustls; warning logged on non-loopback without TLS
- Auth resolution:
  - Loopback: no token required (trusted), but an Origin header check is applied for CSWSH protection on WebSocket upgrades
  - Non-loopback: token mandatory. `--token`/`WSH_TOKEN` (≥16 chars), auto-generated (32-char alphanumeric) if unspecified, or `--no-auth` to disable (warning logged)
  - Bearer token in `Authorization` header
  - `/auth/ws-ticket` for short-lived tickets (browsers can't set WS headers)

**4.4 WebSocket Overview:**
- Per-session: `/ws/raw` (binary PTY I/O) and `/ws/json` (JSON-RPC with subscriptions)
- Server-level: `/ws/json` (multiplexed, includes session lifecycle events)
- Lifecycle: upgrade → `{"connected": true}` → subscribe → stream → close
- Binary frames: raw PTY output / stdin input. Text frames: JSON method calls and events.
- Backpressure: bounded mpsc (256 slots) with event coalescing
- Keepalive: 30s ping interval, 10s timeout

**4.5 MCP Transport:**
- Streamable HTTP at `/mcp` (served on both UDS and TCP)
- Stdio bridge via `wsh mcp`: connects to `/mcp/ws` over UDS, bridges stdin/stdout ↔ WebSocket. The WebSocket connection registers a `ConnectionGuard`, which provides interest signaling for the ephemeral monitor (prevents premature shutdown while MCP stdio is active). In contrast, HTTP MCP sessions (Streamable HTTP) are deliberately excluded from interest tracking.

### Section 5: Discovery & Management

**5.1 Named Instances:**
- `-L`/`--server-name` (env: `WSH_SERVER_NAME`, default: `"default"`)
- File layout under `$XDG_RUNTIME_DIR/wsh/` (fallback: `/tmp/wsh-$USER/wsh/`):
  - `<name>.http.sock`, `<name>.lock`, `<name>.spawn.lock`
- `--socket` overrides instance name for custom paths
- All subcommands accept `-L`

**5.2 Discovery (`wsh info`):**
- `GET /server/info` over UDS
- Returns: instance name, hostname, version, server ID, socket path, TCP address (if bound), persistence mode, session count
- Enables tooling to discover connection parameters without hardcoding

**5.3 Server Control:**
- `wsh stop` — `POST /server/shutdown` over UDS, wait up to 10s for socket file to disappear
- `wsh persist on/off` — `PUT /server/persist`, toggles ephemeral↔persistent at runtime
- `wsh list` — `GET /sessions`, table of sessions with name, PID, command, size, client count

**5.4 Session Management CLI:**
- `wsh kill <name>` — `DELETE /sessions/{name}`
- `wsh attach <name>` — replay scrollback + screen, then stream via WebSocket
- All commands resolve socket path: `--socket` > `-L`/env > default

### Section 6: Security Model

**6.1 Transport-Based Trust:**
- UDS: trusted (filesystem permissions). TCP loopback: trusted. TCP non-loopback: untrusted (requires auth).

**6.2 Defense in Depth** for TCP:
1. Localhost by default (no TCP unless `--bind`)
2. Token authentication (mandatory non-loopback, auto-generated if unspecified)
3. TLS (`--tls-cert`/`--tls-key`, warning without it on non-loopback)
4. Rate limiting (default 100 req/s non-loopback)
5. IP access control (CIDR blocklist/allowlist via federation config)
6. Base prefix (`--base-prefix` for reverse proxy; `/health` stays at root)

**6.3 UDS-Only Endpoints:**
- `/server/shutdown` restricted to UDS — handlers check a `Transport` extension (injected by transport middleware) and reject non-UDS requests at the handler level, avoiding the need for separate routers

### Section 7: Implementation Guide

Concrete techniques for reproducing this architecture:

**7.1 Instance Locking:** `flock(LOCK_EX | LOCK_NB)` on dedicated lock file, kernel auto-release on exit/crash.

**7.2 Spawn Lock:** Separate file, blocking with bounded retry, acquire → re-check → spawn → release.

**7.3 Daemon Isolation:** `process_group(0)`, null stdio, reaper thread, `current_exe()` re-invocation.

**7.4 HTTP-over-UDS:** hyper/axum accept from `UnixListener`, enable `.with_upgrades()` for WebSocket, chmod 0600.

**7.5 Dual-Router Pattern:** Build routes once, wrap in transport-specific middleware, inject connection metadata.

**7.6 Ephemeral Lifecycle:** Composite interest signal, atomic counters with notification, RAII guards for connection counting, two-phase monitor, grace period on churn. Critical technique: TOCTOU-safe interest checking — register a `Notified` future *before* checking the interest predicate, so that changes between the check and the `select!` are not missed.

**7.7 Graceful Shutdown Ordering:** Stop accepting → remove socket → signal handlers → wait for drain → kill children. Separate cancellation tokens for "stop accepting" vs "close existing." SIGHUP → wait → SIGKILL escalation.

### Section 8: Federation Overview (~150 words)

- Backend registry, `POST /servers` for registration
- Persistent WebSocket connections between federated servers
- Config in `$XDG_CONFIG_HOME/wsh/config.toml`: hostname, IP access control, backend discovery
- Server ID (UUID) prevents routing loops
- Federation uses the same HTTP API — a federated server is just another API client
- Deliberately high-level; federation warrants its own document

## Key Principles

- **Lifecycle-first narrative:** Sections build on each other; readers understand *why* before *how*
- **Patterns + implementation:** Every design decision includes the concrete technique
- **Race-aware:** Explicit coverage of race conditions and their mitigations (spawn lock, ephemeral grace periods, atomic detach+remove)
- **Transport-agnostic API:** One set of handlers, transport-specific middleware
- **Reproducible:** An agent reading this doc should be able to implement the pattern in any language/framework
