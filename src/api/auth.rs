use axum::{extract::Request, middleware::Next, response::Response};

use super::error::ApiError;

/// Extract token from request: check Authorization header first, then ?token= query param.
fn extract_token(req: &Request) -> Option<String> {
    // 1. Check Authorization: Bearer <token> header
    if let Some(auth_header) = req.headers().get("authorization") {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                return Some(token.to_string());
            }
        }
    }

    // 2. Check ?token=<token> query parameter
    if let Some(query) = req.uri().query() {
        for pair in query.split('&') {
            if let Some(value) = pair.strip_prefix("token=") {
                return Some(value.to_string());
            }
        }
    }

    None
}

/// Auth middleware function. Expected token is passed via from_fn closure.
pub async fn require_auth(
    expected_token: String,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    match extract_token(&req) {
        None => Err(ApiError::AuthRequired),
        Some(ref token) if token != &expected_token => Err(ApiError::AuthInvalid),
        Some(_) => Ok(next.run(req).await),
    }
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
        Router::new()
            .route("/test", get(ok_handler))
            .layer(axum::middleware::from_fn(move |req, next| {
                let t = token.clone();
                async move { require_auth(t, req, next).await }
            }))
    }

    // ── extract_token tests ──────────────────────────────────────

    #[test]
    fn extract_token_with_bearer_header() {
        let req = Request::builder()
            .uri("/test")
            .header("authorization", "Bearer my-secret-token")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            extract_token(&req),
            Some("my-secret-token".to_string())
        );
    }

    #[test]
    fn extract_token_with_query_param() {
        let req = Request::builder()
            .uri("/test?token=query-token")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            extract_token(&req),
            Some("query-token".to_string())
        );
    }

    #[test]
    fn extract_token_with_neither() {
        let req = Request::builder()
            .uri("/test")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_token(&req), None);
    }

    #[test]
    fn extract_token_bearer_takes_precedence_over_query() {
        let req = Request::builder()
            .uri("/test?token=query-token")
            .header("authorization", "Bearer bearer-token")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            extract_token(&req),
            Some("bearer-token".to_string())
        );
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
    async fn require_auth_with_valid_query_token_returns_200() {
        let app = test_app("secret".to_string());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test?token=secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
