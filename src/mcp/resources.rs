// MCP resource handlers
//
// Exposes terminal sessions as MCP resources:
// - wsh://sessions              -> list all sessions with dimensions
// - wsh://sessions/{name}/screen    -> current visible screen contents
// - wsh://sessions/{name}/scrollback -> scrollback buffer contents

use rmcp::model::*;

use crate::api::AppState;
use crate::parser::state::{Format, Query};

/// The URI prefix for all wsh resources.
const URI_PREFIX: &str = "wsh://sessions";

/// List all available resources.
///
/// Returns the fixed `wsh://sessions` resource plus dynamic per-session
/// resources (screen and scrollback for each active session).
pub async fn list_resources(state: &AppState) -> Result<ListResourcesResult, ErrorData> {
    let mut resources = Vec::new();

    // Fixed resource: session listing
    resources.push(
        RawResource::new(URI_PREFIX, "sessions")
            .no_annotation(),
    );

    // Dynamic per-session resources
    for name in state.sessions.list() {
        resources.push(
            RawResource::new(
                format!("{URI_PREFIX}/{name}/screen"),
                format!("{name}/screen"),
            )
            .no_annotation(),
        );
        resources.push(
            RawResource::new(
                format!("{URI_PREFIX}/{name}/scrollback"),
                format!("{name}/scrollback"),
            )
            .no_annotation(),
        );
    }

    Ok(ListResourcesResult {
        meta: None,
        next_cursor: None,
        resources,
    })
}

/// List resource templates.
///
/// Returns two URI templates that clients can use to construct resource URIs
/// for any session by name.
pub async fn list_resource_templates() -> Result<ListResourceTemplatesResult, ErrorData> {
    let templates = vec![
        RawResourceTemplate {
            uri_template: format!("{URI_PREFIX}/{{name}}/screen"),
            name: "Session Screen".to_string(),
            title: Some("Terminal Screen".to_string()),
            description: Some(
                "Current visible screen contents of a terminal session, including text, \
                 colors, cursor position, and dimensions."
                    .to_string(),
            ),
            mime_type: Some("application/json".to_string()),
            icons: None,
        }
        .no_annotation(),
        RawResourceTemplate {
            uri_template: format!("{URI_PREFIX}/{{name}}/scrollback"),
            name: "Session Scrollback".to_string(),
            title: Some("Terminal Scrollback".to_string()),
            description: Some(
                "Scrollback buffer contents of a terminal session. Returns historical \
                 output that has scrolled off the visible screen."
                    .to_string(),
            ),
            mime_type: Some("application/json".to_string()),
            icons: None,
        }
        .no_annotation(),
    ];

    Ok(ListResourceTemplatesResult {
        meta: None,
        next_cursor: None,
        resource_templates: templates,
    })
}

/// Read a resource by URI.
///
/// Supported URIs:
/// - `wsh://sessions` -> JSON array of sessions with name, rows, cols
/// - `wsh://sessions/{name}/screen` -> styled screen contents
/// - `wsh://sessions/{name}/scrollback` -> styled scrollback buffer (offset=0, limit=100)
pub async fn read_resource(
    state: &AppState,
    request: ReadResourceRequestParams,
) -> Result<ReadResourceResult, ErrorData> {
    let uri = &request.uri;

    // Parse the URI to determine what resource is being requested
    let (session_name, resource_type) = parse_resource_uri(uri)?;

    match (session_name, resource_type) {
        // wsh://sessions -> list all sessions
        (None, ResourceType::SessionList) => {
            let names = state.sessions.list();
            let sessions: Vec<serde_json::Value> = names
                .into_iter()
                .filter_map(|name| {
                    let session = state.sessions.get(&name)?;
                    let (rows, cols) = session.terminal_size.get();
                    Some(serde_json::json!({
                        "name": name,
                        "pid": session.pid,
                        "command": session.command.clone(),
                        "rows": rows,
                        "cols": cols,
                        "clients": session.clients(),
                    }))
                })
                .collect();

            let json = serde_json::to_string(&sessions).unwrap_or_default();
            Ok(ReadResourceResult {
                contents: vec![ResourceContents::text(json, uri.clone())],
            })
        }

        // wsh://sessions/{name}/screen -> screen contents
        (Some(name), ResourceType::Screen) => {
            let session = state.sessions.get(&name).ok_or_else(|| {
                ErrorData::resource_not_found(
                    format!("session not found: {name}"),
                    None,
                )
            })?;

            let response = session
                .parser
                .query(Query::Screen {
                    format: Format::Styled,
                })
                .await
                .map_err(|e| {
                    ErrorData::internal_error(format!("parser error: {e}"), None)
                })?;

            let json = serde_json::to_string(&response).unwrap_or_default();
            Ok(ReadResourceResult {
                contents: vec![ResourceContents::text(json, uri.clone())],
            })
        }

        // wsh://sessions/{name}/scrollback -> scrollback buffer
        (Some(name), ResourceType::Scrollback) => {
            let session = state.sessions.get(&name).ok_or_else(|| {
                ErrorData::resource_not_found(
                    format!("session not found: {name}"),
                    None,
                )
            })?;

            let response = session
                .parser
                .query(Query::Scrollback {
                    format: Format::Styled,
                    offset: 0,
                    limit: 100,
                })
                .await
                .map_err(|e| {
                    ErrorData::internal_error(format!("parser error: {e}"), None)
                })?;

            let json = serde_json::to_string(&response).unwrap_or_default();
            Ok(ReadResourceResult {
                contents: vec![ResourceContents::text(json, uri.clone())],
            })
        }

        _ => Err(ErrorData::resource_not_found(
            format!("unknown resource: {uri}"),
            None,
        )),
    }
}

/// Resource type parsed from a URI.
#[derive(Debug, PartialEq)]
enum ResourceType {
    SessionList,
    Screen,
    Scrollback,
    Unknown,
}

/// Parse a `wsh://sessions/...` URI into its components.
///
/// Returns `(Option<session_name>, ResourceType)`.
fn parse_resource_uri(uri: &str) -> Result<(Option<String>, ResourceType), ErrorData> {
    // Must start with our prefix
    if !uri.starts_with("wsh://sessions") {
        return Err(ErrorData::resource_not_found(
            format!("unknown resource scheme: {uri}"),
            None,
        ));
    }

    // Exact match: wsh://sessions
    if uri == "wsh://sessions" {
        return Ok((None, ResourceType::SessionList));
    }

    // Must have a '/' after "wsh://sessions"
    let rest = uri.strip_prefix("wsh://sessions/").ok_or_else(|| {
        ErrorData::resource_not_found(
            format!("invalid resource URI: {uri}"),
            None,
        )
    })?;

    // rest should be "{name}/screen" or "{name}/scrollback"
    if let Some((name, resource)) = rest.rsplit_once('/') {
        if name.is_empty() {
            return Err(ErrorData::resource_not_found(
                format!("empty session name in URI: {uri}"),
                None,
            ));
        }
        let resource_type = match resource {
            "screen" => ResourceType::Screen,
            "scrollback" => ResourceType::Scrollback,
            _ => ResourceType::Unknown,
        };
        Ok((Some(name.to_string()), resource_type))
    } else {
        // Just a session name with no sub-resource — not a valid resource
        Err(ErrorData::resource_not_found(
            format!("incomplete resource URI: {uri}"),
            None,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── URI parsing tests ───────────────────────────────────────

    #[test]
    fn parse_sessions_list_uri() {
        let (name, rtype) = parse_resource_uri("wsh://sessions").unwrap();
        assert!(name.is_none());
        assert_eq!(rtype, ResourceType::SessionList);
    }

    #[test]
    fn parse_screen_uri() {
        let (name, rtype) = parse_resource_uri("wsh://sessions/test/screen").unwrap();
        assert_eq!(name.as_deref(), Some("test"));
        assert_eq!(rtype, ResourceType::Screen);
    }

    #[test]
    fn parse_scrollback_uri() {
        let (name, rtype) = parse_resource_uri("wsh://sessions/my-session/scrollback").unwrap();
        assert_eq!(name.as_deref(), Some("my-session"));
        assert_eq!(rtype, ResourceType::Scrollback);
    }

    #[test]
    fn parse_unknown_sub_resource() {
        let (name, rtype) = parse_resource_uri("wsh://sessions/test/unknown").unwrap();
        assert_eq!(name.as_deref(), Some("test"));
        assert_eq!(rtype, ResourceType::Unknown);
    }

    #[test]
    fn parse_invalid_scheme() {
        let result = parse_resource_uri("wsh://invalid");
        assert!(result.is_err());
    }

    #[test]
    fn parse_incomplete_uri() {
        let result = parse_resource_uri("wsh://sessions/test");
        assert!(result.is_err());
    }

    #[test]
    fn parse_empty_session_name() {
        let result = parse_resource_uri("wsh://sessions//screen");
        assert!(result.is_err());
    }

    #[test]
    fn parse_session_name_with_slashes() {
        // "wsh://sessions/a/b/screen" should parse name="a/b", resource="screen"
        let (name, rtype) = parse_resource_uri("wsh://sessions/a/b/screen").unwrap();
        assert_eq!(name.as_deref(), Some("a/b"));
        assert_eq!(rtype, ResourceType::Screen);
    }

    // ── list_resource_templates tests ───────────────────────────

    #[tokio::test]
    async fn list_resource_templates_returns_two_templates() {
        let result = list_resource_templates().await.unwrap();
        assert_eq!(result.resource_templates.len(), 2);

        let names: Vec<&str> = result
            .resource_templates
            .iter()
            .map(|t| t.raw.name.as_str())
            .collect();
        assert!(names.contains(&"Session Screen"));
        assert!(names.contains(&"Session Scrollback"));
    }

    #[tokio::test]
    async fn list_resource_templates_have_uri_templates() {
        let result = list_resource_templates().await.unwrap();
        let uris: Vec<&str> = result
            .resource_templates
            .iter()
            .map(|t| t.raw.uri_template.as_str())
            .collect();
        assert!(uris.contains(&"wsh://sessions/{name}/screen"));
        assert!(uris.contains(&"wsh://sessions/{name}/scrollback"));
    }

    // ── read_resource with unknown URI ──────────────────────────

    #[tokio::test]
    async fn read_resource_unknown_sub_resource_returns_error() {
        let state = AppState {
            sessions: crate::session::SessionRegistry::new(),
            shutdown: crate::shutdown::ShutdownCoordinator::new(),
            server_config: std::sync::Arc::new(crate::api::ServerConfig::new(false)),
            server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            ticket_store: std::sync::Arc::new(crate::api::ticket::TicketStore::new()),
            backends: crate::federation::registry::BackendRegistry::new(),
            federation: std::sync::Arc::new(tokio::sync::Mutex::new(crate::federation::manager::FederationManager::new())),
            ip_access: None,
            hostname: "test".to_string(),
            federation_config_path: None,
            local_token: None,
            default_backend_token: None,
        };

        let request = ReadResourceRequestParams {
            meta: None,
            uri: "wsh://sessions/test/unknown".to_string(),
        };

        let result = read_resource(&state, request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_resource_invalid_uri_returns_error() {
        let state = AppState {
            sessions: crate::session::SessionRegistry::new(),
            shutdown: crate::shutdown::ShutdownCoordinator::new(),
            server_config: std::sync::Arc::new(crate::api::ServerConfig::new(false)),
            server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            ticket_store: std::sync::Arc::new(crate::api::ticket::TicketStore::new()),
            backends: crate::federation::registry::BackendRegistry::new(),
            federation: std::sync::Arc::new(tokio::sync::Mutex::new(crate::federation::manager::FederationManager::new())),
            ip_access: None,
            hostname: "test".to_string(),
            federation_config_path: None,
            local_token: None,
            default_backend_token: None,
        };

        let request = ReadResourceRequestParams {
            meta: None,
            uri: "wsh://invalid".to_string(),
        };

        let result = read_resource(&state, request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_resource_sessions_returns_valid_json() {
        let state = AppState {
            sessions: crate::session::SessionRegistry::new(),
            shutdown: crate::shutdown::ShutdownCoordinator::new(),
            server_config: std::sync::Arc::new(crate::api::ServerConfig::new(false)),
            server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            ticket_store: std::sync::Arc::new(crate::api::ticket::TicketStore::new()),
            backends: crate::federation::registry::BackendRegistry::new(),
            federation: std::sync::Arc::new(tokio::sync::Mutex::new(crate::federation::manager::FederationManager::new())),
            ip_access: None,
            hostname: "test".to_string(),
            federation_config_path: None,
            local_token: None,
            default_backend_token: None,
        };

        let request = ReadResourceRequestParams {
            meta: None,
            uri: "wsh://sessions".to_string(),
        };

        let result = read_resource(&state, request).await.unwrap();
        assert_eq!(result.contents.len(), 1);

        // Extract the text content and verify it's valid JSON
        match &result.contents[0] {
            ResourceContents::TextResourceContents { text, .. } => {
                let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
                assert!(parsed.is_array());
                assert_eq!(parsed.as_array().unwrap().len(), 0);
            }
            _ => panic!("expected text resource contents"),
        }
    }

    #[tokio::test]
    async fn read_resource_nonexistent_session_screen_returns_error() {
        let state = AppState {
            sessions: crate::session::SessionRegistry::new(),
            shutdown: crate::shutdown::ShutdownCoordinator::new(),
            server_config: std::sync::Arc::new(crate::api::ServerConfig::new(false)),
            server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            ticket_store: std::sync::Arc::new(crate::api::ticket::TicketStore::new()),
            backends: crate::federation::registry::BackendRegistry::new(),
            federation: std::sync::Arc::new(tokio::sync::Mutex::new(crate::federation::manager::FederationManager::new())),
            ip_access: None,
            hostname: "test".to_string(),
            federation_config_path: None,
            local_token: None,
            default_backend_token: None,
        };

        let request = ReadResourceRequestParams {
            meta: None,
            uri: "wsh://sessions/nonexistent/screen".to_string(),
        };

        let result = read_resource(&state, request).await;
        assert!(result.is_err());
    }

    // ── list_resources with no sessions ─────────────────────────

    #[tokio::test]
    async fn list_resources_empty_registry() {
        let state = AppState {
            sessions: crate::session::SessionRegistry::new(),
            shutdown: crate::shutdown::ShutdownCoordinator::new(),
            server_config: std::sync::Arc::new(crate::api::ServerConfig::new(false)),
            server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            ticket_store: std::sync::Arc::new(crate::api::ticket::TicketStore::new()),
            backends: crate::federation::registry::BackendRegistry::new(),
            federation: std::sync::Arc::new(tokio::sync::Mutex::new(crate::federation::manager::FederationManager::new())),
            ip_access: None,
            hostname: "test".to_string(),
            federation_config_path: None,
            local_token: None,
            default_backend_token: None,
        };

        let result = list_resources(&state).await.unwrap();
        // Should have exactly 1 resource: the sessions list
        assert_eq!(result.resources.len(), 1);
        assert_eq!(result.resources[0].raw.uri, "wsh://sessions");
    }
}
