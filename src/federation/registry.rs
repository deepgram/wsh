use parking_lot::RwLock;
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendHealth {
    Connecting,
    Healthy,
    Unavailable,
    /// The backend was detected as a self-loop (same server_id as local).
    /// Visible in the registry but non-functional — no retries.
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendRole {
    Member,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackendEntry {
    /// Full base URL including scheme and optional path prefix.
    /// Examples: `http://10.0.1.10:8080`, `https://proxy.example.com/wsh-node-1`
    pub address: String,
    #[serde(skip_serializing)]
    pub token: Option<String>,
    pub hostname: Option<String>,
    pub health: BackendHealth,
    pub role: BackendRole,
    /// Remote server's UUID (populated after first successful /server/info fetch).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
}

impl BackendEntry {
    /// Build an HTTP(S) URL for the given API path.
    ///
    /// Joins the base address with the path, handling trailing slash normalization.
    /// Example: `https://proxy.example.com/wsh` + `/sessions` = `https://proxy.example.com/wsh/sessions`
    pub fn url_for(&self, path: &str) -> String {
        let base = self.address.trim_end_matches('/');
        format!("{}{}", base, path)
    }

    /// Build a WebSocket URL for the given API path.
    ///
    /// Converts `http://` → `ws://` and `https://` → `wss://`.
    pub fn ws_url_for(&self, path: &str) -> String {
        let base = self.address.trim_end_matches('/');
        let ws_base = if base.starts_with("https://") {
            format!("wss://{}", &base["https://".len()..])
        } else if base.starts_with("http://") {
            format!("ws://{}", &base["http://".len()..])
        } else {
            // Shouldn't happen after validation, but be defensive
            format!("ws://{}", base)
        };
        format!("{}{}", ws_base, path)
    }
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

/// Validate a backend address as a full URL with explicit scheme.
///
/// - Must have `http://` or `https://` scheme.
/// - Must have a non-empty host.
/// - Must have a valid port (unless implied by scheme: 80/443).
/// - May include a path prefix (e.g. `/wsh-node-1`).
/// - Must NOT be `0.0.0.0` (unspecified address is never a valid target).
///
/// Localhost and loopback addresses are allowed — self-loop detection is
/// handled at the connection layer via server UUID comparison.
///
/// If a schemeless `host:port` is provided, the error message suggests the fix.
pub fn validate_backend_address(address: &str) -> Result<(), String> {
    // Detect schemeless host:port and give a helpful error.
    if !address.contains("://") {
        return Err(format!(
            "backend address must include a scheme (http:// or https://). \
             Did you mean 'http://{}'?",
            address,
        ));
    }

    // Must be http:// or https://
    let (scheme, rest) = if let Some(rest) = address.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = address.strip_prefix("http://") {
        ("http", rest)
    } else {
        return Err("backend address must use http:// or https:// scheme".into());
    };

    if rest.is_empty() {
        return Err("backend address has empty authority".into());
    }

    // Split off path component (if any). The authority is everything before the first '/'.
    let (authority, _path) = match rest.find('/') {
        Some(pos) => (&rest[..pos], &rest[pos..]),
        None => (rest, ""),
    };

    if authority.is_empty() {
        return Err("backend address has empty authority".into());
    }

    // Parse host:port from authority. Handle IPv6 bracket notation.
    let (host, port_str) = if authority.starts_with('[') {
        let bracket_end = authority
            .find(']')
            .ok_or_else(|| "invalid IPv6 bracket notation".to_string())?;
        let host = &authority[1..bracket_end];
        let rest = &authority[bracket_end + 1..];
        if rest.is_empty() {
            // No port specified — use scheme default
            (host, None)
        } else if rest.starts_with(':') {
            (host, Some(&rest[1..]))
        } else {
            return Err("invalid characters after IPv6 address".into());
        }
    } else if let Some(colon_pos) = authority.rfind(':') {
        let host = &authority[..colon_pos];
        let port = &authority[colon_pos + 1..];
        if port.is_empty() {
            (host, None)
        } else {
            (host, Some(port))
        }
    } else {
        (authority, None)
    };

    if host.is_empty() {
        return Err("backend address has empty host".into());
    }

    // Validate port if explicitly provided.
    if let Some(port_str) = port_str {
        let _port: u16 = port_str
            .parse()
            .map_err(|_| format!("invalid port: '{}'", port_str))?;
    }

    // Reject unspecified address (0.0.0.0 / ::) — never a valid backend target.
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if ip.is_unspecified() {
            return Err("unspecified address (0.0.0.0) is not allowed".into());
        }
    }

    let _ = scheme; // used for validation above
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

    /// Set the server_id for a backend identified by address.
    pub fn set_server_id(&self, address: &str, server_id: &str) {
        let mut backends = self.inner.write();
        if let Some(entry) = backends.iter_mut().find(|b| b.address == address) {
            entry.server_id = Some(server_id.to_string());
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
            address: "http://10.0.1.10:8080".into(),
            token: Some("tok".into()),
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
            server_id: None,
        })
        .unwrap();
        let list = reg.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].address, "http://10.0.1.10:8080");
    }

    #[test]
    fn remove_backend_by_address() {
        let reg = BackendRegistry::new();
        reg.add(BackendEntry {
            address: "http://10.0.1.10:8080".into(),
            token: Some("tok".into()),
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
            server_id: None,
        })
        .unwrap();
        assert!(reg.remove_by_address("http://10.0.1.10:8080"));
        assert!(reg.list().is_empty());
    }

    #[test]
    fn remove_backend_by_hostname() {
        let reg = BackendRegistry::new();
        reg.add(BackendEntry {
            address: "http://10.0.1.10:8080".into(),
            token: Some("tok".into()),
            hostname: Some("prod-1".into()),
            health: BackendHealth::Healthy,
            role: BackendRole::Member,
            server_id: None,
        })
        .unwrap();
        assert!(reg.remove_by_hostname("prod-1"));
        assert!(reg.list().is_empty());
    }

    #[test]
    fn duplicate_address_rejected() {
        let reg = BackendRegistry::new();
        reg.add(BackendEntry {
            address: "http://10.0.1.10:8080".into(),
            token: None,
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
            server_id: None,
        })
        .unwrap();
        assert!(reg
            .add(BackendEntry {
                address: "http://10.0.1.10:8080".into(),
                token: None,
                hostname: None,
                health: BackendHealth::Connecting,
                role: BackendRole::Member,
                server_id: None,
            })
            .is_err());
    }

    #[test]
    fn hostname_collision_rejected() {
        let reg = BackendRegistry::new();
        reg.add(BackendEntry {
            address: "http://10.0.1.10:8080".into(),
            token: None,
            hostname: Some("same-host".into()),
            health: BackendHealth::Healthy,
            role: BackendRole::Member,
            server_id: None,
        })
        .unwrap();
        assert!(reg
            .add(BackendEntry {
                address: "http://10.0.1.11:8080".into(),
                token: None,
                hostname: Some("same-host".into()),
                health: BackendHealth::Healthy,
                role: BackendRole::Member,
                server_id: None,
            })
            .is_err());
    }

    #[test]
    fn set_hostname_updates_entry() {
        let reg = BackendRegistry::new();
        reg.add(BackendEntry {
            address: "http://10.0.1.10:8080".into(),
            token: None,
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
            server_id: None,
        })
        .unwrap();
        reg.set_hostname("http://10.0.1.10:8080", "prod-1").unwrap();
        let list = reg.list();
        assert_eq!(list[0].hostname.as_deref(), Some("prod-1"));
    }

    #[test]
    fn set_health_updates_entry() {
        let reg = BackendRegistry::new();
        reg.add(BackendEntry {
            address: "http://10.0.1.10:8080".into(),
            token: None,
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
            server_id: None,
        })
        .unwrap();
        reg.set_health("http://10.0.1.10:8080", BackendHealth::Unavailable);
        let list = reg.list();
        assert_eq!(list[0].health, BackendHealth::Unavailable);
    }

    #[test]
    fn get_by_hostname() {
        let reg = BackendRegistry::new();
        reg.add(BackendEntry {
            address: "http://10.0.1.10:8080".into(),
            token: None,
            hostname: Some("prod-1".into()),
            health: BackendHealth::Healthy,
            role: BackendRole::Member,
            server_id: None,
        })
        .unwrap();
        let entry = reg.get_by_hostname("prod-1").unwrap();
        assert_eq!(entry.address, "http://10.0.1.10:8080");
    }

    #[test]
    fn healthy_backends_only() {
        let reg = BackendRegistry::new();
        reg.add(BackendEntry {
            address: "http://10.0.1.10:8080".into(),
            token: None,
            hostname: Some("healthy".into()),
            health: BackendHealth::Healthy,
            role: BackendRole::Member,
            server_id: None,
        })
        .unwrap();
        reg.add(BackendEntry {
            address: "http://10.0.1.11:8080".into(),
            token: None,
            hostname: Some("down".into()),
            health: BackendHealth::Unavailable,
            role: BackendRole::Member,
            server_id: None,
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
    fn address_valid_http_ip_port() {
        assert!(validate_backend_address("http://10.0.1.10:8080").is_ok());
    }

    #[test]
    fn address_valid_https_hostname_port() {
        assert!(validate_backend_address("https://example.com:443").is_ok());
    }

    #[test]
    fn address_valid_https_no_port() {
        assert!(validate_backend_address("https://example.com").is_ok());
    }

    #[test]
    fn address_valid_with_path_prefix() {
        assert!(validate_backend_address("https://proxy.example.com/wsh-node-1").is_ok());
    }

    #[test]
    fn address_valid_ipv6_bracket() {
        assert!(validate_backend_address("http://[2001:db8::1]:8080").is_ok());
    }

    #[test]
    fn address_schemeless_rejected_with_suggestion() {
        let err = validate_backend_address("10.0.1.10:8080").unwrap_err();
        assert!(err.contains("http://10.0.1.10:8080"), "should suggest fix: {}", err);
    }

    #[test]
    fn address_bad_scheme_rejected() {
        assert!(validate_backend_address("file:///etc/passwd").is_err());
        assert!(validate_backend_address("ftp://example.com").is_err());
    }

    #[test]
    fn address_empty_authority_rejected() {
        assert!(validate_backend_address("http://").is_err());
    }

    #[test]
    fn address_invalid_port_rejected() {
        assert!(validate_backend_address("http://example.com:99999").is_err());
        assert!(validate_backend_address("http://example.com:abc").is_err());
    }

    #[test]
    fn address_localhost_accepted() {
        assert!(validate_backend_address("http://localhost:8080").is_ok());
        assert!(validate_backend_address("http://LOCALHOST:8080").is_ok());
    }

    #[test]
    fn address_loopback_ipv4_accepted() {
        assert!(validate_backend_address("http://127.0.0.1:8080").is_ok());
        assert!(validate_backend_address("http://127.0.0.2:8080").is_ok());
    }

    #[test]
    fn address_loopback_ipv6_accepted() {
        assert!(validate_backend_address("http://[::1]:8080").is_ok());
    }

    #[test]
    fn address_unspecified_rejected() {
        assert!(validate_backend_address("http://0.0.0.0:8080").is_err());
    }

    #[test]
    fn address_unspecified_ipv6_rejected() {
        assert!(validate_backend_address("http://[::]:8080").is_err());
    }

    #[test]
    fn address_private_ip_accepted() {
        assert!(validate_backend_address("http://10.0.0.1:8080").is_ok());
        assert!(validate_backend_address("http://192.168.1.1:8080").is_ok());
        assert!(validate_backend_address("http://172.16.0.1:8080").is_ok());
    }

    // ── url_for / ws_url_for tests ───────────────────────────────

    #[test]
    fn url_for_simple() {
        let entry = BackendEntry {
            address: "http://10.0.1.10:8080".into(),
            token: None, hostname: None,
            health: BackendHealth::Healthy, role: BackendRole::Member, server_id: None,
        };
        assert_eq!(entry.url_for("/sessions"), "http://10.0.1.10:8080/sessions");
    }

    #[test]
    fn url_for_with_path_prefix() {
        let entry = BackendEntry {
            address: "https://proxy.example.com/wsh-node-1".into(),
            token: None, hostname: None,
            health: BackendHealth::Healthy, role: BackendRole::Member, server_id: None,
        };
        assert_eq!(
            entry.url_for("/sessions"),
            "https://proxy.example.com/wsh-node-1/sessions"
        );
    }

    #[test]
    fn ws_url_for_http() {
        let entry = BackendEntry {
            address: "http://10.0.1.10:8080".into(),
            token: None, hostname: None,
            health: BackendHealth::Healthy, role: BackendRole::Member, server_id: None,
        };
        assert_eq!(entry.ws_url_for("/ws/json"), "ws://10.0.1.10:8080/ws/json");
    }

    #[test]
    fn ws_url_for_https() {
        let entry = BackendEntry {
            address: "https://proxy.example.com/wsh".into(),
            token: None, hostname: None,
            health: BackendHealth::Healthy, role: BackendRole::Member, server_id: None,
        };
        assert_eq!(
            entry.ws_url_for("/ws/json"),
            "wss://proxy.example.com/wsh/ws/json"
        );
    }

    // ── Registry add with validation ─────────────────────────────

    #[test]
    fn registry_add_accepts_localhost_address() {
        let reg = BackendRegistry::new();
        let result = reg.add(BackendEntry {
            address: "http://127.0.0.1:8080".into(),
            token: None,
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
            server_id: None,
        });
        assert!(result.is_ok());
    }

    #[test]
    fn registry_add_rejects_invalid_hostname() {
        let reg = BackendRegistry::new();
        let result = reg.add(BackendEntry {
            address: "http://10.0.1.10:8080".into(),
            token: None,
            hostname: Some("-invalid".into()),
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
            server_id: None,
        });
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RegistryError::InvalidHostname(_)));
    }

    #[test]
    fn registry_add_rejects_schemeless_address() {
        let reg = BackendRegistry::new();
        let result = reg.add(BackendEntry {
            address: "10.0.1.10:8080".into(),
            token: None,
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
            server_id: None,
        });
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RegistryError::InvalidAddress(_)));
    }

    // ── Rejected health and server_id tests ─────────────────────

    #[test]
    fn rejected_health_serializes_as_snake_case() {
        let json = serde_json::to_value(BackendHealth::Rejected).unwrap();
        assert_eq!(json, serde_json::json!("rejected"));
    }

    #[test]
    fn set_server_id_updates_entry() {
        let reg = BackendRegistry::new();
        reg.add(BackendEntry {
            address: "http://10.0.1.10:8080".into(),
            token: None,
            hostname: None,
            health: BackendHealth::Connecting,
            role: BackendRole::Member,
            server_id: None,
        })
        .unwrap();
        reg.set_server_id("http://10.0.1.10:8080", "test-uuid-123");
        let list = reg.list();
        assert_eq!(list[0].server_id.as_deref(), Some("test-uuid-123"));
    }

    #[test]
    fn set_server_id_noop_for_missing_address() {
        let reg = BackendRegistry::new();
        reg.set_server_id("http://nonexistent:8080", "uuid");
        assert!(reg.list().is_empty());
    }
}
