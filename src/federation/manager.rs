use std::collections::HashMap;

use crate::config::FederationConfig;
use crate::federation::auth::resolve_backend_token;
use crate::federation::connection::BackendConnection;
use crate::federation::registry::{BackendEntry, BackendHealth, BackendRegistry, BackendRole};

/// Owns the backend registry and all active WebSocket connections.
///
/// Created from a `FederationConfig` on server startup, it spawns a persistent
/// connection for each configured backend and supports runtime add/remove.
/// The registry is `Clone`-able and shared with `AppState`; the manager itself
/// must be kept alive for the lifetime of the server so the spawned connection
/// tasks are not orphaned.
pub struct FederationManager {
    registry: BackendRegistry,
    connections: HashMap<String, BackendConnection>,
    default_token: Option<String>,
    local_token: Option<String>,
}

impl FederationManager {
    /// Create an empty manager (no backends, no tokens).
    pub fn new() -> Self {
        Self {
            registry: BackendRegistry::new(),
            connections: HashMap::new(),
            default_token: None,
            local_token: None,
        }
    }

    /// Create from config, spawning connections for each configured backend.
    pub fn from_config(
        config: FederationConfig,
        local_token: Option<String>,
        default_token: Option<String>,
    ) -> Self {
        let registry = BackendRegistry::new();
        let mut connections = HashMap::new();

        for backend_config in &config.servers {
            let token = resolve_backend_token(
                backend_config.token.as_deref(),
                default_token.as_deref(),
                local_token.as_deref(),
            );

            let entry = BackendEntry {
                address: backend_config.address.clone(),
                token: token.clone(),
                hostname: None,
                health: BackendHealth::Connecting,
                role: BackendRole::Member,
            };

            if registry.add(entry).is_ok() {
                let conn = BackendConnection::spawn(
                    backend_config.address.clone(),
                    token,
                    registry.clone(),
                );
                connections.insert(backend_config.address.clone(), conn);
            }
        }

        Self {
            registry,
            connections,
            default_token: default_token.or_else(|| config.default_token.clone()),
            local_token,
        }
    }

    /// Get a reference to the backend registry.
    pub fn registry(&self) -> &BackendRegistry {
        &self.registry
    }

    /// Add a backend at runtime. Validates the address, then spawns a connection.
    pub fn add_backend(
        &mut self,
        address: &str,
        token: Option<&str>,
    ) -> Result<(), crate::federation::registry::RegistryError> {
        // Validate address upfront (registry.add() also validates, but fail fast here).
        crate::federation::registry::validate_backend_address(address)
            .map_err(crate::federation::registry::RegistryError::InvalidAddress)?;

        let resolved_token = resolve_backend_token(
            token,
            self.default_token.as_deref(),
            self.local_token.as_deref(),
        );

        let entry = BackendEntry {
            address: address.to_string(),
            token: resolved_token.clone(),
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
        };

        self.registry.add(entry)?;

        let conn = BackendConnection::spawn(
            address.to_string(),
            resolved_token,
            self.registry.clone(),
        );
        self.connections.insert(address.to_string(), conn);

        Ok(())
    }

    /// Remove a backend by address. Shuts down its connection.
    pub fn remove_backend_by_address(&mut self, address: &str) -> bool {
        if let Some(conn) = self.connections.remove(address) {
            conn.shutdown();
        }
        self.registry.remove_by_address(address)
    }

    /// Remove a backend by hostname. Shuts down its connection.
    pub fn remove_backend_by_hostname(&mut self, hostname: &str) -> bool {
        // Find the address for this hostname first.
        if let Some(entry) = self.registry.get_by_hostname(hostname) {
            if let Some(conn) = self.connections.remove(&entry.address) {
                conn.shutdown();
            }
        }
        self.registry.remove_by_hostname(hostname)
    }

    /// Shut down all backend connections.
    pub async fn shutdown_all(&mut self) {
        for (_, conn) in self.connections.drain() {
            conn.shutdown();
        }
        // Note: we don't join here to avoid blocking shutdown.
        // The tasks will be dropped when the runtime shuts down.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BackendServerConfig;

    #[tokio::test]
    async fn manager_from_empty_config() {
        let config = FederationConfig::default();
        let manager = FederationManager::from_config(config, None, None);
        assert!(manager.registry().list().is_empty());
    }

    #[tokio::test]
    async fn manager_from_config_with_backends() {
        let config = FederationConfig {
            server: None,
            default_token: Some("default-tok".into()),
            servers: vec![
                BackendServerConfig {
                    address: "http://10.0.1.10:8080".into(),
                    token: None,
                },
                BackendServerConfig {
                    address: "http://10.0.1.11:8080".into(),
                    token: Some("specific".into()),
                },
            ],
            ip_access: None,
        };
        let mut manager = FederationManager::from_config(config, None, None);
        let backends = manager.registry().list();
        assert_eq!(backends.len(), 2);
        // All start in Connecting state
        assert!(backends
            .iter()
            .all(|b| b.health == BackendHealth::Connecting));
        // Shut down to clean up spawned tasks
        manager.shutdown_all().await;
    }

    #[tokio::test]
    async fn manager_add_and_remove() {
        let mut manager = FederationManager::new();
        manager.add_backend("http://10.0.99.1:9999", Some("tok")).unwrap();
        assert_eq!(manager.registry().list().len(), 1);
        assert!(manager.remove_backend_by_address("http://10.0.99.1:9999"));
        assert!(manager.registry().list().is_empty());
    }

    #[tokio::test]
    async fn manager_rejects_duplicate() {
        let mut manager = FederationManager::new();
        manager.add_backend("http://10.0.99.1:9999", None).unwrap();
        assert!(manager.add_backend("http://10.0.99.1:9999", None).is_err());
        manager.shutdown_all().await;
    }

    #[tokio::test]
    async fn manager_rejects_localhost_address() {
        let mut manager = FederationManager::new();
        let result = manager.add_backend("http://127.0.0.1:9999", None);
        assert!(result.is_err());
    }
}
