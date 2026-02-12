use serde::{Deserialize, Serialize};

use crate::overlay::OverlaySpan;
use crate::parser::events::EventType;
use crate::parser::state::{Format, Query};

// ---------------------------------------------------------------------------
// Envelope types
// ---------------------------------------------------------------------------

/// Incoming WebSocket request (JSON-RPC-ish).
#[derive(Debug, Deserialize)]
pub struct WsRequest {
    /// Optional request id, echoed back in the response.
    pub id: Option<serde_json::Value>,
    /// Method name (e.g. "get_screen", "send_input").
    pub method: String,
    /// Method-specific parameters.
    pub params: Option<serde_json::Value>,
}

/// Server-level WebSocket request — includes optional session field.
///
/// Used by the multiplexed `/ws/json` endpoint where a single WebSocket
/// connection can interact with multiple sessions.
#[derive(Debug, Deserialize)]
pub struct ServerWsRequest {
    /// Optional request id, echoed back in the response.
    pub id: Option<serde_json::Value>,
    /// Method name (e.g. "create_session", "get_screen").
    pub method: String,
    /// Target session name (required for per-session methods).
    pub session: Option<String>,
    /// Method-specific parameters.
    pub params: Option<serde_json::Value>,
}

/// Outgoing WebSocket response.
#[derive(Debug, Serialize)]
pub struct WsResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<WsError>,
}

impl WsResponse {
    /// Build a successful response.
    pub fn success(
        id: Option<serde_json::Value>,
        method: &str,
        result: serde_json::Value,
    ) -> Self {
        Self {
            id,
            method: Some(method.to_owned()),
            result: Some(result),
            error: None,
        }
    }

    /// Build an error response tied to a particular request.
    pub fn error(
        id: Option<serde_json::Value>,
        method: &str,
        code: &str,
        message: &str,
    ) -> Self {
        Self {
            id,
            method: Some(method.to_owned()),
            result: None,
            error: Some(WsError {
                code: code.to_owned(),
                message: message.to_owned(),
            }),
        }
    }

    /// Build a protocol-level error (no method or id available).
    pub fn protocol_error(code: &str, message: &str) -> Self {
        Self {
            id: None,
            method: None,
            result: None,
            error: Some(WsError {
                code: code.to_owned(),
                message: message.to_owned(),
            }),
        }
    }
}

/// Error payload inside a [`WsResponse`].
#[derive(Debug, Serialize, Deserialize)]
pub struct WsError {
    pub code: String,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Method-specific param types
// ---------------------------------------------------------------------------

/// Parameters for the `subscribe` method.
#[derive(Debug, Deserialize)]
pub struct SubscribeParams {
    pub events: Vec<EventType>,
    #[serde(default = "default_interval")]
    pub interval_ms: u64,
    #[serde(default)]
    pub format: Format,
    /// When > 0, the server will emit a `sync` event whenever the terminal has
    /// been idle for this many milliseconds after any activity.
    #[serde(default)]
    pub quiesce_ms: u64,
}

/// Parameters for the `await_quiesce` WebSocket method.
#[derive(Debug, Deserialize)]
pub struct AwaitQuiesceParams {
    pub timeout_ms: u64,
    #[serde(default)]
    pub format: Format,
    pub max_wait_ms: Option<u64>,
    /// Generation from a previous quiescence response. If provided and matches
    /// the current generation, the server waits for new activity before
    /// checking quiescence.
    pub last_generation: Option<u64>,
    /// When true, always observe real silence for `timeout_ms` before responding.
    #[serde(default)]
    pub fresh: bool,
}

fn default_interval() -> u64 {
    100
}

/// Parameters for the `get_screen` method.
#[derive(Debug, Deserialize)]
pub struct ScreenParams {
    #[serde(default)]
    pub format: Format,
}

/// Parameters for the `get_scrollback` method.
#[derive(Debug, Deserialize)]
pub struct ScrollbackParams {
    #[serde(default)]
    pub format: Format,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "default_scrollback_limit")]
    pub limit: usize,
}

fn default_scrollback_limit() -> usize {
    100
}

/// Parameters for the `send_input` method.
#[derive(Debug, Deserialize)]
pub struct SendInputParams {
    pub data: String,
    #[serde(default)]
    pub encoding: InputEncoding,
}

/// Encoding used for [`SendInputParams::data`].
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputEncoding {
    #[default]
    Utf8,
    Base64,
}

// ---------------------------------------------------------------------------
// Overlay param types
// ---------------------------------------------------------------------------

/// Parameters that identify an overlay by id (get / delete).
#[derive(Debug, Deserialize)]
pub struct OverlayIdParams {
    pub id: String,
}

/// Parameters for creating a new overlay.
#[derive(Debug, Deserialize)]
pub struct CreateOverlayParams {
    pub x: u16,
    pub y: u16,
    pub z: Option<i32>,
    pub width: u16,
    pub height: u16,
    #[serde(default)]
    pub background: Option<crate::overlay::BackgroundStyle>,
    pub spans: Vec<OverlaySpan>,
}

/// Parameters for replacing an overlay's spans.
#[derive(Debug, Deserialize)]
pub struct UpdateOverlayParams {
    pub id: String,
    pub spans: Vec<OverlaySpan>,
}

/// Parameters for patching overlay position / z-order.
#[derive(Debug, Deserialize)]
pub struct PatchOverlayParams {
    pub id: String,
    pub x: Option<u16>,
    pub y: Option<u16>,
    pub z: Option<i32>,
    pub width: Option<u16>,
    pub height: Option<u16>,
}

// ---------------------------------------------------------------------------
// Panel param types
// ---------------------------------------------------------------------------

use crate::panel::Position;

/// Parameters that identify a panel by id (get / delete).
#[derive(Debug, Deserialize)]
pub struct PanelIdParams {
    pub id: String,
}

/// Parameters for creating a new panel.
#[derive(Debug, Deserialize)]
pub struct CreatePanelParams {
    pub position: Position,
    pub height: u16,
    pub z: Option<i32>,
    #[serde(default)]
    pub spans: Vec<OverlaySpan>,
}

/// Parameters for fully replacing a panel.
#[derive(Debug, Deserialize)]
pub struct UpdatePanelParams {
    pub id: String,
    pub position: Position,
    pub height: u16,
    pub z: i32,
    pub spans: Vec<OverlaySpan>,
}

/// Parameters for patching panel properties.
#[derive(Debug, Deserialize)]
pub struct PatchPanelParams {
    pub id: String,
    pub position: Option<Position>,
    pub height: Option<u16>,
    pub z: Option<i32>,
    pub spans: Option<Vec<OverlaySpan>>,
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

use crate::session::Session;
use super::handlers::flush_overlays_to_stdout;

/// Parse params from a WsRequest, returning a WsResponse error on failure.
#[allow(clippy::result_large_err)]
fn parse_params<T: serde::de::DeserializeOwned>(req: &WsRequest) -> Result<T, WsResponse> {
    let params = req
        .params
        .as_ref()
        .cloned()
        .unwrap_or(serde_json::Value::Object(Default::default()));
    serde_json::from_value(params).map_err(|e| {
        WsResponse::error(
            req.id.clone(),
            &req.method,
            "invalid_request",
            &format!("Invalid params: {}.", e),
        )
    })
}

/// Dispatch a WebSocket request to the appropriate handler.
pub async fn dispatch(req: &WsRequest, session: &Session) -> WsResponse {
    let id = req.id.clone();
    let method = req.method.as_str();

    match method {
        "get_input_mode" => {
            let mode = session.input_mode.get();
            WsResponse::success(id, method, serde_json::json!({ "mode": mode }))
        }
        "capture_input" => {
            session.input_mode.capture();
            WsResponse::success(id, method, serde_json::json!({}))
        }
        "release_input" => {
            session.input_mode.release();
            WsResponse::success(id, method, serde_json::json!({}))
        }
        "list_overlays" => {
            let overlays = session.overlays.list();
            WsResponse::success(id, method, serde_json::to_value(&overlays).unwrap())
        }
        "clear_overlays" => {
            let old = session.overlays.list();
            session.overlays.clear();
            if session.is_local {
                flush_overlays_to_stdout(&old, &[]);
            }
            WsResponse::success(id, method, serde_json::json!({}))
        }
        "create_overlay" => {
            let params: CreateOverlayParams = match parse_params(req) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let overlay_id = session.overlays.create(params.x, params.y, params.z, params.width, params.height, params.background, params.spans);
            if session.is_local {
                let all = session.overlays.list();
                flush_overlays_to_stdout(&[], &all);
            }
            WsResponse::success(id, method, serde_json::json!({ "id": overlay_id }))
        }
        "get_overlay" => {
            let params: OverlayIdParams = match parse_params(req) {
                Ok(p) => p,
                Err(e) => return e,
            };
            match session.overlays.get(&params.id) {
                Some(overlay) => WsResponse::success(
                    id,
                    method,
                    serde_json::to_value(&overlay).unwrap(),
                ),
                None => WsResponse::error(
                    id,
                    method,
                    "overlay_not_found",
                    &format!("No overlay exists with id '{}'.", params.id),
                ),
            }
        }
        "update_overlay" => {
            let params: UpdateOverlayParams = match parse_params(req) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let old = match session.overlays.get(&params.id) {
                Some(o) => o,
                None => {
                    return WsResponse::error(
                        id,
                        method,
                        "overlay_not_found",
                        &format!("No overlay exists with id '{}'.", params.id),
                    );
                }
            };
            if session.overlays.update(&params.id, params.spans) {
                if session.is_local {
                    flush_overlays_to_stdout(&[old], &session.overlays.list());
                }
                WsResponse::success(id, method, serde_json::json!({}))
            } else {
                WsResponse::error(
                    id,
                    method,
                    "overlay_not_found",
                    &format!("No overlay exists with id '{}'.", params.id),
                )
            }
        }
        "patch_overlay" => {
            let params: PatchOverlayParams = match parse_params(req) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let old = match session.overlays.get(&params.id) {
                Some(o) => o,
                None => {
                    return WsResponse::error(
                        id,
                        method,
                        "overlay_not_found",
                        &format!("No overlay exists with id '{}'.", params.id),
                    );
                }
            };
            if session.overlays.move_to(&params.id, params.x, params.y, params.z, params.width, params.height) {
                if session.is_local {
                    flush_overlays_to_stdout(&[old], &session.overlays.list());
                }
                WsResponse::success(id, method, serde_json::json!({}))
            } else {
                WsResponse::error(
                    id,
                    method,
                    "overlay_not_found",
                    &format!("No overlay exists with id '{}'.", params.id),
                )
            }
        }
        "delete_overlay" => {
            let params: OverlayIdParams = match parse_params(req) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let old = match session.overlays.get(&params.id) {
                Some(o) => o,
                None => {
                    return WsResponse::error(
                        id,
                        method,
                        "overlay_not_found",
                        &format!("No overlay exists with id '{}'.", params.id),
                    );
                }
            };
            if session.overlays.delete(&params.id) {
                if session.is_local {
                    flush_overlays_to_stdout(&[old], &session.overlays.list());
                }
                WsResponse::success(id, method, serde_json::json!({}))
            } else {
                WsResponse::error(
                    id,
                    method,
                    "overlay_not_found",
                    &format!("No overlay exists with id '{}'.", params.id),
                )
            }
        }
        "get_screen" => {
            let params: ScreenParams = match parse_params(req) {
                Ok(p) => p,
                Err(e) => return e,
            };
            match session.parser.query(Query::Screen { format: params.format }).await {
                Ok(resp) => WsResponse::success(
                    id,
                    method,
                    serde_json::to_value(&resp).unwrap(),
                ),
                Err(_) => WsResponse::error(
                    id,
                    method,
                    "parser_unavailable",
                    "Terminal parser is unavailable.",
                ),
            }
        }
        "get_scrollback" => {
            let params: ScrollbackParams = match parse_params(req) {
                Ok(p) => p,
                Err(e) => return e,
            };
            match session
                .parser
                .query(Query::Scrollback {
                    format: params.format,
                    offset: params.offset,
                    limit: params.limit,
                })
                .await
            {
                Ok(resp) => WsResponse::success(
                    id,
                    method,
                    serde_json::to_value(&resp).unwrap(),
                ),
                Err(_) => WsResponse::error(
                    id,
                    method,
                    "parser_unavailable",
                    "Terminal parser is unavailable.",
                ),
            }
        }
        "send_input" => {
            let params: SendInputParams = match parse_params(req) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let bytes = match params.encoding {
                InputEncoding::Utf8 => bytes::Bytes::from(params.data),
                InputEncoding::Base64 => {
                    use base64::Engine;
                    match base64::engine::general_purpose::STANDARD.decode(&params.data) {
                        Ok(decoded) => bytes::Bytes::from(decoded),
                        Err(e) => {
                            return WsResponse::error(
                                id,
                                method,
                                "invalid_request",
                                &format!("Invalid base64: {}.", e),
                            );
                        }
                    }
                }
            };
            match session.input_tx.send(bytes).await {
                Ok(()) => {
                    session.activity.touch();
                    WsResponse::success(id, method, serde_json::json!({}))
                }
                Err(_) => WsResponse::error(
                    id,
                    method,
                    "input_send_failed",
                    "Failed to send input to terminal.",
                ),
            }
        }
        "list_panels" => {
            let panels = session.panels.list();
            WsResponse::success(id, method, serde_json::to_value(&panels).unwrap())
        }
        "clear_panels" => {
            session.panels.clear();
            crate::panel::reconfigure_layout(
                &session.panels,
                &session.terminal_size,
                &session.pty,
                &session.parser,
            )
            .await;
            WsResponse::success(id, method, serde_json::json!({}))
        }
        "create_panel" => {
            let params: CreatePanelParams = match parse_params(req) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let panel_id = session
                .panels
                .create(params.position, params.height, params.z, params.spans);
            crate::panel::reconfigure_layout(
                &session.panels,
                &session.terminal_size,
                &session.pty,
                &session.parser,
            )
            .await;
            WsResponse::success(id, method, serde_json::json!({ "id": panel_id }))
        }
        "get_panel" => {
            let params: PanelIdParams = match parse_params(req) {
                Ok(p) => p,
                Err(e) => return e,
            };
            match session.panels.get(&params.id) {
                Some(panel) => WsResponse::success(
                    id,
                    method,
                    serde_json::to_value(&panel).unwrap(),
                ),
                None => WsResponse::error(
                    id,
                    method,
                    "panel_not_found",
                    &format!("No panel exists with id '{}'.", params.id),
                ),
            }
        }
        "update_panel" => {
            let params: UpdatePanelParams = match parse_params(req) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let old = match session.panels.get(&params.id) {
                Some(p) => p,
                None => {
                    return WsResponse::error(
                        id,
                        method,
                        "panel_not_found",
                        &format!("No panel exists with id '{}'.", params.id),
                    );
                }
            };
            if !session.panels.patch(
                &params.id,
                Some(params.position.clone()),
                Some(params.height),
                Some(params.z),
                Some(params.spans),
            ) {
                return WsResponse::error(
                    id,
                    method,
                    "panel_not_found",
                    &format!("No panel exists with id '{}'.", params.id),
                );
            }
            let needs_reconfigure = old.position != params.position
                || old.height != params.height
                || old.z != params.z;
            if needs_reconfigure {
                crate::panel::reconfigure_layout(
                    &session.panels,
                    &session.terminal_size,
                    &session.pty,
                    &session.parser,
                )
                .await;
            } else {
                crate::panel::flush_panel_content(
                    &session.panels,
                    &params.id,
                    &session.terminal_size,
                );
            }
            WsResponse::success(id, method, serde_json::json!({}))
        }
        "patch_panel" => {
            let params: PatchPanelParams = match parse_params(req) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let old = match session.panels.get(&params.id) {
                Some(p) => p,
                None => {
                    return WsResponse::error(
                        id,
                        method,
                        "panel_not_found",
                        &format!("No panel exists with id '{}'.", params.id),
                    );
                }
            };
            if !session.panels.patch(
                &params.id,
                params.position.clone(),
                params.height,
                params.z,
                params.spans.clone(),
            ) {
                return WsResponse::error(
                    id,
                    method,
                    "panel_not_found",
                    &format!("No panel exists with id '{}'.", params.id),
                );
            }
            let needs_reconfigure = params.position.as_ref().is_some_and(|p| *p != old.position)
                || params.height.is_some_and(|h| h != old.height)
                || params.z.is_some_and(|z| z != old.z);
            if needs_reconfigure {
                crate::panel::reconfigure_layout(
                    &session.panels,
                    &session.terminal_size,
                    &session.pty,
                    &session.parser,
                )
                .await;
            } else if params.spans.is_some() {
                crate::panel::flush_panel_content(
                    &session.panels,
                    &params.id,
                    &session.terminal_size,
                );
            }
            WsResponse::success(id, method, serde_json::json!({}))
        }
        "delete_panel" => {
            let params: PanelIdParams = match parse_params(req) {
                Ok(p) => p,
                Err(e) => return e,
            };
            if !session.panels.delete(&params.id) {
                return WsResponse::error(
                    id,
                    method,
                    "panel_not_found",
                    &format!("No panel exists with id '{}'.", params.id),
                );
            }
            crate::panel::reconfigure_layout(
                &session.panels,
                &session.terminal_size,
                &session.pty,
                &session.parser,
            )
            .await;
            WsResponse::success(id, method, serde_json::json!({}))
        }
        _ => WsResponse::error(
            id,
            method,
            "unknown_method",
            &format!("Unknown method '{}'.", method),
        ),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deserialize_request_with_id() {
        let raw = json!({
            "id": 3,
            "method": "get_screen",
            "params": { "format": "styled" }
        });
        let req: WsRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(req.id, Some(json!(3)));
        assert_eq!(req.method, "get_screen");
        assert!(req.params.is_some());
        let params = req.params.unwrap();
        assert_eq!(params["format"], "styled");
    }

    #[test]
    fn deserialize_request_without_id() {
        let raw = json!({ "method": "capture_input" });
        let req: WsRequest = serde_json::from_value(raw).unwrap();
        assert!(req.id.is_none());
        assert_eq!(req.method, "capture_input");
        assert!(req.params.is_none());
    }

    #[test]
    fn serialize_success_response() {
        let resp = WsResponse::success(Some(json!(1)), "get_screen", json!({ "ok": true }));
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["id"], 1);
        assert_eq!(v["method"], "get_screen");
        assert_eq!(v["result"]["ok"], true);
        assert!(v.get("error").is_none());
    }

    #[test]
    fn serialize_success_response_without_id() {
        let resp = WsResponse::success(None, "get_screen", json!({ "ok": true }));
        let v = serde_json::to_value(&resp).unwrap();
        assert!(v.get("id").is_none());
        assert_eq!(v["method"], "get_screen");
        assert_eq!(v["result"]["ok"], true);
    }

    #[test]
    fn serialize_error_response() {
        let resp = WsResponse::error(Some(json!(5)), "bad_method", "not_found", "unknown method");
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["id"], 5);
        assert_eq!(v["method"], "bad_method");
        assert_eq!(v["error"]["code"], "not_found");
        assert_eq!(v["error"]["message"], "unknown method");
        assert!(v.get("result").is_none());
    }

    #[test]
    fn serialize_protocol_error_no_method() {
        let resp = WsResponse::protocol_error("parse_error", "invalid JSON");
        let v = serde_json::to_value(&resp).unwrap();
        assert!(v.get("id").is_none());
        assert!(v.get("method").is_none());
        assert_eq!(v["error"]["code"], "parse_error");
        assert_eq!(v["error"]["message"], "invalid JSON");
    }

    #[test]
    fn deserialize_send_input_utf8() {
        let raw = json!({ "data": "hello\r" });
        let params: SendInputParams = serde_json::from_value(raw).unwrap();
        assert_eq!(params.data, "hello\r");
        assert_eq!(params.encoding, InputEncoding::Utf8);
    }

    #[test]
    fn deserialize_send_input_base64() {
        let raw = json!({ "data": "aGVsbG8=", "encoding": "base64" });
        let params: SendInputParams = serde_json::from_value(raw).unwrap();
        assert_eq!(params.data, "aGVsbG8=");
        assert_eq!(params.encoding, InputEncoding::Base64);
    }

    #[test]
    fn deserialize_subscribe_params() {
        let raw = json!({
            "events": ["lines", "cursor", "diffs"],
            "interval_ms": 200,
            "format": "plain"
        });
        let params: SubscribeParams = serde_json::from_value(raw).unwrap();
        assert_eq!(params.events.len(), 3);
        assert_eq!(params.events[0], EventType::Lines);
        assert_eq!(params.events[1], EventType::Cursor);
        assert_eq!(params.events[2], EventType::Diffs);
        assert_eq!(params.interval_ms, 200);
        assert_eq!(params.format, Format::Plain);
    }

    // -----------------------------------------------------------------------
    // Dispatch tests
    // -----------------------------------------------------------------------

    use crate::broker::Broker;
    use crate::input::{InputBroadcaster, InputMode};
    use crate::overlay::OverlayStore;
    use crate::parser::Parser;
    use crate::session::Session;
    use crate::shutdown::ShutdownCoordinator;
    use bytes::Bytes;
    use tokio::sync::mpsc;

    fn create_test_session() -> (Session, mpsc::Receiver<Bytes>) {
        let (input_tx, input_rx) = mpsc::channel(64);
        let broker = Broker::new();
        let parser = Parser::spawn(&broker, 80, 24, 1000);
        let session = Session {
            name: "test".to_string(),
            input_tx,
            output_rx: broker.sender(),
            shutdown: ShutdownCoordinator::new(),
            parser,
            overlays: OverlayStore::new(),
            input_mode: InputMode::new(),
            input_broadcaster: InputBroadcaster::new(),
            panels: crate::panel::PanelStore::new(),
            pty: std::sync::Arc::new(crate::pty::Pty::spawn(24, 80, crate::pty::SpawnCommand::default()).expect("failed to spawn PTY for test")),
            terminal_size: crate::terminal::TerminalSize::new(24, 80),
            activity: crate::activity::ActivityTracker::new(),
            is_local: false,
            detach_signal: tokio::sync::broadcast::channel::<()>(1).0,
        };
        (session, input_rx)
    }

    #[tokio::test]
    async fn dispatch_unknown_method() {
        let (session, _rx) = create_test_session();
        let req = WsRequest {
            id: None,
            method: "do_magic".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], "unknown_method");
        assert_eq!(json["method"], "do_magic");
    }

    #[tokio::test]
    async fn dispatch_get_input_mode() {
        let (session, _rx) = create_test_session();
        let req = WsRequest {
            id: Some(serde_json::Value::Number(1.into())),
            method: "get_input_mode".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["id"], 1);
        assert_eq!(json["method"], "get_input_mode");
        assert_eq!(json["result"]["mode"], "passthrough");
    }

    #[tokio::test]
    async fn dispatch_capture_and_release() {
        let (session, _rx) = create_test_session();

        // Capture
        let req = WsRequest {
            id: None,
            method: "capture_input".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &session).await;
        assert!(serde_json::to_value(&resp).unwrap()["result"].is_object());

        // Verify mode changed
        let req = WsRequest {
            id: None,
            method: "get_input_mode".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["result"]["mode"], "capture");

        // Release
        let req = WsRequest {
            id: None,
            method: "release_input".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &session).await;
        assert!(serde_json::to_value(&resp).unwrap()["result"].is_object());

        // Verify
        let req = WsRequest {
            id: None,
            method: "get_input_mode".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["result"]["mode"], "passthrough");
    }

    #[tokio::test]
    async fn dispatch_list_overlays_empty() {
        let (session, _rx) = create_test_session();
        let req = WsRequest {
            id: None,
            method: "list_overlays".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["result"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn dispatch_clear_overlays() {
        let (session, _rx) = create_test_session();
        session.overlays.create(0, 0, None, 80, 1, None, vec![]);
        assert_eq!(session.overlays.list().len(), 1);

        let req = WsRequest {
            id: None,
            method: "clear_overlays".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &session).await;
        assert!(serde_json::to_value(&resp).unwrap()["result"].is_object());
        assert_eq!(session.overlays.list().len(), 0);
    }

    #[tokio::test]
    async fn dispatch_get_screen() {
        let (session, _rx) = create_test_session();
        let req = WsRequest {
            id: Some(serde_json::Value::Number(1.into())),
            method: "get_screen".to_string(),
            params: Some(serde_json::json!({"format": "plain"})),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"]["cols"].is_number());
        assert!(json["result"]["rows"].is_number());
        assert!(json["result"]["lines"].is_array());
    }

    #[tokio::test]
    async fn dispatch_get_screen_no_params() {
        let (session, _rx) = create_test_session();
        let req = WsRequest {
            id: None,
            method: "get_screen".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"]["cols"].is_number());
    }

    #[tokio::test]
    async fn dispatch_get_scrollback() {
        let (session, _rx) = create_test_session();
        let req = WsRequest {
            id: None,
            method: "get_scrollback".to_string(),
            params: Some(serde_json::json!({"format": "plain", "offset": 0, "limit": 10})),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"]["total_lines"].is_number());
        assert!(json["result"]["lines"].is_array());
    }

    #[tokio::test]
    async fn dispatch_send_input_utf8() {
        let (session, mut rx) = create_test_session();
        let req = WsRequest {
            id: None,
            method: "send_input".to_string(),
            params: Some(serde_json::json!({"data": "hello"})),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"].is_object());

        let received = rx.try_recv().unwrap();
        assert_eq!(received.as_ref(), b"hello");
    }

    #[tokio::test]
    async fn dispatch_send_input_base64() {
        let (session, mut rx) = create_test_session();
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"\x03");
        let req = WsRequest {
            id: None,
            method: "send_input".to_string(),
            params: Some(serde_json::json!({"data": encoded, "encoding": "base64"})),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"].is_object());

        let received = rx.try_recv().unwrap();
        assert_eq!(received.as_ref(), b"\x03");
    }

    #[tokio::test]
    async fn dispatch_send_input_bad_base64() {
        let (session, _rx) = create_test_session();
        let req = WsRequest {
            id: None,
            method: "send_input".to_string(),
            params: Some(serde_json::json!({"data": "!!!not-base64!!!", "encoding": "base64"})),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], "invalid_request");
    }

    #[tokio::test]
    async fn dispatch_create_overlay() {
        let (session, _rx) = create_test_session();
        let req = WsRequest {
            id: None,
            method: "create_overlay".to_string(),
            params: Some(serde_json::json!({
                "x": 10, "y": 5, "width": 80, "height": 1,
                "spans": [{"text": "Hello"}]
            })),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"]["id"].is_string());
        assert_eq!(session.overlays.list().len(), 1);
    }

    #[tokio::test]
    async fn dispatch_get_overlay() {
        let (session, _rx) = create_test_session();
        let id = session.overlays.create(5, 10, None, 80, 1, None, vec![crate::overlay::OverlaySpan {
            text: "Test".to_string(),
            id: None, fg: None, bg: None, bold: false, italic: false, underline: false,
        }]);
        let req = WsRequest {
            id: None,
            method: "get_overlay".to_string(),
            params: Some(serde_json::json!({"id": id})),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["result"]["x"], 5);
        assert_eq!(json["result"]["y"], 10);
    }

    #[tokio::test]
    async fn dispatch_get_overlay_not_found() {
        let (session, _rx) = create_test_session();
        let req = WsRequest {
            id: None,
            method: "get_overlay".to_string(),
            params: Some(serde_json::json!({"id": "nonexistent"})),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], "overlay_not_found");
    }

    #[tokio::test]
    async fn dispatch_update_overlay() {
        let (session, _rx) = create_test_session();
        let id = session.overlays.create(0, 0, None, 80, 1, None, vec![crate::overlay::OverlaySpan {
            text: "Old".to_string(),
            id: None, fg: None, bg: None, bold: false, italic: false, underline: false,
        }]);
        let req = WsRequest {
            id: None,
            method: "update_overlay".to_string(),
            params: Some(serde_json::json!({"id": id, "spans": [{"text": "New"}]})),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"].is_object());
        let overlay = session.overlays.get(&id).unwrap();
        assert_eq!(overlay.spans[0].text, "New");
    }

    #[tokio::test]
    async fn dispatch_patch_overlay() {
        let (session, _rx) = create_test_session();
        let id = session.overlays.create(0, 0, None, 80, 1, None, vec![]);
        let req = WsRequest {
            id: None,
            method: "patch_overlay".to_string(),
            params: Some(serde_json::json!({"id": id, "x": 20, "y": 30})),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"].is_object());
        let overlay = session.overlays.get(&id).unwrap();
        assert_eq!(overlay.x, 20);
        assert_eq!(overlay.y, 30);
    }

    #[tokio::test]
    async fn dispatch_delete_overlay() {
        let (session, _rx) = create_test_session();
        let id = session.overlays.create(0, 0, None, 80, 1, None, vec![]);
        let req = WsRequest {
            id: None,
            method: "delete_overlay".to_string(),
            params: Some(serde_json::json!({"id": id})),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"].is_object());
        assert!(session.overlays.get(&id).is_none());
    }

    #[tokio::test]
    async fn dispatch_delete_overlay_not_found() {
        let (session, _rx) = create_test_session();
        let req = WsRequest {
            id: None,
            method: "delete_overlay".to_string(),
            params: Some(serde_json::json!({"id": "nonexistent"})),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], "overlay_not_found");
    }

    // -----------------------------------------------------------------------
    // Panel dispatch tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn dispatch_list_panels_empty() {
        let (session, _rx) = create_test_session();
        let req = WsRequest {
            id: None,
            method: "list_panels".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["result"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn dispatch_create_panel() {
        let (session, _rx) = create_test_session();
        let req = WsRequest {
            id: Some(json!(1)),
            method: "create_panel".to_string(),
            params: Some(json!({
                "position": "top",
                "height": 2,
                "spans": [{"text": "Status"}]
            })),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"]["id"].is_string());
        assert_eq!(json["id"], 1);
        assert_eq!(session.panels.list().len(), 1);
    }

    #[tokio::test]
    async fn dispatch_get_panel() {
        let (session, _rx) = create_test_session();
        let panel_id = session.panels.create(
            crate::panel::Position::Top,
            1,
            None,
            vec![crate::overlay::OverlaySpan {
                text: "Test".to_string(),
                id: None, fg: None, bg: None, bold: false, italic: false, underline: false,
            }],
        );
        let req = WsRequest {
            id: None,
            method: "get_panel".to_string(),
            params: Some(json!({"id": panel_id})),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["result"]["position"], "top");
        assert_eq!(json["result"]["height"], 1);
        assert_eq!(json["result"]["spans"][0]["text"], "Test");
    }

    #[tokio::test]
    async fn dispatch_get_panel_not_found() {
        let (session, _rx) = create_test_session();
        let req = WsRequest {
            id: None,
            method: "get_panel".to_string(),
            params: Some(json!({"id": "nonexistent"})),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], "panel_not_found");
    }

    #[tokio::test]
    async fn dispatch_update_panel() {
        let (session, _rx) = create_test_session();
        let panel_id = session.panels.create(
            crate::panel::Position::Top,
            1,
            None,
            vec![],
        );
        let panel = session.panels.get(&panel_id).unwrap();
        let req = WsRequest {
            id: None,
            method: "update_panel".to_string(),
            params: Some(json!({
                "id": panel_id,
                "position": "bottom",
                "height": 3,
                "z": panel.z,
                "spans": [{"text": "Updated"}]
            })),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"].is_object());
        let updated = session.panels.get(&panel_id).unwrap();
        assert_eq!(updated.position, crate::panel::Position::Bottom);
        assert_eq!(updated.height, 3);
        assert_eq!(updated.spans[0].text, "Updated");
    }

    #[tokio::test]
    async fn dispatch_update_panel_not_found() {
        let (session, _rx) = create_test_session();
        let req = WsRequest {
            id: None,
            method: "update_panel".to_string(),
            params: Some(json!({
                "id": "nonexistent",
                "position": "top",
                "height": 1,
                "z": 0,
                "spans": []
            })),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], "panel_not_found");
    }

    #[tokio::test]
    async fn dispatch_patch_panel() {
        let (session, _rx) = create_test_session();
        let panel_id = session.panels.create(
            crate::panel::Position::Top,
            1,
            None,
            vec![],
        );
        let req = WsRequest {
            id: None,
            method: "patch_panel".to_string(),
            params: Some(json!({
                "id": panel_id,
                "height": 5
            })),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"].is_object());
        let patched = session.panels.get(&panel_id).unwrap();
        assert_eq!(patched.height, 5);
        assert_eq!(patched.position, crate::panel::Position::Top);
    }

    #[tokio::test]
    async fn dispatch_patch_panel_not_found() {
        let (session, _rx) = create_test_session();
        let req = WsRequest {
            id: None,
            method: "patch_panel".to_string(),
            params: Some(json!({"id": "nonexistent", "height": 2})),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], "panel_not_found");
    }

    #[tokio::test]
    async fn dispatch_delete_panel() {
        let (session, _rx) = create_test_session();
        let panel_id = session.panels.create(
            crate::panel::Position::Bottom,
            2,
            None,
            vec![],
        );
        assert_eq!(session.panels.list().len(), 1);
        let req = WsRequest {
            id: None,
            method: "delete_panel".to_string(),
            params: Some(json!({"id": panel_id})),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"].is_object());
        assert!(session.panels.get(&panel_id).is_none());
    }

    #[tokio::test]
    async fn dispatch_delete_panel_not_found() {
        let (session, _rx) = create_test_session();
        let req = WsRequest {
            id: None,
            method: "delete_panel".to_string(),
            params: Some(json!({"id": "nonexistent"})),
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], "panel_not_found");
    }

    #[tokio::test]
    async fn dispatch_clear_panels() {
        let (session, _rx) = create_test_session();
        session.panels.create(crate::panel::Position::Top, 1, None, vec![]);
        session.panels.create(crate::panel::Position::Bottom, 1, None, vec![]);
        assert_eq!(session.panels.list().len(), 2);

        let req = WsRequest {
            id: None,
            method: "clear_panels".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"].is_object());
        assert_eq!(session.panels.list().len(), 0);
    }

    #[tokio::test]
    async fn dispatch_list_panels_after_create() {
        let (session, _rx) = create_test_session();
        session.panels.create(
            crate::panel::Position::Top,
            1,
            None,
            vec![crate::overlay::OverlaySpan {
                text: "A".to_string(),
                id: None, fg: None, bg: None, bold: false, italic: false, underline: false,
            }],
        );
        let req = WsRequest {
            id: None,
            method: "list_panels".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &session).await;
        let json = serde_json::to_value(&resp).unwrap();
        let panels = json["result"].as_array().unwrap();
        assert_eq!(panels.len(), 1);
        assert_eq!(panels[0]["position"], "top");
    }
}
