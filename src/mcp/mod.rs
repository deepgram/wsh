pub mod tools;
pub mod resources;
pub mod prompts;

use std::sync::Arc;
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
use crate::federation::registry::{BackendEntry, BackendHealth};
use crate::parser::state::Query;
use crate::pty::SpawnCommand;
use crate::session::{RegistryError, Session};

/// Maximum allowed value for timeout_ms and max_wait_ms parameters.
const MAX_WAIT_CEILING_MS: u64 = 300_000; // 5 minutes

/// Shared connect and request timeouts for proxy requests to remote backends.
const PROXY_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PROXY_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Longer timeout for idle-wait proxied calls (must exceed max_wait_ms ceiling).
const PROXY_IDLE_TIMEOUT: Duration = Duration::from_secs(330);

use tools::{
    CreateSessionParams, ListSessionsParams, ManageSessionParams, ManageAction,
    SendInputParams, Encoding, GetScreenParams, GetScrollbackParams,
    AwaitIdleParams, RunCommandParams,
    OverlayParams, RemoveOverlayParams, PanelParams, RemovePanelParams,
    InputModeParams, InputModeAction, ScreenModeParams, ScreenModeAction,
    ListServersParams, AddServerParams, RemoveServerParams, ServerStatusParams,
};

// ── Federation helpers ─────────────────────────────────────────────

/// Resolved target for an MCP tool operation.
enum McpSessionTarget {
    /// Handle the request on this server.
    Local,
    /// Proxy the request to a remote backend.
    Remote(BackendEntry),
}

/// Build a reqwest client with standard proxy timeouts.
fn build_proxy_client() -> Result<reqwest::Client, ErrorData> {
    reqwest::Client::builder()
        .connect_timeout(PROXY_CONNECT_TIMEOUT)
        .timeout(PROXY_REQUEST_TIMEOUT)
        .build()
        .map_err(|e| ErrorData::internal_error(format!("failed to build HTTP client: {e}"), None))
}

/// Build a reqwest client with extended timeout for idle-wait operations.
fn build_idle_proxy_client() -> Result<reqwest::Client, ErrorData> {
    reqwest::Client::builder()
        .connect_timeout(PROXY_CONNECT_TIMEOUT)
        .timeout(PROXY_IDLE_TIMEOUT)
        .build()
        .map_err(|e| ErrorData::internal_error(format!("failed to build HTTP client: {e}"), None))
}

/// Make a proxied GET request to a remote backend and return the result as MCP content.
async fn proxy_get(
    backend: &BackendEntry,
    path: &str,
) -> Result<CallToolResult, ErrorData> {
    let url = format!("http://{}{}", backend.address, path);
    let client = build_proxy_client()?;

    let mut req = client.get(&url);
    if let Some(ref token) = backend.token {
        req = req.bearer_auth(token);
    }

    let resp = req.send().await.map_err(|e| {
        ErrorData::internal_error(format!("proxy request failed: {e}"), None)
    })?;

    response_to_call_result(resp).await
}

/// Make a proxied GET request with an extended timeout (for idle waits).
async fn proxy_get_long(
    backend: &BackendEntry,
    path: &str,
) -> Result<CallToolResult, ErrorData> {
    let url = format!("http://{}{}", backend.address, path);
    let client = build_idle_proxy_client()?;

    let mut req = client.get(&url);
    if let Some(ref token) = backend.token {
        req = req.bearer_auth(token);
    }

    let resp = req.send().await.map_err(|e| {
        ErrorData::internal_error(format!("proxy request failed: {e}"), None)
    })?;

    response_to_call_result(resp).await
}

/// Make a proxied POST request with JSON body.
async fn proxy_post_json(
    backend: &BackendEntry,
    path: &str,
    body: serde_json::Value,
) -> Result<CallToolResult, ErrorData> {
    let url = format!("http://{}{}", backend.address, path);
    let client = build_proxy_client()?;

    let mut req = client.post(&url).json(&body);
    if let Some(ref token) = backend.token {
        req = req.bearer_auth(token);
    }

    let resp = req.send().await.map_err(|e| {
        ErrorData::internal_error(format!("proxy request failed: {e}"), None)
    })?;

    response_to_call_result(resp).await
}

/// Make a proxied POST request with raw bytes body.
async fn proxy_post_bytes(
    backend: &BackendEntry,
    path: &str,
    body: Bytes,
) -> Result<CallToolResult, ErrorData> {
    let url = format!("http://{}{}", backend.address, path);
    let client = build_proxy_client()?;

    let mut req = client.post(&url).body(body);
    if let Some(ref token) = backend.token {
        req = req.bearer_auth(token);
    }

    let resp = req.send().await.map_err(|e| {
        ErrorData::internal_error(format!("proxy request failed: {e}"), None)
    })?;

    let status = resp.status();
    if status.is_success() {
        // Input endpoint returns 204 No Content — return a simple success.
        let result = serde_json::json!({"status": "sent"});
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).unwrap_or_default(),
        )]))
    } else {
        let text = resp.text().await.unwrap_or_default();
        Err(ErrorData::internal_error(
            format!("remote server returned {}: {}", status, text),
            None,
        ))
    }
}

/// Make a proxied DELETE request.
async fn proxy_delete(
    backend: &BackendEntry,
    path: &str,
) -> Result<CallToolResult, ErrorData> {
    let url = format!("http://{}{}", backend.address, path);
    let client = build_proxy_client()?;

    let mut req = client.delete(&url);
    if let Some(ref token) = backend.token {
        req = req.bearer_auth(token);
    }

    let resp = req.send().await.map_err(|e| {
        ErrorData::internal_error(format!("proxy request failed: {e}"), None)
    })?;

    let status = resp.status();
    if status.is_success() {
        let result = serde_json::json!({"status": "ok"});
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).unwrap_or_default(),
        )]))
    } else {
        let text = resp.text().await.unwrap_or_default();
        Err(ErrorData::internal_error(
            format!("remote server returned {}: {}", status, text),
            None,
        ))
    }
}

/// Make a proxied PATCH request with JSON body.
async fn proxy_patch_json(
    backend: &BackendEntry,
    path: &str,
    body: serde_json::Value,
) -> Result<CallToolResult, ErrorData> {
    let url = format!("http://{}{}", backend.address, path);
    let client = build_proxy_client()?;

    let mut req = client.patch(&url).json(&body);
    if let Some(ref token) = backend.token {
        req = req.bearer_auth(token);
    }

    let resp = req.send().await.map_err(|e| {
        ErrorData::internal_error(format!("proxy request failed: {e}"), None)
    })?;

    response_to_call_result(resp).await
}

/// Convert an HTTP response into a CallToolResult.
/// Success (2xx) returns the JSON body as MCP content.
/// Error (4xx/5xx) returns an ErrorData with the response body.
async fn response_to_call_result(resp: reqwest::Response) -> Result<CallToolResult, ErrorData> {
    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();

    if status.is_success() {
        // Try to parse as JSON; if it fails, return the raw text.
        Ok(CallToolResult::success(vec![Content::text(body_text)]))
    } else {
        Err(ErrorData::internal_error(
            format!("remote server returned {}: {}", status, body_text),
            None,
        ))
    }
}

// ── MCP server ─────────────────────────────────────────────────────

#[derive(Clone)]
pub struct WshMcpServer {
    state: AppState,
    tool_router: ToolRouter<WshMcpServer>,
    /// Shared counter for active MCP sessions. Decremented on Drop.
    session_counter: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

impl WshMcpServer {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            session_counter: None,
        }
    }

    /// Attach a shared session counter that is decremented when this server is dropped.
    pub fn with_session_counter(mut self, counter: Arc<std::sync::atomic::AtomicUsize>) -> Self {
        self.session_counter = Some(counter);
        self
    }

    fn get_session(&self, name: &str) -> Result<crate::session::Session, ErrorData> {
        self.state
            .sessions
            .get(name)
            .ok_or_else(|| ErrorData::invalid_params(format!("session not found: {name}"), None))
    }

    /// Resolve whether a request targets the local server or a remote backend.
    ///
    /// - `None` always means local.
    /// - Matching the local hostname means local.
    /// - Otherwise, looks up the hostname in the backend registry.
    fn resolve_server(&self, server: Option<&str>) -> Result<McpSessionTarget, ErrorData> {
        match server {
            None => Ok(McpSessionTarget::Local),
            Some(s) if s == self.state.hostname => Ok(McpSessionTarget::Local),
            Some(s) => {
                let backend = self.state.backends.get_by_hostname(s).ok_or_else(|| {
                    ErrorData::invalid_params(format!("server not found: {s}"), None)
                })?;
                if backend.health != BackendHealth::Healthy {
                    return Err(ErrorData::internal_error(
                        format!("server unavailable: {s}"),
                        None,
                    ));
                }
                Ok(McpSessionTarget::Remote(backend))
            }
        }
    }
}

impl Drop for WshMcpServer {
    fn drop(&mut self) {
        if let Some(ref counter) = self.session_counter {
            counter.fetch_sub(1, std::sync::atomic::Ordering::Release);
        }
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
                 Visual feedback via wsh_overlay and wsh_panel. Input capture via wsh_input_mode. \
                 Federation: use the 'server' parameter to target remote servers, or \
                 wsh_list_servers / wsh_add_server / wsh_remove_server to manage backends."
                    .to_string(),
            ),
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        resources::list_resources(&self.state).await
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        resources::list_resource_templates().await
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        resources::read_resource(&self.state, request).await
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        prompts::list_prompts().await
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, ErrorData> {
        prompts::get_prompt(&request.name).await
    }
}

#[tool_router]
impl WshMcpServer {
    /// Create a new terminal session with an interactive shell or a specific command.
    #[tool(description = "Create a new terminal session. Spawns an interactive shell by default, or runs a specific command. Returns the assigned session name and terminal dimensions. Use 'server' to target a remote federated server.")]
    async fn wsh_create_session(
        &self,
        Parameters(params): Parameters<CreateSessionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Federation: proxy to remote if server is specified.
        if let McpSessionTarget::Remote(backend) = self.resolve_server(params.server.as_deref())? {
            let mut body = serde_json::json!({});
            if let Some(name) = &params.name { body["name"] = serde_json::json!(name); }
            if let Some(cmd) = &params.command { body["command"] = serde_json::json!(cmd); }
            if let Some(rows) = params.rows { body["rows"] = serde_json::json!(rows); }
            if let Some(cols) = params.cols { body["cols"] = serde_json::json!(cols); }
            if let Some(cwd) = &params.cwd { body["cwd"] = serde_json::json!(cwd); }
            if let Some(env) = &params.env { body["env"] = serde_json::json!(env); }
            if !params.tags.is_empty() { body["tags"] = serde_json::json!(params.tags); }
            return proxy_post_json(&backend, "/sessions", body).await;
        }

        let param_name = params.name;
        let tags = params.tags;
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

        let rows = params.rows.unwrap_or(24).max(1);
        let cols = params.cols.unwrap_or(80).max(1);

        // Advisory pre-check -- see name_available() doc for TOCTOU rationale.
        // The authoritative check is insert_and_get() below.
        self.state.sessions.name_available(&param_name).map_err(|e| match e {
            RegistryError::NameExists(n) => ErrorData::invalid_params(
                format!("session name already exists: {n}"),
                None,
            ),
            RegistryError::NotFound(n) => ErrorData::internal_error(
                format!("unexpected registry error: {n}"),
                None,
            ),
            RegistryError::MaxSessionsReached => ErrorData::internal_error(
                "maximum number of sessions reached".to_string(),
                None,
            ),
            RegistryError::InvalidTag(msg) => ErrorData::invalid_params(
                format!("invalid tag: {msg}"),
                None,
            ),
            RegistryError::InvalidName(msg) => ErrorData::invalid_params(
                format!("invalid session name: {msg}"),
                None,
            ),
        })?;

        // spawn_with_options calls fork()/exec() -- run on blocking pool.
        let cwd = params.cwd;
        let env = params.env;
        let (session, child_exit_rx) =
            tokio::task::spawn_blocking(move || {
                Session::spawn_with_options("".to_string(), command, rows, cols, cwd, env)
            })
            .await
            .map_err(|e| ErrorData::internal_error(format!("spawn task failed: {e}"), None))?
            .map_err(|e| {
                ErrorData::internal_error(
                    format!("failed to spawn session: {e}"),
                    None,
                )
            })?;

        // Validate and set initial tags before registry insertion
        if !tags.is_empty() {
            for tag in &tags {
                crate::session::validate_tag(tag).map_err(|e| {
                    ErrorData::invalid_params(format!("invalid tag: {e}"), None)
                })?;
            }
            *session.tags.write() = tags.into_iter().collect();
        }

        let (assigned_name, session) =
            match self.state.sessions.insert_and_get(param_name, session.clone()) {
                Ok(result) => result,
                Err(e) => {
                    session.shutdown();
                    return Err(match e {
                        RegistryError::NameExists(n) => ErrorData::invalid_params(
                            format!("session name already exists: {n}"),
                            None,
                        ),
                        RegistryError::NotFound(n) => ErrorData::internal_error(
                            format!("unexpected registry error: {n}"),
                            None,
                        ),
                        RegistryError::MaxSessionsReached => ErrorData::internal_error(
                            "maximum number of sessions reached".to_string(),
                            None,
                        ),
                        RegistryError::InvalidTag(msg) => ErrorData::invalid_params(
                            format!("invalid tag: {msg}"),
                            None,
                        ),
                        RegistryError::InvalidName(msg) => ErrorData::invalid_params(
                            format!("invalid session name: {msg}"),
                            None,
                        ),
                    });
                }
            };

        // Monitor child exit so the session is auto-removed when the process dies.
        self.state
            .sessions
            .monitor_child_exit(assigned_name.clone(), session.client_count.clone(), session.child_exited.clone(), child_exit_rx);

        let mut result_tags: Vec<String> = session.tags.read().iter().cloned().collect();
        result_tags.sort();
        let result = serde_json::json!({
            "name": assigned_name,
            "pid": session.pid,
            "rows": rows,
            "cols": cols,
            "tags": result_tags,
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).unwrap_or_default(),
        )]))
    }

    /// List all sessions or get details for a specific session.
    #[tool(description = "List all terminal sessions, or get details for a specific session by name. Returns session names and terminal dimensions. Use 'server' to target a remote federated server.")]
    async fn wsh_list_sessions(
        &self,
        Parameters(params): Parameters<ListSessionsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Federation: proxy to remote if server is specified.
        if let McpSessionTarget::Remote(backend) = self.resolve_server(params.server.as_deref())? {
            let mut path = "/sessions".to_string();
            if let Some(ref name) = params.session {
                // Single session detail
                path = format!("/sessions/{}", name);
            } else if !params.tag.is_empty() {
                path = format!("/sessions?tag={}", params.tag.join(","));
            }
            return proxy_get(&backend, &path).await;
        }

        if let Some(name) = params.session {
            // Single session detail
            let session = self.get_session(&name)?;
            let (rows, cols) = session.terminal_size.get();
            let mut tags: Vec<String> = session.tags.read().iter().cloned().collect();
            tags.sort();
            let result = serde_json::json!({
                "name": session.name,
                "pid": session.pid,
                "command": session.command,
                "rows": rows,
                "cols": cols,
                "clients": session.clients(),
                "tags": tags,
            });
            Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&result).unwrap_or_default(),
            )]))
        } else {
            // All sessions (optionally filtered by tags)
            let names = if params.tag.is_empty() {
                self.state.sessions.list()
            } else {
                self.state.sessions.sessions_by_tags(&params.tag)
            };
            let sessions: Vec<serde_json::Value> = names
                .into_iter()
                .filter_map(|name| {
                    let session = self.state.sessions.get(&name)?;
                    let (rows, cols) = session.terminal_size.get();
                    let mut tags: Vec<String> = session.tags.read().iter().cloned().collect();
                    tags.sort();
                    Some(serde_json::json!({
                        "name": name,
                        "pid": session.pid,
                        "command": session.command.clone(),
                        "rows": rows,
                        "cols": cols,
                        "clients": session.clients(),
                        "tags": tags,
                    }))
                })
                .collect();

            let result = serde_json::json!(sessions);
            Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&result).unwrap_or_default(),
            )]))
        }
    }

    /// Manage an existing session: kill, rename, detach, add_tags, or remove_tags.
    #[tool(description = "Manage a terminal session. Actions: 'kill' destroys the session, 'rename' changes its name (requires new_name), 'detach' disconnects all streaming clients, 'add_tags' adds tags (requires tags), 'remove_tags' removes tags (requires tags). Use 'server' to target a remote federated server.")]
    async fn wsh_manage_session(
        &self,
        Parameters(params): Parameters<ManageSessionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Federation: proxy to remote if server is specified.
        if let McpSessionTarget::Remote(backend) = self.resolve_server(params.server.as_deref())? {
            return match params.action {
                ManageAction::Kill => {
                    proxy_delete(&backend, &format!("/sessions/{}", params.session)).await
                }
                ManageAction::Rename => {
                    let new_name = params.new_name.ok_or_else(|| {
                        ErrorData::invalid_params("new_name is required for rename action", None)
                    })?;
                    proxy_patch_json(
                        &backend,
                        &format!("/sessions/{}", params.session),
                        serde_json::json!({"name": new_name}),
                    ).await
                }
                ManageAction::Detach => {
                    proxy_post_json(
                        &backend,
                        &format!("/sessions/{}/detach", params.session),
                        serde_json::json!({}),
                    ).await
                }
                ManageAction::AddTags => {
                    if params.tags.is_empty() {
                        return Err(ErrorData::invalid_params(
                            "tags field is required for add_tags action".to_string(),
                            None,
                        ));
                    }
                    proxy_patch_json(
                        &backend,
                        &format!("/sessions/{}", params.session),
                        serde_json::json!({"add_tags": params.tags}),
                    ).await
                }
                ManageAction::RemoveTags => {
                    if params.tags.is_empty() {
                        return Err(ErrorData::invalid_params(
                            "tags field is required for remove_tags action".to_string(),
                            None,
                        ));
                    }
                    proxy_patch_json(
                        &backend,
                        &format!("/sessions/{}", params.session),
                        serde_json::json!({"remove_tags": params.tags}),
                    ).await
                }
            };
        }

        match params.action {
            ManageAction::Kill => {
                let session = self.state
                    .sessions
                    .remove(&params.session)
                    .ok_or_else(|| {
                        ErrorData::invalid_params(
                            format!("session not found: {}", params.session),
                            None,
                        )
                    })?;
                session.force_kill();

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
                        RegistryError::MaxSessionsReached => ErrorData::internal_error(
                            "maximum number of sessions reached".to_string(),
                            None,
                        ),
                        RegistryError::InvalidTag(msg) => ErrorData::invalid_params(
                            format!("invalid tag: {msg}"),
                            None,
                        ),
                        RegistryError::InvalidName(msg) => ErrorData::invalid_params(
                            format!("invalid session name: {msg}"),
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

            ManageAction::AddTags => {
                if params.tags.is_empty() {
                    return Err(ErrorData::invalid_params(
                        "tags field is required for add_tags action".to_string(),
                        None,
                    ));
                }
                self.state
                    .sessions
                    .add_tags(&params.session, &params.tags)
                    .map_err(|e| match e {
                        RegistryError::NotFound(n) => ErrorData::invalid_params(
                            format!("session not found: {n}"),
                            None,
                        ),
                        RegistryError::InvalidTag(e) => ErrorData::invalid_params(
                            format!("invalid tag: {e}"),
                            None,
                        ),
                        _ => ErrorData::internal_error(format!("{e}"), None),
                    })?;
                let session = self.get_session(&params.session)?;
                let mut tags: Vec<String> = session.tags.read().iter().cloned().collect();
                tags.sort();
                let result = serde_json::json!({
                    "status": "tags_added",
                    "session": params.session,
                    "tags": tags,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }

            ManageAction::RemoveTags => {
                if params.tags.is_empty() {
                    return Err(ErrorData::invalid_params(
                        "tags field is required for remove_tags action".to_string(),
                        None,
                    ));
                }
                self.state
                    .sessions
                    .remove_tags(&params.session, &params.tags)
                    .map_err(|e| match e {
                        RegistryError::NotFound(n) => ErrorData::invalid_params(
                            format!("session not found: {n}"),
                            None,
                        ),
                        _ => ErrorData::internal_error(format!("{e}"), None),
                    })?;
                let session = self.get_session(&params.session)?;
                let mut tags: Vec<String> = session.tags.read().iter().cloned().collect();
                tags.sort();
                let result = serde_json::json!({
                    "status": "tags_removed",
                    "session": params.session,
                    "tags": tags,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }
        }
    }

    // ── Terminal I/O tools ───────────────────────────────────────

    /// Send input (keystrokes, text, or binary data) to a terminal session.
    #[tool(description = "Send input to a terminal session. Supports UTF-8 text (default) or base64-encoded binary data. The input is delivered to the PTY exactly as provided -- no newline is appended automatically. Use 'server' to target a remote federated server.")]
    async fn wsh_send_input(
        &self,
        Parameters(params): Parameters<SendInputParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Federation: proxy to remote if server is specified.
        if let McpSessionTarget::Remote(backend) = self.resolve_server(params.server.as_deref())? {
            // Decode the input to raw bytes for the remote HTTP endpoint.
            let data = match params.encoding {
                Encoding::Utf8 => Bytes::from(params.input.into_bytes()),
                Encoding::Base64 => {
                    use base64::Engine;
                    let decoded = base64::engine::general_purpose::STANDARD
                        .decode(&params.input)
                        .map_err(|e| {
                            ErrorData::invalid_params(format!("invalid base64 input: {e}"), None)
                        })?;
                    Bytes::from(decoded)
                }
            };
            return proxy_post_bytes(
                &backend,
                &format!("/sessions/{}/input", params.session),
                data,
            ).await;
        }

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
        tokio::time::timeout(
            Duration::from_secs(5),
            session.input_tx.send(data),
        )
        .await
        .map_err(|_| ErrorData::internal_error("input send timed out", None))?
        .map_err(|e| {
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
    #[tool(description = "Get the current visible screen contents of a terminal session. Returns the screen grid with text, colors, cursor position, and terminal dimensions. Use 'server' to target a remote federated server.")]
    async fn wsh_get_screen(
        &self,
        Parameters(params): Parameters<GetScreenParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Federation: proxy to remote if server is specified.
        if let McpSessionTarget::Remote(backend) = self.resolve_server(params.server.as_deref())? {
            let mut path = format!("/sessions/{}/screen", params.session);
            if matches!(params.format, tools::ScreenFormat::Plain) {
                path.push_str("?format=plain");
            }
            return proxy_get(&backend, &path).await;
        }

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
    #[tool(description = "Get scrollback buffer contents from a terminal session. Returns historical output with pagination support (offset and limit). Useful for reading output that has scrolled off the visible screen. Use 'server' to target a remote federated server.")]
    async fn wsh_get_scrollback(
        &self,
        Parameters(params): Parameters<GetScrollbackParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Federation: proxy to remote if server is specified.
        if let McpSessionTarget::Remote(backend) = self.resolve_server(params.server.as_deref())? {
            let mut path = format!(
                "/sessions/{}/scrollback?offset={}&limit={}",
                params.session, params.offset, params.limit,
            );
            if matches!(params.format, tools::ScreenFormat::Plain) {
                path.push_str("&format=plain");
            }
            return proxy_get(&backend, &path).await;
        }

        let session = self.get_session(&params.session)?;
        let format = params.format.into_parser_format();
        let limit = params.limit.min(10_000);

        let response = session
            .parser
            .query(Query::Scrollback {
                format,
                offset: params.offset,
                limit,
            })
            .await
            .map_err(|e| {
                ErrorData::internal_error(format!("parser error: {e}"), None)
            })?;

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&response).unwrap_or_default(),
        )]))
    }

    /// Wait for a terminal session to become idle.
    #[tool(description = "Wait for a terminal session to become idle (no output for timeout_ms). Returns the activity generation number on success. Returns an error result if max_wait_ms is exceeded before idle is reached. Use 'server' to target a remote federated server.")]
    async fn wsh_await_idle(
        &self,
        Parameters(params): Parameters<AwaitIdleParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Federation: proxy to remote if server is specified.
        if let McpSessionTarget::Remote(backend) = self.resolve_server(params.server.as_deref())? {
            let path = format!(
                "/sessions/{}/idle?timeout_ms={}&max_wait_ms={}",
                params.session, params.timeout_ms, params.max_wait_ms,
            );
            return proxy_get_long(&backend, &path).await;
        }

        let session = self.get_session(&params.session)?;

        let timeout = Duration::from_millis(params.timeout_ms.min(MAX_WAIT_CEILING_MS));
        let max_wait = Duration::from_millis(params.max_wait_ms.min(MAX_WAIT_CEILING_MS));

        match tokio::time::timeout(
            max_wait,
            session.activity.wait_for_idle(timeout, None),
        )
        .await
        {
            Ok(generation) => {
                let result = serde_json::json!({
                    "status": "idle",
                    "generation": generation,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&result).unwrap_or_default(),
                )]))
            }
            Err(_) => {
                let result = serde_json::json!({
                    "error": "idle timeout exceeded max_wait_ms",
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
    #[tool(description = "Send input to a terminal session, wait for idle, then return the screen contents. This is the primary 'run a command' primitive: send input, wait for output to settle, read the result. If idle is not reached within max_wait_ms, the screen is still returned but marked as an error. Use 'server' to target a remote federated server.")]
    async fn wsh_run_command(
        &self,
        Parameters(params): Parameters<RunCommandParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Federation: proxy to remote if server is specified.
        // For run_command on a remote, we execute the three steps (send input,
        // await idle, get screen) as separate proxied HTTP calls so the remote
        // server's activity tracker handles the timing.
        if let McpSessionTarget::Remote(backend) = self.resolve_server(params.server.as_deref())? {
            // 1. Send input
            let input_bytes = Bytes::from(params.input.into_bytes());
            proxy_post_bytes(
                &backend,
                &format!("/sessions/{}/input", params.session),
                input_bytes,
            ).await?;

            // 2. Await idle (use the extended-timeout client)
            let idle_path = format!(
                "/sessions/{}/idle?timeout_ms={}&max_wait_ms={}",
                params.session, params.timeout_ms, params.max_wait_ms,
            );
            let idle_result = proxy_get_long(&backend, &idle_path).await;

            // 3. Get screen regardless of idle outcome
            let mut screen_path = format!("/sessions/{}/screen", params.session);
            if matches!(params.format, tools::ScreenFormat::Plain) {
                screen_path.push_str("?format=plain");
            }
            let screen_result = proxy_get(&backend, &screen_path).await?;

            // Combine results
            match idle_result {
                Ok(_) => Ok(screen_result),
                Err(_) => {
                    // Return screen with error flag
                    let screen_text = screen_result
                        .content
                        .first()
                        .and_then(|c| c.as_text())
                        .map(|t| t.text.clone())
                        .unwrap_or_default();
                    let result = serde_json::json!({
                        "error": "idle timeout exceeded max_wait_ms",
                        "screen": serde_json::from_str::<serde_json::Value>(&screen_text).unwrap_or(serde_json::Value::String(screen_text)),
                    });
                    Ok(CallToolResult::error(vec![Content::text(
                        serde_json::to_string(&result).unwrap_or_default(),
                    )]))
                }
            }
        } else {
            // Local execution
            let session = self.get_session(&params.session)?;

            // 1. Send input
            let data = Bytes::from(params.input.into_bytes());
            tokio::time::timeout(
                Duration::from_secs(5),
                session.input_tx.send(data),
            )
            .await
            .map_err(|_| ErrorData::internal_error("input send timed out", None))?
            .map_err(|e| {
                ErrorData::internal_error(
                    format!("failed to send input: {e}"),
                    None,
                )
            })?;
            // Note: no manual activity.touch() here. The PTY reader calls touch()
            // when output arrives (including the echo of our input). Adding a manual
            // touch would gratuitously reset the idle timer, forcing agents to
            // wait the full timeout_ms even for silent commands.

            // 2. Await idle
            let timeout = Duration::from_millis(params.timeout_ms.min(MAX_WAIT_CEILING_MS));
            let max_wait = Duration::from_millis(params.max_wait_ms.min(MAX_WAIT_CEILING_MS));

            let idle_result = tokio::time::timeout(
                max_wait,
                session.activity.wait_for_idle(timeout, None),
            )
            .await;

            // 3. Get screen regardless of idle outcome
            let format = params.format.into_parser_format();
            let screen = session
                .parser
                .query(Query::Screen { format })
                .await
                .map_err(|e| {
                    ErrorData::internal_error(format!("parser error: {e}"), None)
                })?;

            match idle_result {
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
                        "error": "idle timeout exceeded max_wait_ms",
                        "screen": screen,
                    });
                    Ok(CallToolResult::error(vec![Content::text(
                        serde_json::to_string(&result).unwrap_or_default(),
                    )]))
                }
            }
        }
    }

    // ── Visual feedback tools ────────────────────────────────────

    /// Create, update, or list overlays on a terminal session.
    #[tool(description = "Create, update, or list overlays on a terminal session. Overlays are styled text boxes rendered on top of terminal content. Modes: set list=true to list all overlays; omit id to create a new overlay (x, y, width, height required); provide id to update an existing overlay. Use 'server' to target a remote federated server.")]
    async fn wsh_overlay(
        &self,
        Parameters(params): Parameters<OverlayParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Federation: proxy to remote if server is specified.
        if let McpSessionTarget::Remote(backend) = self.resolve_server(params.server.as_deref())? {
            if params.list {
                return proxy_get(&backend, &format!("/sessions/{}/overlay", params.session)).await;
            }
            match params.id {
                Some(ref id) => {
                    // Update existing overlay
                    let mut body = serde_json::json!({});
                    if let Some(x) = params.x { body["x"] = serde_json::json!(x); }
                    if let Some(y) = params.y { body["y"] = serde_json::json!(y); }
                    if let Some(z) = params.z { body["z"] = serde_json::json!(z); }
                    if let Some(w) = params.width { body["width"] = serde_json::json!(w); }
                    if let Some(h) = params.height { body["height"] = serde_json::json!(h); }
                    if let Some(bg) = &params.background { body["background"] = bg.clone(); }
                    if let Some(sp) = &params.spans { body["spans"] = serde_json::json!(sp); }
                    return proxy_patch_json(
                        &backend,
                        &format!("/sessions/{}/overlay/{}", params.session, id),
                        body,
                    ).await;
                }
                None => {
                    // Create new overlay
                    let mut body = serde_json::json!({});
                    if let Some(x) = params.x { body["x"] = serde_json::json!(x); }
                    if let Some(y) = params.y { body["y"] = serde_json::json!(y); }
                    if let Some(z) = params.z { body["z"] = serde_json::json!(z); }
                    if let Some(w) = params.width { body["width"] = serde_json::json!(w); }
                    if let Some(h) = params.height { body["height"] = serde_json::json!(h); }
                    if let Some(bg) = &params.background { body["background"] = bg.clone(); }
                    if let Some(sp) = &params.spans { body["spans"] = serde_json::json!(sp); }
                    if params.focusable { body["focusable"] = serde_json::json!(true); }
                    return proxy_post_json(
                        &backend,
                        &format!("/sessions/{}/overlay", params.session),
                        body,
                    ).await;
                }
            }
        }

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
                // Atomically patch position/size/spans under a single lock
                // to prevent race conditions where another client could
                // delete the overlay between separate move_to and update calls.
                match session.overlays.patch(
                    &id,
                    params.x,
                    params.y,
                    params.z,
                    params.width,
                    params.height,
                    background,
                    spans,
                ) {
                    Err(e) => return Err(ErrorData::invalid_params(e.to_string(), None)),
                    Ok(false) => return Err(ErrorData::invalid_params(format!("overlay not found: {id}"), None)),
                    Ok(true) => {}
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
                ).map_err(|e| ErrorData::invalid_params(e, None))?;

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
    #[tool(description = "Remove an overlay by ID, or clear all overlays from a terminal session. If id is omitted, all overlays are removed. Use 'server' to target a remote federated server.")]
    async fn wsh_remove_overlay(
        &self,
        Parameters(params): Parameters<RemoveOverlayParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Federation: proxy to remote if server is specified.
        if let McpSessionTarget::Remote(backend) = self.resolve_server(params.server.as_deref())? {
            return match params.id {
                Some(id) => proxy_delete(
                    &backend,
                    &format!("/sessions/{}/overlay/{}", params.session, id),
                ).await,
                None => proxy_delete(
                    &backend,
                    &format!("/sessions/{}/overlay", params.session),
                ).await,
            };
        }

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
    #[tool(description = "Create, update, or list panels on a terminal session. Panels carve out dedicated rows at the top or bottom of the terminal, shrinking the PTY viewport. Modes: set list=true to list all panels; omit id to create (position and height required); provide id to update. Use 'server' to target a remote federated server.")]
    async fn wsh_panel(
        &self,
        Parameters(params): Parameters<PanelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Federation: proxy to remote if server is specified.
        if let McpSessionTarget::Remote(backend) = self.resolve_server(params.server.as_deref())? {
            if params.list {
                return proxy_get(&backend, &format!("/sessions/{}/panel", params.session)).await;
            }
            match params.id {
                Some(ref id) => {
                    let mut body = serde_json::json!({});
                    if let Some(pos) = &params.position { body["position"] = serde_json::json!(pos); }
                    if let Some(h) = params.height { body["height"] = serde_json::json!(h); }
                    if let Some(z) = params.z { body["z"] = serde_json::json!(z); }
                    if let Some(bg) = &params.background { body["background"] = bg.clone(); }
                    if let Some(sp) = &params.spans { body["spans"] = serde_json::json!(sp); }
                    return proxy_patch_json(
                        &backend,
                        &format!("/sessions/{}/panel/{}", params.session, id),
                        body,
                    ).await;
                }
                None => {
                    let mut body = serde_json::json!({});
                    if let Some(pos) = &params.position { body["position"] = serde_json::json!(pos); }
                    if let Some(h) = params.height { body["height"] = serde_json::json!(h); }
                    if let Some(z) = params.z { body["z"] = serde_json::json!(z); }
                    if let Some(bg) = &params.background { body["background"] = bg.clone(); }
                    if let Some(sp) = &params.spans { body["spans"] = serde_json::json!(sp); }
                    if params.focusable { body["focusable"] = serde_json::json!(true); }
                    return proxy_post_json(
                        &backend,
                        &format!("/sessions/{}/panel", params.session),
                        body,
                    ).await;
                }
            }
        }

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
                match session.panels.patch(
                    &id,
                    position,
                    params.height,
                    params.z,
                    background,
                    spans,
                ) {
                    Err(e) => return Err(ErrorData::invalid_params(e.to_string(), None)),
                    Ok(false) => return Err(ErrorData::invalid_params(format!("panel not found: {id}"), None)),
                    Ok(true) => {}
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
                ).map_err(|e| ErrorData::invalid_params(e, None))?;

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
    #[tool(description = "Remove a panel by ID, or clear all panels from a terminal session. If id is omitted, all panels are removed. The PTY viewport is resized to reclaim panel space. Use 'server' to target a remote federated server.")]
    async fn wsh_remove_panel(
        &self,
        Parameters(params): Parameters<RemovePanelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Federation: proxy to remote if server is specified.
        if let McpSessionTarget::Remote(backend) = self.resolve_server(params.server.as_deref())? {
            return match params.id {
                Some(id) => proxy_delete(
                    &backend,
                    &format!("/sessions/{}/panel/{}", params.session, id),
                ).await,
                None => proxy_delete(
                    &backend,
                    &format!("/sessions/{}/panel", params.session),
                ).await,
            };
        }

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
    #[tool(description = "Query or change the input mode and focus state of a terminal session. Without arguments, returns the current mode and focused element. Set mode to 'capture' (input goes to API only) or 'release' (input goes to both API and PTY). Set focus to an overlay/panel ID (must be focusable), or unfocus=true to clear focus. Use 'server' to target a remote federated server.")]
    async fn wsh_input_mode(
        &self,
        Parameters(params): Parameters<InputModeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Federation: proxy to remote if server is specified.
        // Reject conflicting focus+unfocus before any side-effects (local or remote).
        if params.focus.is_some() && params.unfocus {
            return Err(ErrorData::invalid_params(
                "cannot set both 'focus' and 'unfocus'",
                None,
            ));
        }

        if let McpSessionTarget::Remote(backend) = self.resolve_server(params.server.as_deref())? {
            // Query-only: just GET
            if params.mode.is_none() && params.focus.is_none() && !params.unfocus {
                return proxy_get(&backend, &format!("/sessions/{}/input/mode", params.session)).await;
            }
            // Apply mode change
            if let Some(ref action) = params.mode {
                match action {
                    InputModeAction::Capture => {
                        proxy_post_json(
                            &backend,
                            &format!("/sessions/{}/input/capture", params.session),
                            serde_json::json!({}),
                        ).await?;
                    }
                    InputModeAction::Release => {
                        proxy_post_json(
                            &backend,
                            &format!("/sessions/{}/input/release", params.session),
                            serde_json::json!({}),
                        ).await?;
                    }
                }
            }
            // Apply focus
            if let Some(ref id) = params.focus {
                proxy_post_json(
                    &backend,
                    &format!("/sessions/{}/input/focus", params.session),
                    serde_json::json!({"id": id}),
                ).await?;
            }
            // Apply unfocus
            if params.unfocus {
                proxy_post_json(
                    &backend,
                    &format!("/sessions/{}/input/unfocus", params.session),
                    serde_json::json!({}),
                ).await?;
            }
            // Return current state
            return proxy_get(&backend, &format!("/sessions/{}/input/mode", params.session)).await;
        }

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
    #[tool(description = "Query or change the screen mode of a terminal session. Without arguments, returns the current mode ('normal' or 'alt'). Set action to 'enter_alt' to switch to alternate screen mode, or 'exit_alt' to return to normal mode (which cleans up alt-mode overlays and panels). Use 'server' to target a remote federated server.")]
    async fn wsh_screen_mode(
        &self,
        Parameters(params): Parameters<ScreenModeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Federation: proxy to remote if server is specified.
        if let McpSessionTarget::Remote(backend) = self.resolve_server(params.server.as_deref())? {
            if let Some(ref action) = params.action {
                match action {
                    ScreenModeAction::EnterAlt => {
                        proxy_post_json(
                            &backend,
                            &format!("/sessions/{}/screen_mode/enter_alt", params.session),
                            serde_json::json!({}),
                        ).await?;
                    }
                    ScreenModeAction::ExitAlt => {
                        proxy_post_json(
                            &backend,
                            &format!("/sessions/{}/screen_mode/exit_alt", params.session),
                            serde_json::json!({}),
                        ).await?;
                    }
                }
            }
            return proxy_get(&backend, &format!("/sessions/{}/screen_mode", params.session)).await;
        }

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

    // ── Federation server management tools ────────────────────────

    /// List all registered federated backend servers.
    #[tool(description = "List all registered federated backend servers with their hostname, address, health status, and role.")]
    async fn wsh_list_servers(
        &self,
        #[allow(unused_variables)]
        Parameters(params): Parameters<ListServersParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let backends = self.state.backends.list();
        let servers: Vec<serde_json::Value> = backends
            .into_iter()
            .map(|b| {
                serde_json::json!({
                    "address": b.address,
                    "hostname": b.hostname,
                    "health": b.health,
                    "role": b.role,
                })
            })
            .collect();

        let result = serde_json::json!({
            "local_hostname": self.state.hostname,
            "servers": servers,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).unwrap_or_default(),
        )]))
    }

    /// Add a new backend server to the federation.
    #[tool(description = "Add a new backend server to the federation. Provide the address (host:port) and optionally a token. The server will be probed for health and hostname.")]
    async fn wsh_add_server(
        &self,
        Parameters(params): Parameters<AddServerParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut federation = self.state.federation.lock().await;
        federation
            .add_backend(&params.address, params.token.as_deref())
            .map_err(|e| ErrorData::invalid_params(format!("{e}"), None))?;

        let result = serde_json::json!({
            "status": "added",
            "address": params.address,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).unwrap_or_default(),
        )]))
    }

    /// Remove a backend server from the federation.
    #[tool(description = "Remove a backend server from the federation by hostname. Its connection will be shut down.")]
    async fn wsh_remove_server(
        &self,
        Parameters(params): Parameters<RemoveServerParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut federation = self.state.federation.lock().await;
        let removed = federation.remove_backend_by_hostname(&params.hostname);

        if !removed {
            return Err(ErrorData::invalid_params(
                format!("server not found: {}", params.hostname),
                None,
            ));
        }

        let result = serde_json::json!({
            "status": "removed",
            "hostname": params.hostname,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).unwrap_or_default(),
        )]))
    }

    /// Get detailed status for a specific federated backend server.
    #[tool(description = "Get detailed status for a specific federated backend server by hostname. Returns address, health, role, and hostname.")]
    async fn wsh_server_status(
        &self,
        Parameters(params): Parameters<ServerStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let entry = self.state.backends.get_by_hostname(&params.hostname).ok_or_else(|| {
            ErrorData::invalid_params(format!("server not found: {}", params.hostname), None)
        })?;

        let result = serde_json::json!({
            "address": entry.address,
            "hostname": entry.hostname,
            "health": entry.health,
            "role": entry.role,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).unwrap_or_default(),
        )]))
    }
}
