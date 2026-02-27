use parking_lot::RwLock;
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendHealth {
    Connecting,
    Healthy,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendRole {
    Member,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackendEntry {
    pub address: String,
    #[serde(skip_serializing)]
    pub token: Option<String>,
    pub hostname: Option<String>,
    pub health: BackendHealth,
    pub role: BackendRole,
}

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

#[derive(Clone)]
pub struct BackendRegistry {
    inner: Arc<RwLock<Vec<BackendEntry>>>,
}

impl BackendRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Add a backend entry. Rejects duplicate addresses and hostname collisions.
    pub fn add(&self, entry: BackendEntry) -> Result<(), RegistryError> {
        let mut backends = self.inner.write();

        // Check for duplicate address.
        if backends.iter().any(|b| b.address == entry.address) {
            return Err(RegistryError::DuplicateAddress(entry.address));
        }

        // Check for hostname collision (only when the new entry has a hostname).
        if let Some(ref hostname) = entry.hostname {
            if backends
                .iter()
                .any(|b| b.hostname.as_deref() == Some(hostname.as_str()))
            {
                return Err(RegistryError::HostnameCollision(hostname.clone()));
            }
        }

        backends.push(entry);
        Ok(())
    }

    /// Remove a backend by address. Returns true if an entry was removed.
    pub fn remove_by_address(&self, addr: &str) -> bool {
        let mut backends = self.inner.write();
        let len_before = backends.len();
        backends.retain(|b| b.address != addr);
        backends.len() < len_before
    }

    /// Remove a backend by hostname. Returns true if an entry was removed.
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

    /// Set the hostname for a backend identified by address.
    /// Checks for hostname collisions with OTHER backends.
    pub fn set_hostname(&self, address: &str, hostname: &str) -> Result<(), RegistryError> {
        let mut backends = self.inner.write();

        // Check for hostname collision with other backends.
        if backends
            .iter()
            .any(|b| b.address != address && b.hostname.as_deref() == Some(hostname))
        {
            return Err(RegistryError::HostnameCollision(hostname.to_string()));
        }

        // Find the backend and update its hostname.
        let entry = backends
            .iter_mut()
            .find(|b| b.address == address)
            .ok_or_else(|| RegistryError::NotFound(address.to_string()))?;

        entry.hostname = Some(hostname.to_string());
        Ok(())
    }

    /// Set the health status for a backend identified by address.
    pub fn set_health(&self, address: &str, health: BackendHealth) {
        let mut backends = self.inner.write();
        if let Some(entry) = backends.iter_mut().find(|b| b.address == address) {
            entry.health = health;
        }
    }
}

impl Default for BackendRegistry {
    fn default() -> Self {
        Self::new()
    }
}

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
        reg.add(BackendEntry {
            address: "10.0.1.10:8080".into(),
            token: None,
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
        })
        .unwrap();
        assert!(reg
            .add(BackendEntry {
                address: "10.0.1.10:8080".into(),
                token: None,
                hostname: None,
                health: BackendHealth::Connecting,
                role: BackendRole::Member,
            })
            .is_err());
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
        assert!(reg
            .add(BackendEntry {
                address: "10.0.1.11:8080".into(),
                token: None,
                hostname: Some("same-host".into()),
                health: BackendHealth::Healthy,
                role: BackendRole::Member,
            })
            .is_err());
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
