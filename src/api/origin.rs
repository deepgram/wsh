// WebSocket origin validation middleware (Phase 2).

use axum::{extract::Request, middleware::Next, response::Response};
use super::error::ApiError;

/// Check the Origin header on WebSocket upgrade requests.
///
/// When the server runs without auth (localhost), browsers can be tricked
/// into connecting via cross-origin WebSocket requests (CSWSH). This
/// middleware blocks such requests by validating the Origin header.
///
/// Logic:
/// - Non-WebSocket requests: pass through (CORS handles HTTP)
/// - No Origin header: pass through (non-browser clients like curl, agents)
/// - Origin matches allowed list: pass through
/// - Otherwise: reject with 403
pub async fn check_ws_origin(
    allowed_origins: Vec<String>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    // Only check WebSocket upgrade requests
    let is_ws_upgrade = req.headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);

    if !is_ws_upgrade {
        return Ok(next.run(req).await);
    }

    // No Origin header = non-browser client, allow
    let origin = match req.headers().get("origin").and_then(|v| v.to_str().ok()) {
        None => return Ok(next.run(req).await),
        Some(o) => o.to_string(),
    };

    // Check against allowed origins
    if allowed_origins.iter().any(|allowed| allowed == &origin) {
        return Ok(next.run(req).await);
    }

    Err(ApiError::OriginNotAllowed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    async fn ok_handler() -> &'static str { "ok" }

    fn test_app(allowed_origins: Vec<String>) -> Router {
        Router::new()
            .route("/ws", get(ok_handler))
            .route("/http", get(ok_handler))
            .layer(axum::middleware::from_fn(move |req, next| {
                let origins = allowed_origins.clone();
                check_ws_origin(origins, req, next)
            }))
    }

    #[tokio::test]
    async fn ws_upgrade_with_evil_origin_is_rejected() {
        let app = test_app(vec!["http://127.0.0.1:8080".to_string()]);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ws")
                    .header("upgrade", "websocket")
                    .header("origin", "http://evil.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn ws_upgrade_with_allowed_origin_passes() {
        let app = test_app(vec!["http://127.0.0.1:8080".to_string()]);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ws")
                    .header("upgrade", "websocket")
                    .header("origin", "http://127.0.0.1:8080")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ws_upgrade_without_origin_passes() {
        let app = test_app(vec!["http://127.0.0.1:8080".to_string()]);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ws")
                    .header("upgrade", "websocket")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn non_ws_request_with_any_origin_passes() {
        let app = test_app(vec!["http://127.0.0.1:8080".to_string()]);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/http")
                    .header("origin", "http://evil.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
