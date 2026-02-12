use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "web-dist/"]
struct WebAssets;

pub async fn web_asset(uri: axum::http::Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');

    // Try the exact path first, then fall back to index.html for SPA routing
    let file = if path.is_empty() {
        WebAssets::get("index.html")
    } else {
        WebAssets::get(path).or_else(|| WebAssets::get("index.html"))
    };

    match file {
        Some(content) => {
            // Use the original path for MIME detection (not index.html fallback)
            let mime = if path.is_empty() || WebAssets::get(path).is_none() {
                "text/html".to_string()
            } else {
                mime_guess::from_path(path)
                    .first_or_text_plain()
                    .to_string()
            };

            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime)],
                content.data.to_vec(),
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_assets_includes_index_html() {
        let file = WebAssets::get("index.html");
        assert!(file.is_some(), "web-dist/index.html should be embedded");
    }

    #[test]
    fn web_assets_index_contains_html() {
        let file = WebAssets::get("index.html").unwrap();
        let content = std::str::from_utf8(&file.data).unwrap();
        assert!(content.contains("<html"), "index.html should contain HTML");
    }
}
