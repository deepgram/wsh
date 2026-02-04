use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::{broadcast, mpsc};

#[derive(Clone)]
pub struct AppState {
    pub input_tx: mpsc::Sender<Bytes>,
    pub output_rx: broadcast::Sender<Bytes>,
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

async fn ws_raw(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_ws_raw(socket, state))
}

async fn handle_ws_raw(socket: WebSocket, state: AppState) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    let mut output_rx = state.output_rx.subscribe();
    let input_tx = state.input_tx.clone();

    // Task: broadcast PTY output -> WebSocket
    let mut tx_task = tokio::spawn(async move {
        while let Ok(data) = output_rx.recv().await {
            if ws_tx.send(Message::Binary(data.to_vec())).await.is_err() {
                break;
            }
        }
    });

    // Task: WebSocket input -> PTY
    let mut rx_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            let data = match msg {
                Message::Binary(data) => Bytes::from(data),
                Message::Text(text) => Bytes::from(text),
                Message::Close(_) => break,
                _ => continue,
            };
            if input_tx.send(data).await.is_err() {
                break;
            }
        }
    });

    // Wait for either task to finish, then abort the other
    tokio::select! {
        _ = &mut tx_task => rx_task.abort(),
        _ = &mut rx_task => tx_task.abort(),
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/input", post(input))
        .route("/ws/raw", get(ws_raw))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt; // for oneshot()

    /// Creates a test state and returns both the state and the input receiver.
    /// The receiver must be kept alive for the duration of the test to prevent
    /// send failures.
    fn create_test_state() -> (AppState, mpsc::Receiver<Bytes>) {
        let (input_tx, input_rx) = mpsc::channel(64);
        let (output_tx, _) = broadcast::channel(64);
        let state = AppState {
            input_tx,
            output_rx: output_tx,
        };
        (state, input_rx)
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let (state, _input_rx) = create_test_state();
        let app = router(state);

        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
    }

    #[tokio::test]
    async fn test_input_endpoint_success() {
        let (state, _input_rx) = create_test_state();
        let app = router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/input")
                    .body(Body::from("test input"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_input_endpoint_forwards_to_channel() {
        let (state, mut input_rx) = create_test_state();
        let app = router(state);

        let test_data = b"hello world";
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/input")
                    .body(Body::from(test_data.to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // Verify the data was forwarded to the channel
        let received = input_rx.recv().await.expect("should receive data");
        assert_eq!(received.as_ref(), test_data);
    }

    #[tokio::test]
    async fn test_router_has_correct_routes() {
        let (state, _input_rx) = create_test_state();
        let app = router(state);

        // Test /health exists (GET)
        let response = app
            .clone()
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Test /input exists (POST)
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/input")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // Test /ws/raw exists (GET) - will return upgrade required since we're not using WebSocket
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/ws/raw")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // WebSocket upgrade endpoints typically return a non-404 status when accessed without upgrade
        // This confirms the route exists
        assert_ne!(response.status(), StatusCode::NOT_FOUND);

        // Test non-existent route returns 404
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
