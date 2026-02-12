use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

/// Structured error type for all API handlers.
///
/// Each variant maps to an HTTP status code, a machine-readable code string,
/// and a human-readable message. Implements [`IntoResponse`] so handlers can
/// return `Result<T, ApiError>` directly.
#[derive(Debug)]
pub enum ApiError {
    /// 401 - No authentication credentials provided.
    AuthRequired,
    /// 403 - Credentials provided but invalid.
    AuthInvalid,
    /// 404 - Generic not-found.
    NotFound,
    /// 404 - A specific overlay ID was not found.
    OverlayNotFound(String),
    /// 404 - A specific panel ID was not found.
    PanelNotFound(String),
    /// 400 - Malformed or invalid request.
    InvalidRequest(String),
    /// 400 - Invalid overlay specification.
    InvalidOverlay(String),
    /// 400 - Invalid input mode value.
    InvalidInputMode(String),
    /// 400 - Invalid format parameter.
    InvalidFormat(String),
    /// 404 - A specific session name was not found.
    SessionNotFound(String),
    /// 503 - Internal channel is full; back-pressure signal.
    ChannelFull,
    /// 503 - Terminal parser actor is unavailable.
    ParserUnavailable,
    /// 500 - Failed to write input to the PTY.
    InputSendFailed,
    /// 408 - Quiescence wait exceeded max_wait_ms deadline.
    QuiesceTimeout,
    /// 500 - Failed to create a session (PTY spawn error, etc.).
    SessionCreateFailed(String),
    /// 409 - Session name already exists.
    SessionNameConflict(String),
    /// 404 - No sessions exist in the registry.
    NoSessions,
    /// 500 - Catch-all internal error.
    InternalError(String),
}

impl ApiError {
    /// Returns the HTTP status code for this error variant.
    pub fn status_code(&self) -> StatusCode {
        match self {
            ApiError::AuthRequired => StatusCode::UNAUTHORIZED,
            ApiError::AuthInvalid => StatusCode::FORBIDDEN,
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::OverlayNotFound(_) => StatusCode::NOT_FOUND,
            ApiError::PanelNotFound(_) => StatusCode::NOT_FOUND,
            ApiError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::InvalidOverlay(_) => StatusCode::BAD_REQUEST,
            ApiError::InvalidInputMode(_) => StatusCode::BAD_REQUEST,
            ApiError::InvalidFormat(_) => StatusCode::BAD_REQUEST,
            ApiError::SessionNotFound(_) => StatusCode::NOT_FOUND,
            ApiError::ChannelFull => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::ParserUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::InputSendFailed => StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::QuiesceTimeout => StatusCode::REQUEST_TIMEOUT,
            ApiError::SessionCreateFailed(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::SessionNameConflict(_) => StatusCode::CONFLICT,
            ApiError::NoSessions => StatusCode::NOT_FOUND,
            ApiError::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Returns a machine-readable error code string.
    pub fn code(&self) -> &'static str {
        match self {
            ApiError::AuthRequired => "auth_required",
            ApiError::AuthInvalid => "auth_invalid",
            ApiError::NotFound => "not_found",
            ApiError::OverlayNotFound(_) => "overlay_not_found",
            ApiError::PanelNotFound(_) => "panel_not_found",
            ApiError::InvalidRequest(_) => "invalid_request",
            ApiError::InvalidOverlay(_) => "invalid_overlay",
            ApiError::InvalidInputMode(_) => "invalid_input_mode",
            ApiError::InvalidFormat(_) => "invalid_format",
            ApiError::SessionNotFound(_) => "session_not_found",
            ApiError::ChannelFull => "channel_full",
            ApiError::ParserUnavailable => "parser_unavailable",
            ApiError::InputSendFailed => "input_send_failed",
            ApiError::QuiesceTimeout => "quiesce_timeout",
            ApiError::SessionCreateFailed(_) => "session_create_failed",
            ApiError::SessionNameConflict(_) => "session_name_conflict",
            ApiError::NoSessions => "no_sessions",
            ApiError::InternalError(_) => "internal_error",
        }
    }

    /// Returns a human-readable error message.
    pub fn message(&self) -> String {
        match self {
            ApiError::AuthRequired => {
                "Authentication required. Provide a token via Authorization header or ?token= query parameter.".to_string()
            }
            ApiError::AuthInvalid => "Invalid authentication token.".to_string(),
            ApiError::NotFound => "Not found.".to_string(),
            ApiError::OverlayNotFound(id) => format!("No overlay exists with id '{}'.", id),
            ApiError::PanelNotFound(id) => format!("No panel exists with id '{}'.", id),
            ApiError::InvalidRequest(detail) => format!("Invalid request: {}.", detail),
            ApiError::InvalidOverlay(detail) => format!("Invalid overlay: {}.", detail),
            ApiError::InvalidInputMode(detail) => format!("Invalid input mode: {}.", detail),
            ApiError::InvalidFormat(detail) => format!("Invalid format: {}.", detail),
            ApiError::SessionNotFound(name) => format!("Session not found: {}.", name),
            ApiError::ChannelFull => "Server is overloaded. Try again shortly.".to_string(),
            ApiError::ParserUnavailable => "Terminal parser is unavailable.".to_string(),
            ApiError::InputSendFailed => "Failed to send input to terminal.".to_string(),
            ApiError::QuiesceTimeout => {
                "Terminal did not become quiescent within the deadline.".to_string()
            }
            ApiError::SessionCreateFailed(detail) => {
                format!("Failed to create session: {}.", detail)
            }
            ApiError::SessionNameConflict(name) => {
                format!("Session name already exists: {}.", name)
            }
            ApiError::NoSessions => "No sessions exist.".to_string(),
            ApiError::InternalError(detail) => format!("Internal error: {}.", detail),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({
            "error": {
                "code": self.code(),
                "message": self.message(),
            }
        });
        (self.status_code(), Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;

    /// Helper: convert an ApiError into a response and extract the status and
    /// parsed JSON body.
    async fn response_parts(err: ApiError) -> (StatusCode, serde_json::Value) {
        let response = err.into_response();
        let status = response.status();
        let body = Body::new(response.into_body())
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        (status, json)
    }

    // ── Status code tests ──────────────────────────────────────────

    #[tokio::test]
    async fn auth_required_status() {
        let (status, _) = response_parts(ApiError::AuthRequired).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_invalid_status() {
        let (status, _) = response_parts(ApiError::AuthInvalid).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn not_found_status() {
        let (status, _) = response_parts(ApiError::NotFound).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn overlay_not_found_status() {
        let (status, _) = response_parts(ApiError::OverlayNotFound("x".into())).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn invalid_request_status() {
        let (status, _) = response_parts(ApiError::InvalidRequest("x".into())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn invalid_overlay_status() {
        let (status, _) = response_parts(ApiError::InvalidOverlay("x".into())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn invalid_input_mode_status() {
        let (status, _) = response_parts(ApiError::InvalidInputMode("x".into())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn invalid_format_status() {
        let (status, _) = response_parts(ApiError::InvalidFormat("x".into())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn channel_full_status() {
        let (status, _) = response_parts(ApiError::ChannelFull).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn parser_unavailable_status() {
        let (status, _) = response_parts(ApiError::ParserUnavailable).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn input_send_failed_status() {
        let (status, _) = response_parts(ApiError::InputSendFailed).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn session_not_found_status() {
        let (status, _) = response_parts(ApiError::SessionNotFound("x".into())).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn internal_error_status() {
        let (status, _) = response_parts(ApiError::InternalError("x".into())).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    // ── Code string tests ──────────────────────────────────────────

    #[tokio::test]
    async fn auth_required_code() {
        let (_, json) = response_parts(ApiError::AuthRequired).await;
        assert_eq!(json["error"]["code"], "auth_required");
    }

    #[tokio::test]
    async fn auth_invalid_code() {
        let (_, json) = response_parts(ApiError::AuthInvalid).await;
        assert_eq!(json["error"]["code"], "auth_invalid");
    }

    #[tokio::test]
    async fn not_found_code() {
        let (_, json) = response_parts(ApiError::NotFound).await;
        assert_eq!(json["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn overlay_not_found_code() {
        let (_, json) = response_parts(ApiError::OverlayNotFound("id".into())).await;
        assert_eq!(json["error"]["code"], "overlay_not_found");
    }

    #[tokio::test]
    async fn invalid_request_code() {
        let (_, json) = response_parts(ApiError::InvalidRequest("d".into())).await;
        assert_eq!(json["error"]["code"], "invalid_request");
    }

    #[tokio::test]
    async fn invalid_overlay_code() {
        let (_, json) = response_parts(ApiError::InvalidOverlay("d".into())).await;
        assert_eq!(json["error"]["code"], "invalid_overlay");
    }

    #[tokio::test]
    async fn invalid_input_mode_code() {
        let (_, json) = response_parts(ApiError::InvalidInputMode("d".into())).await;
        assert_eq!(json["error"]["code"], "invalid_input_mode");
    }

    #[tokio::test]
    async fn invalid_format_code() {
        let (_, json) = response_parts(ApiError::InvalidFormat("d".into())).await;
        assert_eq!(json["error"]["code"], "invalid_format");
    }

    #[tokio::test]
    async fn channel_full_code() {
        let (_, json) = response_parts(ApiError::ChannelFull).await;
        assert_eq!(json["error"]["code"], "channel_full");
    }

    #[tokio::test]
    async fn parser_unavailable_code() {
        let (_, json) = response_parts(ApiError::ParserUnavailable).await;
        assert_eq!(json["error"]["code"], "parser_unavailable");
    }

    #[tokio::test]
    async fn input_send_failed_code() {
        let (_, json) = response_parts(ApiError::InputSendFailed).await;
        assert_eq!(json["error"]["code"], "input_send_failed");
    }

    #[tokio::test]
    async fn session_not_found_code() {
        let (_, json) = response_parts(ApiError::SessionNotFound("d".into())).await;
        assert_eq!(json["error"]["code"], "session_not_found");
    }

    #[tokio::test]
    async fn internal_error_code() {
        let (_, json) = response_parts(ApiError::InternalError("d".into())).await;
        assert_eq!(json["error"]["code"], "internal_error");
    }

    // ── Message content tests (parameterized variants) ─────────────

    #[tokio::test]
    async fn overlay_not_found_includes_id() {
        let (_, json) = response_parts(ApiError::OverlayNotFound("abc-123".into())).await;
        let msg = json["error"]["message"].as_str().unwrap();
        assert_eq!(msg, "No overlay exists with id 'abc-123'.");
    }

    #[tokio::test]
    async fn invalid_request_includes_detail() {
        let (_, json) =
            response_parts(ApiError::InvalidRequest("missing field 'x'".into())).await;
        let msg = json["error"]["message"].as_str().unwrap();
        assert_eq!(msg, "Invalid request: missing field 'x'.");
    }

    #[tokio::test]
    async fn invalid_overlay_includes_detail() {
        let (_, json) =
            response_parts(ApiError::InvalidOverlay("spans must not be empty".into())).await;
        let msg = json["error"]["message"].as_str().unwrap();
        assert_eq!(msg, "Invalid overlay: spans must not be empty.");
    }

    #[tokio::test]
    async fn invalid_input_mode_includes_detail() {
        let (_, json) =
            response_parts(ApiError::InvalidInputMode("unknown mode 'foo'".into())).await;
        let msg = json["error"]["message"].as_str().unwrap();
        assert_eq!(msg, "Invalid input mode: unknown mode 'foo'.");
    }

    #[tokio::test]
    async fn invalid_format_includes_detail() {
        let (_, json) =
            response_parts(ApiError::InvalidFormat("expected 'html' or 'text'".into())).await;
        let msg = json["error"]["message"].as_str().unwrap();
        assert_eq!(msg, "Invalid format: expected 'html' or 'text'.");
    }

    #[tokio::test]
    async fn session_not_found_includes_name() {
        let (_, json) = response_parts(ApiError::SessionNotFound("my-session".into())).await;
        let msg = json["error"]["message"].as_str().unwrap();
        assert_eq!(msg, "Session not found: my-session.");
    }

    #[tokio::test]
    async fn internal_error_includes_detail() {
        let (_, json) =
            response_parts(ApiError::InternalError("database timeout".into())).await;
        let msg = json["error"]["message"].as_str().unwrap();
        assert_eq!(msg, "Internal error: database timeout.");
    }

    #[tokio::test]
    async fn session_create_failed_status() {
        let (status, _) =
            response_parts(ApiError::SessionCreateFailed("pty error".into())).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn session_create_failed_code() {
        let (_, json) =
            response_parts(ApiError::SessionCreateFailed("pty error".into())).await;
        assert_eq!(json["error"]["code"], "session_create_failed");
    }

    #[tokio::test]
    async fn session_create_failed_includes_detail() {
        let (_, json) =
            response_parts(ApiError::SessionCreateFailed("pty error".into())).await;
        let msg = json["error"]["message"].as_str().unwrap();
        assert_eq!(msg, "Failed to create session: pty error.");
    }

    #[tokio::test]
    async fn session_name_conflict_status() {
        let (status, _) =
            response_parts(ApiError::SessionNameConflict("taken".into())).await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn session_name_conflict_code() {
        let (_, json) =
            response_parts(ApiError::SessionNameConflict("taken".into())).await;
        assert_eq!(json["error"]["code"], "session_name_conflict");
    }

    #[tokio::test]
    async fn session_name_conflict_includes_name() {
        let (_, json) =
            response_parts(ApiError::SessionNameConflict("taken".into())).await;
        let msg = json["error"]["message"].as_str().unwrap();
        assert_eq!(msg, "Session name already exists: taken.");
    }

    // ── JSON structure tests ───────────────────────────────────────

    #[tokio::test]
    async fn response_has_error_wrapper() {
        let (_, json) = response_parts(ApiError::NotFound).await;
        assert!(json.get("error").is_some(), "response must have 'error' key");
        assert!(
            json["error"].get("code").is_some(),
            "error must have 'code' key"
        );
        assert!(
            json["error"].get("message").is_some(),
            "error must have 'message' key"
        );
    }

    #[tokio::test]
    async fn response_content_type_is_json() {
        let response = ApiError::NotFound.into_response();
        let ct = response
            .headers()
            .get("content-type")
            .expect("response must have content-type header");
        assert!(
            ct.to_str().unwrap().contains("application/json"),
            "content-type must be application/json"
        );
    }

    // ── Fixed-message variant tests ────────────────────────────────

    #[tokio::test]
    async fn auth_required_message() {
        let (_, json) = response_parts(ApiError::AuthRequired).await;
        assert_eq!(
            json["error"]["message"],
            "Authentication required. Provide a token via Authorization header or ?token= query parameter."
        );
    }

    #[tokio::test]
    async fn auth_invalid_message() {
        let (_, json) = response_parts(ApiError::AuthInvalid).await;
        assert_eq!(json["error"]["message"], "Invalid authentication token.");
    }

    #[tokio::test]
    async fn not_found_message() {
        let (_, json) = response_parts(ApiError::NotFound).await;
        assert_eq!(json["error"]["message"], "Not found.");
    }

    #[tokio::test]
    async fn channel_full_message() {
        let (_, json) = response_parts(ApiError::ChannelFull).await;
        assert_eq!(
            json["error"]["message"],
            "Server is overloaded. Try again shortly."
        );
    }

    #[tokio::test]
    async fn parser_unavailable_message() {
        let (_, json) = response_parts(ApiError::ParserUnavailable).await;
        assert_eq!(
            json["error"]["message"],
            "Terminal parser is unavailable."
        );
    }

    #[tokio::test]
    async fn input_send_failed_message() {
        let (_, json) = response_parts(ApiError::InputSendFailed).await;
        assert_eq!(
            json["error"]["message"],
            "Failed to send input to terminal."
        );
    }
}
