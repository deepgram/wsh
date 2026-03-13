# Federation (Multi-Server Clusters)

`wsh` supports federation -- a single hub server orchestrating sessions across multiple backend servers. This lets you distribute terminal sessions across machines while managing everything from one API endpoint.

**Configure via TOML** (`~/.config/wsh/config.toml`):

```toml
# Optional: override the hub's hostname
[server]
hostname = "orchestrator"

# Default auth token for backends
default_token = "shared-secret"

# Backend servers (addresses require http:// or https:// scheme)
[[servers]]
address = "http://10.0.1.10:8080"

[[servers]]
address = "https://10.0.1.11:8443"
token = "per-server-token"

# Optional: IP access control for backend registration (SSRF mitigation)
[ip_access]
blocklist = ["169.254.0.0/16"]
allowlist = ["10.0.0.0/8", "192.168.0.0/16"]
```

**Or manage at runtime via CLI or API:**

```bash
# Start the hub with a config file
wsh server --config ~/.config/wsh/config.toml

# Or start and add backends at runtime via CLI
wsh server --bind 127.0.0.1:8080
wsh hub add http://10.0.1.10:8080
wsh hub add https://10.0.1.11:8443 --token per-server-token

# List all servers in the cluster
wsh hub list

# Check this server's status
wsh info

# Remove a backend
wsh hub remove backend-1

# Reload config from file (picks up new backends)
wsh hub reload

# Create a session on a specific backend
wsh list --server backend-1
wsh kill remote-build --server backend-1

# Or manage via HTTP API
curl -X POST http://localhost:8080/servers \
  -H 'Content-Type: application/json' \
  -d '{"address": "http://10.0.1.10:8080"}'
curl http://localhost:8080/servers
curl -X POST 'http://localhost:8080/sessions?server=backend-1' \
  -H 'Content-Type: application/json' \
  -d '{"name": "remote-build"}'
curl http://localhost:8080/sessions
```

The hub proxies session operations transparently -- all existing session endpoints work the same, with an optional `?server=<hostname>` parameter for targeting specific backends. Session listings aggregate across all healthy servers.
