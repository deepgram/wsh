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
