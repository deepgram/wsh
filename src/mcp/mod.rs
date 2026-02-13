pub mod tools;
pub mod resources;
pub mod prompts;

use std::time::Duration;

use bytes::Bytes;
use rmcp::{
    handler::server::router::tool::ToolRouter,
    model::*,
    tool, tool_router,
    handler::server::wrapper::Parameters,
};

use crate::api::AppState;
use crate::parser::state::Query;
use crate::pty::SpawnCommand;
use crate::session::{RegistryError, Session};

use tools::{
    CreateSessionParams, ListSessionsParams, ManageSessionParams, ManageAction,
    SendInputParams, Encoding, GetScreenParams, GetScrollbackParams,
    AwaitQuiesceParams, RunCommandParams,
};

#[derive(Clone)]
pub struct WshMcpServer {
    state: AppState,
    tool_router: ToolRouter<WshMcpServer>,
}

impl WshMcpServer {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    fn get_session(&self, name: &str) -> Result<crate::session::Session, ErrorData> {
        self.state
            .sessions
            .get(name)
            .ok_or_else(|| ErrorData::invalid_params(format!("session not found: {name}"), None))
    }
}

#[tool_router]
impl WshMcpServer {
    /// Create a new terminal session with an interactive shell or a specific command.
    #[tool(description = "Create a new terminal session. Spawns an interactive shell by default, or runs a specific command. Returns the assigned session name and terminal dimensions.")]
    async fn wsh_create_session(
        &self,
        Parameters(params): Parameters<CreateSessionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let command = match params.command {
            Some(cmd) => SpawnCommand::Command {
                command: cmd,
                interactive: true,
            },
            None => SpawnCommand::Shell {
                interactive: true,
                shell: None,
            },
        };

        let rows = params.rows.unwrap_or(24);
        let cols = params.cols.unwrap_or(80);

        let (session, child_exit_rx) =
            Session::spawn_with_options("".to_string(), command, rows, cols, params.cwd, params.env)
                .map_err(|e| {
                    ErrorData::internal_error(
                        format!("failed to spawn session: {e}"),
                        None,
                    )
                })?;

        let assigned_name =
            self.state.sessions.insert(params.name, session).map_err(
                |e| match e {
                    RegistryError::NameExists(n) => ErrorData::invalid_params(
                        format!("session name already exists: {n}"),
                        None,
                    ),
                    RegistryError::NotFound(n) => ErrorData::internal_error(
                        format!("unexpected registry error: {n}"),
                        None,
                    ),
                },
            )?;

        // Monitor child exit so SessionEvent::Exited fires when the process dies.
        self.state
            .sessions
            .monitor_child_exit(assigned_name.clone(), child_exit_rx);

        let result = serde_json::json!({
            "name": assigned_name,
            "rows": rows,
            "cols": cols,
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).unwrap_or_default(),
        )]))
    }

    /// List all sessions or get details for a specific session.
    #[tool(description = "List all terminal sessions, or get details for a specific session by name. Returns session names and terminal dimensions.")]
    async fn wsh_list_sessions(
        &self,
        Parameters(params): Parameters<ListSessionsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(name) = params.session {
            // Single session detail
            let session = self.get_session(&name)?;
            let (rows, cols) = session.terminal_size.get();
            let result = serde_json::json!({
                "name": session.name,
                "rows": rows,
                "cols": cols,
            });
            Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&result).unwrap_or_default(),
            )]))
        } else {
            // All sessions
            let names = self.state.sessions.list();
            let sessions: Vec<serde_json::Value> = names
                .into_iter()
                .filter_map(|name| {
                    let session = self.state.sessions.get(&name)?;
                    let (rows, cols) = session.terminal_size.get();
                    Some(serde_json::json!({
                        "name": name,
                        "rows": rows,
                        "cols": cols,
                    }))
                })
                .collect();

            let result = serde_json::json!(sessions);
            Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&result).unwrap_or_default(),
            )]))
        }
    }

    /// Manage an existing session: kill, rename, or detach.
    #[tool(description = "Manage a terminal session. Actions: 'kill' destroys the session, 'rename' changes its name (requires new_name), 'detach' disconnects all streaming clients.")]
    async fn wsh_manage_session(
        &self,
        Parameters(params): Parameters<ManageSessionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match params.action {
            ManageAction::Kill => {
                self.state
                    .sessions
                    .remove(&params.session)
                    .ok_or_else(|| {
                        ErrorData::invalid_params(
                            format!("session not found: {}", params.session),
                            None,
                        )
                    })?;

                let result = serde_json::json!({
                    "status": "killed",
                    "session": params.session,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }

            ManageAction::Rename => {
                let new_name = params.new_name.ok_or_else(|| {
                    ErrorData::invalid_params(
                        "new_name is required for rename action",
                        None,
                    )
                })?;

                self.state
                    .sessions
                    .rename(&params.session, &new_name)
                    .map_err(|e| match e {
                        RegistryError::NameExists(n) => ErrorData::invalid_params(
                            format!("session name already exists: {n}"),
                            None,
                        ),
                        RegistryError::NotFound(n) => ErrorData::invalid_params(
                            format!("session not found: {n}"),
                            None,
                        ),
                    })?;

                let result = serde_json::json!({
                    "status": "renamed",
                    "old_name": params.session,
                    "new_name": new_name,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }

            ManageAction::Detach => {
                let session = self.get_session(&params.session)?;
                session.detach();

                let result = serde_json::json!({
                    "status": "detached",
                    "session": params.session,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }
        }
    }

    // ── Terminal I/O tools ───────────────────────────────────────

    /// Send input (keystrokes, text, or binary data) to a terminal session.
    #[tool(description = "Send input to a terminal session. Supports UTF-8 text (default) or base64-encoded binary data. The input is delivered to the PTY exactly as provided — no newline is appended automatically.")]
    async fn wsh_send_input(
        &self,
        Parameters(params): Parameters<SendInputParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let session = self.get_session(&params.session)?;

        let data = match params.encoding {
            Encoding::Utf8 => Bytes::from(params.input.into_bytes()),
            Encoding::Base64 => {
                use base64::Engine;
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(&params.input)
                    .map_err(|e| {
                        ErrorData::invalid_params(
                            format!("invalid base64 input: {e}"),
                            None,
                        )
                    })?;
                Bytes::from(decoded)
            }
        };

        let len = data.len();
        session.input_tx.send(data).await.map_err(|e| {
            ErrorData::internal_error(
                format!("failed to send input: {e}"),
                None,
            )
        })?;
        session.activity.touch();

        let result = serde_json::json!({
            "status": "sent",
            "bytes": len,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).unwrap_or_default(),
        )]))
    }

    /// Get the current visible screen contents of a terminal session.
    #[tool(description = "Get the current visible screen contents of a terminal session. Returns the screen grid with text, colors, cursor position, and terminal dimensions.")]
    async fn wsh_get_screen(
        &self,
        Parameters(params): Parameters<GetScreenParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let session = self.get_session(&params.session)?;
        let format = params.format.into_parser_format();

        let response = session
            .parser
            .query(Query::Screen { format })
            .await
            .map_err(|e| {
                ErrorData::internal_error(format!("parser error: {e}"), None)
            })?;

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&response).unwrap_or_default(),
        )]))
    }

    /// Get scrollback buffer contents from a terminal session.
    #[tool(description = "Get scrollback buffer contents from a terminal session. Returns historical output with pagination support (offset and limit). Useful for reading output that has scrolled off the visible screen.")]
    async fn wsh_get_scrollback(
        &self,
        Parameters(params): Parameters<GetScrollbackParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let session = self.get_session(&params.session)?;
        let format = params.format.into_parser_format();

        let response = session
            .parser
            .query(Query::Scrollback {
                format,
                offset: params.offset,
                limit: params.limit,
            })
            .await
            .map_err(|e| {
                ErrorData::internal_error(format!("parser error: {e}"), None)
            })?;

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&response).unwrap_or_default(),
        )]))
    }

    /// Wait for a terminal session to become quiescent (idle).
    #[tool(description = "Wait for a terminal session to become quiescent (no output for timeout_ms). Returns the activity generation number on success. Returns an error result if max_wait_ms is exceeded before quiescence is reached.")]
    async fn wsh_await_quiesce(
        &self,
        Parameters(params): Parameters<AwaitQuiesceParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let session = self.get_session(&params.session)?;

        let timeout = Duration::from_millis(params.timeout_ms);
        let max_wait = Duration::from_millis(params.max_wait_ms);

        match tokio::time::timeout(
            max_wait,
            session.activity.wait_for_quiescence(timeout, None),
        )
        .await
        {
            Ok(generation) => {
                let result = serde_json::json!({
                    "status": "quiescent",
                    "generation": generation,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }
            Err(_) => {
                let result = serde_json::json!({
                    "error": "quiesce timeout exceeded max_wait_ms",
                    "timeout_ms": params.timeout_ms,
                    "max_wait_ms": params.max_wait_ms,
                });
                Ok(CallToolResult::error(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }
        }
    }

    /// Send input and wait for the terminal to become idle, then return the screen.
    #[tool(description = "Send input to a terminal session, wait for quiescence, then return the screen contents. This is the primary 'run a command' primitive: send input, wait for output to settle, read the result. If quiescence is not reached within max_wait_ms, the screen is still returned but marked as an error.")]
    async fn wsh_run_command(
        &self,
        Parameters(params): Parameters<RunCommandParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let session = self.get_session(&params.session)?;

        // 1. Send input
        let data = Bytes::from(params.input.into_bytes());
        session.input_tx.send(data).await.map_err(|e| {
            ErrorData::internal_error(
                format!("failed to send input: {e}"),
                None,
            )
        })?;
        session.activity.touch();

        // 2. Await quiescence
        let timeout = Duration::from_millis(params.timeout_ms);
        let max_wait = Duration::from_millis(params.max_wait_ms);

        let quiesce_result = tokio::time::timeout(
            max_wait,
            session.activity.wait_for_quiescence(timeout, None),
        )
        .await;

        // 3. Get screen regardless of quiescence outcome
        let format = params.format.into_parser_format();
        let screen = session
            .parser
            .query(Query::Screen { format })
            .await
            .map_err(|e| {
                ErrorData::internal_error(format!("parser error: {e}"), None)
            })?;

        match quiesce_result {
            Ok(generation) => {
                let result = serde_json::json!({
                    "screen": screen,
                    "generation": generation,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }
            Err(_) => {
                let result = serde_json::json!({
                    "error": "quiesce timeout exceeded max_wait_ms",
                    "screen": screen,
                });
                Ok(CallToolResult::error(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }
        }
    }
}
