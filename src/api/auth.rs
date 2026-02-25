use std::sync::Arc;

use axum::{extract::Request, middleware::Next, response::Response};
use subtle::ConstantTimeEq;

use super::error::ApiError;
use super::ticket::TicketStore;

/// Extract a Bearer token from the Authorization header.
fn extract_bearer(req: &Request) -> Option<String> {
    if let Some(auth_header) = req.headers().get("authorization") {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                return Some(token.to_string());
            }
        }
    }
    None
}

/// Extract a `?ticket=` value from the query string.
fn extract_ticket(req: &Request) -> Option<String> {
    req.uri().query().and_then(|query| {
        query.split('&').find_map(|pair| {
            pair.strip_prefix("ticket=").map(|v| v.to_string())
        })
    })
}

/// Check if this request is a WebSocket upgrade.
fn is_ws_upgrade(req: &Request) -> bool {
    req.headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
}

/// Auth middleware function.
///
/// Authentication flow:
/// 1. Try Bearer token from Authorization header
/// 2. If missing/invalid AND the request is a WebSocket upgrade, try `?ticket=` query param
///    against the TicketStore (single-use, 30s TTL)
/// 3. Otherwise reject
pub async fn require_auth(
    expected_token: String,
    ticket_store: Option<Arc<TicketStore>>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    // Try Bearer token first
    if let Some(ref token) = extract_bearer(&req) {
        if token.as_bytes().ct_eq(expected_token.as_bytes()).into() {
            return Ok(next.run(req).await);
        }
        return Err(ApiError::AuthInvalid);
    }

    // For WebSocket upgrades, try ticket-based auth
    if is_ws_upgrade(&req) {
        if let Some(ref store) = ticket_store {
            if let Some(ticket) = extract_ticket(&req) {
                if store.validate(&ticket) {
                    return Ok(next.run(req).await);
                }
            }
        }
    }

    Err(ApiError::AuthRequired)
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

    async fn ok_handler() -> &'static str {
        "ok"
    }

    fn test_app(token: String) -> Router {
        test_app_with_tickets(token, None)
    }

    fn test_app_with_tickets(token: String, store: Option<Arc<TicketStore>>) -> Router {
        Router::new()
            .route("/test", get(ok_handler))
            .layer(axum::middleware::from_fn(move |req, next| {
                let t = token.clone();
                let s = store.clone();
                async move { require_auth(t, s, req, next).await }
            }))
    }

    // ── extract_bearer tests ──────────────────────────────────────

    #[test]
    fn extract_bearer_with_header() {
        let req = Request::builder()
            .uri("/test")
            .header("authorization", "Bearer my-secret-token")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            extract_bearer(&req),
            Some("my-secret-token".to_string())
        );
    }

    #[test]
    fn extract_bearer_with_neither() {
        let req = Request::builder()
            .uri("/test")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_bearer(&req), None);
    }

    // ── extract_ticket tests ──────────────────────────────────────

    #[test]
    fn extract_ticket_from_query() {
        let req = Request::builder()
            .uri("/test?ticket=abc123")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_ticket(&req), Some("abc123".to_string()));
    }

    #[test]
    fn extract_ticket_missing() {
        let req = Request::builder()
            .uri("/test?other=value")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_ticket(&req), None);
    }

    // ── is_ws_upgrade tests ──────────────────────────────────────

    #[test]
    fn ws_upgrade_detected() {
        let req = Request::builder()
            .uri("/test")
            .header("upgrade", "websocket")
            .body(Body::empty())
            .unwrap();
        assert!(is_ws_upgrade(&req));
    }

    #[test]
    fn non_ws_not_detected() {
        let req = Request::builder()
            .uri("/test")
            .body(Body::empty())
            .unwrap();
        assert!(!is_ws_upgrade(&req));
    }

    // ── require_auth middleware tests ─────────────────────────────

    #[tokio::test]
    async fn require_auth_with_valid_token_returns_200() {
        let app = test_app("secret".to_string());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn require_auth_with_missing_token_returns_401() {
        let app = test_app("secret".to_string());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn require_auth_with_wrong_token_returns_403() {
        let app = test_app("secret".to_string());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("authorization", "Bearer wrong-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn query_token_no_longer_accepted() {
        let app = test_app("secret".to_string());

        // ?token= should NOT work anymore (removed)
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test?token=secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    // ── Ticket-based WS auth tests ───────────────────────────────

    #[tokio::test]
    async fn ticket_accepted_on_ws_upgrade() {
        let store = Arc::new(TicketStore::new());
        let ticket = store.create().unwrap();
        let app = test_app_with_tickets("secret".to_string(), Some(store));

        let response = app
            .oneshot(
                Request::builder()
                    .uri(&format!("/test?ticket={ticket}"))
                    .header("upgrade", "websocket")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ticket_rejected_on_non_ws_request() {
        let store = Arc::new(TicketStore::new());
        let ticket = store.create().unwrap();
        let app = test_app_with_tickets("secret".to_string(), Some(store));

        // Ticket without WS upgrade header should be rejected
        let response = app
            .oneshot(
                Request::builder()
                    .uri(&format!("/test?ticket={ticket}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn ticket_single_use() {
        let store = Arc::new(TicketStore::new());
        let ticket = store.create().unwrap();
        let app = test_app_with_tickets("secret".to_string(), Some(store));

        // First use succeeds
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&format!("/test?ticket={ticket}"))
                    .header("upgrade", "websocket")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Second use fails (consumed)
        let response = app
            .oneshot(
                Request::builder()
                    .uri(&format!("/test?ticket={ticket}"))
                    .header("upgrade", "websocket")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn invalid_ticket_rejected() {
        let store = Arc::new(TicketStore::new());
        let app = test_app_with_tickets("secret".to_string(), Some(store));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test?ticket=bogus")
                    .header("upgrade", "websocket")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
