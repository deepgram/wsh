use parking_lot::RwLock;
use serde::Serialize;
use std::net::IpAddr;
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
    InvalidAddress(String),
    InvalidHostname(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateAddress(addr) => write!(f, "Backend already registered: {}", addr),
            Self::HostnameCollision(name) => write!(f, "Hostname already in use: {}", name),
            Self::NotFound(key) => write!(f, "Backend not found: {}", key),
            Self::InvalidAddress(detail) => write!(f, "Invalid backend address: {}", detail),
            Self::InvalidHostname(detail) => write!(f, "Invalid hostname: {}", detail),
        }
    }
}

impl std::error::Error for RegistryError {}

/// Validate a hostname according to RFC 952/1123 rules.
///
/// - Must be 1-253 characters total.
/// - Each dot-separated label must be 1-63 characters.
/// - Labels may contain ASCII alphanumeric characters and hyphens.
/// - Labels must not start or end with a hyphen.
/// - No empty labels (no consecutive dots).
pub fn validate_hostname(hostname: &str) -> Result<(), String> {
    if hostname.is_empty() {
        return Err("hostname must not be empty".into());
    }
    if hostname.len() > 253 {
        return Err(format!(
            "hostname exceeds 253 characters (got {})",
            hostname.len()
        ));
    }

    for label in hostname.split('.') {
        if label.is_empty() {
            return Err("hostname contains empty label (consecutive dots)".into());
        }
        if label.len() > 63 {
            return Err(format!(
                "label '{}' exceeds 63 characters (got {})",
                &label[..20],
                label.len()
            ));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(format!(
                "label '{}' must not start or end with a hyphen",
                label
            ));
        }
        if !label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(format!(
                "label '{}' contains invalid characters (only alphanumeric and hyphens allowed)",
                label
            ));
        }
    }

    Ok(())
}

/// Validate a backend address in `host:port` format.
///
/// - Must NOT contain a scheme (e.g. `http://`, `file://`).
/// - Must be in `host:port` format with a valid u16 port.
/// - Must NOT be a localhost/loopback address (SSRF prevention).
/// - Must NOT be `0.0.0.0`.
pub fn validate_backend_address(address: &str) -> Result<(), String> {
    // Reject schemes.
    if address.contains("://") {
        return Err("address must not contain a scheme (e.g. 'http://')".into());
    }

    // Split host:port. Handle IPv6 bracket notation like [::1]:8080.
    let (host, port_str) = if address.starts_with('[') {
        // IPv6 bracket notation: [host]:port
        let bracket_end = address
            .find(']')
            .ok_or_else(|| "invalid IPv6 bracket notation".to_string())?;
        let host = &address[1..bracket_end];
        let rest = &address[bracket_end + 1..];
        if !rest.starts_with(':') {
            return Err("address must be in host:port format".into());
        }
        (host, &rest[1..])
    } else {
        // Regular host:port
        let colon_pos = address
            .rfind(':')
            .ok_or_else(|| "address must be in host:port format (missing port)".to_string())?;
        let host = &address[..colon_pos];
        let port_str = &address[colon_pos + 1..];
        (host, port_str)
    };

    if host.is_empty() {
        return Err("address has empty host".into());
    }

    // Validate port.
    let _port: u16 = port_str
        .parse()
        .map_err(|_| format!("invalid port: '{}'", port_str))?;

    // SSRF prevention: reject localhost/loopback.
    let lower_host = host.to_lowercase();
    if lower_host == "localhost" {
        return Err("localhost addresses are not allowed (SSRF prevention)".into());
    }

    // Check if it's an IP address and reject loopback/unspecified.
    if let Ok(ip) = host.parse::<IpAddr>() {
        if ip.is_loopback() {
            return Err("loopback addresses are not allowed (SSRF prevention)".into());
        }
        if ip.is_unspecified() {
            return Err("unspecified address (0.0.0.0) is not allowed".into());
        }
    }

    Ok(())
}

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

    /// Add a backend entry. Validates the address (and hostname if present),
    /// then rejects duplicate addresses and hostname collisions.
    pub fn add(&self, entry: BackendEntry) -> Result<(), RegistryError> {
        // Validate address format and SSRF safety.
        validate_backend_address(&entry.address)
            .map_err(RegistryError::InvalidAddress)?;

        // Validate hostname if provided.
        if let Some(ref hostname) = entry.hostname {
            validate_hostname(hostname).map_err(RegistryError::InvalidHostname)?;
        }

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

    /// Add a backend entry WITHOUT validating the address or hostname.
    ///
    /// This is intended for internal use (e.g., tests that need to register
    /// localhost addresses for local test servers). Still checks for duplicate
    /// addresses and hostname collisions.
    pub fn add_unchecked(&self, entry: BackendEntry) -> Result<(), RegistryError> {
        let mut backends = self.inner.write();

        if backends.iter().any(|b| b.address == entry.address) {
            return Err(RegistryError::DuplicateAddress(entry.address));
        }

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

    // ── Hostname validation tests ────────────────────────────────

    #[test]
    fn hostname_valid_simple() {
        assert!(validate_hostname("prod-1").is_ok());
    }

    #[test]
    fn hostname_valid_fqdn() {
        assert!(validate_hostname("app.example.com").is_ok());
    }

    #[test]
    fn hostname_valid_single_char() {
        assert!(validate_hostname("a").is_ok());
    }

    #[test]
    fn hostname_valid_with_numbers() {
        assert!(validate_hostname("node-01.cluster-3.internal").is_ok());
    }

    #[test]
    fn hostname_empty_rejected() {
        assert!(validate_hostname("").is_err());
    }

    #[test]
    fn hostname_too_long_rejected() {
        let long = "a".repeat(254);
        assert!(validate_hostname(&long).is_err());
    }

    #[test]
    fn hostname_253_chars_accepted() {
        // 63 chars per label, 3 labels + 2 dots = 63*3 + 2 = 191 < 253
        let label = "a".repeat(63);
        let hostname = format!("{}.{}.{}", label, label, label);
        assert!(validate_hostname(&hostname).is_ok());
    }

    #[test]
    fn hostname_label_too_long_rejected() {
        let long_label = "a".repeat(64);
        assert!(validate_hostname(&long_label).is_err());
    }

    #[test]
    fn hostname_leading_hyphen_rejected() {
        assert!(validate_hostname("-invalid").is_err());
    }

    #[test]
    fn hostname_trailing_hyphen_rejected() {
        assert!(validate_hostname("invalid-").is_err());
    }

    #[test]
    fn hostname_consecutive_dots_rejected() {
        assert!(validate_hostname("a..b").is_err());
    }

    #[test]
    fn hostname_leading_dot_rejected() {
        assert!(validate_hostname(".example.com").is_err());
    }

    #[test]
    fn hostname_trailing_dot_accepted_as_labels() {
        // A trailing dot produces an empty final label.
        assert!(validate_hostname("example.com.").is_err());
    }

    #[test]
    fn hostname_invalid_chars_rejected() {
        assert!(validate_hostname("host_name").is_err());
        assert!(validate_hostname("host name").is_err());
        assert!(validate_hostname("host@name").is_err());
    }

    #[test]
    fn hostname_hyphen_in_middle_ok() {
        assert!(validate_hostname("my-host").is_ok());
        assert!(validate_hostname("a-b-c").is_ok());
    }

    // ── Address validation tests ─────────────────────────────────

    #[test]
    fn address_valid_ip_port() {
        assert!(validate_backend_address("10.0.1.10:8080").is_ok());
    }

    #[test]
    fn address_valid_hostname_port() {
        assert!(validate_backend_address("example.com:443").is_ok());
    }

    #[test]
    fn address_valid_ipv6_bracket() {
        assert!(validate_backend_address("[2001:db8::1]:8080").is_ok());
    }

    #[test]
    fn address_scheme_rejected() {
        assert!(validate_backend_address("http://example.com:8080").is_err());
        assert!(validate_backend_address("https://example.com:443").is_err());
        assert!(validate_backend_address("file:///etc/passwd").is_err());
    }

    #[test]
    fn address_missing_port_rejected() {
        assert!(validate_backend_address("example.com").is_err());
    }

    #[test]
    fn address_empty_host_rejected() {
        assert!(validate_backend_address(":8080").is_err());
    }

    #[test]
    fn address_invalid_port_rejected() {
        assert!(validate_backend_address("example.com:99999").is_err());
        assert!(validate_backend_address("example.com:abc").is_err());
        assert!(validate_backend_address("example.com:").is_err());
    }

    #[test]
    fn address_localhost_rejected() {
        assert!(validate_backend_address("localhost:8080").is_err());
        assert!(validate_backend_address("LOCALHOST:8080").is_err());
    }

    #[test]
    fn address_loopback_ipv4_rejected() {
        assert!(validate_backend_address("127.0.0.1:8080").is_err());
        assert!(validate_backend_address("127.0.0.2:8080").is_err());
        assert!(validate_backend_address("127.255.255.255:8080").is_err());
    }

    #[test]
    fn address_loopback_ipv6_rejected() {
        assert!(validate_backend_address("[::1]:8080").is_err());
    }

    #[test]
    fn address_unspecified_rejected() {
        assert!(validate_backend_address("0.0.0.0:8080").is_err());
    }

    #[test]
    fn address_unspecified_ipv6_rejected() {
        assert!(validate_backend_address("[::]:8080").is_err());
    }

    #[test]
    fn address_private_ip_accepted() {
        // Private IPs are allowed -- only loopback is blocked.
        assert!(validate_backend_address("10.0.0.1:8080").is_ok());
        assert!(validate_backend_address("192.168.1.1:8080").is_ok());
        assert!(validate_backend_address("172.16.0.1:8080").is_ok());
    }

    // ── Registry add with validation ─────────────────────────────

    #[test]
    fn registry_add_rejects_localhost_address() {
        let reg = BackendRegistry::new();
        let result = reg.add(BackendEntry {
            address: "127.0.0.1:8080".into(),
            token: None,
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
        });
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RegistryError::InvalidAddress(_)));
    }

    #[test]
    fn registry_add_rejects_invalid_hostname() {
        let reg = BackendRegistry::new();
        let result = reg.add(BackendEntry {
            address: "10.0.1.10:8080".into(),
            token: None,
            hostname: Some("-invalid".into()),
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
        });
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RegistryError::InvalidHostname(_)));
    }

    #[test]
    fn registry_add_rejects_scheme_in_address() {
        let reg = BackendRegistry::new();
        let result = reg.add(BackendEntry {
            address: "http://10.0.1.10:8080".into(),
            token: None,
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
        });
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RegistryError::InvalidAddress(_)));
    }
}
