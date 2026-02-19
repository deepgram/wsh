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

    /// Tags to assign to the session at creation time.
    #[serde(default)]
    #[schemars(description = "Tags to assign to the session at creation time.")]
    pub tags: Vec<String>,
}

/// Parameters for the `wsh_list_sessions` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListSessionsParams {
    /// If provided, return details for a single session instead of all sessions.
    #[schemars(description = "If provided, return details for this specific session instead of listing all.")]
    pub session: Option<String>,

    /// Filter sessions by tags (union/OR semantics). Only used when `session` is None.
    #[serde(default)]
    #[schemars(description = "Filter sessions by tags (union/OR semantics). Only used when session is not specified.")]
    pub tag: Vec<String>,
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
    /// Add tags to the session. Requires `tags`.
    AddTags,
    /// Remove tags from the session. Requires `tags`.
    RemoveTags,
}

/// Parameters for the `wsh_manage_session` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageSessionParams {
    /// The name of the session to manage.
    #[schemars(description = "The name of the target session.")]
    pub session: String,

    /// The action to perform on the session.
    #[schemars(description = "The action to perform: kill, rename, detach, add_tags, or remove_tags.")]
    pub action: ManageAction,

    /// New name for the session (required when action is 'rename').
    #[schemars(description = "New name for the session. Required when action is 'rename'.")]
    pub new_name: Option<String>,

    /// Tags to add or remove (used with `add_tags` and `remove_tags` actions).
    #[serde(default)]
    #[schemars(description = "Tags to add or remove. Required when action is 'add_tags' or 'remove_tags'.")]
    pub tags: Vec<String>,
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

// ── Visual feedback parameter types ─────────────────────────────

/// Parameters for the `wsh_overlay` tool (create, update, or list overlays).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OverlayParams {
    /// The name of the target session.
    #[schemars(description = "The name of the target session.")]
    pub session: String,

    /// Overlay ID. Omit to create a new overlay; provide to update an existing one.
    #[schemars(description = "Overlay ID. Omit to create a new overlay; provide to update an existing one.")]
    pub id: Option<String>,

    /// X position (column) of the overlay. Required for create.
    #[schemars(description = "X position (column) of the overlay. Required when creating.")]
    pub x: Option<u16>,

    /// Y position (row) of the overlay. Required for create.
    #[schemars(description = "Y position (row) of the overlay. Required when creating.")]
    pub y: Option<u16>,

    /// Z-index for stacking order. Auto-assigned if omitted on create.
    #[schemars(description = "Z-index for stacking order. Auto-assigned if omitted on create.")]
    pub z: Option<i32>,

    /// Width in columns. Required for create.
    #[schemars(description = "Width in columns. Required when creating.")]
    pub width: Option<u16>,

    /// Height in rows. Required for create.
    #[schemars(description = "Height in rows. Required when creating.")]
    pub height: Option<u16>,

    /// Background style as JSON object (e.g. {\"bg\": \"blue\"}).
    #[schemars(description = "Background style object. Example: {\"bg\": \"blue\"} or {\"bg\": {\"r\":0,\"g\":0,\"b\":128}}.")]
    pub background: Option<serde_json::Value>,

    /// Styled text spans to render in the overlay.
    #[schemars(description = "Array of styled text spans. Each span has 'text' (required), and optional 'id', 'fg', 'bg', 'bold', 'italic', 'underline'.")]
    pub spans: Option<Vec<serde_json::Value>>,

    /// Whether this overlay can receive input focus. Defaults to false.
    #[serde(default)]
    #[schemars(description = "Whether this overlay can receive input focus. Defaults to false.")]
    pub focusable: bool,

    /// If true, list all overlays for the current screen mode instead of creating/updating.
    #[serde(default)]
    #[schemars(description = "If true, list all overlays for the current screen mode. All other parameters are ignored.")]
    pub list: bool,
}

/// Parameters for the `wsh_remove_overlay` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RemoveOverlayParams {
    /// The name of the target session.
    #[schemars(description = "The name of the target session.")]
    pub session: String,

    /// Overlay ID to remove. If omitted, all overlays are cleared.
    #[schemars(description = "Overlay ID to remove. If omitted, all overlays are cleared.")]
    pub id: Option<String>,
}

/// Parameters for the `wsh_panel` tool (create, update, or list panels).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PanelParams {
    /// The name of the target session.
    #[schemars(description = "The name of the target session.")]
    pub session: String,

    /// Panel ID. Omit to create a new panel; provide to update an existing one.
    #[schemars(description = "Panel ID. Omit to create a new panel; provide to update an existing one.")]
    pub id: Option<String>,

    /// Panel position: \"top\" or \"bottom\". Required for create.
    #[schemars(description = "Panel position: 'top' or 'bottom'. Required when creating.")]
    pub position: Option<String>,

    /// Height in rows. Required for create.
    #[schemars(description = "Panel height in rows. Required when creating.")]
    pub height: Option<u16>,

    /// Z-index for stacking order. Auto-assigned if omitted on create.
    #[schemars(description = "Z-index for stacking order. Auto-assigned if omitted on create.")]
    pub z: Option<i32>,

    /// Background style as JSON object.
    #[schemars(description = "Background style object. Example: {\"bg\": \"blue\"}.")]
    pub background: Option<serde_json::Value>,

    /// Styled text spans to render in the panel.
    #[schemars(description = "Array of styled text spans. Each span has 'text' (required), and optional 'id', 'fg', 'bg', 'bold', 'italic', 'underline'.")]
    pub spans: Option<Vec<serde_json::Value>>,

    /// Whether this panel can receive input focus. Defaults to false.
    #[serde(default)]
    #[schemars(description = "Whether this panel can receive input focus. Defaults to false.")]
    pub focusable: bool,

    /// If true, list all panels for the current screen mode instead of creating/updating.
    #[serde(default)]
    #[schemars(description = "If true, list all panels for the current screen mode. All other parameters are ignored.")]
    pub list: bool,
}

/// Parameters for the `wsh_remove_panel` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RemovePanelParams {
    /// The name of the target session.
    #[schemars(description = "The name of the target session.")]
    pub session: String,

    /// Panel ID to remove. If omitted, all panels are cleared.
    #[schemars(description = "Panel ID to remove. If omitted, all panels are cleared.")]
    pub id: Option<String>,
}

// ── Input & screen mode parameter types ─────────────────────────

/// Action to perform on the input mode.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputModeAction {
    /// Switch to capture mode (input goes to API subscribers only).
    Capture,
    /// Switch to passthrough mode (input goes to both API subscribers and PTY).
    Release,
}

/// Parameters for the `wsh_input_mode` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InputModeParams {
    /// The name of the target session.
    #[schemars(description = "The name of the target session.")]
    pub session: String,

    /// Action to change the input mode. Omit to query the current mode without changing it.
    #[schemars(description = "Action to change the input mode: 'capture' or 'release'. Omit to query without changing.")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<InputModeAction>,

    /// ID of an overlay or panel to focus. The target must have focusable=true.
    #[schemars(description = "ID of an overlay or panel to focus. The target must have focusable=true.")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,

    /// If true, remove focus from any currently focused element.
    #[serde(default)]
    #[schemars(description = "If true, remove focus from any currently focused element.")]
    pub unfocus: bool,
}

/// Action to perform on the screen mode.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScreenModeAction {
    /// Enter alternate screen mode.
    EnterAlt,
    /// Exit alternate screen mode (cleans up alt-mode overlays and panels).
    ExitAlt,
}

/// Parameters for the `wsh_screen_mode` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ScreenModeParams {
    /// The name of the target session.
    #[schemars(description = "The name of the target session.")]
    pub session: String,

    /// Action to change the screen mode. Omit to query the current mode without changing it.
    #[schemars(description = "Action to change the screen mode: 'enter_alt' or 'exit_alt'. Omit to query without changing.")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<ScreenModeAction>,
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
        assert!(params.tags.is_empty());
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
        assert!(params.tag.is_empty());
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
        assert!(params.tags.is_empty());
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

    // ── CreateSessionParams with tags ───────────────────────────

    #[test]
    fn create_session_params_with_tags() {
        let json = serde_json::json!({
            "name": "tagged",
            "tags": ["build", "ci"]
        });
        let params: CreateSessionParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.name.as_deref(), Some("tagged"));
        assert_eq!(params.tags, vec!["build", "ci"]);
    }

    #[test]
    fn create_session_params_tags_default_to_empty() {
        let json = serde_json::json!({"name": "no-tags"});
        let params: CreateSessionParams = serde_json::from_value(json).unwrap();
        assert!(params.tags.is_empty());
    }

    // ── ListSessionsParams with tag filter ──────────────────────

    #[test]
    fn list_sessions_params_with_tag_filter() {
        let json = serde_json::json!({"tag": ["build", "test"]});
        let params: ListSessionsParams = serde_json::from_value(json).unwrap();
        assert!(params.session.is_none());
        assert_eq!(params.tag, vec!["build", "test"]);
    }

    #[test]
    fn list_sessions_params_tag_defaults_to_empty() {
        let json = serde_json::json!({"session": "s"});
        let params: ListSessionsParams = serde_json::from_value(json).unwrap();
        assert!(params.tag.is_empty());
    }

    // ── ManageAction tag variants ───────────────────────────────

    #[test]
    fn manage_action_add_tags() {
        let json = serde_json::json!("add_tags");
        let action: ManageAction = serde_json::from_value(json).unwrap();
        assert!(matches!(action, ManageAction::AddTags));
    }

    #[test]
    fn manage_action_remove_tags() {
        let json = serde_json::json!("remove_tags");
        let action: ManageAction = serde_json::from_value(json).unwrap();
        assert!(matches!(action, ManageAction::RemoveTags));
    }

    // ── ManageSessionParams with tags ───────────────────────────

    #[test]
    fn manage_session_params_add_tags() {
        let json = serde_json::json!({
            "session": "my-session",
            "action": "add_tags",
            "tags": ["build", "ci"]
        });
        let params: ManageSessionParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "my-session");
        assert!(matches!(params.action, ManageAction::AddTags));
        assert_eq!(params.tags, vec!["build", "ci"]);
    }

    #[test]
    fn manage_session_params_remove_tags() {
        let json = serde_json::json!({
            "session": "my-session",
            "action": "remove_tags",
            "tags": ["old-tag"]
        });
        let params: ManageSessionParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "my-session");
        assert!(matches!(params.action, ManageAction::RemoveTags));
        assert_eq!(params.tags, vec!["old-tag"]);
    }

    #[test]
    fn manage_session_params_tags_default_to_empty() {
        let json = serde_json::json!({
            "session": "s",
            "action": "kill"
        });
        let params: ManageSessionParams = serde_json::from_value(json).unwrap();
        assert!(params.tags.is_empty());
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

    // ── OverlayParams ──────────────────────────────────────────

    #[test]
    fn overlay_params_create_minimal() {
        let json = serde_json::json!({
            "session": "my-session",
            "x": 0,
            "y": 0,
            "width": 40,
            "height": 5
        });
        let params: OverlayParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "my-session");
        assert!(params.id.is_none());
        assert_eq!(params.x, Some(0));
        assert_eq!(params.y, Some(0));
        assert_eq!(params.width, Some(40));
        assert_eq!(params.height, Some(5));
        assert!(params.z.is_none());
        assert!(params.background.is_none());
        assert!(params.spans.is_none());
        assert!(!params.focusable);
        assert!(!params.list);
    }

    #[test]
    fn overlay_params_update_with_id() {
        let json = serde_json::json!({
            "session": "s",
            "id": "overlay-123",
            "x": 10,
            "spans": [{"text": "hello", "bold": true}]
        });
        let params: OverlayParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.id.as_deref(), Some("overlay-123"));
        assert_eq!(params.x, Some(10));
        assert!(params.spans.is_some());
        assert_eq!(params.spans.unwrap().len(), 1);
    }

    #[test]
    fn overlay_params_list_mode() {
        let json = serde_json::json!({
            "session": "s",
            "list": true
        });
        let params: OverlayParams = serde_json::from_value(json).unwrap();
        assert!(params.list);
    }

    #[test]
    fn overlay_params_with_background() {
        let json = serde_json::json!({
            "session": "s",
            "x": 0,
            "y": 0,
            "width": 20,
            "height": 3,
            "background": {"bg": "blue"}
        });
        let params: OverlayParams = serde_json::from_value(json).unwrap();
        assert!(params.background.is_some());
    }

    #[test]
    fn overlay_params_missing_session() {
        let json = serde_json::json!({"x": 0, "y": 0, "width": 10, "height": 1});
        let result = serde_json::from_value::<OverlayParams>(json);
        assert!(result.is_err());
    }

    // ── RemoveOverlayParams ────────────────────────────────────

    #[test]
    fn remove_overlay_params_with_id() {
        let json = serde_json::json!({
            "session": "s",
            "id": "overlay-abc"
        });
        let params: RemoveOverlayParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "s");
        assert_eq!(params.id.as_deref(), Some("overlay-abc"));
    }

    #[test]
    fn remove_overlay_params_clear_all() {
        let json = serde_json::json!({"session": "s"});
        let params: RemoveOverlayParams = serde_json::from_value(json).unwrap();
        assert!(params.id.is_none());
    }

    #[test]
    fn remove_overlay_params_missing_session() {
        let json = serde_json::json!({});
        let result = serde_json::from_value::<RemoveOverlayParams>(json);
        assert!(result.is_err());
    }

    // ── PanelParams ────────────────────────────────────────────

    #[test]
    fn panel_params_create_minimal() {
        let json = serde_json::json!({
            "session": "my-session",
            "position": "bottom",
            "height": 2
        });
        let params: PanelParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "my-session");
        assert!(params.id.is_none());
        assert_eq!(params.position.as_deref(), Some("bottom"));
        assert_eq!(params.height, Some(2));
        assert!(!params.focusable);
        assert!(!params.list);
    }

    #[test]
    fn panel_params_update_with_id() {
        let json = serde_json::json!({
            "session": "s",
            "id": "panel-456",
            "height": 3,
            "spans": [{"text": "status line"}]
        });
        let params: PanelParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.id.as_deref(), Some("panel-456"));
        assert_eq!(params.height, Some(3));
        assert!(params.spans.is_some());
    }

    #[test]
    fn panel_params_list_mode() {
        let json = serde_json::json!({
            "session": "s",
            "list": true
        });
        let params: PanelParams = serde_json::from_value(json).unwrap();
        assert!(params.list);
    }

    #[test]
    fn panel_params_missing_session() {
        let json = serde_json::json!({"position": "top", "height": 1});
        let result = serde_json::from_value::<PanelParams>(json);
        assert!(result.is_err());
    }

    #[test]
    fn panel_params_with_all_fields() {
        let json = serde_json::json!({
            "session": "s",
            "position": "top",
            "height": 4,
            "z": 10,
            "background": {"bg": {"r": 50, "g": 50, "b": 50}},
            "spans": [{"text": "hello"}],
            "focusable": true
        });
        let params: PanelParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.position.as_deref(), Some("top"));
        assert_eq!(params.height, Some(4));
        assert_eq!(params.z, Some(10));
        assert!(params.background.is_some());
        assert!(params.spans.is_some());
        assert!(params.focusable);
    }

    // ── RemovePanelParams ──────────────────────────────────────

    #[test]
    fn remove_panel_params_with_id() {
        let json = serde_json::json!({
            "session": "s",
            "id": "panel-xyz"
        });
        let params: RemovePanelParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "s");
        assert_eq!(params.id.as_deref(), Some("panel-xyz"));
    }

    #[test]
    fn remove_panel_params_clear_all() {
        let json = serde_json::json!({"session": "s"});
        let params: RemovePanelParams = serde_json::from_value(json).unwrap();
        assert!(params.id.is_none());
    }

    #[test]
    fn remove_panel_params_missing_session() {
        let json = serde_json::json!({});
        let result = serde_json::from_value::<RemovePanelParams>(json);
        assert!(result.is_err());
    }

    // ── InputModeAction ───────────────────────────────────────────

    #[test]
    fn input_mode_action_capture() {
        let json = serde_json::json!("capture");
        let action: InputModeAction = serde_json::from_value(json).unwrap();
        assert!(matches!(action, InputModeAction::Capture));
    }

    #[test]
    fn input_mode_action_release() {
        let json = serde_json::json!("release");
        let action: InputModeAction = serde_json::from_value(json).unwrap();
        assert!(matches!(action, InputModeAction::Release));
    }

    #[test]
    fn input_mode_action_invalid() {
        let json = serde_json::json!("toggle");
        let result = serde_json::from_value::<InputModeAction>(json);
        assert!(result.is_err());
    }

    // ── InputModeParams ───────────────────────────────────────────

    #[test]
    fn input_mode_params_query_only() {
        let json = serde_json::json!({"session": "my-session"});
        let params: InputModeParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "my-session");
        assert!(params.mode.is_none());
        assert!(params.focus.is_none());
        assert!(!params.unfocus);
    }

    #[test]
    fn input_mode_params_capture() {
        let json = serde_json::json!({
            "session": "s",
            "mode": "capture"
        });
        let params: InputModeParams = serde_json::from_value(json).unwrap();
        assert!(matches!(params.mode, Some(InputModeAction::Capture)));
    }

    #[test]
    fn input_mode_params_release() {
        let json = serde_json::json!({
            "session": "s",
            "mode": "release"
        });
        let params: InputModeParams = serde_json::from_value(json).unwrap();
        assert!(matches!(params.mode, Some(InputModeAction::Release)));
    }

    #[test]
    fn input_mode_params_with_focus() {
        let json = serde_json::json!({
            "session": "s",
            "focus": "overlay-123"
        });
        let params: InputModeParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.focus.as_deref(), Some("overlay-123"));
    }

    #[test]
    fn input_mode_params_with_unfocus() {
        let json = serde_json::json!({
            "session": "s",
            "unfocus": true
        });
        let params: InputModeParams = serde_json::from_value(json).unwrap();
        assert!(params.unfocus);
    }

    #[test]
    fn input_mode_params_all_fields() {
        let json = serde_json::json!({
            "session": "s",
            "mode": "capture",
            "focus": "panel-1",
            "unfocus": false
        });
        let params: InputModeParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "s");
        assert!(matches!(params.mode, Some(InputModeAction::Capture)));
        assert_eq!(params.focus.as_deref(), Some("panel-1"));
        assert!(!params.unfocus);
    }

    #[test]
    fn input_mode_params_missing_session() {
        let json = serde_json::json!({"mode": "capture"});
        let result = serde_json::from_value::<InputModeParams>(json);
        assert!(result.is_err());
    }

    // ── ScreenModeAction ──────────────────────────────────────────

    #[test]
    fn screen_mode_action_enter_alt() {
        let json = serde_json::json!("enter_alt");
        let action: ScreenModeAction = serde_json::from_value(json).unwrap();
        assert!(matches!(action, ScreenModeAction::EnterAlt));
    }

    #[test]
    fn screen_mode_action_exit_alt() {
        let json = serde_json::json!("exit_alt");
        let action: ScreenModeAction = serde_json::from_value(json).unwrap();
        assert!(matches!(action, ScreenModeAction::ExitAlt));
    }

    #[test]
    fn screen_mode_action_invalid() {
        let json = serde_json::json!("toggle");
        let result = serde_json::from_value::<ScreenModeAction>(json);
        assert!(result.is_err());
    }

    // ── ScreenModeParams ──────────────────────────────────────────

    #[test]
    fn screen_mode_params_query_only() {
        let json = serde_json::json!({"session": "my-session"});
        let params: ScreenModeParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session, "my-session");
        assert!(params.action.is_none());
    }

    #[test]
    fn screen_mode_params_enter_alt() {
        let json = serde_json::json!({
            "session": "s",
            "action": "enter_alt"
        });
        let params: ScreenModeParams = serde_json::from_value(json).unwrap();
        assert!(matches!(params.action, Some(ScreenModeAction::EnterAlt)));
    }

    #[test]
    fn screen_mode_params_exit_alt() {
        let json = serde_json::json!({
            "session": "s",
            "action": "exit_alt"
        });
        let params: ScreenModeParams = serde_json::from_value(json).unwrap();
        assert!(matches!(params.action, Some(ScreenModeAction::ExitAlt)));
    }

    #[test]
    fn screen_mode_params_missing_session() {
        let json = serde_json::json!({"action": "enter_alt"});
        let result = serde_json::from_value::<ScreenModeParams>(json);
        assert!(result.is_err());
    }
}
