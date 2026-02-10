# WebSocket Request/Response Protocol Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add request/response method dispatch to the `/ws/json` WebSocket endpoint, bringing it to feature parity with the HTTP API.

**Architecture:** Define a `WsRequest`/`WsResponse` message protocol in a new `src/api/ws_methods.rs` module. Refactor `handle_ws_json` to dispatch incoming messages as either `subscribe` (the existing flow, now unified) or one of the new methods. Each method handler is a plain async function that takes `&AppState` + params and returns a result or error. The existing event push mechanism is untouched.

**Tech Stack:** Rust, serde_json, axum WebSocket, base64 crate (new dependency for `send_input` binary encoding)

**Design doc:** `docs/plans/2026-02-10-websocket-request-response-design.md`

---

### Task 1: Add `base64` dependency

We need the `base64` crate for decoding base64-encoded input in `send_input`.

**Files:**
- Modify: `Cargo.toml:6-24`

**Step 1: Add the dependency**

Add `base64 = "0.22"` to `[dependencies]` in `Cargo.toml`, after the `bytes` line:

```toml
base64 = "0.22"
```

**Step 2: Verify it compiles**

Run: `nix develop -c sh -c "cargo check"`
Expected: compiles successfully

**Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: add base64 dependency for WebSocket send_input encoding"
```

---

### Task 2: Define WebSocket request/response types

Create the message types that represent the unified protocol: `WsRequest`, `WsResponse`, method-specific params, and the error shape.

**Files:**
- Create: `src/api/ws_methods.rs`
- Modify: `src/api/mod.rs:1-4` (add `mod ws_methods;`)

**Step 1: Write the failing test**

At the bottom of `src/api/ws_methods.rs`, add a `#[cfg(test)]` module with tests for deserialization of requests and serialization of responses:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_request_with_id() {
        let json = r#"{"id": 3, "method": "get_screen", "params": {"format": "styled"}}"#;
        let req: WsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.id, Some(serde_json::Value::Number(3.into())));
        assert_eq!(req.method, "get_screen");
        assert!(req.params.is_some());
    }

    #[test]
    fn deserialize_request_without_id() {
        let json = r#"{"method": "capture_input"}"#;
        let req: WsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.id, None);
        assert_eq!(req.method, "capture_input");
        assert!(req.params.is_none());
    }

    #[test]
    fn serialize_success_response() {
        let resp = WsResponse::success(
            Some(serde_json::Value::Number(5.into())),
            "get_input_mode",
            serde_json::json!({"mode": "passthrough"}),
        );
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["id"], 5);
        assert_eq!(json["method"], "get_input_mode");
        assert_eq!(json["result"]["mode"], "passthrough");
        assert!(json.get("error").is_none());
    }

    #[test]
    fn serialize_success_response_without_id() {
        let resp = WsResponse::success(
            None,
            "capture_input",
            serde_json::json!({}),
        );
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("id").is_none());
        assert_eq!(json["method"], "capture_input");
    }

    #[test]
    fn serialize_error_response() {
        let resp = WsResponse::error(
            Some(serde_json::Value::Number(7.into())),
            "get_overlay",
            "overlay_not_found",
            "No overlay exists with id 'abc'.",
        );
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["id"], 7);
        assert_eq!(json["method"], "get_overlay");
        assert_eq!(json["error"]["code"], "overlay_not_found");
        assert_eq!(json["error"]["message"], "No overlay exists with id 'abc'.");
        assert!(json.get("result").is_none());
    }

    #[test]
    fn serialize_protocol_error_no_method() {
        let resp = WsResponse::protocol_error("invalid_request", "Missing 'method' field.");
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("method").is_none());
        assert!(json.get("id").is_none());
        assert_eq!(json["error"]["code"], "invalid_request");
    }

    #[test]
    fn deserialize_send_input_utf8() {
        let json = r#"{"data": "hello\r"}"#;
        let params: SendInputParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.data, "hello\r");
        assert_eq!(params.encoding, InputEncoding::Utf8);
    }

    #[test]
    fn deserialize_send_input_base64() {
        let json = r#"{"data": "aGVsbG8=", "encoding": "base64"}"#;
        let params: SendInputParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.encoding, InputEncoding::Base64);
    }

    #[test]
    fn deserialize_subscribe_params() {
        let json = r#"{"events": ["lines", "cursor"], "interval_ms": 50, "format": "plain"}"#;
        let params: SubscribeParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.events.len(), 2);
        assert_eq!(params.interval_ms, 50);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop -c sh -c "cargo test --lib api::ws_methods::tests -- 2>&1 | head -30"`
Expected: FAIL — module and types don't exist yet

**Step 3: Write the types**

In `src/api/ws_methods.rs`:

```rust
use serde::{Deserialize, Serialize};

use crate::parser::events::EventType;
use crate::parser::state::Format;

/// Incoming WebSocket request from client.
#[derive(Debug, Deserialize)]
pub struct WsRequest {
    /// Optional client-chosen ID, echoed in response.
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    /// Method name to invoke.
    pub method: String,
    /// Method-specific parameters.
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// Outgoing WebSocket response to client.
#[derive(Debug, Serialize)]
pub struct WsResponse {
    /// Echoed from request, omitted if not provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    /// Echoed method name, omitted for protocol-level errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Successful result (mutually exclusive with error).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error (mutually exclusive with result).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<WsError>,
}

#[derive(Debug, Serialize)]
pub struct WsError {
    pub code: String,
    pub message: String,
}

impl WsResponse {
    pub fn success(id: Option<serde_json::Value>, method: &str, result: serde_json::Value) -> Self {
        Self {
            id,
            method: Some(method.to_string()),
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<serde_json::Value>, method: &str, code: &str, message: &str) -> Self {
        Self {
            id,
            method: Some(method.to_string()),
            result: None,
            error: Some(WsError {
                code: code.to_string(),
                message: message.to_string(),
            }),
        }
    }

    /// Protocol-level error (malformed request, no method/id available).
    pub fn protocol_error(code: &str, message: &str) -> Self {
        Self {
            id: None,
            method: None,
            result: None,
            error: Some(WsError {
                code: code.to_string(),
                message: message.to_string(),
            }),
        }
    }
}

// --- Method-specific param types ---

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

#[derive(Debug, Deserialize)]
pub struct ScreenParams {
    #[serde(default)]
    pub format: Format,
}

#[derive(Debug, Deserialize)]
pub struct ScrollbackParams {
    #[serde(default)]
    pub format: Format,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    100
}

#[derive(Debug, Deserialize)]
pub struct SendInputParams {
    pub data: String,
    #[serde(default)]
    pub encoding: InputEncoding,
}

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InputEncoding {
    #[default]
    Utf8,
    Base64,
}

#[derive(Debug, Deserialize)]
pub struct OverlayIdParams {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateOverlayParams {
    pub x: u16,
    pub y: u16,
    #[serde(default)]
    pub z: Option<i32>,
    pub spans: Vec<crate::overlay::OverlaySpan>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateOverlayParams {
    pub id: String,
    pub spans: Vec<crate::overlay::OverlaySpan>,
}

#[derive(Debug, Deserialize)]
pub struct PatchOverlayParams {
    pub id: String,
    pub x: Option<u16>,
    pub y: Option<u16>,
    pub z: Option<i32>,
}
```

Add `mod ws_methods;` to `src/api/mod.rs` alongside the existing module declarations (after `mod handlers;`):

```rust
pub mod ws_methods;
```

**Step 4: Run tests to verify they pass**

Run: `nix develop -c sh -c "cargo test --lib api::ws_methods::tests"`
Expected: all 9 tests PASS

**Step 5: Commit**

```bash
git add src/api/ws_methods.rs src/api/mod.rs
git commit -m "feat(ws): add request/response message types for WebSocket protocol"
```

---

### Task 3: Implement method dispatch function

Add an `async fn dispatch(req: &WsRequest, state: &AppState) -> WsResponse` function that routes each method to the appropriate handler logic. Start with a skeleton that handles `unknown_method` and the simple non-param methods (`get_input_mode`, `capture_input`, `release_input`, `list_overlays`, `clear_overlays`).

**Files:**
- Modify: `src/api/ws_methods.rs`

**Step 1: Write the failing tests**

Add to the test module in `ws_methods.rs`. These tests need `AppState`, so reuse the `create_test_state` pattern from `src/api/mod.rs` tests:

```rust
    use crate::api::AppState;
    use crate::broker::Broker;
    use crate::input::{InputBroadcaster, InputMode};
    use crate::overlay::OverlayStore;
    use crate::parser::Parser;
    use crate::shutdown::ShutdownCoordinator;
    use tokio::sync::mpsc;
    use bytes::Bytes;

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
```

**Step 2: Run tests to verify they fail**

Run: `nix develop -c sh -c "cargo test --lib api::ws_methods::tests -- 2>&1 | head -30"`
Expected: FAIL — `dispatch` doesn't exist yet

**Step 3: Implement the dispatch function**

Add to `src/api/ws_methods.rs` (above the test module):

```rust
use super::AppState;
use super::handlers::flush_overlays_to_stdout;

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
        _ => WsResponse::error(
            id,
            method,
            "unknown_method",
            &format!("Unknown method '{}'.", method),
        ),
    }
}
```

Note: `flush_overlays_to_stdout` needs to be made `pub(super)` in `handlers.rs` (it's currently a private function). Change:

```rust
fn flush_overlays_to_stdout(to_erase: &[Overlay], to_render: &[Overlay]) {
```

to:

```rust
pub(super) fn flush_overlays_to_stdout(to_erase: &[Overlay], to_render: &[Overlay]) {
```

**Step 4: Run tests to verify they pass**

Run: `nix develop -c sh -c "cargo test --lib api::ws_methods::tests"`
Expected: all tests PASS

**Step 5: Commit**

```bash
git add src/api/ws_methods.rs src/api/handlers.rs
git commit -m "feat(ws): add dispatch function with simple methods (input mode, overlays)"
```

---

### Task 4: Implement screen, scrollback, and send_input methods

Add the remaining query and mutation methods to `dispatch`.

**Files:**
- Modify: `src/api/ws_methods.rs`

**Step 1: Write the failing tests**

```rust
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
        // Default format (styled) should work
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
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"\x03"); // Ctrl+C
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
```

**Step 2: Run tests to verify they fail**

Run: `nix develop -c sh -c "cargo test --lib api::ws_methods::tests -- 2>&1 | head -30"`
Expected: FAIL — new methods not yet in dispatch

**Step 3: Implement the methods**

Add these arms to the `match method` block in `dispatch`:

```rust
        "get_screen" => {
            let params: ScreenParams = match parse_params(req) {
                Ok(p) => p,
                Err(resp) => return resp,
            };
            match state.parser.query(crate::parser::state::Query::Screen { format: params.format }).await {
                Ok(response) => WsResponse::success(id, method, serde_json::to_value(&response).unwrap()),
                Err(_) => WsResponse::error(id, method, "parser_unavailable", "Terminal parser is unavailable."),
            }
        }
        "get_scrollback" => {
            let params: ScrollbackParams = match parse_params(req) {
                Ok(p) => p,
                Err(resp) => return resp,
            };
            match state.parser.query(crate::parser::state::Query::Scrollback {
                format: params.format,
                offset: params.offset,
                limit: params.limit,
            }).await {
                Ok(response) => WsResponse::success(id, method, serde_json::to_value(&response).unwrap()),
                Err(_) => WsResponse::error(id, method, "parser_unavailable", "Terminal parser is unavailable."),
            }
        }
        "send_input" => {
            let params: SendInputParams = match parse_params(req) {
                Ok(p) => p,
                Err(resp) => return resp,
            };
            let bytes = match params.encoding {
                InputEncoding::Utf8 => bytes::Bytes::from(params.data),
                InputEncoding::Base64 => {
                    use base64::Engine;
                    match base64::engine::general_purpose::STANDARD.decode(&params.data) {
                        Ok(decoded) => bytes::Bytes::from(decoded),
                        Err(e) => return WsResponse::error(
                            id, method, "invalid_request",
                            &format!("Invalid base64: {}.", e),
                        ),
                    }
                }
            };
            match state.input_tx.send(bytes).await {
                Ok(()) => WsResponse::success(id, method, serde_json::json!({})),
                Err(_) => WsResponse::error(id, method, "input_send_failed", "Failed to send input to terminal."),
            }
        }
```

Also add this helper function above `dispatch`:

```rust
/// Parse params from a WsRequest, returning a WsResponse error on failure.
fn parse_params<T: serde::de::DeserializeOwned>(req: &WsRequest) -> Result<T, WsResponse> {
    let params = req.params.as_ref().cloned().unwrap_or(serde_json::Value::Object(Default::default()));
    serde_json::from_value(params).map_err(|e| {
        WsResponse::error(
            req.id.clone(),
            &req.method,
            "invalid_request",
            &format!("Invalid params: {}.", e),
        )
    })
}
```

**Step 4: Run tests to verify they pass**

Run: `nix develop -c sh -c "cargo test --lib api::ws_methods::tests"`
Expected: all tests PASS

**Step 5: Commit**

```bash
git add src/api/ws_methods.rs
git commit -m "feat(ws): add get_screen, get_scrollback, and send_input dispatch methods"
```

---

### Task 5: Implement overlay CRUD methods

Add `create_overlay`, `get_overlay`, `update_overlay`, `patch_overlay`, and `delete_overlay` to dispatch.

**Files:**
- Modify: `src/api/ws_methods.rs`

**Step 1: Write the failing tests**

```rust
    #[tokio::test]
    async fn dispatch_create_overlay() {
        let (state, _rx) = create_test_state();
        let req = WsRequest {
            id: None,
            method: "create_overlay".to_string(),
            params: Some(serde_json::json!({
                "x": 10, "y": 5,
                "spans": [{"text": "Hello"}]
            })),
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"]["id"].is_string());
        assert_eq!(state.overlays.list().len(), 1);
    }

    #[tokio::test]
    async fn dispatch_get_overlay() {
        let (state, _rx) = create_test_state();
        let id = state.overlays.create(5, 10, None, vec![crate::overlay::OverlaySpan {
            text: "Test".to_string(),
            fg: None, bg: None, bold: false, italic: false, underline: false,
        }]);

        let req = WsRequest {
            id: None,
            method: "get_overlay".to_string(),
            params: Some(serde_json::json!({"id": id})),
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["result"]["x"], 5);
        assert_eq!(json["result"]["y"], 10);
    }

    #[tokio::test]
    async fn dispatch_get_overlay_not_found() {
        let (state, _rx) = create_test_state();
        let req = WsRequest {
            id: None,
            method: "get_overlay".to_string(),
            params: Some(serde_json::json!({"id": "nonexistent"})),
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], "overlay_not_found");
    }

    #[tokio::test]
    async fn dispatch_update_overlay() {
        let (state, _rx) = create_test_state();
        let id = state.overlays.create(0, 0, None, vec![crate::overlay::OverlaySpan {
            text: "Old".to_string(),
            fg: None, bg: None, bold: false, italic: false, underline: false,
        }]);

        let req = WsRequest {
            id: None,
            method: "update_overlay".to_string(),
            params: Some(serde_json::json!({
                "id": id,
                "spans": [{"text": "New"}]
            })),
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"].is_object());

        let overlay = state.overlays.get(&id).unwrap();
        assert_eq!(overlay.spans[0].text, "New");
    }

    #[tokio::test]
    async fn dispatch_patch_overlay() {
        let (state, _rx) = create_test_state();
        let id = state.overlays.create(0, 0, None, vec![]);

        let req = WsRequest {
            id: None,
            method: "patch_overlay".to_string(),
            params: Some(serde_json::json!({"id": id, "x": 20, "y": 30})),
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"].is_object());

        let overlay = state.overlays.get(&id).unwrap();
        assert_eq!(overlay.x, 20);
        assert_eq!(overlay.y, 30);
    }

    #[tokio::test]
    async fn dispatch_delete_overlay() {
        let (state, _rx) = create_test_state();
        let id = state.overlays.create(0, 0, None, vec![]);

        let req = WsRequest {
            id: None,
            method: "delete_overlay".to_string(),
            params: Some(serde_json::json!({"id": id})),
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["result"].is_object());
        assert!(state.overlays.get(&id).is_none());
    }

    #[tokio::test]
    async fn dispatch_delete_overlay_not_found() {
        let (state, _rx) = create_test_state();
        let req = WsRequest {
            id: None,
            method: "delete_overlay".to_string(),
            params: Some(serde_json::json!({"id": "nonexistent"})),
        };
        let resp = dispatch(&req, &state).await;
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], "overlay_not_found");
    }
```

**Step 2: Run tests to verify they fail**

Run: `nix develop -c sh -c "cargo test --lib api::ws_methods::tests -- 2>&1 | head -30"`
Expected: FAIL — overlay methods not in dispatch

**Step 3: Implement the overlay methods**

Add these arms to `dispatch`:

```rust
        "create_overlay" => {
            let params: CreateOverlayParams = match parse_params(req) {
                Ok(p) => p,
                Err(resp) => return resp,
            };
            let overlay_id = state.overlays.create(params.x, params.y, params.z, params.spans);
            let all = state.overlays.list();
            flush_overlays_to_stdout(&[], &all);
            WsResponse::success(id, method, serde_json::json!({"id": overlay_id}))
        }
        "get_overlay" => {
            let params: OverlayIdParams = match parse_params(req) {
                Ok(p) => p,
                Err(resp) => return resp,
            };
            match state.overlays.get(&params.id) {
                Some(overlay) => WsResponse::success(id, method, serde_json::to_value(&overlay).unwrap()),
                None => WsResponse::error(id, method, "overlay_not_found", &format!("No overlay exists with id '{}'.", params.id)),
            }
        }
        "update_overlay" => {
            let params: UpdateOverlayParams = match parse_params(req) {
                Ok(p) => p,
                Err(resp) => return resp,
            };
            let old = match state.overlays.get(&params.id) {
                Some(o) => o,
                None => return WsResponse::error(id, method, "overlay_not_found", &format!("No overlay exists with id '{}'.", params.id)),
            };
            if state.overlays.update(&params.id, params.spans) {
                let all = state.overlays.list();
                flush_overlays_to_stdout(&[old], &all);
                WsResponse::success(id, method, serde_json::json!({}))
            } else {
                WsResponse::error(id, method, "overlay_not_found", &format!("No overlay exists with id '{}'.", params.id))
            }
        }
        "patch_overlay" => {
            let params: PatchOverlayParams = match parse_params(req) {
                Ok(p) => p,
                Err(resp) => return resp,
            };
            let old = match state.overlays.get(&params.id) {
                Some(o) => o,
                None => return WsResponse::error(id, method, "overlay_not_found", &format!("No overlay exists with id '{}'.", params.id)),
            };
            if state.overlays.move_to(&params.id, params.x, params.y, params.z) {
                let all = state.overlays.list();
                flush_overlays_to_stdout(&[old], &all);
                WsResponse::success(id, method, serde_json::json!({}))
            } else {
                WsResponse::error(id, method, "overlay_not_found", &format!("No overlay exists with id '{}'.", params.id))
            }
        }
        "delete_overlay" => {
            let params: OverlayIdParams = match parse_params(req) {
                Ok(p) => p,
                Err(resp) => return resp,
            };
            let old = match state.overlays.get(&params.id) {
                Some(o) => o,
                None => return WsResponse::error(id, method, "overlay_not_found", &format!("No overlay exists with id '{}'.", params.id)),
            };
            if state.overlays.delete(&params.id) {
                let remaining = state.overlays.list();
                flush_overlays_to_stdout(&[old], &remaining);
                WsResponse::success(id, method, serde_json::json!({}))
            } else {
                WsResponse::error(id, method, "overlay_not_found", &format!("No overlay exists with id '{}'.", params.id))
            }
        }
```

**Step 4: Run tests to verify they pass**

Run: `nix develop -c sh -c "cargo test --lib api::ws_methods::tests"`
Expected: all tests PASS

**Step 5: Commit**

```bash
git add src/api/ws_methods.rs
git commit -m "feat(ws): add overlay CRUD dispatch methods"
```

---

### Task 6: Refactor `handle_ws_json` to use the unified protocol

Replace the current two-phase handshake (wait for subscribe → event loop) with the unified protocol: after sending `{"connected": true}`, all incoming messages are dispatched through `ws_methods::dispatch`. The `subscribe` method is handled as a special case that returns a response AND updates the event subscription state.

**Files:**
- Modify: `src/api/handlers.rs:120-311`

**Step 1: Write the failing test**

Add a new integration test file. This test verifies the new protocol works end-to-end: connect, send a method call, get a response.

Create `tests/ws_json_methods.rs`:

```rust
//! Integration tests for WebSocket JSON request/response protocol.

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use wsh::{
    api,
    broker::Broker,
    input::{InputBroadcaster, InputMode},
    overlay::OverlayStore,
    parser::Parser,
    shutdown::ShutdownCoordinator,
};

fn create_test_state() -> (api::AppState, mpsc::Receiver<Bytes>) {
    let (input_tx, input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    let state = api::AppState {
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

async fn start_server(app: axum::Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

/// Helper: receive next text message, parse as JSON.
async fn recv_json(
    ws: &mut futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) -> serde_json::Value {
    let deadline = Duration::from_secs(2);
    let msg = tokio::time::timeout(deadline, ws.next())
        .await
        .expect("timeout waiting for message")
        .expect("stream ended")
        .expect("ws error");
    match msg {
        Message::Text(text) => serde_json::from_str(&text).expect("invalid JSON"),
        other => panic!("expected text message, got {:?}", other),
    }
}

#[tokio::test]
async fn test_ws_method_get_input_mode() {
    let (state, _rx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (ws, _) = connect_async(format!("ws://{}/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    // Read "connected" message
    let msg = recv_json(&mut rx).await;
    assert_eq!(msg["connected"], true);

    // Send method call (no subscribe needed first!)
    tx.send(Message::Text(
        serde_json::json!({"id": 1, "method": "get_input_mode"}).to_string(),
    ))
    .await
    .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["method"], "get_input_mode");
    assert_eq!(resp["result"]["mode"], "passthrough");
}

#[tokio::test]
async fn test_ws_method_get_screen() {
    let (state, _rx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (ws, _) = connect_async(format!("ws://{}/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    let _ = recv_json(&mut rx).await; // connected

    tx.send(Message::Text(
        serde_json::json!({"method": "get_screen", "params": {"format": "plain"}}).to_string(),
    ))
    .await
    .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["method"], "get_screen");
    assert!(resp["result"]["cols"].is_number());
    assert!(resp["result"]["rows"].is_number());
}

#[tokio::test]
async fn test_ws_method_send_input() {
    let (state, mut input_rx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (ws, _) = connect_async(format!("ws://{}/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    let _ = recv_json(&mut rx).await; // connected

    tx.send(Message::Text(
        serde_json::json!({"method": "send_input", "params": {"data": "hello"}}).to_string(),
    ))
    .await
    .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["method"], "send_input");
    assert!(resp["result"].is_object());

    // Verify input reached the channel
    let received = tokio::time::timeout(Duration::from_secs(1), input_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.as_ref(), b"hello");
}

#[tokio::test]
async fn test_ws_subscribe_then_events() {
    let (state, _rx) = create_test_state();
    let broker_tx = state.output_rx.clone();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (ws, _) = connect_async(format!("ws://{}/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    let _ = recv_json(&mut rx).await; // connected

    // Subscribe
    tx.send(Message::Text(
        serde_json::json!({
            "method": "subscribe",
            "params": {"events": ["lines"], "format": "plain"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    // Should get subscribe response
    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["method"], "subscribe");
    assert!(resp["result"]["events"].is_array());

    // Should get sync event
    let sync = recv_json(&mut rx).await;
    assert_eq!(sync["event"], "sync");

    // Now push data and expect line events
    broker_tx.send(Bytes::from("Hello\r\n")).unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut found_line = false;
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(Message::Text(text)))) =
            tokio::time::timeout(Duration::from_millis(200), rx.next()).await
        {
            let json: serde_json::Value = serde_json::from_str(&text).unwrap();
            if json.get("event") == Some(&serde_json::json!("line")) {
                found_line = true;
                break;
            }
        }
    }
    assert!(found_line, "should receive line events after subscribing");
}

#[tokio::test]
async fn test_ws_unknown_method() {
    let (state, _rx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (ws, _) = connect_async(format!("ws://{}/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    let _ = recv_json(&mut rx).await; // connected

    tx.send(Message::Text(
        serde_json::json!({"method": "nonexistent"}).to_string(),
    ))
    .await
    .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["method"], "nonexistent");
    assert_eq!(resp["error"]["code"], "unknown_method");
}

#[tokio::test]
async fn test_ws_malformed_request() {
    let (state, _rx) = create_test_state();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (ws, _) = connect_async(format!("ws://{}/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    let _ = recv_json(&mut rx).await; // connected

    // Send JSON without method field
    tx.send(Message::Text(r#"{"id": 1}"#.to_string()))
        .await
        .unwrap();

    let resp = recv_json(&mut rx).await;
    assert_eq!(resp["error"]["code"], "invalid_request");
    // No method or id since parsing failed
}

#[tokio::test]
async fn test_ws_methods_interleaved_with_events() {
    let (state, _rx) = create_test_state();
    let broker_tx = state.output_rx.clone();
    let app = api::router(state, None);
    let addr = start_server(app).await;

    let (ws, _) = connect_async(format!("ws://{}/ws/json", addr))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    let _ = recv_json(&mut rx).await; // connected

    // Subscribe first
    tx.send(Message::Text(
        serde_json::json!({
            "method": "subscribe",
            "params": {"events": ["lines"], "format": "plain"}
        })
        .to_string(),
    ))
    .await
    .unwrap();

    let _ = recv_json(&mut rx).await; // subscribe response
    let _ = recv_json(&mut rx).await; // sync event

    // Now send a method call WHILE events could be flowing
    broker_tx.send(Bytes::from("data\r\n")).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    tx.send(Message::Text(
        serde_json::json!({"id": 42, "method": "get_input_mode"}).to_string(),
    ))
    .await
    .unwrap();

    // Collect messages until we see our response
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut found_response = false;
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(Message::Text(text)))) =
            tokio::time::timeout(Duration::from_millis(200), rx.next()).await
        {
            let json: serde_json::Value = serde_json::from_str(&text).unwrap();
            if json.get("method") == Some(&serde_json::json!("get_input_mode")) {
                assert_eq!(json["id"], 42);
                assert_eq!(json["result"]["mode"], "passthrough");
                found_response = true;
                break;
            }
            // Other messages (line events) are fine, skip them
        }
    }
    assert!(
        found_response,
        "should receive method response even while events are streaming"
    );
}
```

**Step 2: Run tests to verify they fail**

Run: `nix develop -c sh -c "cargo test --test ws_json_methods -- 2>&1 | head -30"`
Expected: FAIL — the current handler waits for Subscribe before entering the event loop

**Step 3: Rewrite `handle_ws_json`**

Replace the `handle_ws_json` function in `src/api/handlers.rs` with the new unified protocol handler. The key changes:

1. After sending `{"connected": true}`, go straight into the main loop (no subscribe-wait phase)
2. All incoming text messages are parsed as `WsRequest` and dispatched
3. The `subscribe` method is handled specially: it updates subscription state and returns a response, then triggers a sync event
4. Events stream independently based on current subscription state

```rust
async fn handle_ws_json(socket: WebSocket, state: AppState) {
    let (_guard, mut shutdown_rx) = state.shutdown.register();
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Send connected message
    let connected_msg = serde_json::json!({ "connected": true });
    if ws_tx
        .send(Message::Text(connected_msg.to_string()))
        .await
        .is_err()
    {
        return;
    }

    // Mutable subscription state (initially no subscription)
    let mut subscribed_types: Vec<crate::parser::events::EventType> = Vec::new();
    let mut sub_format = crate::parser::state::Format::default();

    // Subscribe to parser events (stream is always active, filtering is local)
    let mut events = Box::pin(state.parser.subscribe());

    // Input subscription (lazily created when EventType::Input is subscribed)
    let mut input_rx: Option<tokio::sync::broadcast::Receiver<crate::input::InputEvent>> = None;

    // Main event loop
    loop {
        tokio::select! {
            event = events.next() => {
                match event {
                    Some(event) if !subscribed_types.is_empty() => {
                        let should_send = match &event {
                            crate::parser::events::Event::Line { .. } => {
                                subscribed_types.contains(&EventType::Lines)
                            }
                            crate::parser::events::Event::Cursor { .. } => {
                                subscribed_types.contains(&EventType::Cursor)
                            }
                            crate::parser::events::Event::Mode { .. } => {
                                subscribed_types.contains(&EventType::Mode)
                            }
                            crate::parser::events::Event::Diff { .. } => {
                                subscribed_types.contains(&EventType::Diffs)
                            }
                            crate::parser::events::Event::Reset { .. }
                            | crate::parser::events::Event::Sync { .. } => true,
                        };

                        if should_send {
                            if let Ok(json) = serde_json::to_string(&event) {
                                if ws_tx.send(Message::Text(json)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    None => break,
                    _ => {} // No subscription active, discard
                }
            }

            input_event = async {
                match &mut input_rx {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match input_event {
                    Ok(event) => {
                        if let Ok(json) = serde_json::to_string(&event) {
                            if ws_tx.send(Message::Text(json)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        input_rx = None;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                }
            }

            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // Parse as WsRequest
                        let req = match serde_json::from_str::<super::ws_methods::WsRequest>(&text) {
                            Ok(req) => req,
                            Err(_e) => {
                                let err = super::ws_methods::WsResponse::protocol_error(
                                    "invalid_request",
                                    "Invalid JSON or missing 'method' field.",
                                );
                                if let Ok(json) = serde_json::to_string(&err) {
                                    let _ = ws_tx.send(Message::Text(json)).await;
                                }
                                continue;
                            }
                        };

                        // Handle subscribe specially (needs to update local state)
                        if req.method == "subscribe" {
                            let params_value = req.params.clone().unwrap_or(serde_json::Value::Object(Default::default()));
                            match serde_json::from_value::<super::ws_methods::SubscribeParams>(params_value) {
                                Ok(params) => {
                                    subscribed_types = params.events.clone();
                                    sub_format = params.format;

                                    // Set up input subscription if needed
                                    if subscribed_types.contains(&EventType::Input) {
                                        if input_rx.is_none() {
                                            input_rx = Some(state.input_broadcaster.subscribe());
                                        }
                                    } else {
                                        input_rx = None;
                                    }

                                    // Send response
                                    let event_names: Vec<String> = subscribed_types.iter()
                                        .map(|e| format!("{:?}", e).to_lowercase())
                                        .collect();
                                    let resp = super::ws_methods::WsResponse::success(
                                        req.id.clone(),
                                        "subscribe",
                                        serde_json::json!({"events": event_names}),
                                    );
                                    if let Ok(json) = serde_json::to_string(&resp) {
                                        if ws_tx.send(Message::Text(json)).await.is_err() {
                                            break;
                                        }
                                    }

                                    // Send sync event
                                    if let Ok(crate::parser::state::QueryResponse::Screen(screen)) = state
                                        .parser
                                        .query(crate::parser::state::Query::Screen { format: sub_format })
                                        .await
                                    {
                                        let scrollback_lines = screen.total_lines;
                                        let sync_event = crate::parser::events::Event::Sync {
                                            seq: 0,
                                            screen,
                                            scrollback_lines,
                                        };
                                        if let Ok(json) = serde_json::to_string(&sync_event) {
                                            if ws_tx.send(Message::Text(json)).await.is_err() {
                                                break;
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    let resp = super::ws_methods::WsResponse::error(
                                        req.id.clone(),
                                        "subscribe",
                                        "invalid_request",
                                        &format!("Invalid subscribe params: {}.", e),
                                    );
                                    if let Ok(json) = serde_json::to_string(&resp) {
                                        let _ = ws_tx.send(Message::Text(json)).await;
                                    }
                                }
                            }
                        } else {
                            // Dispatch all other methods
                            let resp = super::ws_methods::dispatch(&req, &state).await;
                            if let Ok(json) = serde_json::to_string(&resp) {
                                if ws_tx.send(Message::Text(json)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => continue,
                }
            }

            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::debug!("WebSocket handler received shutdown signal");
                    let close_frame = CloseFrame {
                        code: axum::extract::ws::close_code::NORMAL,
                        reason: "server shutting down".into(),
                    };
                    let _ = ws_tx.send(Message::Close(Some(close_frame))).await;
                    let _ = ws_tx.flush().await;
                    break;
                }
            }
        }
    }
}
```

Remove the `Subscribe` import from the top of handlers.rs (it's no longer used directly there), and add `EventType` to the import if not already present:

```rust
use crate::parser::{
    events::{Event, EventType},
    state::{Format, Query, QueryResponse},
};
```

**Step 4: Run the new integration tests**

Run: `nix develop -c sh -c "cargo test --test ws_json_methods"`
Expected: all tests PASS

**Step 5: Run ALL existing tests to check for regressions**

Run: `nix develop -c sh -c "cargo test"`
Expected: all tests PASS. The existing `test_websocket_line_event_includes_total_lines` test in `api_integration.rs` uses the old subscribe protocol (bare `{"events": [...]}` without `method`). This will now fail because the handler expects `WsRequest` format. Update that test to use the new protocol:

Change the subscribe message from:
```rust
let subscribe_msg = serde_json::json!({"events": ["lines"]});
```
to:
```rust
let subscribe_msg = serde_json::json!({"method": "subscribe", "params": {"events": ["lines"]}});
```

And update the response check — instead of reading a `{"subscribed": [...]}` message, it will now get `{"method": "subscribe", "result": {"events": [...]}}` followed by a sync event. Adjust accordingly: after sending subscribe, read messages until you find one with `"event": "line"` (the subscribe response and sync event will come first but the loop already skips non-line messages).

**Step 6: Run all tests again**

Run: `nix develop -c sh -c "cargo test"`
Expected: all tests PASS

**Step 7: Commit**

```bash
git add src/api/handlers.rs tests/ws_json_methods.rs tests/api_integration.rs
git commit -m "feat(ws): unified request/response protocol for /ws/json endpoint

Replaces the two-phase handshake (subscribe-then-events) with a unified
protocol where all client messages are method calls. Subscribe is now a
regular method. Requests and events coexist on the same connection.

Adds integration tests for method dispatch, subscribe, interleaved
events, malformed requests, and unknown methods."
```

---

### Task 7: Update OpenAPI spec and API documentation

Update the API documentation to reflect the new WebSocket protocol.

**Files:**
- Modify: `docs/api/openapi.yaml`
- Modify: `docs/api/README.md`

**Step 1: Update the OpenAPI spec**

Add a description of the WebSocket JSON protocol methods to the `/ws/json` endpoint documentation in `docs/api/openapi.yaml`. Document the request/response framing and list all available methods.

**Step 2: Update the README**

Add a "WebSocket Methods" section to `docs/api/README.md` documenting the unified protocol, message framing, available methods, and examples.

**Step 3: Run doc tests**

Run: `nix develop -c sh -c "cargo test --test api_integration -- test_openapi_spec_endpoint test_docs_endpoint"`
Expected: PASS (embedded docs still load correctly)

**Step 4: Commit**

```bash
git add docs/api/openapi.yaml docs/api/README.md
git commit -m "docs: update API documentation with WebSocket request/response protocol"
```

---

### Task 8: Final validation

Run the full test suite one more time to ensure everything works together.

**Step 1: Run all tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: all tests PASS

**Step 2: Run clippy**

Run: `nix develop -c sh -c "cargo clippy -- -D warnings"`
Expected: no warnings

**Step 3: Manual smoke test**

Start wsh, connect to `/ws/json` with websocat or similar, and verify:
1. `{"connected": true}` is received
2. `{"method": "get_screen"}` returns a screen response
3. `{"method": "subscribe", "params": {"events": ["lines"]}}` starts events
4. `{"method": "send_input", "params": {"data": "echo hi\r"}}` injects input
