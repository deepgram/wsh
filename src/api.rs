use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use crate::parser::{
    state::{Format, Query},
    Parser,
};
use crate::shutdown::ShutdownCoordinator;

#[derive(Clone)]
pub struct AppState {
    pub input_tx: mpsc::Sender<Bytes>,
    pub output_rx: broadcast::Sender<Bytes>,
    pub shutdown: ShutdownCoordinator,
    pub parser: Parser,
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
    // Register this connection for graceful shutdown tracking
    let (_guard, mut shutdown_rx) = state.shutdown.register();

    let (mut ws_tx, mut ws_rx) = socket.split();

    let mut output_rx = state.output_rx.subscribe();
    let input_tx = state.input_tx.clone();

    // Main loop: handle PTY output, WebSocket input, and shutdown signal
    loop {
        tokio::select! {
            // PTY output -> WebSocket
            result = output_rx.recv() => {
                match result {
                    Ok(data) => {
                        if ws_tx.send(Message::Binary(data.to_vec())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }

            // WebSocket input -> PTY
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        if input_tx.send(Bytes::from(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        if input_tx.send(Bytes::from(text)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => continue, // Ping/Pong handled automatically
                    Some(Err(_)) => break,
                }
            }

            // Shutdown signal
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::debug!("WebSocket received shutdown signal, closing");
                    // Send close frame to client
                    let close_frame = CloseFrame {
                        code: axum::extract::ws::close_code::NORMAL,
                        reason: "server shutting down".into(),
                    };
                    let _ = ws_tx.send(Message::Close(Some(close_frame))).await;
                    break;
                }
            }
        }
    }

    // _guard is dropped here, decrementing active connection count
}

#[derive(Deserialize)]
struct ScreenQuery {
    #[serde(default)]
    format: Format,
}

async fn screen(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<ScreenQuery>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let response = state
        .parser
        .query(Query::Screen { format: params.format })
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
        })?;

    Ok(Json(response))
}

#[derive(Deserialize)]
struct ScrollbackQuery {
    #[serde(default)]
    format: Format,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    100
}

async fn scrollback(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<ScrollbackQuery>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let response = state
        .parser
        .query(Query::Scrollback {
            format: params.format,
            offset: params.offset,
            limit: params.limit,
        })
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
        })?;

    Ok(Json(response))
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/input", post(input))
        .route("/ws/raw", get(ws_raw))
        .route("/screen", get(screen))
        .route("/scrollback", get(scrollback))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::Broker;
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
        let broker = Broker::new();
        let parser = Parser::spawn(&broker, 80, 24, 1000);
        let state = AppState {
            input_tx,
            output_rx: broker.sender(),
            shutdown: ShutdownCoordinator::new(),
            parser,
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
