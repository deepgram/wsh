# Server Model: Process Lifecycle & Communication

This document describes `wsh`'s server process lifecycle and communication architecture as a **blueprint for building similar systems**. The patterns here — auto-spawning ephemeral daemons, HTTP-over-Unix-domain-sockets, dual-transport routers, interest-based lifecycle management — are general-purpose infrastructure applicable to any project that needs a persistent daemon with a structured API.

Where `wsh`-specific features appear, they illustrate the pattern with a concrete example. Your project's domain-specific payload (terminal sessions, database connections, build jobs, agent pools) rides on top of the same infrastructure.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Server Lifecycle](#2-server-lifecycle)
3. [Auto-Spawn Protocol](#3-auto-spawn-protocol)
4. [Communication Architecture](#4-communication-architecture)
5. [Discovery & Management](#5-discovery--management)
6. [Security Model](#6-security-model)
7. [Implementation Guide](#7-implementation-guide)
8. [Federation Overview](#8-federation-overview)

---

## 1. Overview

The core pattern is a **client/server split** where a daemon process owns all state and clients are disposable thin connections.

**The server** is a long-lived process that manages your domain's resources (in `wsh`: PTY sessions, terminal state, visual overlays). It exposes a structured API over both a Unix domain socket (always) and TCP (optionally). It can run in two modes:

- **Dedicated**: started explicitly, stays alive indefinitely, shut down explicitly.
- **Ephemeral**: spawned automatically by a client when no server is running, exits when no longer needed.

**Clients** connect, do work, and disconnect. The server doesn't care — state persists across client lifetimes. A client that crashes and reconnects picks up where it left off. Multiple clients can connect simultaneously and share the same resources.

**Communication is unified**: the server speaks HTTP/1.1 and WebSocket over both Unix socket and TCP. The same handlers serve both transports. A transport-aware middleware layer handles the security differences (UDS is trusted; TCP gets authentication, rate limiting, and CORS). This means you write your API once and get local and remote access with appropriate security for each.

```
┌─────────────────────────────────────────────────────────────┐
│                        Server Daemon                        │
│                                                             │
│  ┌───────────────────┐     ┌────────────────────────────┐  │
│  │  Domain State      │     │  API (HTTP + WebSocket)    │  │
│  │  (your resources)  │────▶│                            │  │
│  │                    │     │  UDS: bare (trusted)       │  │
│  │                    │◀────│  TCP: auth + rate limit    │  │
│  └───────────────────┘     └────────────────────────────┘  │
│                                      │                      │
└──────────────────────────────────────│──────────────────────┘
                                       │
              ┌────────────────────────┼────────────────┐
              │                        │                │
              ▼                        ▼                ▼
        ┌──────────┐           ┌──────────┐     ┌──────────┐
        │ CLI      │           │ Agents / │     │ Web UI / │
        │ Client   │           │ Scripts  │     │ Remote   │
        │ (UDS)    │           │ (UDS)    │     │ (TCP)    │
        └──────────┘           └──────────┘     └──────────┘
```

---

## 2. Server Lifecycle

### 2.1 Startup Sequence

A server starts up in a fixed order. Each step depends on the previous one succeeding.

**Step 1: Validate configuration.** Check that all provided configuration is internally consistent before doing any work. In `wsh`, this means verifying the base prefix format (must start with `/`, must not end with `/`) and that TLS cert and key are both provided or both absent.

**Step 2: Resolve security settings.** Determine what authentication and rate limiting will be applied based on the bind address and flags. The rules are transport-dependent (see [Section 6: Security Model](#6-security-model)). Make all security decisions up front, before binding any sockets.

**Step 3: Initialize shared state.** Create the central state object that will be shared across all connections. This includes:

- A **resource registry** for managing your domain objects (in `wsh`: a `SessionRegistry` that maps names to sessions, capped at a configurable maximum).
- A **shutdown coordinator** for tracking active connections and signaling graceful shutdown.
- A **server configuration** object with runtime-mutable settings (in `wsh`: an `AtomicBool` for the persistence toggle).

**Step 4: Acquire the instance lock.** Use `flock(LOCK_EX | LOCK_NB)` on a dedicated lock file to ensure only one server runs per named instance. This must happen before binding sockets — if another server holds the lock, fail immediately with a clear error rather than racing on socket bind. The lock is held for the server's entire lifetime and released automatically by the kernel on exit or crash. No stale-lock cleanup is needed.

**Step 5: Bind the UDS listener.** This is the server's primary local API surface and is always active:

1. Remove the socket file if it exists. The instance lock guarantees we own this path — any existing file is stale.
2. Create parent directories if needed.
3. Bind a Unix stream listener to the socket path.
4. Set permissions to `0600` (owner read/write only) immediately after bind.
5. Wrap with the local API router (no auth middleware — see [Section 4.1](#41-dual-transport-design)).

**Step 6: Optionally bind the TCP listener.** If a network bind address is configured:

1. Bind a TCP listener.
2. Wrap with the full API router (auth, rate limiting, CORS, base prefix).
3. If TLS is configured, set up the TLS acceptor for HTTPS/WSS.
4. If binding to loopback, optionally attempt a secondary IPv6 loopback bind (best-effort; failure is non-fatal).

**Step 7: Start the ephemeral monitor.** If the server was started in ephemeral mode, spawn a background task that watches for the point when the server is no longer needed (see [Section 2.3](#23-ephemeral-mode)).

**Step 8: Enter the main serving loop.** Accept connections on all listeners until a shutdown signal arrives.

### 2.2 Dedicated Mode

A dedicated server is started explicitly (in `wsh`: `wsh server`) and stays alive until explicitly stopped. It does not exit when all resources are gone — it waits for new ones to be created.

This is the default for explicit server startup. The server can be stopped by:

- **Signal**: Ctrl+C (SIGINT) or SIGTERM from the OS.
- **API**: A shutdown endpoint called over UDS (see [Section 5.3](#53-server-control)).

A dedicated server supports a **runtime persistence toggle**: an API endpoint that switches the server between persistent and ephemeral behavior without restarting. This uses an atomic boolean checked by the ephemeral monitor on every iteration. In `wsh`, `wsh persist on/off` calls `PUT /server/persist`. This is useful for promoting an auto-spawned ephemeral server to persistent when you want it to outlive its current workload.

### 2.3 Ephemeral Mode

An ephemeral server is spawned automatically by clients (see [Section 3](#3-auto-spawn-protocol)) and exits when it's no longer needed. "No longer needed" is defined by **interest**: a composite signal with two components:

1. **Resources exist** — the resource registry is non-empty (in `wsh`: at least one session exists).
2. **Persistent connections are held** — at least one long-lived connection (WebSocket, streaming transport) is active.

Interest is tracked with atomic counters and RAII guards. When a persistent connection opens, it increments the counter via a `ConnectionGuard`. When it closes (or the guard is dropped on disconnect), the counter decrements. Both increment and decrement fire a notification that wakes the ephemeral monitor.

The ephemeral monitor runs as a background task with **two phases**:

**Phase 1: Orphan Guard.** The server starts a 30-second idle timer. If no interest materializes within this window, the server shuts down. This handles the case where a client spawns the server daemon but crashes (or is killed) before creating any resources. Without this guard, the orphaned daemon would live forever.

The phase uses a TOCTOU-safe pattern: register a notification future *before* checking the interest predicate, so changes between the check and the async wait are not missed.

```
Phase 1 loop:
  1. Register interest_changed notification
  2. Check has_interest()
     → true: break to Phase 2
  3. select!:
     - Resource event received → break to Phase 2
     - Interest notification → re-check (loop)
     - 30s timer expires → shut down
```

**Phase 2: Normal Monitoring.** Once interest has been established at least once, the monitor watches for it to drain to zero:

- On a resource destruction event: check interest immediately. If zero, shut down.
- On event lag (the event channel overflowed due to rapid churn): apply a **2-second grace period** before rechecking. This handles destroy-then-create sequences where the registry is momentarily empty between a resource being destroyed and a new one being created.
- On interest notification (connection count changed): re-check interest.
- Check the persistence toggle on every iteration. If it has been switched to persistent, enter an infinite wait (the server behaves as dedicated until toggled back).

```
Phase 2 loop:
  1. Register interest_changed notification
  2. If persistent mode: wait for events indefinitely (don't shut down)
  3. Check has_interest()
     → false: shut down
  4. select!:
     - Destroyed event AND !has_interest() → shut down
     - Lagged events → sleep 2s, re-check interest
     - Interest notification → re-check (loop)
```

### 2.4 Graceful Shutdown

Shutdown is triggered by SIGINT, SIGTERM, the ephemeral monitor, or the shutdown API endpoint. The sequence is ordered carefully to prevent races and ensure clean resource cleanup:

**Step 1: Stop accepting new connections.** Cancel the HTTP listener tasks. No new connections are accepted after this point.

**Step 2: Remove the UDS socket file.** Do this immediately — before waiting for existing connections to close. This prevents new clients from attempting to connect during teardown. Clients that try to connect will see "socket not found" and either fail or trigger a new server spawn (if ephemeral), rather than hanging on a socket that will never respond.

**Step 3: Signal existing connections to close.** Use the shutdown coordinator to notify all WebSocket handlers that they should send a close frame and exit their streaming loops.

**Step 4: Wait for connections to drain.** Apply a bounded timeout (in `wsh`: 5 seconds total, which includes a 100ms post-drain grace period; the grace period is inside the timeout, not additive). If connections don't close within the timeout, proceed anyway.

**Step 5: Shut down backend connections.** If the server has persistent outbound connections (e.g., federation), close them.

**Step 6: Drain resources.** Signal all managed resources to shut down. In `wsh`, this means sending SIGHUP to child processes. If they don't exit within a grace period (3 seconds), escalate to SIGKILL.

**Step 7: Await background tasks.** Wait for server tasks to complete with a bounded timeout (in `wsh`: 5 seconds).

**Step 8: Final cleanup.** Wait for any escalated resource kills (SIGKILL) to complete.

**Why this order matters:**

- Removing the socket (step 2) before waiting for drain (step 4) ensures no new connections sneak in during the teardown window.
- Signaling handlers (step 3) before draining resources (step 6) gives clients a chance to receive a clean close frame rather than a connection reset.
- Bounded timeouts on every wait step ensure the server eventually exits even if a client misbehaves.

---

## 3. Auto-Spawn Protocol

The auto-spawn protocol makes the server an invisible implementation detail for single-user workflows. A client runs, the server appears if needed, and the client connects — all transparently.

### 3.1 Detection

The client checks whether a server is already running by sending `GET /health` to the HTTP socket path (`<name>.http.sock`). There are three outcomes:

- **200 OK**: Server is running. Proceed to use it.
- **Connection refused / socket not found**: No server is running. Proceed to spawn.
- **Other error**: Something unexpected. Report to the user.

The health endpoint is deliberately minimal — it returns `{"status": "ok"}` with no logic. It exists solely as a liveness probe.

### 3.2 Race Prevention

Multiple clients may discover "no server" simultaneously and all try to spawn one. A **spawn lock** prevents duplicate daemons:

1. **Acquire the spawn lock.** This is a separate lock file (`<name>.spawn.lock`) from the instance lock (`<name>.lock`). Use a two-phase acquisition: first, 50 non-blocking `flock(LOCK_EX | LOCK_NB)` attempts at 100ms intervals (5 seconds total). If all fail, fall back to a single blocking `flock(LOCK_EX)` call.

2. **Re-check health.** After acquiring the lock, send `GET /health` again. Another client may have spawned the server while we waited for the lock.

3. **Spawn if still needed.** If the health check still fails, spawn the daemon (see [Section 3.3](#33-daemon-spawning)).

4. **Use existing server.** If the health check succeeds (another client spawned while we waited), skip spawning and proceed to connect.

**Why a separate lock?** The instance lock is held by the *server* for its lifetime. If clients tried to acquire it, they'd block until the server exits — the opposite of what we want. The spawn lock coordinates *clients with each other*, ensuring only one spawns the daemon while the rest wait and then use it.

### 3.3 Daemon Spawning

The client spawns the server as a fully detached daemon:

1. **Re-invoke the current executable** (`std::env::current_exe()` or equivalent) with server arguments. In `wsh`: `wsh server --ephemeral --socket <path> --server-name <name>`. Always spawn in ephemeral mode — if the user wanted a persistent server, they'd start one explicitly.

2. **Detach the process:**
   - Redirect stdin, stdout, and stderr to `/dev/null` (or equivalent). The server must not hold references to the client's terminal.
   - Create a new process group (`process_group(0)` on Unix via `setsid` or equivalent). This ensures the server is not killed by SIGHUP when the client's terminal closes.

3. **Prevent zombies.** If the client is a long-lived process, it must either `waitpid()` on the server child or spawn a reaper thread that does. Otherwise the server process becomes a zombie when it exits.

The result: the server daemon is completely independent of the client. It survives the client's exit, terminal closure, and signal delivery.

### 3.4 Connection Wait

After spawning, the client polls `GET /health` on the HTTP socket every 50ms with a 5-second timeout. Once the health check succeeds, the server is ready to accept API requests.

This polling approach is simpler and more reliable than trying to synchronize on server readiness signals through the filesystem or pipes. The health endpoint is the server's own declaration that it's ready.

### 3.5 Full Client Flow

The general pattern for a client that auto-spawns and connects:

1. **Resolve the socket path** from the instance name (or an explicit override).
2. **Health check → auto-spawn if needed** (steps 3.1–3.4 above).
3. **Create a resource** on the server via HTTP. In `wsh`: `POST /sessions` with terminal size, optional name, and optional tags.
4. **Open a WebSocket** over UDS for bidirectional streaming to the resource.
5. **Protocol handshake**: exchange an initial confirmation message, then subscribe to relevant event streams.
6. **Bidirectional streaming loop**: forward local input to the server, render server output locally.
7. **On disconnect**: clean up local state (in `wsh`: restore terminal mode, erase overlays/panels).

Steps 1–2 are universal infrastructure (every client does them identically). Steps 3–7 carry your domain-specific payload.

---

## 4. Communication Architecture

### 4.1 Dual-Transport Design

The server serves the same API over two transports with different security postures:

**UDS (always active):** The Unix domain socket serves the "core router" — all API routes with no authentication, no rate limiting, no CORS, and no path prefix. UDS is a trusted transport because access is controlled by filesystem permissions (the socket is `chmod 0600`). Any process that can open the socket is running as the owning user on the local machine.

**TCP (opt-in):** When a network bind address is configured, the server also serves over TCP with a full security middleware stack: token authentication, rate limiting, CORS policy, and optional path prefix. This is the untrusted transport, appropriate for remote access.

Both transports share the same application state and the same handler functions. A transport middleware layer injects connection metadata (`UdsConnectInfo` or `TcpConnectInfo`) so that handlers can distinguish the transport when needed. This enables transport-restricted endpoints (e.g., the shutdown endpoint is UDS-only) without maintaining separate routers.

```
                  ┌─────────────────────────────────────┐
                  │           API Routes                 │
                  │  (handlers, state, business logic)   │
                  └──────────┬──────────┬───────────────┘
                             │          │
              ┌──────────────┘          └──────────────┐
              ▼                                        ▼
   ┌─────────────────────┐              ┌─────────────────────────┐
   │  UDS Router          │              │  TCP Router              │
   │  • No auth           │              │  • Token auth            │
   │  • No rate limit     │              │  • Rate limiting         │
   │  • No CORS           │              │  • CORS policy           │
   │  • No path prefix    │              │  • Optional path prefix  │
   │  • 0600 permissions  │              │  • Optional TLS          │
   └─────────────────────┘              └─────────────────────────┘
```

### 4.2 HTTP-over-UDS

The key unifying decision in this architecture is **serving standard HTTP/1.1 over the Unix domain socket** rather than inventing a custom binary protocol.

Most HTTP server libraries (hyper, Go's `net/http`, Python's `uvicorn`) can accept connections from a Unix listener just as easily as from a TCP listener. The HTTP layer doesn't care about the underlying transport — it sees a bidirectional byte stream either way. This means:

- **One API implementation** serves both transports. You don't maintain a custom protocol parser alongside your HTTP handlers.
- **Standard tooling works.** `curl --unix-socket /path/to/sock http://localhost/health` just works. So does any HTTP client library that supports Unix sockets.
- **WebSocket upgrade works identically** on both transports. The upgrade is an HTTP-level concern, not a transport-level one.
- **Debugging is easier.** HTTP is a well-understood text protocol with abundant tooling.

The socket file uses a distinctive extension (in `wsh`: `.http.sock`) to signal that it speaks HTTP, distinguishing it from any other Unix sockets the application might use.

The one technical requirement: when setting up the HTTP connection on the server side, enable upgrade support (in hyper: `.with_upgrades()` on the handshake) so that WebSocket upgrade requests over UDS work correctly.

### 4.3 TCP Binding & Authentication

When TCP is enabled, additional security measures apply based on the bind address:

**Loopback binding** (127.0.0.1 / ::1): No token authentication is required — localhost is treated as trusted, similar to UDS. However, a WebSocket Origin header check is still applied to mitigate Cross-Site WebSocket Hijacking (CSWSH), where a malicious webpage could attempt to connect to your local server.

When binding to IPv4 loopback, consider also binding to IPv6 loopback (`[::1]`) as a separate, best-effort listener. If the IPv6 bind fails (e.g., IPv6 not available), log it at debug level and continue — this should never be fatal.

**Non-loopback binding**: Token authentication is mandatory. Three resolution paths:

1. **User-provided token** (via CLI flag or environment variable): validate minimum length (in `wsh`: 16 characters) and use as-is.
2. **No token specified**: auto-generate a cryptographically random token (in `wsh`: 32 alphanumeric characters) and print it to stderr so the user can retrieve it.
3. **Explicit no-auth** (via a `--no-auth` flag): disable authentication entirely. Log a warning. This is for deployments behind a reverse proxy or VPN that handles authentication externally.

The token is checked as a Bearer token in the `Authorization` header. For WebSocket connections from browsers (which cannot set custom headers on the upgrade request), provide an `/auth/ws-ticket` endpoint that generates a short-lived, single-use ticket. The client passes this ticket as a query parameter during the WebSocket upgrade.

**TLS**: Support native TLS via certificate and key file paths. When binding to a non-loopback address without TLS, log a warning. For environments where TLS termination happens elsewhere (reverse proxy, VPN, SSH tunnel), this is acceptable — the warning ensures the operator has made a conscious choice.

### 4.4 WebSocket Overview

WebSocket connections provide real-time bidirectional communication between server and clients. The general patterns:

**Connection lifecycle:** Upgrade → initial handshake message → subscribe to event streams → bidirectional streaming → close frame. In `wsh`, the server sends `{"connected": true}` immediately after upgrade, then the client sends a `subscribe` message specifying which event types it wants.

**Binary vs text frames:** Use binary frames for high-throughput opaque data (in `wsh`: raw PTY I/O bytes) and text frames for structured messages (JSON method calls, event notifications, error responses). This separation lets you optimize the hot path (binary) while keeping the control plane human-readable (JSON).

**Backpressure:** A slow client must not block the server. Use a bounded channel (in `wsh`: 256 slots) between the handler and a dedicated send task. When the channel fills, coalesce events (e.g., merge multiple screen updates into one) rather than dropping or blocking. This ensures the client always sees the latest state, even if it falls behind.

**Keepalive:** Send ping frames at regular intervals (in `wsh`: every 30 seconds) with a response timeout (10 seconds). If no pong arrives, close the connection. This detects dead connections (e.g., a client machine that lost network connectivity) that TCP keepalive might not catch for minutes.

**Scoped vs multiplexed:** You can offer per-resource WebSockets (one connection per managed resource) and/or a server-level multiplexed WebSocket (one connection, events tagged with a resource identifier). Per-resource connections are simpler for clients that interact with one resource; multiplexed connections are better for dashboards or orchestrators that monitor many resources. In `wsh`, both are available.

### 4.5 Persistent Connections and Interest Tracking

This pattern is critical for ephemeral servers: **some connections should keep the server alive, and others should not.**

The distinction:

- **Persistent connections** (WebSocket streams, long-lived transports): these represent ongoing client interest. An ephemeral server must not shut down while they're active.
- **Stateless connections** (HTTP request/response cycles): these are transient. The server should not stay alive just because someone sent a GET request 10 seconds ago.

The mechanism: persistent connections register a `ConnectionGuard` (RAII) on creation. The guard increments an atomic interest counter and notifies the ephemeral monitor. When the connection closes (or the guard is dropped on error), the counter decrements and the monitor is notified again.

**Example from `wsh`:** The MCP stdio bridge (`wsh mcp`) is a process that translates between MCP's stdio protocol and the server's WebSocket API. It connects to the server's `/mcp/ws` endpoint over UDS and registers a `ConnectionGuard`. As long as the bridge process is running, the ephemeral server stays alive — even if there are zero sessions. This is correct because the MCP bridge represents an active client that may create sessions at any moment.

In contrast, Streamable HTTP MCP sessions (stateless request/response cycles to `/mcp`) are deliberately excluded from interest tracking. A single tool call should not prevent an ephemeral server from shutting down after all sessions are gone.

This distinction is essential: without it, an ephemeral server either shuts down prematurely (killing active clients) or lingers forever (defeating the purpose of ephemeral mode).

---

## 5. Discovery & Management

### 5.1 Named Instances

The server supports running multiple named instances on a single machine. Each instance is identified by a name (default: `"default"`) and gets its own isolated file set under a well-known runtime directory:

**Directory convention:**
- Primary: `$XDG_RUNTIME_DIR/<app>/` (e.g., `$XDG_RUNTIME_DIR/wsh/`)
- Fallback: `/tmp/<app>-$USER/` (when `$XDG_RUNTIME_DIR` is unset)

**File layout per instance:**
- `<name>.http.sock` — the HTTP/WebSocket API socket
- `<name>.lock` — the instance lock (held by the running server via flock)
- `<name>.spawn.lock` — the client spawn coordination lock

The instance name is specified via a CLI flag (in `wsh`: `-L` / `--server-name`), an environment variable (`WSH_SERVER_NAME`), or left as the default. All subcommands accept the instance name so they target the correct server.

An explicit `--socket` flag can override the name-derived path entirely, for custom socket placement (e.g., in a project-specific directory). When overriding, the lock files still derive from the instance name.

### 5.2 Discovery

Clients and tooling need to discover a server's connection parameters without hardcoding paths or ports. The pattern: a `GET /server/info` endpoint over UDS that returns the server's configuration.

In `wsh`, this returns:

```json
{
  "instance_name": "default",
  "hostname": "my-machine",
  "version": "0.9.0",
  "server_id": "550e8400-e29b-41d4-a716-446655440000",
  "socket_path": "/run/user/1000/wsh/default.http.sock",
  "tcp_addr": "0.0.0.0:8080",
  "persistent": true,
  "session_count": 3
}
```

The `server_id` (UUID, generated fresh on each startup) uniquely identifies a server instance across restarts. The `tcp_addr` is null when no TCP listener is bound. Domain-specific state (like session count) is included alongside the infrastructure fields.

This endpoint enables:
- CLI tools that display server status (`wsh info`)
- Agents that need to discover the TCP address for remote access
- Health dashboards that monitor multiple named instances
- Automation scripts that verify a server is running before sending work

### 5.3 Server Control

**Shutdown:** A `POST /server/shutdown` endpoint over UDS triggers graceful shutdown. The client then polls for the socket file to disappear as confirmation that shutdown completed (in `wsh`: 10-second timeout). If the socket is already gone when the client tries to connect, it can safely assume the server isn't running.

This endpoint is restricted to UDS transport only — remote clients cannot shut down the server even with a valid authentication token. The handler checks a `Transport` extension (injected by transport middleware) and rejects non-UDS requests.

**Persistence toggle:** A `PUT /server/persist` endpoint toggles between ephemeral and persistent behavior at runtime without restarting. The change takes effect immediately — the ephemeral monitor checks the toggle on every iteration.

### 5.4 Domain-Specific Management

These patterns are conditional — include them if your project needs them:

**If your project manages multiple resources** (sessions, connections, jobs, worker pools): provide a list endpoint and a CLI command to display them. In `wsh`, `GET /sessions` returns an array of session info (name, PID, command, terminal size, connected client count), and `wsh list` renders it as a table. This gives operators visibility into what the server is managing.

**If clients need to reconnect to existing resources:** provide a reconnect flow that replays state before entering the streaming loop. In `wsh`, `wsh attach <name>` fetches the session's scrollback history and current screen contents via HTTP, renders them to the terminal, then opens a WebSocket for live streaming. The client sees a seamless continuation of the session.

**If resources can be destroyed individually:** provide a delete endpoint. In `wsh`, `DELETE /sessions/{name}` kills a session's child process and removes it from the registry, broadcasting a destruction event to all connected clients.

**Socket path resolution order:** All CLI subcommands should resolve the server's socket path using the same precedence: explicit `--socket` flag > instance name (from flag or env var) > default. This ensures consistent behavior across all commands.

---

## 6. Security Model

### 6.1 Transport-Based Trust

The security model is grounded in a simple principle: **trust the transport, not the request.**

| Transport | Trust Level | Authentication | Rationale |
|-----------|-------------|----------------|-----------|
| UDS | Trusted | None | Filesystem permissions (0600) restrict access to the owning user. Any process that can open the socket is already the right user on the local machine. |
| TCP loopback | Trusted | Origin check only | Same machine, but WebSocket Origin checking is applied to mitigate CSWSH attacks from malicious web pages. |
| TCP non-loopback | Untrusted | Token required | Remote access over the network requires explicit authentication. |

### 6.2 Defense in Depth

When TCP is enabled, multiple layers protect the server:

1. **Localhost by default.** No TCP listener unless explicitly configured. The UDS socket is the only default API surface.

2. **Token authentication.** Mandatory on non-loopback addresses. Auto-generated if not provided. Minimum length enforced (in `wsh`: 16 characters for user-supplied tokens, 32 characters for auto-generated).

3. **TLS.** Native support via certificate and key files. A warning is logged when binding to a non-loopback address without TLS — the operator must make a conscious choice to run unencrypted.

4. **Rate limiting.** Applied on non-loopback addresses with a sensible default (in `wsh`: 100 requests/second). Configurable via CLI flag.

5. **IP access control.** CIDR-based blocklists and allowlists, configured via config file. There are no hardcoded defaults — the operator owns the threat model entirely.

6. **Base prefix.** A configurable path prefix (e.g., `/wsh`) nests all API routes for clean reverse-proxy deployment. The `/health` endpoint remains at the root for load balancer probes.

### 6.3 Transport-Restricted Endpoints

Some operations should only be available over the trusted transport. The shutdown endpoint is the canonical example — you don't want remote clients (even authenticated ones) to be able to kill the server.

The implementation pattern: transport middleware injects a `Transport` enum (UDS or TCP) as a request extension. Handlers that need to restrict access check the extension and return an error for non-UDS requests. This avoids maintaining separate route trees for each transport.

---

## 7. Implementation Guide

This section provides concrete techniques for implementing the patterns described above. The examples use Rust with tokio/hyper/axum, but the patterns are language-agnostic.

### 7.1 Instance Locking with flock

Use `flock(LOCK_EX | LOCK_NB)` on a dedicated lock file for mutual exclusion between server instances.

```
lock_fd = open("<runtime_dir>/<name>.lock", O_CREAT | O_RDWR, 0600)
result  = flock(lock_fd, LOCK_EX | LOCK_NB)

if result == EWOULDBLOCK:
    // Another server holds the lock. Exit with a clear error.
    exit("Server instance '<name>' is already running")

// Lock acquired. Hold lock_fd open for the server's lifetime.
// The kernel releases the lock on exit, crash, or fd close.
```

Key properties:
- **Non-blocking**: the server fails fast rather than waiting for another instance to exit.
- **Crash-safe**: the kernel releases the lock when the process exits, regardless of how it exits (clean shutdown, SIGKILL, crash).
- **No cleanup needed**: there is no stale-lock problem. If the lock file exists but no process holds it, `flock` acquires it immediately.

Use a dedicated lock file, not the socket file. The socket file has a different lifecycle (created on bind, removed on shutdown) and you need the lock to exist before the socket does.

### 7.2 Spawn Lock for Client Coordination

The spawn lock prevents multiple clients from spawning duplicate server daemons. It uses a separate lock file from the instance lock.

```
// Phase 1: Bounded non-blocking retry
for i in 0..50:
    result = flock(spawn_lock_fd, LOCK_EX | LOCK_NB)
    if result == OK:
        break
    sleep(100ms)

// Phase 2: Blocking fallback (if Phase 1 exhausted)
if !locked:
    flock(spawn_lock_fd, LOCK_EX)  // blocks until available

// Critical section:
if health_check() == OK:
    // Another client spawned while we waited. Use their server.
    release_lock()
    return

spawn_server_daemon()
wait_for_health()
release_lock()
```

The two-phase approach balances responsiveness (most clients acquire the lock quickly via non-blocking attempts) with correctness (the blocking fallback handles pathological contention).

### 7.3 Daemon Process Isolation

Spawn the server as a fully detached daemon that survives the client's exit:

```
command = current_exe() + ["server", "--ephemeral", "--socket", path, "--server-name", name]

process = spawn(command,
    stdin  = /dev/null,    // Don't hold client's terminal
    stdout = /dev/null,    // Server logs via its own mechanism
    stderr = /dev/null,    // (tracing, log files, etc.)
    process_group = 0,     // New process group (setsid)
)

// Prevent zombie: reap the child in a background thread
spawn_thread(|| waitpid(process.pid))
```

Why each setting matters:
- **Null stdio**: if the server inherits the client's terminal file descriptors, it will hold the terminal open even after the client exits, preventing the terminal from closing.
- **New process group**: without this, the server is in the client's process group and receives SIGHUP when the client's controlling terminal closes.
- **Reaper thread**: the spawning process is the server's parent. If it doesn't `waitpid`, the server becomes a zombie on exit (PID lingers in the process table).

### 7.4 HTTP-over-UDS

Serving HTTP over a Unix socket requires minimal changes from TCP serving:

```rust
// Rust with axum + hyper:
let listener = tokio::net::UnixListener::bind(&socket_path)?;

// Remove stale socket first (instance lock guarantees ownership)
let _ = std::fs::remove_file(&socket_path);
std::fs::create_dir_all(socket_path.parent().unwrap())?;
let listener = tokio::net::UnixListener::bind(&socket_path)?;

// Set owner-only permissions
std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;

// Serve with the same router you'd use for TCP
axum::serve(listener, router.into_make_service_with_connect_info::<UdsConnectInfo>())
    .with_graceful_shutdown(shutdown_signal)
    .await?;
```

The critical detail: enable HTTP upgrade support on the connection so WebSocket upgrades work. In hyper, this means calling `.with_upgrades()` on the HTTP/1.1 handshake. Without this, WebSocket upgrade requests will fail silently.

### 7.5 Dual-Router Pattern

Build your API routes once, then wrap them differently for each transport:

```
fn api_routes(state: AppState) -> Router {
    // All your endpoints go here — shared between transports
    Router::new()
        .route("/health", get(health))
        .route("/resources", get(list).post(create))
        .route("/resources/{id}", get(show).delete(destroy))
        .route("/resources/{id}/ws", get(ws_upgrade))
        .route("/server/info", get(server_info))
        .route("/server/shutdown", post(shutdown))
        .with_state(state)
}

fn uds_router(state: AppState) -> Router {
    // Bare routes + transport metadata. No auth.
    api_routes(state)
        .layer(UdsTransportLayer)
}

fn tcp_router(state: AppState, config: &Config) -> Router {
    // Same routes + security middleware
    api_routes(state)
        .layer(TcpTransportLayer)
        .layer(AuthLayer::new(config.token))
        .layer(RateLimitLayer::new(config.rate_limit))
        .layer(CorsLayer::new(config.cors_origins))
}
```

Transport-restricted endpoints (like shutdown) check the injected `Transport` extension:

```rust
async fn shutdown(transport: Extension<Transport>, state: State<AppState>) -> Result<()> {
    if *transport != Transport::Uds {
        return Err(Error::UdsOnly);
    }
    state.shutdown_notify.cancel();
    Ok(())
}
```

### 7.6 Ephemeral Lifecycle Management

The ephemeral monitor combines several concurrency primitives:

**Interest counter with notification:**

```rust
struct ShutdownCoordinator {
    active: AtomicUsize,          // Number of persistent connections
    interest_notify: Notify,      // Wakes the ephemeral monitor
    shutdown: CancellationToken,  // Signals shutdown to handlers
    all_closed: Notify,           // Signals when active count reaches 0
}

impl ShutdownCoordinator {
    fn register(&self) -> ConnectionGuard {
        self.active.fetch_add(1, Ordering::SeqCst);
        self.interest_notify.notify_waiters();
        ConnectionGuard { coordinator: self }
    }

    fn active_count(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    fn interest_changed(&self) -> Notified<'_> {
        self.interest_notify.notified()
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let prev = self.coordinator.active.fetch_sub(1, Ordering::SeqCst);
        self.coordinator.interest_notify.notify_waiters();
        if prev == 1 {
            self.coordinator.all_closed.notify_waiters();
        }
    }
}
```

**TOCTOU-safe interest checking:**

The critical technique in the monitor loop: register the notification future *before* checking the predicate. If you check first and register second, a change between the two operations is missed.

```rust
loop {
    // 1. Register FIRST — captures all future notifications
    let notified = shutdown.interest_changed();

    // 2. Check SECOND — if state changed before registration,
    //    we see it here. If it changes after, notified fires.
    if !has_interest(&registry, &shutdown) {
        return true; // shut down
    }

    // 3. Wait for something to change
    select! {
        event = events.recv() => { /* handle event */ }
        _ = notified => { /* interest changed, re-check */ }
    }
}
```

If you reverse steps 1 and 2 (check, then register), there's a window where the last connection can close between your check (which sees interest) and your registration (which starts listening). The monitor would then block forever, waiting for a notification that already fired.

### 7.7 Graceful Shutdown Ordering

The shutdown sequence requires two separate cancellation mechanisms:

1. **Listener cancellation token**: stops accepting new connections. Fired first.
2. **Handler shutdown signal**: tells existing WebSocket handlers to send close frames and exit. Fired second, after listeners are stopped.

```
// Pseudocode for the shutdown sequence
fn graceful_shutdown():
    // Phase 1: Stop accepting
    listener_cancel.cancel()
    remove_file(socket_path)

    // Phase 2: Close existing connections
    shutdown_coordinator.shutdown()

    // Phase 3: Wait for drain
    timeout(5s):
        shutdown_coordinator.wait_all_closed()
        sleep(100ms)  // grace period for final messages

    // Phase 4: Clean up resources
    for resource in registry.drain():
        resource.signal(SIGHUP)

    timeout(5s):
        wait_for_tasks()

    // Phase 5: Escalate if needed
    for resource in signaled_resources:
        if resource.still_alive():
            resource.signal(SIGKILL)
            resource.wait()
```

The 100ms grace period after drain (inside the 5s timeout) allows final messages to flush through the network stack. Without it, the last close frame from a WebSocket handler might not reach the client before the socket is torn down.

---

## 8. Federation Overview

`wsh` supports federating multiple server instances across machines. This is a brief overview of the extension point — federation has its own design surface that warrants a separate document.

**Backend registry:** Servers register with each other via `POST /servers`, providing hostname, address, and capabilities. The registry maps hostnames to backend entries.

**Persistent connections:** Federated servers maintain persistent WebSocket connections to each other for real-time event forwarding and cross-server awareness.

**Configuration:** Federation config lives in a config file (`$XDG_CONFIG_HOME/wsh/config.toml` or via `--config` flag), covering hostname resolution, IP access control (CIDR allowlist/blocklist), and backend discovery.

**Loop prevention:** Each server generates a UUID (`server_id`) on startup. This ID is included in forwarded requests to prevent routing loops in federated topologies.

**Key design point:** Federation builds on top of the same HTTP API that local clients use. A federated server is just another API client with persistent connections. No separate protocol is needed.
