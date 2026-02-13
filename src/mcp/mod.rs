pub mod tools;
pub mod resources;
pub mod prompts;

use std::future::Future;
use std::time::Duration;

use bytes::Bytes;
use rmcp::{
    handler::server::router::tool::ToolRouter,
    model::*,
    tool, tool_router, tool_handler,
    handler::server::wrapper::Parameters,
    service::RequestContext,
    RoleServer,
    ServerHandler,
};

use crate::api::AppState;
use crate::parser::state::Query;
use crate::pty::SpawnCommand;
use crate::session::{RegistryError, Session};

use tools::{
    CreateSessionParams, ListSessionsParams, ManageSessionParams, ManageAction,
    SendInputParams, Encoding, GetScreenParams, GetScrollbackParams,
    AwaitQuiesceParams, RunCommandParams,
    OverlayParams, RemoveOverlayParams, PanelParams, RemovePanelParams,
    InputModeParams, InputModeAction, ScreenModeParams, ScreenModeAction,
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

#[tool_handler(router = self.tool_router)]
impl ServerHandler for WshMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
            server_info: Implementation {
                name: "wsh".to_string(),
                title: None,
                version: env!("CARGO_PKG_VERSION").to_string(),
                description: Some(
                    "An API for your terminal. Exposes terminal sessions as structured, \
                     programmable interfaces for AI agents and automation."
                        .to_string(),
                ),
                icons: None,
                website_url: None,
            },
            instructions: Some(
                "wsh exposes terminal sessions as an API. Use wsh_run_command for the common \
                 send/wait/read loop. Use wsh_create_session to start sessions, \
                 wsh_list_sessions to discover them, wsh_manage_session to kill/rename/detach. \
                 Visual feedback via wsh_overlay and wsh_panel. Input capture via wsh_input_mode."
                    .to_string(),
            ),
        }
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, ErrorData>> + Send + '_ {
        async { resources::list_resources(&self.state).await }
    }

    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, ErrorData>> + Send + '_ {
        async { resources::list_resource_templates().await }
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResult, ErrorData>> + Send + '_ {
        async move { resources::read_resource(&self.state, request).await }
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, ErrorData>> + Send + '_ {
        async { prompts::list_prompts().await }
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<GetPromptResult, ErrorData>> + Send + '_ {
        async move { prompts::get_prompt(&request.name).await }
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

        // Monitor child exit so the session is auto-removed when the process dies.
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

    // ── Visual feedback tools ────────────────────────────────────

    /// Create, update, or list overlays on a terminal session.
    #[tool(description = "Create, update, or list overlays on a terminal session. Overlays are styled text boxes rendered on top of terminal content. Modes: set list=true to list all overlays; omit id to create a new overlay (x, y, width, height required); provide id to update an existing overlay.")]
    async fn wsh_overlay(
        &self,
        Parameters(params): Parameters<OverlayParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let session = self.get_session(&params.session)?;
        let current_mode = *session.screen_mode.read();

        // LIST mode
        if params.list {
            let overlays = session.overlays.list_by_mode(current_mode);
            let result = serde_json::json!({
                "overlays": overlays,
            });
            return Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&result).unwrap_or_default(),
            )]));
        }

        // Deserialize spans if provided
        let spans = match &params.spans {
            Some(raw) => {
                let spans: Vec<crate::overlay::OverlaySpan> = raw
                    .iter()
                    .map(|v| serde_json::from_value(v.clone()))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| {
                        ErrorData::invalid_params(format!("invalid span: {e}"), None)
                    })?;
                Some(spans)
            }
            None => None,
        };

        // Deserialize background if provided
        let background = match &params.background {
            Some(raw) => {
                let bg: crate::overlay::BackgroundStyle =
                    serde_json::from_value(raw.clone()).map_err(|e| {
                        ErrorData::invalid_params(format!("invalid background: {e}"), None)
                    })?;
                Some(bg)
            }
            None => None,
        };

        match params.id {
            // UPDATE existing overlay
            Some(id) => {
                // Patch position/size if any fields provided
                let has_position_fields = params.x.is_some()
                    || params.y.is_some()
                    || params.z.is_some()
                    || params.width.is_some()
                    || params.height.is_some()
                    || background.is_some();

                if has_position_fields {
                    let found = session.overlays.move_to(
                        &id,
                        params.x,
                        params.y,
                        params.z,
                        params.width,
                        params.height,
                        background,
                    );
                    if !found {
                        return Err(ErrorData::invalid_params(
                            format!("overlay not found: {id}"),
                            None,
                        ));
                    }
                }

                // Replace spans if provided
                if let Some(spans) = spans {
                    let found = session.overlays.update(&id, spans);
                    if !found {
                        return Err(ErrorData::invalid_params(
                            format!("overlay not found: {id}"),
                            None,
                        ));
                    }
                }

                let _ = session
                    .visual_update_tx
                    .send(crate::protocol::VisualUpdate::OverlaysChanged);

                let result = serde_json::json!({
                    "status": "updated",
                    "id": id,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }

            // CREATE new overlay
            None => {
                let x = params.x.ok_or_else(|| {
                    ErrorData::invalid_params("x is required when creating an overlay", None)
                })?;
                let y = params.y.ok_or_else(|| {
                    ErrorData::invalid_params("y is required when creating an overlay", None)
                })?;
                let width = params.width.ok_or_else(|| {
                    ErrorData::invalid_params("width is required when creating an overlay", None)
                })?;
                let height = params.height.ok_or_else(|| {
                    ErrorData::invalid_params("height is required when creating an overlay", None)
                })?;

                let id = session.overlays.create(
                    x,
                    y,
                    params.z,
                    width,
                    height,
                    background,
                    spans.unwrap_or_default(),
                    params.focusable,
                    current_mode,
                );

                let _ = session
                    .visual_update_tx
                    .send(crate::protocol::VisualUpdate::OverlaysChanged);

                let result = serde_json::json!({
                    "status": "created",
                    "id": id,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }
        }
    }

    /// Remove an overlay or clear all overlays from a terminal session.
    #[tool(description = "Remove an overlay by ID, or clear all overlays from a terminal session. If id is omitted, all overlays are removed.")]
    async fn wsh_remove_overlay(
        &self,
        Parameters(params): Parameters<RemoveOverlayParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let session = self.get_session(&params.session)?;

        match params.id {
            Some(id) => {
                let found = session.overlays.delete(&id);
                if !found {
                    return Err(ErrorData::invalid_params(
                        format!("overlay not found: {id}"),
                        None,
                    ));
                }
                session.focus.clear_if_focused(&id);
                let _ = session
                    .visual_update_tx
                    .send(crate::protocol::VisualUpdate::OverlaysChanged);

                let result = serde_json::json!({
                    "status": "removed",
                    "id": id,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }
            None => {
                session.overlays.clear();
                session.focus.unfocus();
                let _ = session
                    .visual_update_tx
                    .send(crate::protocol::VisualUpdate::OverlaysChanged);

                let result = serde_json::json!({
                    "status": "cleared",
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }
        }
    }

    /// Create, update, or list panels on a terminal session.
    #[tool(description = "Create, update, or list panels on a terminal session. Panels carve out dedicated rows at the top or bottom of the terminal, shrinking the PTY viewport. Modes: set list=true to list all panels; omit id to create (position and height required); provide id to update.")]
    async fn wsh_panel(
        &self,
        Parameters(params): Parameters<PanelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let session = self.get_session(&params.session)?;
        let current_mode = *session.screen_mode.read();

        // LIST mode
        if params.list {
            let panels = session.panels.list_by_mode(current_mode);
            let result = serde_json::json!({
                "panels": panels,
            });
            return Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&result).unwrap_or_default(),
            )]));
        }

        // Deserialize spans if provided
        let spans = match &params.spans {
            Some(raw) => {
                let spans: Vec<crate::overlay::OverlaySpan> = raw
                    .iter()
                    .map(|v| serde_json::from_value(v.clone()))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| {
                        ErrorData::invalid_params(format!("invalid span: {e}"), None)
                    })?;
                Some(spans)
            }
            None => None,
        };

        // Deserialize background if provided
        let background = match &params.background {
            Some(raw) => {
                let bg: crate::overlay::BackgroundStyle =
                    serde_json::from_value(raw.clone()).map_err(|e| {
                        ErrorData::invalid_params(format!("invalid background: {e}"), None)
                    })?;
                Some(bg)
            }
            None => None,
        };

        // Parse position string to Position enum
        let position = match &params.position {
            Some(s) => {
                let pos: crate::panel::Position = match s.as_str() {
                    "top" => crate::panel::Position::Top,
                    "bottom" => crate::panel::Position::Bottom,
                    other => {
                        return Err(ErrorData::invalid_params(
                            format!("invalid position: '{other}', must be 'top' or 'bottom'"),
                            None,
                        ));
                    }
                };
                Some(pos)
            }
            None => None,
        };

        match params.id {
            // UPDATE existing panel
            Some(id) => {
                let found = session.panels.patch(
                    &id,
                    position,
                    params.height,
                    params.z,
                    background,
                    spans,
                );
                if !found {
                    return Err(ErrorData::invalid_params(
                        format!("panel not found: {id}"),
                        None,
                    ));
                }

                crate::panel::reconfigure_layout(
                    &session.panels,
                    &session.terminal_size,
                    &session.pty,
                    &session.parser,
                )
                .await;

                let _ = session
                    .visual_update_tx
                    .send(crate::protocol::VisualUpdate::PanelsChanged);

                let result = serde_json::json!({
                    "status": "updated",
                    "id": id,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }

            // CREATE new panel
            None => {
                let position = position.ok_or_else(|| {
                    ErrorData::invalid_params(
                        "position is required when creating a panel ('top' or 'bottom')",
                        None,
                    )
                })?;
                let height = params.height.ok_or_else(|| {
                    ErrorData::invalid_params("height is required when creating a panel", None)
                })?;

                let id = session.panels.create(
                    position,
                    height,
                    params.z,
                    background,
                    spans.unwrap_or_default(),
                    params.focusable,
                    current_mode,
                );

                crate::panel::reconfigure_layout(
                    &session.panels,
                    &session.terminal_size,
                    &session.pty,
                    &session.parser,
                )
                .await;

                let _ = session
                    .visual_update_tx
                    .send(crate::protocol::VisualUpdate::PanelsChanged);

                let result = serde_json::json!({
                    "status": "created",
                    "id": id,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }
        }
    }

    /// Remove a panel or clear all panels from a terminal session.
    #[tool(description = "Remove a panel by ID, or clear all panels from a terminal session. If id is omitted, all panels are removed. The PTY viewport is resized to reclaim panel space.")]
    async fn wsh_remove_panel(
        &self,
        Parameters(params): Parameters<RemovePanelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let session = self.get_session(&params.session)?;

        match params.id {
            Some(id) => {
                let found = session.panels.delete(&id);
                if !found {
                    return Err(ErrorData::invalid_params(
                        format!("panel not found: {id}"),
                        None,
                    ));
                }
                session.focus.clear_if_focused(&id);

                crate::panel::reconfigure_layout(
                    &session.panels,
                    &session.terminal_size,
                    &session.pty,
                    &session.parser,
                )
                .await;

                let _ = session
                    .visual_update_tx
                    .send(crate::protocol::VisualUpdate::PanelsChanged);

                let result = serde_json::json!({
                    "status": "removed",
                    "id": id,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }
            None => {
                session.panels.clear();
                session.focus.unfocus();

                crate::panel::reconfigure_layout(
                    &session.panels,
                    &session.terminal_size,
                    &session.pty,
                    &session.parser,
                )
                .await;

                let _ = session
                    .visual_update_tx
                    .send(crate::protocol::VisualUpdate::PanelsChanged);

                let result = serde_json::json!({
                    "status": "cleared",
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }
        }
    }

    // ── Input & screen mode tools ────────────────────────────────

    /// Query or change the input mode and focus state of a terminal session.
    #[tool(description = "Query or change the input mode and focus state of a terminal session. Without arguments, returns the current mode and focused element. Set mode to 'capture' (input goes to API only) or 'release' (input goes to both API and PTY). Set focus to an overlay/panel ID (must be focusable), or unfocus=true to clear focus.")]
    async fn wsh_input_mode(
        &self,
        Parameters(params): Parameters<InputModeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let session = self.get_session(&params.session)?;

        // Apply mode change if requested
        if let Some(ref action) = params.mode {
            match action {
                InputModeAction::Capture => {
                    session.input_mode.capture();
                }
                InputModeAction::Release => {
                    session.input_mode.release();
                    session.focus.unfocus();
                }
            }
        }

        // Apply focus change if requested
        if let Some(ref id) = params.focus {
            // Validate that the target is focusable
            let is_focusable = if let Some(overlay) = session.overlays.get(id) {
                overlay.focusable
            } else if let Some(panel) = session.panels.get(id) {
                panel.focusable
            } else {
                return Err(ErrorData::invalid_params(
                    format!("no overlay or panel with id '{id}'"),
                    None,
                ));
            };

            if !is_focusable {
                return Err(ErrorData::invalid_params(
                    format!("target '{id}' is not focusable"),
                    None,
                ));
            }

            session.focus.focus(id.clone());
        }

        // Apply unfocus if requested
        if params.unfocus {
            session.focus.unfocus();
        }

        // Return current state
        let mode = session.input_mode.get();
        let mode_str = match mode {
            crate::input::mode::Mode::Passthrough => "passthrough",
            crate::input::mode::Mode::Capture => "capture",
        };
        let focused_element = session.focus.focused();

        let result = serde_json::json!({
            "mode": mode_str,
            "focused_element": focused_element,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).unwrap_or_default(),
        )]))
    }

    /// Query or change the screen mode of a terminal session.
    #[tool(description = "Query or change the screen mode of a terminal session. Without arguments, returns the current mode ('normal' or 'alt'). Set action to 'enter_alt' to switch to alternate screen mode, or 'exit_alt' to return to normal mode (which cleans up alt-mode overlays and panels).")]
    async fn wsh_screen_mode(
        &self,
        Parameters(params): Parameters<ScreenModeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let session = self.get_session(&params.session)?;

        if let Some(ref action) = params.action {
            match action {
                ScreenModeAction::EnterAlt => {
                    let mut mode = session.screen_mode.write();
                    if *mode == crate::overlay::ScreenMode::Alt {
                        return Err(ErrorData::invalid_params(
                            "session is already in alternate screen mode",
                            None,
                        ));
                    }
                    *mode = crate::overlay::ScreenMode::Alt;
                }
                ScreenModeAction::ExitAlt => {
                    {
                        let mut mode = session.screen_mode.write();
                        if *mode == crate::overlay::ScreenMode::Normal {
                            return Err(ErrorData::invalid_params(
                                "session is not in alternate screen mode",
                                None,
                            ));
                        }
                        *mode = crate::overlay::ScreenMode::Normal;
                    }
                    // Clean up alt-mode overlays and panels
                    session
                        .overlays
                        .delete_by_mode(crate::overlay::ScreenMode::Alt);
                    session
                        .panels
                        .delete_by_mode(crate::overlay::ScreenMode::Alt);
                    session.focus.unfocus();
                    crate::panel::reconfigure_layout(
                        &session.panels,
                        &session.terminal_size,
                        &session.pty,
                        &session.parser,
                    )
                    .await;
                    let _ = session
                        .visual_update_tx
                        .send(crate::protocol::VisualUpdate::OverlaysChanged);
                    let _ = session
                        .visual_update_tx
                        .send(crate::protocol::VisualUpdate::PanelsChanged);
                }
            }
        }

        // Return current state
        let current_mode = *session.screen_mode.read();
        let mode_str = match current_mode {
            crate::overlay::ScreenMode::Normal => "normal",
            crate::overlay::ScreenMode::Alt => "alt",
        };

        let result = serde_json::json!({
            "mode": mode_str,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).unwrap_or_default(),
        )]))
    }
}
