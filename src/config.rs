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
    /// Per-server auth token.
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
        let contents =
            toml::to_string_pretty(self).map_err(|e| ConfigError::SerializeFailed(e))?;
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
            Self::ReadFailed(path, e) => {
                write!(f, "Failed to read config {}: {}", path.display(), e)
            }
            Self::ParseFailed(path, e) => {
                write!(f, "Failed to parse config {}: {}", path.display(), e)
            }
            Self::WriteFailed(path, e) => {
                write!(f, "Failed to write config {}: {}", path.display(), e)
            }
            Self::SerializeFailed(e) => write!(f, "Failed to serialize config: {}", e),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Resolve the server's hostname. Uses config override if present,
/// otherwise falls back to system hostname.
pub fn resolve_hostname(server_config: Option<&ServerIdentityConfig>) -> String {
    if let Some(config) = server_config {
        if let Some(hostname) = &config.hostname {
            return hostname.clone();
        }
    }
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string())
}

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
        assert!(!hostname.is_empty());
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
