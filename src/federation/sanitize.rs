//! Sanitization and validation for responses received from remote backends.
//!
//! When proxying data from untrusted backends, we must validate structure and
//! enforce size limits to prevent injection attacks and resource exhaustion.

use serde_json::Value;

/// Maximum number of sessions allowed in a session list response.
const MAX_SESSIONS: usize = 1000;

/// Maximum length of a session name from a remote backend.
const MAX_SESSION_NAME_LEN: usize = 100;

/// Allowed fields on a session object received from a remote backend.
const ALLOWED_SESSION_FIELDS: &[&str] = &[
    "name",
    "pid",
    "command",
    "rows",
    "cols",
    "clients",
    "tags",
    "server",
    "last_activity_ms",
];

/// Validate a session name received from a remote backend.
///
/// Allows alphanumeric characters, hyphens, underscores, and dots.
/// Must be 1-100 characters long.
pub fn validate_session_name_from_remote(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_SESSION_NAME_LEN {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Sanitize a session list response from a remote backend.
///
/// - Ensures the value is an array.
/// - Each element must be an object with at least a `name` string field.
/// - Strips unexpected fields, keeping only the allowed set.
/// - Enforces a maximum of 1000 sessions.
/// - Enforces a maximum of 100 characters per session name.
pub fn sanitize_session_list(value: &Value) -> Result<Value, String> {
    let arr = value
        .as_array()
        .ok_or_else(|| "session list must be an array".to_string())?;

    if arr.len() > MAX_SESSIONS {
        return Err(format!(
            "session list exceeds maximum of {} entries (got {})",
            MAX_SESSIONS,
            arr.len()
        ));
    }

    let mut sanitized = Vec::with_capacity(arr.len());

    for (i, item) in arr.iter().enumerate() {
        let obj = item
            .as_object()
            .ok_or_else(|| format!("session list entry {} is not an object", i))?;

        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("session list entry {} missing 'name' string field", i))?;

        if !validate_session_name_from_remote(name) {
            return Err(format!(
                "session list entry {} has invalid name: '{}'",
                i,
                &name[..name.len().min(50)]
            ));
        }

        // Build a new object with only allowed fields.
        let mut clean = serde_json::Map::new();
        for &field in ALLOWED_SESSION_FIELDS {
            if let Some(val) = obj.get(field) {
                clean.insert(field.to_string(), val.clone());
            }
        }

        sanitized.push(Value::Object(clean));
    }

    Ok(Value::Array(sanitized))
}

/// Sanitize a generic proxy response from a remote backend.
///
/// - Enforces a maximum serialized size (in bytes).
/// - Rejects non-object/non-array root types.
/// - Returns the value unchanged if valid.
pub fn sanitize_proxy_response(value: &Value, max_size: usize) -> Result<Value, String> {
    // Reject non-object/non-array root types.
    if !value.is_object() && !value.is_array() {
        return Err("proxy response must be an object or array".into());
    }

    // Check serialized size.
    let serialized = serde_json::to_string(value)
        .map_err(|e| format!("failed to serialize response: {}", e))?;

    if serialized.len() > max_size {
        return Err(format!(
            "proxy response exceeds maximum size of {} bytes (got {})",
            max_size,
            serialized.len()
        ));
    }

    Ok(value.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── validate_session_name_from_remote ─────────────────────────

    #[test]
    fn valid_session_name_simple() {
        assert!(validate_session_name_from_remote("my-session"));
    }

    #[test]
    fn valid_session_name_with_dots_underscores() {
        assert!(validate_session_name_from_remote("my.session_01"));
    }

    #[test]
    fn valid_session_name_single_char() {
        assert!(validate_session_name_from_remote("a"));
    }

    #[test]
    fn invalid_session_name_empty() {
        assert!(!validate_session_name_from_remote(""));
    }

    #[test]
    fn invalid_session_name_too_long() {
        let name = "a".repeat(101);
        assert!(!validate_session_name_from_remote(&name));
    }

    #[test]
    fn valid_session_name_max_length() {
        let name = "a".repeat(100);
        assert!(validate_session_name_from_remote(&name));
    }

    #[test]
    fn invalid_session_name_spaces() {
        assert!(!validate_session_name_from_remote("my session"));
    }

    #[test]
    fn invalid_session_name_special_chars() {
        assert!(!validate_session_name_from_remote("my@session"));
        assert!(!validate_session_name_from_remote("my/session"));
        assert!(!validate_session_name_from_remote("my;session"));
    }

    // ── sanitize_session_list ─────────────────────────────────────

    #[test]
    fn sanitize_session_list_valid() {
        let input = json!([
            {"name": "sess-1", "pid": 1234, "command": "bash", "rows": 24, "cols": 80},
            {"name": "sess-2", "pid": 5678, "command": "zsh"}
        ]);
        let result = sanitize_session_list(&input).unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["name"], "sess-1");
        assert_eq!(arr[1]["name"], "sess-2");
    }

    #[test]
    fn sanitize_session_list_strips_unexpected_fields() {
        let input = json!([
            {"name": "sess-1", "pid": 1234, "secret": "should-be-stripped", "internal_data": 42}
        ]);
        let result = sanitize_session_list(&input).unwrap();
        let obj = result.as_array().unwrap()[0].as_object().unwrap();
        assert!(obj.contains_key("name"));
        assert!(obj.contains_key("pid"));
        assert!(!obj.contains_key("secret"));
        assert!(!obj.contains_key("internal_data"));
    }

    #[test]
    fn sanitize_session_list_keeps_allowed_fields() {
        let input = json!([{
            "name": "s1",
            "pid": 100,
            "command": "bash",
            "rows": 24,
            "cols": 80,
            "clients": 2,
            "tags": ["web"],
            "server": "host-1",
            "last_activity_ms": 500
        }]);
        let result = sanitize_session_list(&input).unwrap();
        let obj = result.as_array().unwrap()[0].as_object().unwrap();
        assert_eq!(obj.len(), 9);
        for field in ALLOWED_SESSION_FIELDS {
            assert!(obj.contains_key(*field), "missing allowed field: {}", field);
        }
    }

    #[test]
    fn sanitize_session_list_rejects_non_array() {
        assert!(sanitize_session_list(&json!({"name": "sess-1"})).is_err());
        assert!(sanitize_session_list(&json!("hello")).is_err());
        assert!(sanitize_session_list(&json!(42)).is_err());
    }

    #[test]
    fn sanitize_session_list_rejects_missing_name() {
        let input = json!([{"pid": 1234}]);
        assert!(sanitize_session_list(&input).is_err());
    }

    #[test]
    fn sanitize_session_list_rejects_non_object_entry() {
        let input = json!(["not-an-object"]);
        assert!(sanitize_session_list(&input).is_err());
    }

    #[test]
    fn sanitize_session_list_rejects_invalid_session_name() {
        let input = json!([{"name": "invalid session!"}]);
        assert!(sanitize_session_list(&input).is_err());
    }

    #[test]
    fn sanitize_session_list_rejects_too_long_name() {
        let name = "a".repeat(101);
        let input = json!([{"name": name}]);
        assert!(sanitize_session_list(&input).is_err());
    }

    #[test]
    fn sanitize_session_list_rejects_too_many_sessions() {
        let sessions: Vec<Value> = (0..1001)
            .map(|i| json!({"name": format!("s{}", i)}))
            .collect();
        let input = Value::Array(sessions);
        assert!(sanitize_session_list(&input).is_err());
    }

    #[test]
    fn sanitize_session_list_accepts_max_sessions() {
        let sessions: Vec<Value> = (0..1000)
            .map(|i| json!({"name": format!("s{}", i)}))
            .collect();
        let input = Value::Array(sessions);
        assert!(sanitize_session_list(&input).is_ok());
    }

    #[test]
    fn sanitize_session_list_empty_array_ok() {
        assert!(sanitize_session_list(&json!([])).is_ok());
    }

    // ── sanitize_proxy_response ───────────────────────────────────

    #[test]
    fn sanitize_proxy_response_valid_object() {
        let input = json!({"key": "value"});
        assert!(sanitize_proxy_response(&input, 1_048_576).is_ok());
    }

    #[test]
    fn sanitize_proxy_response_valid_array() {
        let input = json!([1, 2, 3]);
        assert!(sanitize_proxy_response(&input, 1_048_576).is_ok());
    }

    #[test]
    fn sanitize_proxy_response_rejects_string() {
        let input = json!("hello");
        assert!(sanitize_proxy_response(&input, 1_048_576).is_err());
    }

    #[test]
    fn sanitize_proxy_response_rejects_number() {
        let input = json!(42);
        assert!(sanitize_proxy_response(&input, 1_048_576).is_err());
    }

    #[test]
    fn sanitize_proxy_response_rejects_null() {
        let input = json!(null);
        assert!(sanitize_proxy_response(&input, 1_048_576).is_err());
    }

    #[test]
    fn sanitize_proxy_response_rejects_bool() {
        let input = json!(true);
        assert!(sanitize_proxy_response(&input, 1_048_576).is_err());
    }

    #[test]
    fn sanitize_proxy_response_rejects_oversized() {
        let big_value = "x".repeat(100);
        let input = json!({"data": big_value});
        // The object + key + quotes will be larger than 50 bytes.
        assert!(sanitize_proxy_response(&input, 50).is_err());
    }

    #[test]
    fn sanitize_proxy_response_accepts_at_limit() {
        let input = json!({"a": 1});
        let serialized = serde_json::to_string(&input).unwrap();
        let size = serialized.len();
        assert!(sanitize_proxy_response(&input, size).is_ok());
    }

    #[test]
    fn sanitize_proxy_response_rejects_just_over_limit() {
        let input = json!({"a": 1});
        let serialized = serde_json::to_string(&input).unwrap();
        let size = serialized.len();
        assert!(sanitize_proxy_response(&input, size - 1).is_err());
    }
}
