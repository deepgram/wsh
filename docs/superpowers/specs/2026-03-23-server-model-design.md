# Server Model Documentation Design

**Date:** 2026-03-23
**Output:** `docs/SERVER_MODEL.md`
**Audience:** AI coding agents and developers implementing similar client/server terminal architectures in other projects.

## Purpose

Document the `wsh` server process lifecycle and communication architecture as a **blueprint for applying these patterns in new projects**. The document is not primarily an overview of `wsh` internals for their own sake — it teaches the general-purpose patterns (auto-spawn, locking, HTTP-over-UDS, ephemeral lifecycle, dual-transport) with enough detail that another agent or developer can reproduce this architecture in an unrelated project. Where `wsh`-specific features are referenced, they serve as concrete illustrations of the pattern, framed conditionally (e.g., "if your project manages multiple sessions, consider...").

## Framing Principle

Every `wsh`-specific detail should pass one test: **is this a universal pattern, or a `wsh`-specific concern?**

- **Universal patterns** (auto-spawn, locking, ephemeral lifecycle, HTTP-over-UDS, dual-router, graceful shutdown, interest tracking): describe directly as the pattern to implement.
- **Common-but-optional patterns** (session management, client enumeration, state replay on reconnect, persistence toggle): frame conditionally — "if your project needs X, here's how `wsh` solves it."
- **`wsh`-specific concerns** (PTY management, terminal state machine, overlay/panel rendering, scrollback): omit or mention only in passing as the domain-specific payload that rides on top of the infrastructure.

## Document Structure

### Section 1: Overview (~200 words)

Establish the core mental model:

- Client/server architecture where a daemon process owns all state and clients are disposable thin connections. In `wsh`, "state" means PTY sessions, terminal buffers, and visual overlays; in your project it could be anything that needs to survive client disconnects.
- Why this pattern: state survives client disconnects, multiple clients can share it, and the server can run headless for API-only access.
- Two modes: **dedicated** for persistent operation, **ephemeral** (auto-spawned) for transparent single-user experience where the server is an invisible implementation detail.
- Communication is unified: HTTP-over-UDS is always available, TCP is opt-in. The same API serves both transports with transport-aware middleware for security differences.
- The doc teaches general-purpose patterns using `wsh` as the reference implementation, with `wsh`-specific details framed conditionally.

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
6. Drain sessions: SIGHUP child processes, schedule SIGKILL after 3s
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

**3.5 Full Client Flow** — The general pattern for a client that auto-spawns and connects:
1. Resolve socket path from instance name or explicit override
2. Health check → auto-spawn if needed (steps 3.1–3.4)
3. Create a resource on the server (in `wsh`: `POST /sessions`; in your project: whatever your domain requires)
4. Open a WebSocket over UDS for bidirectional streaming
5. Protocol handshake: exchange an initial confirmation message, then subscribe to relevant event streams
6. Bidirectional streaming loop
7. On disconnect: clean up local state

The key insight: steps 1–2 are universal infrastructure (every client does them identically), while steps 3–7 carry domain-specific payload.

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

**4.4 WebSocket Overview** — Universal patterns for real-time bidirectional communication:
- **Connection lifecycle**: upgrade → initial handshake message → subscribe to event streams → bidirectional streaming → close frame. In `wsh`: `{"connected": true}` handshake, then `subscribe` with event types.
- **Binary vs text frames**: use binary frames for high-throughput opaque data (in `wsh`: raw PTY I/O), text frames for structured messages (JSON method calls and events).
- **Backpressure**: bounded channel between handler and send task (in `wsh`: 256 slots) with event coalescing to prevent slow clients from blocking the server.
- **Keepalive**: ping/pong at regular intervals (in `wsh`: 30s ping, 10s timeout) to detect dead connections.
- **Scoped vs multiplexed**: per-resource WebSockets (one connection per session) vs server-level multiplexed WebSockets (one connection, events tagged with resource ID). Choose based on your client patterns.

**4.5 Persistent Connections and Interest Tracking** — A critical pattern for ephemeral servers:
- Some clients need persistent connections (WebSocket, long-lived streams) that should keep an ephemeral server alive. Others (stateless HTTP requests) should not.
- The pattern: persistent connections register a `ConnectionGuard` (RAII) that increments the server's interest counter. Stateless connections do not.
- Example from `wsh`: The MCP stdio bridge (`wsh mcp`) connects via WebSocket and registers a guard — the server stays alive while the bridge is active. In contrast, Streamable HTTP MCP sessions are deliberately excluded from interest tracking because they are stateless request/response cycles.
- This distinction is essential: without it, an ephemeral server cannot know when it's safe to shut down.

### Section 5: Discovery & Management

**5.1 Named Instances** — Universal pattern for running multiple server instances on one machine:
- A `--server-name` flag (with env var fallback and a sensible default like `"default"`)
- Each instance gets its own file set under a well-known runtime directory (`$XDG_RUNTIME_DIR/<app>/`; fallback: `/tmp/<app>-$USER/`):
  - `<name>.http.sock`, `<name>.lock`, `<name>.spawn.lock`
- An explicit `--socket` flag overrides the name-derived path for custom socket placement
- All subcommands accept the instance name so they target the right server

**5.2 Discovery** — Pattern: a `GET /server/info` endpoint over UDS:
- Returns: instance name, hostname, version, server ID, socket path, TCP address (if bound), persistence mode, plus domain-specific state (in `wsh`: session count)
- Enables tooling and agents to discover connection parameters without hardcoding paths or ports

**5.3 Server Control** — Universal patterns:
- **Stop**: `POST /server/shutdown` over UDS, then poll for socket file disappearance as confirmation (in `wsh`: 10s timeout)
- **Persistence toggle**: `PUT /server/persist` to convert ephemeral↔persistent at runtime. Useful for promoting an auto-spawned ephemeral server when you want it to outlive current work.

**5.4 Domain-Specific Management** — Conditional patterns. Include these if your project needs them:
- **If your project manages multiple resources** (sessions, connections, jobs, etc.): a list endpoint (`GET /resources`) and a CLI command to display them. In `wsh`: `wsh list` shows sessions with PID, command, terminal size, and connected client count.
- **If clients need to reconnect to existing resources**: a reconnect/attach flow that replays state before entering the streaming loop. In `wsh`: `wsh attach` replays scrollback and screen contents, then resumes WebSocket streaming.
- **If resources can be destroyed individually**: a delete endpoint. In `wsh`: `wsh kill <name>` sends `DELETE /sessions/{name}`.

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

- **Blueprint, not tour:** The doc teaches patterns you can apply in new projects, not `wsh` internals for their own sake
- **Lifecycle-first narrative:** Sections build on each other; readers understand *why* before *how*
- **Patterns + implementation:** Every design decision includes the concrete technique
- **Conditional framing:** Domain-specific features (session management, state replay, resource listing) are framed as "if your project needs X" rather than presented as universal requirements
- **Race-aware:** Explicit coverage of race conditions and their mitigations (spawn lock, ephemeral grace periods, TOCTOU-safe interest checking)
- **Transport-agnostic API:** One set of handlers, transport-specific middleware
- **Reproducible:** An agent reading this doc should be able to implement the pattern in any language/framework
