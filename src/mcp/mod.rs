pub mod tools;
pub mod resources;
pub mod prompts;

use rmcp::{
    handler::server::router::tool::ToolRouter,
    model::*,
    tool, tool_router,
    handler::server::wrapper::Parameters,
};

use crate::api::AppState;
use crate::pty::SpawnCommand;
use crate::session::{RegistryError, Session};

use tools::{CreateSessionParams, ListSessionsParams, ManageSessionParams, ManageAction};

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
}
