# Authentication

wsh uses bearer token authentication to protect terminal access when exposed
on a network.

## When Authentication Is Required

Authentication depends on the bind address:

| Bind address | Auth required | Rationale |
|-------------|---------------|-----------|
| `127.0.0.1` (default) | No | Localhost -- you already have local access |
| `::1` | No | IPv6 loopback |
| Any other address | Yes | Network-accessible -- must authenticate |

When no authentication is required, all endpoints are open. When authentication
is required, every endpoint except `/health`, `/docs`, and `/openapi.yaml`
requires a valid token.

## Token Configuration

### Automatic Token Generation

When binding to a non-loopback address without specifying `--token`, wsh
generates a random 32-character alphanumeric token and prints it to stderr:

```
$ wsh --bind 0.0.0.0:8080
wsh: API token: aB3kM9xR2pL7nQ4wT8yF1vJ6hD5gC0eS
```

In standalone mode (no subcommand), the token is printed to stderr after the
ephemeral server is spawned.

### Retrieving the Token

If the token has scrolled off-screen or the server was started by another
process, retrieve it from a running server via the Unix socket:

```bash
$ wsh token
aB3kM9xR2pL7nQ4wT8yF1vJ6hD5gC0eS
```

This uses the local Unix socket (no auth required) to retrieve the configured
token. It prints the token to stdout, or exits with an error if no token is
configured (i.e., the server is on localhost).

### Providing Your Own Token

Use the `--token` flag or `WSH_TOKEN` environment variable:

```bash
# Via flag
wsh --bind 0.0.0.0:8080 --token my-secret-token

# Via environment variable
export WSH_TOKEN=my-secret-token
wsh --bind 0.0.0.0:8080
```

The flag takes precedence over the environment variable.

## Sending Credentials

### Authorization Header

All HTTP and WebSocket requests authenticate via the `Authorization` header:

```
Authorization: Bearer <token>
```

```bash
curl -H "Authorization: Bearer my-secret-token" http://host:8080/sessions
```

### WebSocket Authentication (Ticket Exchange)

Browser WebSocket connections cannot set custom HTTP headers, so wsh provides
a ticket exchange flow:

1. **Acquire a ticket** by authenticating with your Bearer token:

   ```bash
   curl -X POST -H "Authorization: Bearer my-secret-token" \
     http://host:8080/auth/ws-ticket
   ```

   Response:

   ```json
   { "ticket": "aB3kM9xR2pL7nQ4wT8yF1vJ6hD5gC0eS" }
   ```

2. **Connect the WebSocket** with the ticket as a query parameter:

   ```bash
   websocat 'ws://host:8080/sessions/default/ws/json?ticket=aB3kM9xR2pL7nQ4wT8yF1vJ6hD5gC0eS'
   ```

Tickets are:
- **Single-use**: consumed on first WebSocket upgrade
- **Short-lived**: expire after 30 seconds
- **Limited**: at most 1024 pending tickets at a time

For non-browser clients that can set headers (e.g., `websocat -H`), the
Authorization header works directly on WebSocket upgrade requests:

```bash
websocat -H 'Authorization: Bearer my-secret-token' ws://host:8080/sessions/default/ws/json
```

## Error Responses

| Status | Code | Meaning |
|--------|------|---------|
| `401` | `auth_required` | No token provided |
| `403` | `auth_invalid` | Token provided but incorrect |

**401 example:**

```json
{
  "error": {
    "code": "auth_required",
    "message": "Authentication required. Provide a token via the Authorization header."
  }
}
```

**403 example:**

```json
{
  "error": {
    "code": "auth_invalid",
    "message": "Invalid authentication token."
  }
}
```

## Web UI

When the web UI (`/ui`) connects to a server that requires authentication, it
automatically detects the 401/403 response and presents a token prompt. Enter
the token (from `wsh token` or the server's stderr output) and the web UI will
store it in `localStorage` for future sessions. The web UI uses the ticket
exchange flow internally -- it acquires a short-lived ticket via
`POST /auth/ws-ticket` before each WebSocket connection.

## Security Notes

- wsh provides **authentication**, not **encryption**. For remote access over
  untrusted networks, use SSH tunneling, Tailscale/WireGuard, or a reverse
  proxy with TLS.
- Tokens are compared in constant time to prevent timing attacks.
- `/health`, `/docs`, and `/openapi.yaml` are always unauthenticated so
  monitoring tools and documentation browsers work without credentials.
- When binding to a non-localhost address, rate limiting (100 req/s per IP)
  is applied by default. Override with `--rate-limit`.
