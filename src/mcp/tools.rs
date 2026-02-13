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

// ── Terminal I/O parameter types ─────────────────────────────────

/// Input encoding for `wsh_send_input`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    /// UTF-8 text (default).
    Utf8,
    /// Base64-encoded binary data.
    Base64,
}

fn default_encoding() -> Encoding {
    Encoding::Utf8
}

/// Parameters for the `wsh_send_input` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SendInputParams {
    /// The name of the target session.
    #[schemars(description = "The name of the target session.")]
    pub session: String,

    /// The input data to send. Interpretation depends on `encoding`.
    #[schemars(description = "The input data to send. For utf8 encoding, this is plain text. For base64 encoding, this is base64-encoded binary data.")]
    pub input: String,

    /// How to interpret the `input` field. Defaults to `utf8`.
    #[serde(default = "default_encoding")]
    #[schemars(description = "Input encoding: 'utf8' (default) for plain text, 'base64' for binary data.")]
    pub encoding: Encoding,
}

/// Screen content format for query results.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScreenFormat {
    /// Styled output with color and attribute spans (default).
    #[default]
    Styled,
    /// Plain text without formatting.
    Plain,
}

impl ScreenFormat {
    /// Convert to the parser's internal `Format` enum.
    pub fn into_parser_format(self) -> crate::parser::state::Format {
        match self {
            Self::Styled => crate::parser::state::Format::Styled,
            Self::Plain => crate::parser::state::Format::Plain,
        }
    }
}

/// Parameters for the `wsh_get_screen` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetScreenParams {
    /// The name of the target session.
    #[schemars(description = "The name of the target session.")]
    pub session: String,

    /// Output format for screen content. Defaults to `styled`.
    #[serde(default)]
    #[schemars(description = "Output format: 'styled' (default) includes color/attribute spans, 'plain' returns raw text.")]
    pub format: ScreenFormat,
}

fn default_limit() -> usize {
    100
}

/// Parameters for the `wsh_get_scrollback` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetScrollbackParams {
    /// The name of the target session.
    #[schemars(description = "The name of the target session.")]
    pub session: String,

    /// Line offset into the scrollback buffer. Defaults to 0 (most recent).
    #[serde(default)]
    #[schemars(description = "Line offset into the scrollback buffer. Defaults to 0 (most recent).")]
    pub offset: usize,

    /// Maximum number of lines to return. Defaults to 100.
    #[serde(default = "default_limit")]
    #[schemars(description = "Maximum number of lines to return. Defaults to 100.")]
    pub limit: usize,

    /// Output format for scrollback content. Defaults to `styled`.
    #[serde(default)]
    #[schemars(description = "Output format: 'styled' (default) includes color/attribute spans, 'plain' returns raw text.")]
    pub format: ScreenFormat,
}

fn default_timeout_ms() -> u64 {
    2000
}

fn default_max_wait_ms() -> u64 {
    30000
}

/// Parameters for the `wsh_await_quiesce` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AwaitQuiesceParams {
    /// The name of the target session.
    #[schemars(description = "The name of the target session.")]
    pub session: String,

    /// Quiescence timeout in milliseconds. The terminal must be idle for this
    /// duration before quiescence is declared. Defaults to 2000.
    #[serde(default = "default_timeout_ms")]
    #[schemars(description = "Quiescence timeout in milliseconds. Terminal must be idle for this long. Defaults to 2000.")]
    pub timeout_ms: u64,

    /// Maximum wall-clock time to wait in milliseconds. If quiescence is not
    /// reached within this deadline, an error is returned. Defaults to 30000.
    #[serde(default = "default_max_wait_ms")]
    #[schemars(description = "Maximum wall-clock time to wait in milliseconds. Defaults to 30000.")]
    pub max_wait_ms: u64,
}

/// Parameters for the `wsh_run_command` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RunCommandParams {
    /// The name of the target session.
    #[schemars(description = "The name of the target session.")]
    pub session: String,

    /// The input to send (typically a command followed by a newline).
    #[schemars(description = "The input to send to the terminal (e.g. a command string). A newline is NOT appended automatically.")]
    pub input: String,

    /// Quiescence timeout in milliseconds. Defaults to 2000.
    #[serde(default = "default_timeout_ms")]
    #[schemars(description = "Quiescence timeout in milliseconds. Defaults to 2000.")]
    pub timeout_ms: u64,

    /// Maximum wall-clock time to wait in milliseconds. Defaults to 30000.
    #[serde(default = "default_max_wait_ms")]
    #[schemars(description = "Maximum wall-clock time to wait in milliseconds. Defaults to 30000.")]
    pub max_wait_ms: u64,

    /// Output format for the screen snapshot. Defaults to `styled`.
    #[serde(default)]
    #[schemars(description = "Output format: 'styled' (default) includes color/attribute spans, 'plain' returns raw text.")]
    pub format: ScreenFormat,
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

    // ── SendInputParams ─────────────────────────────────────────

    #[test]
    fn send_input_params_defaults_to_utf8() {
        let json = serde_json::json!({
            "session": "my-session",
            "input": "hello\n"
        });
        let params: SendInputParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "my-session");
        assert_eq!(params.input, "hello\n");
        assert!(matches!(params.encoding, Encoding::Utf8));
    }

    #[test]
    fn send_input_params_with_base64() {
        let json = serde_json::json!({
            "session": "my-session",
            "input": "aGVsbG8K",
            "encoding": "base64"
        });
        let params: SendInputParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "my-session");
        assert_eq!(params.input, "aGVsbG8K");
        assert!(matches!(params.encoding, Encoding::Base64));
    }

    #[test]
    fn send_input_params_missing_input() {
        let json = serde_json::json!({"session": "s"});
        let result = serde_json::from_value::<SendInputParams>(json);
        assert!(result.is_err());
    }

    #[test]
    fn send_input_params_invalid_encoding() {
        let json = serde_json::json!({
            "session": "s",
            "input": "x",
            "encoding": "hex"
        });
        let result = serde_json::from_value::<SendInputParams>(json);
        assert!(result.is_err());
    }

    // ── GetScreenParams ─────────────────────────────────────────

    #[test]
    fn get_screen_params_defaults_to_styled() {
        let json = serde_json::json!({"session": "my-session"});
        let params: GetScreenParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "my-session");
        assert!(matches!(params.format, ScreenFormat::Styled));
    }

    #[test]
    fn get_screen_params_plain_format() {
        let json = serde_json::json!({
            "session": "my-session",
            "format": "plain"
        });
        let params: GetScreenParams = serde_json::from_value(json).unwrap();
        assert!(matches!(params.format, ScreenFormat::Plain));
    }

    #[test]
    fn screen_format_into_parser_format() {
        let styled = ScreenFormat::Styled;
        assert!(matches!(
            styled.into_parser_format(),
            crate::parser::state::Format::Styled
        ));

        let plain = ScreenFormat::Plain;
        assert!(matches!(
            plain.into_parser_format(),
            crate::parser::state::Format::Plain
        ));
    }

    // ── GetScrollbackParams ─────────────────────────────────────

    #[test]
    fn get_scrollback_params_defaults() {
        let json = serde_json::json!({"session": "my-session"});
        let params: GetScrollbackParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "my-session");
        assert_eq!(params.offset, 0);
        assert_eq!(params.limit, 100);
        assert!(matches!(params.format, ScreenFormat::Styled));
    }

    #[test]
    fn get_scrollback_params_with_pagination() {
        let json = serde_json::json!({
            "session": "my-session",
            "offset": 50,
            "limit": 25,
            "format": "plain"
        });
        let params: GetScrollbackParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.offset, 50);
        assert_eq!(params.limit, 25);
        assert!(matches!(params.format, ScreenFormat::Plain));
    }

    // ── AwaitQuiesceParams ──────────────────────────────────────

    #[test]
    fn await_quiesce_params_defaults() {
        let json = serde_json::json!({"session": "my-session"});
        let params: AwaitQuiesceParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "my-session");
        assert_eq!(params.timeout_ms, 2000);
        assert_eq!(params.max_wait_ms, 30000);
    }

    #[test]
    fn await_quiesce_params_custom_timeouts() {
        let json = serde_json::json!({
            "session": "s",
            "timeout_ms": 500,
            "max_wait_ms": 10000
        });
        let params: AwaitQuiesceParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.timeout_ms, 500);
        assert_eq!(params.max_wait_ms, 10000);
    }

    // ── RunCommandParams ────────────────────────────────────────

    #[test]
    fn run_command_params_defaults() {
        let json = serde_json::json!({
            "session": "my-session",
            "input": "ls -la\n"
        });
        let params: RunCommandParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "my-session");
        assert_eq!(params.input, "ls -la\n");
        assert_eq!(params.timeout_ms, 2000);
        assert_eq!(params.max_wait_ms, 30000);
        assert!(matches!(params.format, ScreenFormat::Styled));
    }

    #[test]
    fn run_command_params_all_fields() {
        let json = serde_json::json!({
            "session": "s",
            "input": "echo hi\n",
            "timeout_ms": 1000,
            "max_wait_ms": 5000,
            "format": "plain"
        });
        let params: RunCommandParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.timeout_ms, 1000);
        assert_eq!(params.max_wait_ms, 5000);
        assert!(matches!(params.format, ScreenFormat::Plain));
    }

    #[test]
    fn run_command_params_missing_input() {
        let json = serde_json::json!({"session": "s"});
        let result = serde_json::from_value::<RunCommandParams>(json);
        assert!(result.is_err());
    }
}
