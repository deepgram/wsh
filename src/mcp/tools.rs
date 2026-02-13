// MCP tool parameter types and helpers

use serde::Deserialize;
use std::collections::HashMap;

/// Parameters for the `wsh_create_session` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateSessionParams {
    /// Optional session name. If not provided, an auto-generated name is assigned.
    #[schemars(description = "Optional session name. If omitted, an auto-generated numeric name is assigned.")]
    pub name: Option<String>,

    /// Optional command to run. If not provided, an interactive shell is spawned.
    #[schemars(description = "Command to execute. If omitted, an interactive shell is spawned.")]
    pub command: Option<String>,

    /// Terminal rows. Defaults to 24.
    #[schemars(description = "Terminal height in rows. Defaults to 24.")]
    pub rows: Option<u16>,

    /// Terminal columns. Defaults to 80.
    #[schemars(description = "Terminal width in columns. Defaults to 80.")]
    pub cols: Option<u16>,

    /// Working directory for the spawned process.
    #[schemars(description = "Working directory for the spawned process.")]
    pub cwd: Option<String>,

    /// Environment variable overrides for the spawned process.
    #[schemars(description = "Additional environment variables for the spawned process.")]
    pub env: Option<HashMap<String, String>>,
}

/// Parameters for the `wsh_list_sessions` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListSessionsParams {
    /// If provided, return details for a single session instead of all sessions.
    #[schemars(description = "If provided, return details for this specific session instead of listing all.")]
    pub session: Option<String>,
}

/// Action to perform on a session.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ManageAction {
    /// Kill (destroy) the session.
    Kill,
    /// Rename the session. Requires `new_name`.
    Rename,
    /// Detach all streaming clients from the session.
    Detach,
}

/// Parameters for the `wsh_manage_session` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageSessionParams {
    /// The name of the session to manage.
    #[schemars(description = "The name of the target session.")]
    pub session: String,

    /// The action to perform on the session.
    #[schemars(description = "The action to perform: kill, rename, or detach.")]
    pub action: ManageAction,

    /// New name for the session (required when action is 'rename').
    #[schemars(description = "New name for the session. Required when action is 'rename'.")]
    pub new_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── CreateSessionParams ──────────────────────────────────────

    #[test]
    fn create_session_params_all_defaults() {
        let json = serde_json::json!({});
        let params: CreateSessionParams = serde_json::from_value(json).unwrap();
        assert!(params.name.is_none());
        assert!(params.command.is_none());
        assert!(params.rows.is_none());
        assert!(params.cols.is_none());
        assert!(params.cwd.is_none());
        assert!(params.env.is_none());
    }

    #[test]
    fn create_session_params_all_fields() {
        let json = serde_json::json!({
            "name": "my-session",
            "command": "bash",
            "rows": 30,
            "cols": 120,
            "cwd": "/tmp",
            "env": {"FOO": "bar"}
        });
        let params: CreateSessionParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.name.as_deref(), Some("my-session"));
        assert_eq!(params.command.as_deref(), Some("bash"));
        assert_eq!(params.rows, Some(30));
        assert_eq!(params.cols, Some(120));
        assert_eq!(params.cwd.as_deref(), Some("/tmp"));
        let env = params.env.unwrap();
        assert_eq!(env.get("FOO").map(|s| s.as_str()), Some("bar"));
    }

    #[test]
    fn create_session_params_invalid_rows_type() {
        let json = serde_json::json!({"rows": "not-a-number"});
        let result = serde_json::from_value::<CreateSessionParams>(json);
        assert!(result.is_err());
    }

    // ── ListSessionsParams ───────────────────────────────────────

    #[test]
    fn list_sessions_params_empty() {
        let json = serde_json::json!({});
        let params: ListSessionsParams = serde_json::from_value(json).unwrap();
        assert!(params.session.is_none());
    }

    #[test]
    fn list_sessions_params_with_session() {
        let json = serde_json::json!({"session": "my-session"});
        let params: ListSessionsParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session.as_deref(), Some("my-session"));
    }

    // ── ManageAction ─────────────────────────────────────────────

    #[test]
    fn manage_action_kill() {
        let json = serde_json::json!("kill");
        let action: ManageAction = serde_json::from_value(json).unwrap();
        assert!(matches!(action, ManageAction::Kill));
    }

    #[test]
    fn manage_action_rename() {
        let json = serde_json::json!("rename");
        let action: ManageAction = serde_json::from_value(json).unwrap();
        assert!(matches!(action, ManageAction::Rename));
    }

    #[test]
    fn manage_action_detach() {
        let json = serde_json::json!("detach");
        let action: ManageAction = serde_json::from_value(json).unwrap();
        assert!(matches!(action, ManageAction::Detach));
    }

    #[test]
    fn manage_action_invalid() {
        let json = serde_json::json!("invalid_action");
        let result = serde_json::from_value::<ManageAction>(json);
        assert!(result.is_err());
    }

    // ── ManageSessionParams ──────────────────────────────────────

    #[test]
    fn manage_session_params_kill() {
        let json = serde_json::json!({
            "session": "my-session",
            "action": "kill"
        });
        let params: ManageSessionParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "my-session");
        assert!(matches!(params.action, ManageAction::Kill));
        assert!(params.new_name.is_none());
    }

    #[test]
    fn manage_session_params_rename() {
        let json = serde_json::json!({
            "session": "old-name",
            "action": "rename",
            "new_name": "new-name"
        });
        let params: ManageSessionParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "old-name");
        assert!(matches!(params.action, ManageAction::Rename));
        assert_eq!(params.new_name.as_deref(), Some("new-name"));
    }

    #[test]
    fn manage_session_params_detach() {
        let json = serde_json::json!({
            "session": "my-session",
            "action": "detach"
        });
        let params: ManageSessionParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "my-session");
        assert!(matches!(params.action, ManageAction::Detach));
    }

    #[test]
    fn manage_session_params_missing_session() {
        let json = serde_json::json!({"action": "kill"});
        let result = serde_json::from_value::<ManageSessionParams>(json);
        assert!(result.is_err());
    }

    #[test]
    fn manage_session_params_missing_action() {
        let json = serde_json::json!({"session": "my-session"});
        let result = serde_json::from_value::<ManageSessionParams>(json);
        assert!(result.is_err());
    }
}
