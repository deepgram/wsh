use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use serde::Serialize;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct AppState {
    pub input_tx: mpsc::Sender<Bytes>,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn input(State(state): State<AppState>, body: Bytes) -> StatusCode {
    match state.input_tx.send(body).await {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(e) => {
            tracing::error!("Failed to send input to PTY: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/input", post(input))
        .with_state(state)
}
