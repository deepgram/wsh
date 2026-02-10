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
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

use super::AppState;
use super::handlers::flush_overlays_to_stdout;

/// Parse params from a WsRequest, returning a WsResponse error on failure.
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
pub async fn dispatch(req: &WsRequest, state: &AppState) -> WsResponse {
    let id = req.id.clone();
    let method = req.method.as_str();

    match method {
        "get_input_mode" => {
            let mode = state.input_mode.get();
            WsResponse::success(id, method, serde_json::json!({ "mode": mode }))
        }
        "capture_input" => {
            state.input_mode.capture();
            WsResponse::success(id, method, serde_json::json!({}))
        }
        "release_input" => {
            state.input_mode.release();
            WsResponse::success(id, method, serde_json::json!({}))
        }
        "list_overlays" => {
            let overlays = state.overlays.list();
            WsResponse::success(id, method, serde_json::to_value(&overlays).unwrap())
        }
        "clear_overlays" => {
            let old = state.overlays.list();
            state.overlays.clear();
            flush_overlays_to_stdout(&old, &[]);
            WsResponse::success(id, method, serde_json::json!({}))
        }
        "get_screen" => {
            let params: ScreenParams = match parse_params(req) {
                Ok(p) => p,
                Err(e) => return e,
            };
            match state.parser.query(Query::Screen { format: params.format }).await {
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
            match state
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
            match state.input_tx.send(bytes).await {
                Ok(()) => WsResponse::success(id, method, serde_json::json!({})),
                Err(_) => WsResponse::error(
                    id,
                    method,
                    "input_send_failed",
                    "Failed to send input to terminal.",
                ),
            }
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

    use crate::api::AppState;
    use crate::broker::Broker;
    use crate::input::{InputBroadcaster, InputMode};
    use crate::overlay::OverlayStore;
    use crate::parser::Parser;
    use crate::shutdown::ShutdownCoordinator;
    use bytes::Bytes;
    use tokio::sync::mpsc;

    fn create_test_state() -> (AppState, mpsc::Receiver<Bytes>) {
        let (input_tx, input_rx) = mpsc::channel(64);
        let broker = Broker::new();
        let parser = Parser::spawn(&broker, 80, 24, 1000);
        let state = AppState {
            input_tx,
            output_rx: broker.sender(),
            shutdown: ShutdownCoordinator::new(),
            parser,
            overlays: OverlayStore::new(),
            input_mode: InputMode::new(),
            input_broadcaster: InputBroadcaster::new(),
        };
        (state, input_rx)
    }

    #[tokio::test]
    async fn dispatch_unknown_method() {
        let (state, _rx) = create_test_state();
        let req = WsRequest {
            id: None,
            method: "do_magic".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], "unknown_method");
        assert_eq!(json["method"], "do_magic");
    }

    #[tokio::test]
    async fn dispatch_get_input_mode() {
        let (state, _rx) = create_test_state();
        let req = WsRequest {
            id: Some(serde_json::Value::Number(1.into())),
            method: "get_input_mode".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["id"], 1);
        assert_eq!(json["method"], "get_input_mode");
        assert_eq!(json["result"]["mode"], "passthrough");
    }

    #[tokio::test]
    async fn dispatch_capture_and_release() {
        let (state, _rx) = create_test_state();

        // Capture
        let req = WsRequest {
            id: None,
            method: "capture_input".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &state).await;
        assert!(serde_json::to_value(&resp).unwrap()["result"].is_object());

        // Verify mode changed
        let req = WsRequest {
            id: None,
            method: "get_input_mode".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["result"]["mode"], "capture");

        // Release
        let req = WsRequest {
            id: None,
            method: "release_input".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &state).await;
        assert!(serde_json::to_value(&resp).unwrap()["result"].is_object());

        // Verify
        let req = WsRequest {
            id: None,
            method: "get_input_mode".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["result"]["mode"], "passthrough");
    }

    #[tokio::test]
    async fn dispatch_list_overlays_empty() {
        let (state, _rx) = create_test_state();
        let req = WsRequest {
            id: None,
            method: "list_overlays".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["result"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn dispatch_clear_overlays() {
        let (state, _rx) = create_test_state();
        state.overlays.create(0, 0, None, vec![]);
        assert_eq!(state.overlays.list().len(), 1);

        let req = WsRequest {
            id: None,
            method: "clear_overlays".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &state).await;
        assert!(serde_json::to_value(&resp).unwrap()["result"].is_object());
        assert_eq!(state.overlays.list().len(), 0);
    }

    #[tokio::test]
    async fn dispatch_get_screen() {
        let (state, _rx) = create_test_state();
        let req = WsRequest {
            id: Some(serde_json::Value::Number(1.into())),
            method: "get_screen".to_string(),
            params: Some(serde_json::json!({"format": "plain"})),
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"]["cols"].is_number());
        assert!(json["result"]["rows"].is_number());
        assert!(json["result"]["lines"].is_array());
    }

    #[tokio::test]
    async fn dispatch_get_screen_no_params() {
        let (state, _rx) = create_test_state();
        let req = WsRequest {
            id: None,
            method: "get_screen".to_string(),
            params: None,
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"]["cols"].is_number());
    }

    #[tokio::test]
    async fn dispatch_get_scrollback() {
        let (state, _rx) = create_test_state();
        let req = WsRequest {
            id: None,
            method: "get_scrollback".to_string(),
            params: Some(serde_json::json!({"format": "plain", "offset": 0, "limit": 10})),
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"]["total_lines"].is_number());
        assert!(json["result"]["lines"].is_array());
    }

    #[tokio::test]
    async fn dispatch_send_input_utf8() {
        let (state, mut rx) = create_test_state();
        let req = WsRequest {
            id: None,
            method: "send_input".to_string(),
            params: Some(serde_json::json!({"data": "hello"})),
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"].is_object());

        let received = rx.try_recv().unwrap();
        assert_eq!(received.as_ref(), b"hello");
    }

    #[tokio::test]
    async fn dispatch_send_input_base64() {
        let (state, mut rx) = create_test_state();
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"\x03");
        let req = WsRequest {
            id: None,
            method: "send_input".to_string(),
            params: Some(serde_json::json!({"data": encoded, "encoding": "base64"})),
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"].is_object());

        let received = rx.try_recv().unwrap();
        assert_eq!(received.as_ref(), b"\x03");
    }

    #[tokio::test]
    async fn dispatch_send_input_bad_base64() {
        let (state, _rx) = create_test_state();
        let req = WsRequest {
            id: None,
            method: "send_input".to_string(),
            params: Some(serde_json::json!({"data": "!!!not-base64!!!", "encoding": "base64"})),
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], "invalid_request");
    }
}
