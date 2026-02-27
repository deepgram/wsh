# Backend Server Federation

> Every `wsh` server is always a hub. A server with zero backends is a
> cluster of one. Register a backend and you're orchestrating.

## Problem

`wsh` gives AI agents the ability to interact with terminals. But each
`wsh` server manages sessions on a single machine. To coordinate work
across multiple machines -- running distributed jobs, monitoring a
fleet, horizontally scaling agent-driven workflows -- something needs to
connect to each `wsh` server individually and track the topology.

Today, that "something" must be built externally. This design makes it
intrinsic: every `wsh` server can proxy to other `wsh` servers,
presenting a unified API across the cluster.

## Core Principle

There is no "hub mode." Federation is an intrinsic capability of every
`wsh` server. The distinction between a single-server instance and a
multi-server orchestrator is simply whether any backends are registered.
The API surface is identical in both cases.

This means:

- Skills, MCP tools, and CLI commands work unchanged across
  single-server and multi-server deployments.
- Any server can become an orchestrator at runtime by registering
  backends. No restart required.
- AI agents don't need different guidance for local vs. distributed
  operation.

## Architecture

### Backend Registry

Each server maintains a **backend registry** -- a list of remote `wsh`
servers it proxies to. Each registry entry contains:

| Field | Description |
|-------|-------------|
| `address` | Network address (`host:port`) |
| `token` | Auth token for this backend (optional; see token cascade) |
| `hostname` | Server's self-reported hostname (populated on connect) |
| `health` | Current health state (healthy, unavailable, connecting) |
| `role` | Capability designation (v1: always `"member"`; reserved for future use) |

The local server is always implicitly present. `GET /servers` always
returns at least one entry (self).

### Persistent WebSocket Connections

For each registered backend, the server maintains a **persistent
WebSocket connection**. This single connection serves three purposes:

1. **Health signal.** Connection alive = healthy. Connection drop =
   instant failure detection. Ping/pong frames detect silent failures
   (network partition, frozen process).

2. **Event bus.** Client subscriptions (screen updates, idle
   notifications, overlay/panel changes) are forwarded through this
   connection. When a client subscribes to a remote session's events,
   the server subscribes on the backend's WebSocket and relays events
   back.

3. **Proxy channel.** API calls targeting remote sessions are
   multiplexed over this connection.

When a backend becomes unreachable:

1. Mark it as unavailable immediately.
2. Hide its sessions from `GET /sessions` responses.
3. Retry reconnection in the background with exponential backoff.
4. When the backend recovers, reconnect and re-establish state
   (re-subscribe to active event streams, refresh session list).

### Server Identity

Each server has a **hostname** that serves as its identity in the
cluster:

- **Default:** system hostname via `gethostname()`.
- **Override:** `--hostname` CLI flag or `[server] hostname = "..."` in
  TOML config.

When the server connects to a backend, it queries the backend's hostname
via `GET /server/info`. This self-reported hostname becomes the
backend's identifier in API responses and the `server` field on
sessions.

**Collision handling:** If two backends report the same hostname, the
server rejects the second registration and surfaces an error. Operators
must resolve the conflict by overriding one hostname.

## API Surface

### Unified Session Operations

The same endpoints work for local and remote sessions. Every session
response includes a `server` field containing the owning server's
hostname.

| Endpoint | Change from current |
|----------|-------------------|
| `GET /sessions` | Returns sessions from all servers. Each includes `server` field. |
| `POST /sessions` | Optional `server` parameter targets a specific backend. Omitted = local. Unknown server = 404. |
| `/sessions/:name/*` | All session endpoints accept `?server=hostname` for disambiguation. Omitted = always local. |

**Addressing rule:** `?server` omission always means local. This is a
hard rule, not a convenience shortcut. Even if a session name is
globally unique across the cluster, omitting `?server` targets the
local server. This prevents TOCTOU attacks where a compromised backend
creates a session with a known name to intercept requests.

### Server Management (new endpoints)

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/servers` | GET | List all servers (always includes self). Returns hostname, address, health, session count. |
| `/servers` | POST | Register a new backend. Body: `{ address, token?, name? }`. |
| `/servers/:hostname` | GET | Detailed status for one server (health, latency, session count, uptime). |
| `/servers/:hostname` | DELETE | Deregister a backend. |
| `/servers/reload` | POST | Hot-reload backend list from config file. |
| `/server/info` | GET | This server's own identity: hostname, version, uptime, capabilities. |

### Token Resolution (outbound auth)

When connecting to a backend, the server resolves the auth token using
a cascade:

1. **Per-server token** -- explicit in the config for that specific
   backend.
2. **Default token** -- `default_token` field in the config file.
3. **Local server's own token** -- the `--token` the server was started
   with.

The first non-empty value wins. This means homogeneous clusters (same
token everywhere) need zero per-server configuration.

## Config File

TOML format. Loaded via `wsh server --config <path>`. Default path:
`~/.config/wsh/config.toml`. If `--config` is not supplied and no
default config file exists, the server starts normally with no backends.

```toml
[server]
hostname = "orchestrator-1"   # Optional; overrides gethostname()

# Default token for backends that don't specify their own.
# Falls back to the server's --token if omitted.
default_token = "shared-cluster-secret"

[[servers]]
address = "10.0.1.10:8080"
# Uses default_token; hostname learned on connect

[[servers]]
address = "10.0.1.11:8080"
token = "prod-2-specific-token"   # Overrides default_token
# hostname learned on connect

[[servers]]
address = "192.168.1.50:8080"
# Uses default_token; hostname learned on connect
```

**Bidirectional sync:** When backends are added or removed via API, the
config file is updated automatically. This ensures the config file is
always the source of truth, even after runtime mutations.

**Hot reload:** `POST /servers/reload` re-reads the config file and
reconciles the backend registry (add new entries, remove deleted ones,
update changed tokens/addresses). Also available via CLI:
`wsh servers reload`.

## MCP Tools

### Extended Existing Tools

All existing session tools gain an optional `server` parameter:

| Tool | New parameter |
|------|--------------|
| `wsh_list_sessions` | `server` -- filter to one server's sessions |
| `wsh_create_session` | `server` -- target server (omit = local) |
| `wsh_manage_session` | `server` -- target remote session |
| `wsh_send_input` | `server` -- target remote session |
| `wsh_get_screen` | `server` -- target remote session |
| `wsh_get_scrollback` | `server` -- target remote session |
| `wsh_await_idle` | `server` -- target remote session |
| `wsh_run_command` | `server` -- target remote session |
| `wsh_overlay` | `server` -- target remote session |
| `wsh_remove_overlay` | `server` -- target remote session |
| `wsh_panel` | `server` -- target remote session |
| `wsh_remove_panel` | `server` -- target remote session |
| `wsh_input_mode` | `server` -- target remote session |
| `wsh_screen_mode` | `server` -- target remote session |

### New Server Management Tools

| Tool | Description |
|------|-------------|
| `wsh_list_servers` | List registered backends with health, session count, address, hostname. |
| `wsh_add_server` | Register a new backend (address + optional token). |
| `wsh_remove_server` | Deregister a backend by hostname. |
| `wsh_server_status` | Detailed info: health, latency, session count, uptime. |

## Skills

### New Skill: Cluster Orchestration

`skills/wsh/cluster-orchestration.md` teaches agents distributed
workflow patterns:

- Registering and monitoring backend servers
- Creating sessions on specific servers
- Distributed workflows: "start a session with tag X on each server,
  then for each session with tag X, run Y"
- Cross-server quiescence: waiting for idle across multiple servers
- Handling backend failures: retry, skip, failover strategies
- Fleet-wide monitoring and status aggregation

The skill describes patterns and strategies (the "what"), not protocol
details (the "how"). It is protocol-agnostic, consistent with the
existing skill philosophy.

### Updated Existing Skills

- **Core skill**: Documents the `server` parameter as a universal
  optional on all session operations.
- **Multi-Session skill**: Notes that sessions can span servers, tags
  work cross-server, and quiescence aggregates across the cluster.

## CLI

### New Subcommands

```
wsh servers                              List all servers (including self)
wsh servers add <address> [--token ...]  Register a new backend
wsh servers remove <hostname>            Deregister a backend
wsh servers reload                       Hot-reload from config file
wsh servers status <hostname>            Detailed health/stats
```

### Extended Existing Commands

Existing session commands gain `--server` / `-s`:

```
wsh list                        All sessions across all servers
wsh list -s prod-1              Sessions on prod-1 only
wsh create -s prod-2            Create session on prod-2
wsh kill my-session -s prod-1   Kill specific remote session
```

All CLI commands communicate with the local server over the Unix domain
socket. The local server handles proxying to backends transparently.

## Socket Protocol

New frame types for server management:

| Type | Name | Direction | Payload |
|------|------|-----------|---------|
| `0x20` | ListServers | client -> server | JSON (optional filters) |
| `0x21` | ListServersResponse | server -> client | JSON (server list) |
| `0x22` | AddServer | client -> server | JSON (address, token, name) |
| `0x23` | AddServerResponse | server -> client | JSON (result) |
| `0x24` | RemoveServer | client -> server | JSON (server name) |
| `0x25` | RemoveServerResponse | server -> client | JSON (result) |
| `0x26` | ReloadConfig | client -> server | empty |
| `0x27` | ReloadConfigResponse | server -> client | JSON (result) |
| `0x28` | ServerInfo | client -> server | JSON (server name) |
| `0x29` | ServerInfoResponse | server -> client | JSON (detailed status) |

Existing session frames (`CreateSession`, `ListSessions`, etc.) gain an
optional `server` field in their JSON payloads.

## Web UI

### Sidebar

The sidebar gains **server** as a grouping dimension alongside the
existing tag grouping. Available grouping modes:

- Tag (existing, default)
- Server
- Server > Tag (two-level hierarchy)
- Tag > Server (two-level hierarchy)

### Session Mini-Previews

Every session mini-preview displays a **server badge** -- a small
indicator showing which server owns the session. Visible regardless of
the current grouping mode.

### Command Palette

New commands:

- **"Go to server: ..."** -- jump to a server's sessions (filterable,
  same pattern as "Go to tag: ...").
- **"Servers"** -- open server list/status view.

### Server List View

Accessible from sidebar or command palette. Shows all registered
backends with health status, session count, and connection latency.

### Connection Model

The web UI connects to one server's WebSocket endpoint. That server
proxies all cross-server events. The web UI does not need to know about
federation topology -- it is just another client of the unified API.

## Security

### Threat Model

Federation expands the attack surface. A single-server `wsh` instance
has one trust boundary (client <-> server). Federation adds:

- **Server <-> backend connections** -- the server sends auth tokens
  over the network to backends.
- **Proxy pass-through** -- client requests traverse the server to a
  backend; input validation must happen at both layers.
- **Config file as credential store** -- the TOML config contains
  backend tokens.
- **Cross-server state aggregation** -- session listings, idle
  detection, and events merge data from multiple trust domains.

### Principles

1. **Auth is always enforced.** Non-localhost bindings require
   `--token`, regardless of whether backends are registered.
   Server-to-backend connections use token auth. No unauthenticated
   network paths.

2. **Validate at every boundary.** The server validates inbound client
   requests AND validates/sanitizes responses from backends before
   forwarding. A compromised backend must not be able to inject
   malicious data into the API responses.

3. **Least privilege by default.** The `role` field on registry entries
   is reserved for future capability restrictions. New backends
   registered via auto-discovery (future) will require explicit
   promotion before accepting sessions.

4. **Credentials are protected.** Config file permissions are validated
   on load (warn if world-readable). Tokens are never logged. Tokens
   are never included in API responses.

5. **Defense in depth.** Rate limiting, CSP headers, CSWSH protection,
   and constant-time token comparison apply to all endpoints
   identically.

### OWASP Top 10 Coverage

| Risk | Mitigation |
|------|-----------|
| **Injection** | Session names, server names, and all user inputs validated against strict patterns. No shell interpolation. Query parameters parsed by framework (axum). |
| **Broken Authentication** | Token auth required for non-localhost. Constant-time comparison. Ticket exchange for WebSocket. Server-to-backend tokens follow same standards. Minimum token length enforced. |
| **Sensitive Data Exposure** | Tokens never in logs or API responses. Config file permission check. TLS is operator's responsibility (documented). |
| **Broken Access Control** | `?server` omission always means local (no TOCTOU). Backends can't influence request routing. Compromised backend can't redirect requests to itself. |
| **Security Misconfiguration** | Non-localhost without token = hard error. Secure defaults. CSP on all responses. |
| **XSS** | CSP with strict policy. Terminal output rendered as text nodes. Server badges and names escaped. |
| **SSRF** | Connects only to explicitly registered backends. No user-controlled URL fetching. Backend addresses validated (no `file://`, `0.0.0.0`). Self-loop prevention via server UUID comparison (not address blocking). Localhost/loopback allowed for local multi-server testing. |
| **Insecure Deserialization** | All cross-server data is JSON with schema validation. Backend responses validated before forwarding. |

### Backend Response Validation

A compromised backend could return malicious data. Mitigations:

- **Schema validation**: Responses from backends are validated against
  expected shapes before forwarding. Unexpected fields are stripped.
- **Size limits**: Responses bounded by existing 16 MiB frame limit.
  Session listings from a single backend are capped.
- **Sanitization**: Server-reported hostnames, session names, and other
  strings validated against the same strict patterns as locally-created
  resources.

## Testing

### Unit Tests

- Backend registry CRUD operations
- Token resolution cascade (per-server, default, local)
- Hostname collision detection and rejection
- Config file parsing, serialization, and bidirectional sync
- Backend response schema validation and sanitization

### Integration Tests

- Proxy request routing (local vs. remote sessions)
- Health detection: backend up, down, recovery, silent failure
- Cross-server session listing with `server` field
- Session creation on remote backends
- `?server` disambiguation for colliding session names
- WebSocket event forwarding across server boundary
- Cross-server quiescence (idle detection across backends)
- Config hot-reload: add, remove, update backends

### End-to-End Tests

- Spin up multiple `wsh server` instances with unique `--server-name`
  values
- Register backends via config file and API
- Full workflows through HTTP, WebSocket, MCP, and CLI
- Backend failure and recovery during active sessions
- Web UI server grouping and badge rendering

### Security Tests

- Malformed backend responses (invalid JSON, unexpected fields,
  oversized payloads)
- Token enumeration attempts against server management endpoints
- Self-loop detection via server UUID (registering self as backend)
- Address validation (non-HTTP schemes, unspecified addresses)
- Cross-server injection via crafted session names or hostnames
- Config file with world-readable permissions

## Placement (v1 and future)

v1 supports **explicit placement only**. Clients always specify which
server to create sessions on. If `server` is omitted, the session is
created locally.

The `POST /sessions` API accepts a `server` field. This is designed so
that a `strategy` field can be added later without breaking changes:

```json
{
  "command": "bash",
  "server": "prod-1"
}
```

Future placement strategies (not implemented in v1):

- Round-robin across healthy backends
- Least-loaded (fewest sessions)
- Tag affinity (prefer backends with matching tags)
- Custom (agent-defined placement logic)

## Deferred Features

Documented in `docs/FUTURE.md`:

- **Transitive hub chaining**: Hub A -> Hub B -> Server C session
  visibility.
- **Observer safety**: Role-based backend capabilities, discovered vs.
  enrolled separation.
- **Auto-discovery**: mDNS/gossip-based server announcement.
- **Pull-based registration**: Servers register themselves with a hub
  on startup.
- **Direct connections**: Clients bypass the proxy for data-heavy
  operations.
- **Pluggable placement strategies**: Automatic session distribution
  across backends.
