# CLI Reference

## Top-Level Flags

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--bind` | | `127.0.0.1:8080` | Address to bind the API server |
| `--token` | `WSH_TOKEN` | (auto-generated) | Authentication token |
| `--shell` | | `$SHELL` or `/bin/sh` | Shell to spawn |
| `-c` | | | Command string to execute (like `sh -c`) |
| `-i` | | | Force interactive mode |
| `--name` | | `default` | Name for the session |
| `--tag` | | | Tag for the session (repeatable) |
| `--alt-screen` | | | Use alternate screen buffer |
| `-L`, `--server-name` | `WSH_SERVER_NAME` | `default` | Server instance name (like tmux `-L`) |

## Subcommands

| Subcommand | Description |
|------------|-------------|
| `server` | Start the headless daemon (HTTP/WS + Unix socket) |
| `attach <name>` | Attach to an existing session on the server |
| `list` | List active sessions |
| `kill <name>` | Kill (destroy) a session |
| `tag <name>` | Add or remove tags on a session |
| `detach <name>` | Detach all clients from a session (session stays alive) |
| `token` | Print the server's auth token (retrieved via Unix socket) |
| `persist` | Upgrade a running server to persistent mode |
| `stop` | Stop the running wsh server |
| `servers` | Manage federated backend servers |
| `mcp` | Start an MCP server over stdio (for AI hosts) |

### `server` Flags

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--bind` | | `127.0.0.1:8080` | Address to bind the API server |
| `--token` | `WSH_TOKEN` | (auto-generated) | Authentication token |
| `--socket` | | (derived from `-L`) | Path to the Unix domain socket (overrides `-L`) |
| `-L`, `--server-name` | `WSH_SERVER_NAME` | `default` | Server instance name (like tmux `-L`) |
| `--ephemeral` | | | Exit when the last session ends |
| `--max-sessions` | | (no limit) | Maximum number of concurrent sessions |
| `--config` | `WSH_CONFIG` | `~/.config/wsh/config.toml` | Path to federation config file (TOML) |
| `--hostname` | `WSH_HOSTNAME` | (system hostname) | Override system hostname for server identity |
| `--base-prefix` | `WSH_BASE_PREFIX` | (none) | Base path prefix for all API routes (e.g., `/wsh`) |
| `--cors-origin` | | (none) | Allowed CORS origins (repeatable) |
| `--rate-limit` | | (disabled) | Rate limit in requests per second |
| `--tls-cert` | `WSH_TLS_CERT` | (none) | Path to TLS certificate file (PEM). Requires `--tls-key` |
| `--tls-key` | `WSH_TLS_KEY` | (none) | Path to TLS private key file (PEM). Requires `--tls-cert` |

### `attach` Flags

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--scrollback` | | `all` | Scrollback to replay: `all`, `none`, or a number of lines |
| `--socket` | | (derived from `-L`) | Path to the Unix domain socket (overrides `-L`) |
| `-L`, `--server-name` | `WSH_SERVER_NAME` | `default` | Server instance name |
| `--alt-screen` | | | Use alternate screen buffer |

### `list`, `kill`, `detach`, `token`, `tag`, `stop` Flags

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--socket` | | (derived from `-L`) | Path to the Unix domain socket (overrides `-L`) |
| `-L`, `--server-name` | `WSH_SERVER_NAME` | `default` | Server instance name |

### `list`, `kill` Federation Flag

| Flag | Default | Description |
|------|---------|-------------|
| `-s`, `--server` | (local) | Target a specific federated server by hostname |

### `servers` Subcommands

| Subcommand | Description |
|------------|-------------|
| `servers list` | List all servers (local + federated backends) |
| `servers add <address>` | Add a remote backend server |
| `servers remove <hostname>` | Remove a backend by hostname |
| `servers info` | Show local server info (hostname, version) |
| `servers reload` | Reload federation config from file |

`servers add` accepts an optional `--token <TOKEN>` flag for per-backend authentication.

### `persist` Flags

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--bind` | | `127.0.0.1:8080` | Address of the HTTP/WS API server |
| `--token` | `WSH_TOKEN` | | Authentication token |

## Named Instances

Run multiple independent servers with `-L` (like tmux's `-L`):

```bash
# Start two isolated servers
wsh server -L project-a --bind 127.0.0.1:8080
wsh server -L project-b --bind 127.0.0.1:9090

# Each has its own sessions
wsh -L project-a                  # connects to project-a
wsh list -L project-b             # lists project-b's sessions

# Or set per-project defaults via .envrc
export WSH_SERVER_NAME=myproject
wsh                               # automatically uses "myproject" instance
```

Each instance gets its own socket and lock file under `$XDG_RUNTIME_DIR/wsh/`. The default instance name is `default`.

## Authentication & TLS

When binding to localhost (default), no authentication is required. When
binding to a non-loopback address, bearer token auth is required:

```bash
# Auto-generated token (printed to stderr on startup)
wsh --bind 0.0.0.0:8080

# User-provided token
wsh --bind 0.0.0.0:8080 --token my-secret

# Or via environment variable
WSH_TOKEN=my-secret wsh --bind 0.0.0.0:8080
```

Authenticate via header or query parameter:

```bash
curl -H "Authorization: Bearer my-secret" http://host:8080/sessions/default/screen
curl 'http://host:8080/sessions/default/screen?token=my-secret'
```

### Native TLS

Enable HTTPS/WSS with `--tls-cert` and `--tls-key`:

```bash
wsh server --bind 0.0.0.0:8443 --tls-cert cert.pem --tls-key key.pem
```

When binding to a non-loopback address without TLS, a warning is logged
recommending either native TLS or a TLS-terminating reverse proxy.

See [api/authentication.md](api/authentication.md) for details.

### MCP Authentication

The `/mcp` endpoint is subject to the same bearer token requirement as the
rest of the API. When a `--token` is configured, MCP clients must attach the
token as an `Authorization: Bearer <token>` header on every HTTP request.
How to supply a bearer token varies by MCP client -- consult your client's
documentation. (The MCP specification defines an OAuth 2.1-based
authorization flow, but `wsh` does not currently implement it; a static
bearer token is used instead.)

If your MCP client does not support bearer tokens, bind the server to
localhost (the default) so that no token is required. A future release may
add the option to restrict `/mcp` to localhost connections regardless of the
server's bind address.
