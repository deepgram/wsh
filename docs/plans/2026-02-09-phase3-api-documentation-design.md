# Phase 3: API Documentation & Onboarding

> Design document for Phase 3 of wsh. This phase makes wsh's API
> thoroughly documented and properly authenticated so that humans and AI
> agents can build against it effectively.

---

## Scope

Phase 3 delivers five things:

1. **Authentication** -- bearer token auth for non-localhost bindings
2. **Structured errors** -- consistent JSON error format with fine-grained codes
3. **CLI flags** -- `--token`, `--shell`, env var support
4. **API documentation** -- hand-written OpenAPI spec + narrative markdown guides
5. **Doc serving** -- wsh serves its own docs and OpenAPI spec at runtime

Rate limiting and connection limits are explicitly deferred. Buffer size
configurability (scrollback, channel capacities) is deferred -- current
defaults are sensible and nobody has asked to change them.

---

## Authentication

### When Auth Applies

Auth is conditional on the bind address:

- **Loopback** (`127.0.0.1`, `::1`): no auth required. You already have
  local access to the machine.
- **Non-loopback** (e.g., `0.0.0.0`, `192.168.1.50`): bearer token
  required on all routes except `GET /health`.

### Token Lifecycle

Resolution order:

1. `--token <TOKEN>` CLI flag (highest priority)
2. `WSH_TOKEN` environment variable
3. Auto-generate a random 32-byte hex token

When auto-generated, the token is printed to stderr on startup:

```
wsh: API token (required for non-localhost): a3f8c1...
```

Printed to stderr so it doesn't interfere with terminal output.

### HTTP Auth

Token is accepted from two sources, checked in order:

1. `Authorization: Bearer <token>` header (preferred)
2. `?token=<token>` query parameter (convenience fallback)

The header is documented as the primary method. The query parameter is a
convenience for quick `curl` testing and simple scripts.

Failure responses:

- Missing token: `401 Unauthorized` with `auth_required` error code
- Wrong token: `403 Forbidden` with `auth_invalid` error code

### WebSocket Auth

Token passed as query parameter during the HTTP upgrade handshake:

```
/ws/json?token=xxx
/ws/raw?token=xxx
```

Validated before the WebSocket connection is established. Same error
codes on failure.

### Exempt Routes

`GET /health` is always unauthenticated -- useful for load balancers,
monitoring, and liveness probes.

### Implementation

A conditional axum middleware layer. At startup, if the bind address is
non-loopback, the router (minus `/health`) is wrapped with an auth
layer. If localhost, no layer is added -- zero overhead.

---

## Structured Errors

### Response Format

Every error response is JSON with a consistent shape:

```json
{
  "error": {
    "code": "overlay_not_found",
    "message": "No overlay exists with id 'abc-123'"
  }
}
```

- `code`: stable, machine-readable identifier. API consumers match on this.
- `message`: human-readable description, may include context (IDs,
  parameter names). Not stable -- do not match on this.

### Error Code Catalog

| Code                  | HTTP Status | When                                          |
|-----------------------|-------------|-----------------------------------------------|
| `auth_required`       | 401         | No token provided on a protected route        |
| `auth_invalid`        | 403         | Token provided but doesn't match              |
| `not_found`           | 404         | Unknown route                                 |
| `overlay_not_found`   | 404         | Overlay ID doesn't exist                      |
| `invalid_request`     | 400         | Malformed JSON, missing fields, bad query params |
| `invalid_overlay`     | 400         | Overlay validation failure (e.g., empty spans) |
| `invalid_input_mode`  | 400         | Bad value for input mode                      |
| `invalid_format`      | 400         | Unknown format query param (not plain/styled) |
| `channel_full`        | 503         | Internal channel at capacity                  |
| `parser_unavailable`  | 503         | Parser task has died                          |
| `input_send_failed`   | 500         | Failed to send input to PTY                   |
| `internal_error`      | 500         | Unexpected/unclassified error                 |

### Implementation

A single `ApiError` enum in `src/api/error.rs`. Each variant carries its
HTTP status, code string, and message. Implements axum's `IntoResponse`
so handlers return `Result<T, ApiError>`.

---

## CLI Flags

### Updated Interface

```
wsh [OPTIONS] [-c <command>] [-i]

Options:
  --bind <ADDR>       Bind address [default: 127.0.0.1:8080]
  --token <TOKEN>     Auth token [env: WSH_TOKEN]
  --shell <PATH>      Shell to spawn (overrides $SHELL)
  -c <COMMAND>        Execute command string
  -i                  Force interactive mode
  -h, --help          Print help
  -V, --version       Print version
```

### Shell Resolution Order

1. `--shell` flag (highest priority)
2. `$SHELL` environment variable
3. `/bin/sh` (fallback)

### What We Are NOT Making Configurable

Buffer sizes (broadcast capacity, scrollback limit, channel sizes) are
internal tuning knobs with sensible defaults. Configurability deferred
until someone needs it.

### Config Approach

No separate config file or config module. The `Args` struct (clap) is
the config. Values are read in `main()` and threaded to the components
that need them.

---

## Documentation

### File Structure

```
docs/api/
  openapi.yaml          # OpenAPI 3.1 spec (hand-written, canonical)
  README.md             # API overview, quick start, navigation
  authentication.md     # Auth model, token setup, examples
  websocket.md          # WebSocket protocols, subscription, events
  errors.md             # Complete error code catalog
  overlays.md           # Overlay system guide with examples
  input-capture.md      # Input capture guide with examples
README.md               # Repo root -- end-user onboarding
```

### Content by File

- **`docs/api/README.md`**: API overview, 30-second quick start with
  curl, links to topic pages.
- **`docs/api/authentication.md`**: When auth applies, how to pass
  tokens (header vs query param), WebSocket auth, token generation,
  curl examples.
- **`docs/api/websocket.md`**: The two WebSocket endpoints, `/ws/json`
  subscription protocol, event types and shapes, reconnection, raw mode
  for xterm.js compatibility.
- **`docs/api/errors.md`**: Error response format, full code table,
  example responses, error handling guidance.
- **`docs/api/overlays.md`**: Overlay concepts, CRUD lifecycle,
  positioning, styling, z-index, curl examples for each operation.
- **`docs/api/input-capture.md`**: Passthrough vs capture modes, escape
  hatch, input event subscription, key format, examples.
- **`openapi.yaml`**: Every endpoint, every request/response schema,
  every error code.
- **Root `README.md`**: What wsh is, installation, basic usage,
  link to API docs.

### Audience

Primary: API consumers (developers and AI agents building against the
wsh API). The root README also serves end users getting started.

### Served Endpoints

- `GET /openapi.yaml` -- raw OpenAPI spec (`Content-Type: application/yaml` or `text/yaml`)
- `GET /docs` -- serves `docs/api/README.md` (`Content-Type: text/markdown`)

Both embedded at compile time via `include_str!()`. The markdown files
in the repo are the single source of truth -- they cannot drift from the
binary because the binary literally contains them.

Not doing HTML rendering server-side. Raw markdown is simple and
agents/tooling consume it directly.

---

## Implementation Plan

### Module Changes

Split `src/api.rs` into a module directory:

```
src/api/
  mod.rs        # Router assembly, AppState, re-exports
  handlers.rs   # All endpoint handler functions
  error.rs      # ApiError enum, IntoResponse impl
  auth.rs       # Auth middleware, token validation
```

### New Dependencies

- `clap` `env` feature (for `WSH_TOKEN` env var support)
- `rand` (for token generation)

No other new crates.

### Implementation Order

1. **Restructure `src/api.rs` into module** -- pure refactor, no
   behavior change, all 131 tests still pass.
2. **`ApiError` type** -- add `src/api/error.rs`, implement
   `IntoResponse`, migrate all handlers from ad-hoc error returns to
   `Result<T, ApiError>`.
3. **CLI flags** -- add `--token` (with env support), `--shell`, thread
   values through to components.
4. **Auth middleware** -- add `src/api/auth.rs`, conditional layer based
   on bind address, token generation on startup.
5. **Documentation files** -- write the markdown guides and OpenAPI spec.
6. **Doc serving endpoints** -- `GET /openapi.yaml` and `GET /docs`
   with `include_str!()`.
7. **Tests** -- unit tests for auth and errors, integration tests for
   auth enforcement, update existing tests for new error format.

### Testing Strategy

- **Auth middleware**: unit tests for valid token, invalid token, missing
  token, localhost bypass, health exemption.
- **Error types**: unit tests for each variant's status code and JSON
  serialization.
- **Integration tests**: existing tests updated for new structured error
  format; new tests for auth enforcement end-to-end.
