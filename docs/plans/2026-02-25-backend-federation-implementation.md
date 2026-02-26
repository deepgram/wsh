# Backend Server Federation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make every `wsh` server a federation-capable hub that can proxy to remote `wsh` servers, presenting a unified API across the cluster.

**Architecture:** Every server maintains a backend registry of remote servers, connected via persistent WebSocket. Session operations are unified — same endpoints for local and remote, with `?server=` for disambiguation. Config file (TOML) provides initial backends; API allows runtime mutation. No "hub mode" — federation is intrinsic.

**Tech Stack:** Rust, tokio, axum 0.8, tokio-tungstenite (WS client), toml (config parsing), serde, Preact (web UI)

**Design doc:** `docs/plans/2026-02-25-backend-federation-design.md`

---

## Phase 1: Foundation — Server Identity & Config

### Task 1: Add `toml` dependency and config types

**Files:**
- Modify: `Cargo.toml`
- Create: `src/config.rs`
- Modify: `src/lib.rs`
- Test: `src/config.rs` (inline `#[cfg(test)]` module)

**Step 1: Write the failing test**

In `src/config.rs`, write a test that parses a TOML string into a
`FederationConfig` struct:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let toml = r#"
            [[servers]]
            address = "10.0.1.10:8080"
        "#;
        let config: FederationConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.servers[0].address, "10.0.1.10:8080");
        assert!(config.servers[0].token.is_none());
        assert!(config.server.is_none());
        assert!(config.default_token.is_none());
    }

    #[test]
    fn parse_full_config() {
        let toml = r#"
            default_token = "shared-secret"

            [server]
            hostname = "orchestrator-1"

            [[servers]]
            address = "10.0.1.10:8080"

            [[servers]]
            address = "10.0.1.11:8080"
            token = "per-server-token"
        "#;
        let config: FederationConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.default_token.as_deref(), Some("shared-secret"));
        assert_eq!(
            config.server.as_ref().unwrap().hostname.as_deref(),
            Some("orchestrator-1")
        );
        assert_eq!(config.servers.len(), 2);
        assert_eq!(
            config.servers[1].token.as_deref(),
            Some("per-server-token")
        );
    }

    #[test]
    fn parse_empty_config() {
        let toml = "";
        let config: FederationConfig = toml::from_str(toml).unwrap();
        assert!(config.servers.is_empty());
    }

    #[test]
    fn serialize_roundtrip() {
        let config = FederationConfig {
            server: Some(ServerIdentityConfig {
                hostname: Some("my-host".into()),
            }),
            default_token: Some("tok".into()),
            servers: vec![
                BackendServerConfig {
                    address: "10.0.1.10:8080".into(),
                    token: None,
                },
                BackendServerConfig {
                    address: "10.0.1.11:8080".into(),
                    token: Some("specific".into()),
                },
            ],
        };
        let serialized = toml::to_string_pretty(&config).unwrap();
        let reparsed: FederationConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(reparsed.servers.len(), 2);
        assert_eq!(reparsed.default_token.as_deref(), Some("tok"));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop -c sh -c "cargo test -p wsh --lib config::tests -- --nocapture"`
Expected: FAIL — module `config` does not exist

**Step 3: Write minimal implementation**

Add `toml` to `Cargo.toml` under `[dependencies]`:
```toml
toml = "0.8"
```

Create `src/config.rs`:
```rust
use serde::{Deserialize, Serialize};

/// Top-level federation config, loaded from TOML.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FederationConfig {
    /// Local server identity overrides.
    pub server: Option<ServerIdentityConfig>,
    /// Default token used for backends that don't specify their own.
    pub default_token: Option<String>,
    /// Backend servers to connect to.
    #[serde(default)]
    pub servers: Vec<BackendServerConfig>,
}

/// Server identity section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerIdentityConfig {
    /// Override system hostname.
    pub hostname: Option<String>,
}

/// A single backend server entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendServerConfig {
    /// Network address (host:port).
    pub address: String,
    /// Per-server auth token. Falls back to default_token, then
    /// local server's own token.
    pub token: Option<String>,
}

impl FederationConfig {
    /// Load config from a TOML file path. Returns None if file doesn't exist.
    pub fn load(path: &std::path::Path) -> Result<Option<Self>, ConfigError> {
        if !path.exists() {
            return Ok(None);
        }
        let contents = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::ReadFailed(path.to_path_buf(), e))?;
        let config: Self =
            toml::from_str(&contents).map_err(|e| ConfigError::ParseFailed(path.to_path_buf(), e))?;
        Ok(Some(config))
    }

    /// Save config to a TOML file path.
    pub fn save(&self, path: &std::path::Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ConfigError::WriteFailed(path.to_path_buf(), e))?;
        }
        let contents = toml::to_string_pretty(self)
            .map_err(|e| ConfigError::SerializeFailed(e))?;
        std::fs::write(path, contents)
            .map_err(|e| ConfigError::WriteFailed(path.to_path_buf(), e))?;
        Ok(())
    }
}

/// Errors that can occur when loading or saving config.
#[derive(Debug)]
pub enum ConfigError {
    ReadFailed(std::path::PathBuf, std::io::Error),
    ParseFailed(std::path::PathBuf, toml::de::Error),
    WriteFailed(std::path::PathBuf, std::io::Error),
    SerializeFailed(toml::ser::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReadFailed(path, e) => write!(f, "Failed to read config {}: {}", path.display(), e),
            Self::ParseFailed(path, e) => write!(f, "Failed to parse config {}: {}", path.display(), e),
            Self::WriteFailed(path, e) => write!(f, "Failed to write config {}: {}", path.display(), e),
            Self::SerializeFailed(e) => write!(f, "Failed to serialize config: {}", e),
        }
    }
}

impl std::error::Error for ConfigError {}
```

Add to `src/lib.rs`:
```rust
pub mod config;
```

**Step 4: Run test to verify it passes**

Run: `nix develop -c sh -c "cargo test -p wsh --lib config::tests -- --nocapture"`
Expected: PASS (all 4 tests)

**Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/config.rs src/lib.rs
git commit -m "feat(federation): add TOML config parsing for backend servers"
```

---

### Task 2: Server hostname resolution

**Files:**
- Modify: `src/config.rs`
- Test: `src/config.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[test]
fn resolve_hostname_from_config() {
    let config = FederationConfig {
        server: Some(ServerIdentityConfig {
            hostname: Some("my-custom-host".into()),
        }),
        ..Default::default()
    };
    assert_eq!(
        resolve_hostname(config.server.as_ref()),
        "my-custom-host"
    );
}

#[test]
fn resolve_hostname_falls_back_to_system() {
    let hostname = resolve_hostname(None);
    // Should not be empty — gethostname always returns something
    assert!(!hostname.is_empty());
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop -c sh -c "cargo test -p wsh --lib config::tests -- --nocapture"`
Expected: FAIL — `resolve_hostname` not found

**Step 3: Write minimal implementation**

Add to `src/config.rs`:
```rust
/// Resolve the server's hostname. Uses config override if present,
/// otherwise falls back to system hostname.
pub fn resolve_hostname(server_config: Option<&ServerIdentityConfig>) -> String {
    if let Some(config) = server_config {
        if let Some(hostname) = &config.hostname {
            return hostname.clone();
        }
    }
    // Fall back to system hostname
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string())
}
```

Add `hostname` to `Cargo.toml`:
```toml
hostname = "0.4"
```

**Step 4: Run tests**

Run: `nix develop -c sh -c "cargo test -p wsh --lib config::tests -- --nocapture"`
Expected: PASS

**Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/config.rs
git commit -m "feat(federation): server hostname resolution with config override"
```

---

### Task 3: Backend registry data structure

**Files:**
- Create: `src/federation/mod.rs`
- Create: `src/federation/registry.rs`
- Modify: `src/lib.rs`
- Test: `src/federation/registry.rs` (inline tests)

**Step 1: Write the failing tests**

In `src/federation/registry.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_registry_is_empty() {
        let reg = BackendRegistry::new();
        assert!(reg.list().is_empty());
    }

    #[test]
    fn add_and_list_backend() {
        let reg = BackendRegistry::new();
        reg.add(BackendEntry {
            address: "10.0.1.10:8080".into(),
            token: Some("tok".into()),
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
        })
        .unwrap();
        let list = reg.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].address, "10.0.1.10:8080");
    }

    #[test]
    fn remove_backend_by_address() {
        let reg = BackendRegistry::new();
        reg.add(BackendEntry {
            address: "10.0.1.10:8080".into(),
            token: Some("tok".into()),
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
        })
        .unwrap();
        assert!(reg.remove_by_address("10.0.1.10:8080"));
        assert!(reg.list().is_empty());
    }

    #[test]
    fn remove_backend_by_hostname() {
        let reg = BackendRegistry::new();
        reg.add(BackendEntry {
            address: "10.0.1.10:8080".into(),
            token: Some("tok".into()),
            hostname: Some("prod-1".into()),
            health: BackendHealth::Healthy,
            role: BackendRole::Member,
        })
        .unwrap();
        assert!(reg.remove_by_hostname("prod-1"));
        assert!(reg.list().is_empty());
    }

    #[test]
    fn duplicate_address_rejected() {
        let reg = BackendRegistry::new();
        let entry = BackendEntry {
            address: "10.0.1.10:8080".into(),
            token: None,
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
        };
        reg.add(entry.clone()).unwrap();
        assert!(reg.add(entry).is_err());
    }

    #[test]
    fn hostname_collision_rejected() {
        let reg = BackendRegistry::new();
        reg.add(BackendEntry {
            address: "10.0.1.10:8080".into(),
            token: None,
            hostname: Some("same-host".into()),
            health: BackendHealth::Healthy,
            role: BackendRole::Member,
        })
        .unwrap();
        let result = reg.add(BackendEntry {
            address: "10.0.1.11:8080".into(),
            token: None,
            hostname: Some("same-host".into()),
            health: BackendHealth::Healthy,
            role: BackendRole::Member,
        });
        assert!(result.is_err());
    }

    #[test]
    fn set_hostname_updates_entry() {
        let reg = BackendRegistry::new();
        reg.add(BackendEntry {
            address: "10.0.1.10:8080".into(),
            token: None,
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
        })
        .unwrap();
        reg.set_hostname("10.0.1.10:8080", "prod-1").unwrap();
        let list = reg.list();
        assert_eq!(list[0].hostname.as_deref(), Some("prod-1"));
    }

    #[test]
    fn set_health_updates_entry() {
        let reg = BackendRegistry::new();
        reg.add(BackendEntry {
            address: "10.0.1.10:8080".into(),
            token: None,
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
        })
        .unwrap();
        reg.set_health("10.0.1.10:8080", BackendHealth::Unavailable);
        let list = reg.list();
        assert_eq!(list[0].health, BackendHealth::Unavailable);
    }

    #[test]
    fn get_by_hostname() {
        let reg = BackendRegistry::new();
        reg.add(BackendEntry {
            address: "10.0.1.10:8080".into(),
            token: None,
            hostname: Some("prod-1".into()),
            health: BackendHealth::Healthy,
            role: BackendRole::Member,
        })
        .unwrap();
        let entry = reg.get_by_hostname("prod-1").unwrap();
        assert_eq!(entry.address, "10.0.1.10:8080");
    }

    #[test]
    fn healthy_backends_only() {
        let reg = BackendRegistry::new();
        reg.add(BackendEntry {
            address: "10.0.1.10:8080".into(),
            token: None,
            hostname: Some("healthy".into()),
            health: BackendHealth::Healthy,
            role: BackendRole::Member,
        })
        .unwrap();
        reg.add(BackendEntry {
            address: "10.0.1.11:8080".into(),
            token: None,
            hostname: Some("down".into()),
            health: BackendHealth::Unavailable,
            role: BackendRole::Member,
        })
        .unwrap();
        let healthy = reg.healthy();
        assert_eq!(healthy.len(), 1);
        assert_eq!(healthy[0].hostname.as_deref(), Some("healthy"));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop -c sh -c "cargo test -p wsh --lib federation::registry::tests -- --nocapture"`
Expected: FAIL — module not found

**Step 3: Write minimal implementation**

Create `src/federation/mod.rs`:
```rust
pub mod registry;
```

Create `src/federation/registry.rs`:
```rust
use parking_lot::RwLock;
use serde::Serialize;
use std::sync::Arc;

/// Health state of a backend server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendHealth {
    /// Initial state — connecting for the first time.
    Connecting,
    /// WebSocket is connected and healthy.
    Healthy,
    /// Connection lost, attempting reconnection.
    Unavailable,
}

/// Role/capability of a backend (v1: always Member).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendRole {
    /// Full cluster member — can accept session creation.
    Member,
}

/// A registered backend server.
#[derive(Debug, Clone, Serialize)]
pub struct BackendEntry {
    pub address: String,
    #[serde(skip_serializing)]
    pub token: Option<String>,
    pub hostname: Option<String>,
    pub health: BackendHealth,
    pub role: BackendRole,
}

/// Thread-safe registry of backend servers.
#[derive(Clone)]
pub struct BackendRegistry {
    inner: Arc<RwLock<Vec<BackendEntry>>>,
}

/// Errors from registry operations.
#[derive(Debug)]
pub enum RegistryError {
    DuplicateAddress(String),
    HostnameCollision(String),
    NotFound(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateAddress(addr) => write!(f, "Backend already registered: {}", addr),
            Self::HostnameCollision(name) => write!(f, "Hostname already in use: {}", name),
            Self::NotFound(key) => write!(f, "Backend not found: {}", key),
        }
    }
}

impl std::error::Error for RegistryError {}

impl BackendRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Add a backend. Rejects duplicate addresses and hostname collisions.
    pub fn add(&self, entry: BackendEntry) -> Result<(), RegistryError> {
        let mut backends = self.inner.write();
        // Check duplicate address
        if backends.iter().any(|b| b.address == entry.address) {
            return Err(RegistryError::DuplicateAddress(entry.address));
        }
        // Check hostname collision
        if let Some(ref hostname) = entry.hostname {
            if backends.iter().any(|b| b.hostname.as_deref() == Some(hostname)) {
                return Err(RegistryError::HostnameCollision(hostname.clone()));
            }
        }
        backends.push(entry);
        Ok(())
    }

    /// Remove a backend by address. Returns true if found.
    pub fn remove_by_address(&self, address: &str) -> bool {
        let mut backends = self.inner.write();
        let len_before = backends.len();
        backends.retain(|b| b.address != address);
        backends.len() < len_before
    }

    /// Remove a backend by hostname. Returns true if found.
    pub fn remove_by_hostname(&self, hostname: &str) -> bool {
        let mut backends = self.inner.write();
        let len_before = backends.len();
        backends.retain(|b| b.hostname.as_deref() != Some(hostname));
        backends.len() < len_before
    }

    /// List all backends.
    pub fn list(&self) -> Vec<BackendEntry> {
        self.inner.read().clone()
    }

    /// List only healthy backends.
    pub fn healthy(&self) -> Vec<BackendEntry> {
        self.inner
            .read()
            .iter()
            .filter(|b| b.health == BackendHealth::Healthy)
            .cloned()
            .collect()
    }

    /// Look up a backend by hostname.
    pub fn get_by_hostname(&self, hostname: &str) -> Option<BackendEntry> {
        self.inner
            .read()
            .iter()
            .find(|b| b.hostname.as_deref() == Some(hostname))
            .cloned()
    }

    /// Look up a backend by address.
    pub fn get_by_address(&self, address: &str) -> Option<BackendEntry> {
        self.inner
            .read()
            .iter()
            .find(|b| b.address == address)
            .cloned()
    }

    /// Set the hostname for a backend (called after connecting and
    /// querying the backend's /server/info).
    pub fn set_hostname(
        &self,
        address: &str,
        hostname: &str,
    ) -> Result<(), RegistryError> {
        let mut backends = self.inner.write();
        // Check for hostname collision with OTHER backends
        if backends
            .iter()
            .any(|b| b.address != address && b.hostname.as_deref() == Some(hostname))
        {
            return Err(RegistryError::HostnameCollision(hostname.to_string()));
        }
        if let Some(entry) = backends.iter_mut().find(|b| b.address == address) {
            entry.hostname = Some(hostname.to_string());
            Ok(())
        } else {
            Err(RegistryError::NotFound(address.to_string()))
        }
    }

    /// Update health state for a backend.
    pub fn set_health(&self, address: &str, health: BackendHealth) {
        let mut backends = self.inner.write();
        if let Some(entry) = backends.iter_mut().find(|b| b.address == address) {
            entry.health = health;
        }
    }
}
```

Add to `src/lib.rs`:
```rust
pub mod federation;
```

**Step 4: Run tests**

Run: `nix develop -c sh -c "cargo test -p wsh --lib federation::registry::tests -- --nocapture"`
Expected: PASS (all 10 tests)

**Step 5: Commit**

```bash
git add src/federation/ src/lib.rs
git commit -m "feat(federation): backend registry data structure with health tracking"
```

---

### Task 4: Token resolution cascade

**Files:**
- Create: `src/federation/auth.rs`
- Modify: `src/federation/mod.rs`
- Test: `src/federation/auth.rs` (inline tests)

**Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_server_token_wins() {
        let token = resolve_backend_token(
            Some("per-server"),
            Some("default"),
            Some("local"),
        );
        assert_eq!(token.as_deref(), Some("per-server"));
    }

    #[test]
    fn default_token_fallback() {
        let token = resolve_backend_token(None, Some("default"), Some("local"));
        assert_eq!(token.as_deref(), Some("default"));
    }

    #[test]
    fn local_token_fallback() {
        let token = resolve_backend_token(None, None, Some("local"));
        assert_eq!(token.as_deref(), Some("local"));
    }

    #[test]
    fn no_token_available() {
        let token = resolve_backend_token(None, None, None);
        assert!(token.is_none());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop -c sh -c "cargo test -p wsh --lib federation::auth::tests -- --nocapture"`
Expected: FAIL — module not found

**Step 3: Write minimal implementation**

Create `src/federation/auth.rs`:
```rust
/// Resolve the auth token for connecting to a backend server.
///
/// Cascade order:
/// 1. Per-server token (explicit in config for this backend)
/// 2. Default token (from config file)
/// 3. Local server's own token (--token flag)
///
/// Returns None if no token is available at any level.
pub fn resolve_backend_token(
    per_server: Option<&str>,
    default_token: Option<&str>,
    local_token: Option<&str>,
) -> Option<String> {
    per_server
        .or(default_token)
        .or(local_token)
        .map(|s| s.to_string())
}
```

Add to `src/federation/mod.rs`:
```rust
pub mod auth;
```

**Step 4: Run tests**

Run: `nix develop -c sh -c "cargo test -p wsh --lib federation::auth::tests -- --nocapture"`
Expected: PASS

**Step 5: Commit**

```bash
git add src/federation/auth.rs src/federation/mod.rs
git commit -m "feat(federation): token resolution cascade for backend auth"
```

---

### Task 5: Wire config loading into server startup

**Files:**
- Modify: `src/main.rs` (CLI args + `run_server()`)
- Modify: `src/api/mod.rs` (AppState)
- Modify: all test files that construct AppState (9+ files)
- Test: compile check + existing tests still pass

**Step 1: Add `--config` and `--hostname` CLI args**

In `src/main.rs`, add to the `Server` variant of `Commands`:
```rust
/// Path to federation config file (TOML). Default: ~/.config/wsh/config.toml
#[arg(long, env = "WSH_CONFIG")]
config: Option<String>,

/// Override system hostname for server identity.
#[arg(long, env = "WSH_HOSTNAME")]
hostname: Option<String>,
```

**Step 2: Add federation fields to AppState**

In `src/api/mod.rs`, modify `AppState`:
```rust
pub struct AppState {
    pub sessions: SessionRegistry,
    pub shutdown: ShutdownCoordinator,
    pub server_config: Arc<ServerConfig>,
    pub server_ws_count: Arc<AtomicUsize>,
    pub mcp_session_count: Arc<AtomicUsize>,
    pub ticket_store: Arc<ticket::TicketStore>,
    pub backends: crate::federation::registry::BackendRegistry,
    pub hostname: String,
    pub federation_config_path: Option<std::path::PathBuf>,
    pub local_token: Option<String>,
    pub default_backend_token: Option<String>,
}
```

**Step 3: Update ALL AppState construction sites**

This is the most tedious part. Every test file that constructs AppState
needs the new fields. Search for `AppState {` across the codebase and
add the new fields with sensible defaults:

```rust
backends: wsh::federation::registry::BackendRegistry::new(),
hostname: "test".to_string(),
federation_config_path: None,
local_token: None,
default_backend_token: None,
```

Files to update (search with `rg "AppState \{" src/ tests/`):
- `src/main.rs` (run_server)
- `src/mcp/mod.rs` (if AppState is constructed there)
- `tests/api_integration.rs`
- `tests/auth_integration.rs`
- `tests/e2e_http.rs`
- `tests/e2e_websocket_input.rs`
- `tests/idle_integration.rs`
- `tests/input_capture_integration.rs`
- `tests/mcp_http.rs`
- `tests/overlay_integration.rs`
- `tests/panel_integration.rs`
- `tests/session_management.rs`
- `tests/tagging_integration.rs`
- `tests/ws_json_methods.rs`
- `tests/ws_server_integration.rs`
- Any other files found by the search

**Step 4: Wire config loading in run_server()**

In `src/main.rs` `run_server()`, before building AppState:

```rust
// Resolve config path
let config_path = config_arg
    .map(std::path::PathBuf::from)
    .unwrap_or_else(|| {
        dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("~/.config"))
            .join("wsh")
            .join("config.toml")
    });

// Load federation config (optional — missing file is fine)
let fed_config = crate::config::FederationConfig::load(&config_path)
    .map_err(|e| eprintln!("Warning: {}", e))
    .ok()
    .flatten()
    .unwrap_or_default();

// Resolve hostname
let hostname = hostname_arg
    .or_else(|| fed_config.server.as_ref()?.hostname.clone())
    .unwrap_or_else(|| crate::config::resolve_hostname(None));

// Build backend registry from config
let backends = crate::federation::registry::BackendRegistry::new();
// Note: actual backend connections are wired in Phase 2
```

Also add `dirs` dependency to `Cargo.toml`:
```toml
dirs = "5"
```

**Step 5: Run full test suite to verify nothing broke**

Run: `nix develop -c sh -c "cargo test"`
Expected: PASS — all existing tests still pass

**Step 6: Commit**

```bash
git add -A
git commit -m "feat(federation): wire config loading and hostname into server startup

Add --config and --hostname CLI flags. Extend AppState with federation
fields (backends, hostname, config path, token defaults). Update all
test construction sites."
```

---

### Task 6: GET /server/info endpoint

**Files:**
- Modify: `src/api/mod.rs` (add route)
- Modify: `src/api/handlers.rs` (add handler)
- Test: `tests/api_integration.rs` (or new `tests/federation_api.rs`)

**Step 1: Write the failing test**

Add to `tests/api_integration.rs` (or create `tests/federation_api.rs`):

```rust
#[tokio::test]
async fn server_info_returns_hostname() {
    let (app, _, _) = create_test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/server/info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_json(response).await;
    assert_eq!(body["hostname"], "test");
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop -c sh -c "cargo test federation_api -- --nocapture"`
Expected: FAIL — 404 not found

**Step 3: Write minimal implementation**

In `src/api/handlers.rs`, add:
```rust
pub(super) async fn server_info(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "hostname": state.hostname,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
```

In `src/api/mod.rs`, add route inside the protected router:
```rust
.route("/server/info", get(handlers::server_info))
```

**Step 4: Run tests**

Run: `nix develop -c sh -c "cargo test federation_api -- --nocapture"`
Expected: PASS

**Step 5: Commit**

```bash
git add src/api/mod.rs src/api/handlers.rs tests/
git commit -m "feat(federation): add GET /server/info endpoint"
```

---

## Phase 2: Backend Connections

### Task 7: Persistent WebSocket client to backends

**Files:**
- Create: `src/federation/connection.rs`
- Modify: `src/federation/mod.rs`
- Modify: `Cargo.toml` (move `tokio-tungstenite` from dev to normal deps)
- Test: `src/federation/connection.rs` (inline tests with mock server)

This task implements the persistent WebSocket connection to a single
backend, with ping/pong keepalive, automatic reconnection with
exponential backoff, and health state updates.

**Step 1: Write tests**

Tests for the connection manager are integration-style: spawn a local
axum WS server, connect to it, verify health transitions when it goes
down and comes back.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::timeout;
    use std::time::Duration;

    #[tokio::test]
    async fn connects_and_becomes_healthy() {
        // Spawn a minimal WS echo server
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // Accept one connection, keep it open
            let (stream, _) = listener.accept().await.unwrap();
            let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            // Just hold the connection open
            let (_sink, mut stream_rx) = ws.split();
            while stream_rx.next().await.is_some() {}
        });

        let registry = BackendRegistry::new();
        registry.add(BackendEntry {
            address: addr.to_string(),
            token: None,
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
        }).unwrap();

        let handle = BackendConnection::spawn(
            addr.to_string(),
            None,
            registry.clone(),
        );

        // Wait for healthy
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some(entry) = registry.get_by_address(&addr.to_string()) {
                    if entry.health == BackendHealth::Healthy {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("should become healthy within 5s");

        handle.shutdown();
    }
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop -c sh -c "cargo test -p wsh --lib federation::connection::tests -- --nocapture"`
Expected: FAIL — module not found

**Step 3: Write implementation**

Move `tokio-tungstenite` from `[dev-dependencies]` to `[dependencies]`
in `Cargo.toml`. Also add `futures-util` if not already present (needed
for `SplitSink`/`SplitStream`).

Create `src/federation/connection.rs` implementing:
- `BackendConnection::spawn()` — spawns a tokio task that:
  1. Connects to `ws://{address}/ws/json` with optional Bearer token
  2. On success: updates registry health to `Healthy`
  3. Runs a select! loop: ping timer (every 15s), incoming messages,
     shutdown signal
  4. On disconnect: updates health to `Unavailable`, waits with
     exponential backoff (1s, 2s, 4s, ... up to 60s), retries
- `BackendConnection::shutdown()` — signals the task to stop
- The connection task queries `GET /server/info` via HTTP on first
  connect to learn the hostname, then calls `registry.set_hostname()`

This is a substantial implementation. The key structure:

```rust
pub struct BackendConnection {
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl BackendConnection {
    pub fn spawn(
        address: String,
        token: Option<String>,
        registry: BackendRegistry,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(connection_loop(
            address, token, registry, shutdown_rx,
        ));
        Self { shutdown_tx, task }
    }

    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

async fn connection_loop(
    address: String,
    token: Option<String>,
    registry: BackendRegistry,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);

    loop {
        // Check shutdown before connecting
        if *shutdown_rx.borrow() { return; }

        // Attempt connection
        match connect(&address, token.as_deref()).await {
            Ok(ws_stream) => {
                backoff = Duration::from_secs(1); // Reset on success

                // Query hostname via HTTP
                if let Ok(hostname) = fetch_hostname(&address, token.as_deref()).await {
                    let _ = registry.set_hostname(&address, &hostname);
                }

                registry.set_health(&address, BackendHealth::Healthy);

                // Run connection loop until disconnect
                run_connection(ws_stream, &mut shutdown_rx).await;

                // If we're here, connection dropped
                if *shutdown_rx.borrow() { return; }
                registry.set_health(&address, BackendHealth::Unavailable);
            }
            Err(_) => {
                registry.set_health(&address, BackendHealth::Unavailable);
            }
        }

        // Wait before retry
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = shutdown_rx.changed() => { return; }
        }
        backoff = (backoff * 2).min(max_backoff);
    }
}
```

**Step 4: Run tests**

Run: `nix develop -c sh -c "cargo test -p wsh --lib federation::connection::tests -- --nocapture"`
Expected: PASS

**Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/federation/
git commit -m "feat(federation): persistent WebSocket client with reconnection"
```

---

### Task 8: Wire backend connections into server startup

**Files:**
- Create: `src/federation/manager.rs`
- Modify: `src/federation/mod.rs`
- Modify: `src/main.rs` (run_server)
- Test: verify startup with config containing backends

The `FederationManager` holds the registry + all active connections.
On startup, it reads config and spawns connections. It also provides
methods for runtime add/remove that the API handlers will use.

**Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn manager_from_empty_config() {
        let config = crate::config::FederationConfig::default();
        let manager = FederationManager::from_config(config, None, None);
        assert!(manager.registry().list().is_empty());
    }

    #[tokio::test]
    async fn manager_add_and_remove() {
        let manager = FederationManager::new();
        manager.add_backend("127.0.0.1:9999", Some("tok")).unwrap();
        assert_eq!(manager.registry().list().len(), 1);
        manager.remove_backend_by_address("127.0.0.1:9999");
        assert!(manager.registry().list().is_empty());
    }
}
```

**Step 2-5: Implement, test, commit**

The manager wraps `BackendRegistry` + a `HashMap<String, BackendConnection>`
keyed by address. `add_backend()` adds to registry and spawns a
connection. `remove_backend()` shuts down the connection and removes
from registry. `shutdown_all()` stops all connections.

Wire into `run_server()`: after building AppState, create
`FederationManager::from_config(fed_config, local_token, default_token)`.
Store the manager's registry in AppState. On graceful shutdown, call
`manager.shutdown_all()`.

```bash
git commit -m "feat(federation): FederationManager wires config into live connections"
```

---

## Phase 3: API Surface

### Task 9: Add `server` field to SessionInfo

**Files:**
- Modify: `src/session.rs` (SessionInfo struct)
- Modify: `src/api/handlers.rs` (session list/get handlers)
- Modify: `src/protocol.rs` (SessionInfoMsg)
- Modify: all tests that assert on SessionInfo shape
- Test: existing tests + new test for `server` field

**Step 1: Add `server` field to `SessionInfo`**

In `src/session.rs`:
```rust
#[derive(Serialize, Clone)]
pub struct SessionInfo {
    pub name: String,
    pub server: String,   // NEW — owning server's hostname
    pub pid: Option<u32>,
    pub command: String,
    pub rows: u16,
    pub cols: u16,
    pub clients: usize,
    pub tags: Vec<String>,
}
```

Update `SessionInfo` construction in `SessionRegistry::list()` and
wherever `SessionInfo` is built to include `server` from AppState's
hostname.

Similarly update `SessionInfoMsg` in `src/protocol.rs`.

**Step 2: Update handlers to pass hostname**

The list_sessions and get_session handlers need access to the hostname
(from AppState) to populate the `server` field.

**Step 3: Run full test suite, fix any assertion mismatches**

Run: `nix develop -c sh -c "cargo test"`
Expected: Some tests may fail if they assert on exact JSON shape. Fix them.

**Step 4: Commit**

```bash
git commit -m "feat(federation): add server field to SessionInfo responses"
```

---

### Task 10: GET /servers and POST /servers endpoints

**Files:**
- Modify: `src/api/handlers.rs`
- Modify: `src/api/mod.rs` (routes)
- Modify: `src/api/error.rs` (new error variants)
- Create: `tests/federation_api.rs`

**Step 1: Write failing tests**

```rust
#[tokio::test]
async fn list_servers_includes_self() {
    // GET /servers should always return at least the local server
    let response = client.get("/servers").await;
    assert_eq!(response.status(), 200);
    let body: Vec<serde_json::Value> = response.json().await;
    assert!(body.len() >= 1);
    assert_eq!(body[0]["hostname"], "test");
    assert_eq!(body[0]["health"], "healthy");
}

#[tokio::test]
async fn add_server_returns_success() {
    let response = client.post("/servers")
        .json(&json!({ "address": "10.0.1.10:8080" }))
        .await;
    assert_eq!(response.status(), 201);
}

#[tokio::test]
async fn add_duplicate_server_returns_conflict() {
    client.post("/servers")
        .json(&json!({ "address": "10.0.1.10:8080" }))
        .await;
    let response = client.post("/servers")
        .json(&json!({ "address": "10.0.1.10:8080" }))
        .await;
    assert_eq!(response.status(), 409);
}

#[tokio::test]
async fn delete_server_removes_it() {
    client.post("/servers")
        .json(&json!({ "address": "10.0.1.10:8080" }))
        .await;
    // Simulate hostname assignment
    let response = client.delete("/servers/prod-1").await;
    // ... etc
}
```

**Step 2-5: Implement handlers, add routes, add error variants, commit**

New `ApiError` variants:
- `ServerNotFound(String)` — 404
- `ServerAlreadyRegistered(String)` — 409
- `InvalidServerAddress(String)` — 400

```bash
git commit -m "feat(federation): server management API endpoints (GET/POST/DELETE /servers)"
```

---

### Task 11: `?server=` query parameter on session endpoints

**Files:**
- Modify: `src/api/handlers.rs`
- Modify: `src/api/mod.rs`
- Test: `tests/federation_api.rs`

This is the key integration point. When `?server=X` is present and X
is not the local hostname, the handler must proxy the request to the
appropriate backend.

**Step 1: Create a session resolution helper**

```rust
/// Extract the optional `server` query param. If absent or matching
/// local hostname, return the local session. If it names a registered
/// backend, proxy the request.
fn resolve_session_target(
    state: &AppState,
    session_name: &str,
    server_param: Option<&str>,
) -> Result<SessionTarget, ApiError> {
    match server_param {
        None => {
            // Local
            let session = state.sessions.get(session_name)
                .ok_or_else(|| ApiError::SessionNotFound(session_name.into()))?;
            Ok(SessionTarget::Local(session))
        }
        Some(server) if server == state.hostname => {
            // Explicit local
            let session = state.sessions.get(session_name)
                .ok_or_else(|| ApiError::SessionNotFound(session_name.into()))?;
            Ok(SessionTarget::Local(session))
        }
        Some(server) => {
            // Remote — look up backend
            let backend = state.backends.get_by_hostname(server)
                .ok_or_else(|| ApiError::ServerNotFound(server.into()))?;
            if backend.health != BackendHealth::Healthy {
                return Err(ApiError::ServerUnavailable(server.into()));
            }
            Ok(SessionTarget::Remote { backend, session: session_name.into() })
        }
    }
}
```

**Step 2: Update handlers to use resolve_session_target**

Start with a few key handlers: `get_screen`, `send_input`,
`get_scrollback`. For remote targets, proxy the request over the
backend's WebSocket connection.

**Step 3: Implement proxy dispatch**

The proxy sends a WS JSON-RPC request to the backend and awaits the
response. This uses the persistent WebSocket connection managed by
`BackendConnection`.

**Step 4: Tests**

Write integration tests that:
1. Stand up two `wsh` servers
2. Register one as a backend of the other
3. Create a session on the backend
4. Access it through the frontend server via `?server=`

**Step 5: Commit**

```bash
git commit -m "feat(federation): proxy session operations via ?server= query parameter"
```

---

### Task 12: Cross-server session listing

**Files:**
- Modify: `src/api/handlers.rs` (list_sessions)
- Test: `tests/federation_api.rs`

**Step 1: Modify list_sessions to aggregate**

When listing sessions, the handler should:
1. Get local sessions (with `server: local_hostname`)
2. For each healthy backend, query sessions via WS
3. Merge results, adding `server: backend_hostname` to each

**Step 2: Tests**

Verify that `GET /sessions` returns sessions from both local and remote
servers, each with the correct `server` field.

**Step 3: Commit**

```bash
git commit -m "feat(federation): aggregate sessions from all backends in GET /sessions"
```

---

### Task 13: Cross-server session creation

**Files:**
- Modify: `src/api/handlers.rs` (create_session)
- Test: `tests/federation_api.rs`

When `POST /sessions` includes `"server": "prod-1"` and that's a remote
backend, proxy the creation request. Return the session info with the
remote server's hostname.

When `server` is omitted or matches local, create locally (existing
behavior).

When `server` names a non-existent or unavailable backend, return 404
or 503 respectively.

```bash
git commit -m "feat(federation): proxy session creation to remote backends"
```

---

## Phase 4: Socket Protocol & CLI

### Task 14: New socket protocol frame types

**Files:**
- Modify: `src/protocol.rs`
- Test: `src/protocol.rs` (inline tests)

Add frame type constants and message structs for:
- `LIST_SERVERS` (0x20) / `LIST_SERVERS_RESPONSE` (0x21)
- `ADD_SERVER` (0x22) / `ADD_SERVER_RESPONSE` (0x23)
- `REMOVE_SERVER` (0x24) / `REMOVE_SERVER_RESPONSE` (0x25)
- `RELOAD_CONFIG` (0x26) / `RELOAD_CONFIG_RESPONSE` (0x27)
- `SERVER_INFO` (0x28) / `SERVER_INFO_RESPONSE` (0x29)

Also add optional `server` field to `CreateSessionMsg`,
`ListSessionsResponseMsg`/`SessionInfoMsg`, and other existing message
structs.

```bash
git commit -m "feat(federation): add server management frame types to socket protocol"
```

---

### Task 15: Handle new frames in socket server

**Files:**
- Modify: `src/server.rs` (handle_client)
- Test: `tests/server_client_e2e.rs`

Wire the new frame types into `handle_client()` so they dispatch to the
backend registry / federation manager.

```bash
git commit -m "feat(federation): handle server management frames in socket server"
```

---

### Task 16: CLI subcommands for server management

**Files:**
- Modify: `src/main.rs`
- Modify: `src/client.rs`
- Test: e2e tests

Add `Servers` subcommand with sub-subcommands:
```rust
Servers {
    #[command(subcommand)]
    action: Option<ServersAction>,
    server_name: String,
    socket: Option<String>,
}

enum ServersAction {
    Add { address: String, token: Option<String> },
    Remove { hostname: String },
    Reload,
    Status { hostname: String },
}
```

Default (no action) = list servers.

```bash
git commit -m "feat(federation): wsh servers CLI subcommands"
```

---

### Task 17: Add `--server` / `-s` flag to existing CLI commands

**Files:**
- Modify: `src/main.rs` (List, Kill, Detach, Tag commands)
- Modify: `src/client.rs`
- Modify: `src/protocol.rs` (add server field to existing msgs)

Add optional `--server` / `-s` flag to `List`, `Kill`, `Detach`, `Tag`.
The client sends the `server` field in the protocol message; the server
routes accordingly.

```bash
git commit -m "feat(federation): add --server flag to existing CLI commands"
```

---

## Phase 5: MCP Tools

### Task 18: Add `server` parameter to existing MCP tools

**Files:**
- Modify: `src/mcp/tools.rs` (all param structs)
- Modify: `src/mcp/mod.rs` (tool implementations)
- Test: `tests/mcp_http.rs`

Add `pub server: Option<String>` to every session-scoped param struct:
`SendInputParams`, `GetScreenParams`, `GetScrollbackParams`,
`AwaitIdleParams`, `RunCommandParams`, `OverlayParams`,
`RemoveOverlayParams`, `PanelParams`, `RemovePanelParams`,
`InputModeParams`, `ScreenModeParams`, `CreateSessionParams`,
`ListSessionsParams`, `ManageSessionParams`.

In each tool implementation, resolve the session target using the same
`resolve_session_target` logic from the HTTP handlers.

```bash
git commit -m "feat(federation): add server parameter to all MCP session tools"
```

---

### Task 19: New MCP server management tools

**Files:**
- Modify: `src/mcp/tools.rs` (new param structs)
- Modify: `src/mcp/mod.rs` (new tool implementations)
- Test: `tests/mcp_http.rs`

Add 4 new tools:
- `wsh_list_servers` — `ListServersParams {}` (no required params)
- `wsh_add_server` — `AddServerParams { address: String, token: Option<String> }`
- `wsh_remove_server` — `RemoveServerParams { hostname: String }`
- `wsh_server_status` — `ServerStatusParams { hostname: String }`

Register them via `#[tool_router]` alongside existing tools.

```bash
git commit -m "feat(federation): MCP server management tools"
```

---

## Phase 6: Web UI

### Task 20: Add `server` field to frontend types

**Files:**
- Modify: `web/src/api/types.ts`
- Modify: `web/src/state/sessions.ts`

Add `server: string` to `SessionInfo` interface. Update session state
management to track server per session.

```bash
git commit -m "feat(federation): add server field to frontend session types"
```

---

### Task 21: Server badge on session mini-previews

**Files:**
- Modify: `web/src/components/ThumbnailCell.tsx` (or equivalent)
- Modify: `web/src/components/Sidebar.tsx`
- Add CSS for server badge

Add a small badge/pill showing the server hostname on each session
thumbnail. Style it distinctly (muted background, small font) so it's
visible but not dominant.

```bash
git commit -m "feat(federation): server badge on session mini-previews"
```

---

### Task 22: Server grouping in sidebar

**Files:**
- Modify: `web/src/state/groups.ts`
- Modify: `web/src/components/Sidebar.tsx`

Add a grouping mode selector (tag / server / server>tag / tag>server).
The `groups` computed signal needs to support grouping by the `server`
field in addition to tags.

```bash
git commit -m "feat(federation): server grouping dimension in sidebar"
```

---

### Task 23: Command palette — "Go to server" command

**Files:**
- Modify: `web/src/components/CommandPalette.tsx`
- Modify: `web/src/state/sessions.ts` (derive unique server list)

Add "Go to server: ..." command that filters sessions to a specific
server, matching the pattern of "Go to tag: ...".

```bash
git commit -m "feat(federation): command palette server navigation"
```

---

### Task 24: Server list/status view

**Files:**
- Create: `web/src/components/ServerList.tsx`
- Modify: `web/src/api/ws.ts` (add listServers method)
- Modify: `web/src/components/CommandPalette.tsx`

A view (accessible from command palette or sidebar) showing all
registered backends with: hostname, address, health indicator (green/
yellow/red dot), session count, connection latency.

```bash
git commit -m "feat(federation): server list/status view in web UI"
```

---

## Phase 7: Security & Hardening

### Task 25: Hostname and address validation

**Files:**
- Modify: `src/federation/registry.rs`
- Modify: `src/api/handlers.rs` (server management endpoints)
- Test: unit tests + integration tests

Validate:
- Hostnames: alphanumeric + hyphens + dots, 1-253 chars, no leading/
  trailing hyphens
- Addresses: must be `host:port` format, no `file://` or other schemes,
  no localhost/loopback when registering as backend (prevent SSRF)

```bash
git commit -m "security(federation): validate hostnames and addresses on registration"
```

---

### Task 26: Backend response sanitization

**Files:**
- Create: `src/federation/sanitize.rs`
- Modify: `src/federation/mod.rs`
- Test: unit tests

When proxying responses from backends:
- Validate JSON schema matches expected shape
- Strip unexpected fields
- Validate string fields (session names, hostnames) against patterns
- Enforce size limits per response
- Reject responses that fail validation with a 502 Bad Gateway

```bash
git commit -m "security(federation): sanitize and validate backend responses"
```

---

### Task 27: Config file permission check

**Files:**
- Modify: `src/config.rs`
- Test: unit tests (platform-specific)

On Unix: warn (log) if config file is world-readable (mode & 0o004).
The config may contain tokens.

```bash
git commit -m "security(federation): warn on world-readable config file"
```

---

### Task 28: Security integration tests

**Files:**
- Create: `tests/federation_security.rs`

Tests:
- SSRF: attempt to register `127.0.0.1:8080` or `[::1]:8080` as
  backend — should be rejected
- Malformed backend responses: mock a backend that returns invalid JSON,
  oversized payloads, unexpected fields — verify the hub handles
  gracefully
- Token not leaked: verify `GET /servers` response does not include
  tokens
- Config permission check: create a world-readable config, verify
  warning is logged

```bash
git commit -m "test(federation): security integration tests"
```

---

## Phase 8: Skills, Documentation & Polish

### Task 29: Cluster Orchestration skill

**Files:**
- Create: `skills/wsh/cluster-orchestration.md`
- Modify: `src/mcp/prompts.rs` (add new prompt)
- Modify: `src/mcp/mod.rs` (register prompt)

Write the skill document covering:
- Server registration and monitoring patterns
- Distributed session creation across servers
- Tag-based cross-server workflows
- Cross-server quiescence patterns
- Failure handling strategies

The skill must be protocol-agnostic (the "what", not the "how").

```bash
git commit -m "feat(federation): cluster orchestration skill for AI agents"
```

---

### Task 30: Update existing skills

**Files:**
- Modify: `skills/wsh/core.md` — document `server` parameter
- Modify: `skills/wsh/multi-session.md` — note cross-server sessions

```bash
git commit -m "docs(federation): update core and multi-session skills"
```

---

### Task 31: Update API documentation

**Files:**
- Modify: `docs/API.md` (or equivalent) — new endpoints, `server`
  field, query parameter
- Modify: `docs/openapi.yaml` (if it exists) — new endpoints/schemas
- Modify: `README.md` — mention federation capability

```bash
git commit -m "docs(federation): update API docs and README"
```

---

### Task 32: End-to-end integration test

**Files:**
- Create: `tests/federation_e2e.rs`

Full end-to-end test:
1. Start `wsh server` instance A (the hub) with a TOML config
2. Start `wsh server` instance B (a backend)
3. Register B as a backend of A (via config or API)
4. Wait for B to become healthy in A's server list
5. Create a session on B through A's API
6. Send input, read screen, verify output — all through A
7. List sessions on A — verify both local and remote appear
8. Kill B — verify A marks it unavailable and hides its sessions
9. Restart B — verify A reconnects and sessions reappear
10. Clean up

Both instances use unique `--server-name` values and ephemeral mode.

```bash
git commit -m "test(federation): full end-to-end federation test"
```

---

## Implementation Notes

### Files touched summary

**New files:**
- `src/config.rs`
- `src/federation/mod.rs`
- `src/federation/registry.rs`
- `src/federation/auth.rs`
- `src/federation/connection.rs`
- `src/federation/manager.rs`
- `src/federation/sanitize.rs`
- `tests/federation_api.rs`
- `tests/federation_security.rs`
- `tests/federation_e2e.rs`
- `skills/wsh/cluster-orchestration.md`
- `web/src/components/ServerList.tsx`

**Modified files:**
- `Cargo.toml` (new deps: `toml`, `hostname`, `dirs`; promote `tokio-tungstenite`)
- `src/lib.rs` (new modules)
- `src/main.rs` (CLI args, server startup)
- `src/api/mod.rs` (AppState, routes)
- `src/api/handlers.rs` (new handlers, proxy logic)
- `src/api/error.rs` (new variants)
- `src/api/ws_methods.rs` (server field support)
- `src/session.rs` (SessionInfo server field)
- `src/protocol.rs` (new frame types, server fields)
- `src/server.rs` (handle new frames)
- `src/client.rs` (server management commands)
- `src/mcp/mod.rs` (new tools, server param)
- `src/mcp/tools.rs` (new param structs)
- `src/mcp/prompts.rs` (new prompt)
- `web/src/api/types.ts`
- `web/src/state/sessions.ts`
- `web/src/state/groups.ts`
- `web/src/components/Sidebar.tsx`
- `web/src/components/ThumbnailCell.tsx`
- `web/src/components/CommandPalette.tsx`
- `web/src/api/ws.ts`
- `skills/wsh/core.md`
- `skills/wsh/multi-session.md`
- 9+ integration test files (AppState construction update)

### Dependency changes

- Add: `toml = "0.8"`, `hostname = "0.4"`, `dirs = "5"`
- Move: `tokio-tungstenite` from dev-deps to deps
- Add: `futures-util` if not already in deps (needed for WS stream
  splitting)

### Running tests

All cargo commands MUST be wrapped:
```bash
nix develop -c sh -c "cargo test"
nix develop -c sh -c "cargo test -p wsh --lib federation -- --nocapture"
nix develop -c sh -c "cargo test --test federation_e2e -- --nocapture"
```

### E2E test server instances

All e2e tests MUST use unique `--server-name` values to avoid lock
contention when running in parallel.
