//! Integration tests for API endpoints.
//!
//! These tests verify that the HTTP API works correctly through the full router:
//! - Health endpoint returns expected response
//! - POST /input sends data through to the channel (simulating PTY input)
//! - WebSocket /ws/raw receives PTY output broadcasts
//! - WebSocket can send input that reaches the PTY channel

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tower::ServiceExt;
use wsh::api::{router, AppState};

/// Creates a test application with channels for input/output.
/// Returns the router, input receiver, and output sender for test verification.
fn create_test_app() -> (axum::Router, mpsc::Receiver<Bytes>, broadcast::Sender<Bytes>) {
    let (input_tx, input_rx) = mpsc::channel(64);
    let (output_tx, _) = broadcast::channel(64);
    let state = AppState {
        input_tx,
        output_rx: output_tx.clone(),
    };
    (router(state), input_rx, output_tx)
}

/// Starts the server on a random available port and returns the address.
async fn start_test_server(app: axum::Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Give the server a moment to start
    tokio::time::sleep(Duration::from_millis(10)).await;

    addr
}

#[tokio::test]
async fn test_full_api_health_check() {
    let (app, _input_rx, _output_tx) = create_test_app();

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
async fn test_api_input_to_pty() {
    let (app, mut input_rx, _output_tx) = create_test_app();

    let test_input = b"hello from API test";

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/input")
                .body(Body::from(test_input.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify the input was forwarded to the channel
    let received = tokio::time::timeout(Duration::from_secs(1), input_rx.recv())
        .await
        .expect("timed out waiting for input")
        .expect("channel closed unexpectedly");

    assert_eq!(received.as_ref(), test_input);
}

#[tokio::test]
async fn test_api_input_multiple_requests() {
    // Test that multiple sequential inputs are all forwarded correctly
    let (input_tx, mut input_rx) = mpsc::channel(64);
    let (output_tx, _) = broadcast::channel(64);
    let state = AppState {
        input_tx,
        output_rx: output_tx,
    };
    let app = router(state);

    let inputs = vec!["first input", "second input", "third input"];

    // Clone app for each request since oneshot consumes it
    for (i, input) in inputs.iter().enumerate() {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/input")
                    .body(Body::from(*input))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::NO_CONTENT,
            "Request {} failed",
            i
        );
    }

    // Verify all inputs were received in order
    for expected in inputs {
        let received = tokio::time::timeout(Duration::from_secs(1), input_rx.recv())
            .await
            .expect("timed out waiting for input")
            .expect("channel closed unexpectedly");

        assert_eq!(
            String::from_utf8_lossy(&received),
            expected,
            "Input mismatch"
        );
    }
}

#[tokio::test]
async fn test_websocket_upgrade_response() {
    // Test that /ws/raw endpoint exists and responds appropriately to non-upgrade requests
    let (app, _input_rx, _output_tx) = create_test_app();

    // A regular GET without upgrade headers should not return 404
    let response = app
        .oneshot(Request::builder().uri("/ws/raw").body(Body::empty()).unwrap())
        .await
        .unwrap();

    // WebSocket endpoints typically return an error status (not 404) when accessed
    // without proper upgrade headers
    assert_ne!(
        response.status(),
        StatusCode::NOT_FOUND,
        "WebSocket endpoint should exist"
    );
}

#[tokio::test]
async fn test_websocket_receives_pty_output() {
    let (input_tx, _input_rx) = mpsc::channel(64);
    let (output_tx, _) = broadcast::channel(64);
    let state = AppState {
        input_tx,
        output_rx: output_tx.clone(),
    };
    let app = router(state);

    let addr = start_test_server(app).await;
    let ws_url = format!("ws://{}/ws/raw", addr);

    // Connect WebSocket client
    let (mut ws_stream, _response) = connect_async(&ws_url)
        .await
        .expect("Failed to connect WebSocket");

    // Give the connection a moment to establish
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Simulate PTY output by publishing to the broadcast channel
    let test_output = Bytes::from("PTY output test data");
    output_tx
        .send(test_output.clone())
        .expect("Failed to send to broadcast channel");

    // Receive the message on the WebSocket
    let received = tokio::time::timeout(Duration::from_secs(2), ws_stream.next())
        .await
        .expect("timed out waiting for WebSocket message")
        .expect("WebSocket stream ended")
        .expect("WebSocket error");

    match received {
        Message::Binary(data) => {
            assert_eq!(data, test_output.to_vec(), "Received data mismatch");
        }
        other => panic!("Expected binary message, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_websocket_sends_input_to_pty() {
    let (input_tx, mut input_rx) = mpsc::channel(64);
    let (output_tx, _) = broadcast::channel(64);
    let state = AppState {
        input_tx,
        output_rx: output_tx,
    };
    let app = router(state);

    let addr = start_test_server(app).await;
    let ws_url = format!("ws://{}/ws/raw", addr);

    // Connect WebSocket client
    let (mut ws_stream, _response) = connect_async(&ws_url)
        .await
        .expect("Failed to connect WebSocket");

    // Give the connection a moment to establish
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send input via WebSocket
    let test_input = b"WebSocket input test";
    ws_stream
        .send(Message::Binary(test_input.to_vec()))
        .await
        .expect("Failed to send WebSocket message");

    // Verify the input was forwarded to the channel
    let received = tokio::time::timeout(Duration::from_secs(2), input_rx.recv())
        .await
        .expect("timed out waiting for input on channel")
        .expect("channel closed unexpectedly");

    assert_eq!(received.as_ref(), test_input);
}

#[tokio::test]
async fn test_websocket_text_input_to_pty() {
    // Test that text messages are also handled
    let (input_tx, mut input_rx) = mpsc::channel(64);
    let (output_tx, _) = broadcast::channel(64);
    let state = AppState {
        input_tx,
        output_rx: output_tx,
    };
    let app = router(state);

    let addr = start_test_server(app).await;
    let ws_url = format!("ws://{}/ws/raw", addr);

    let (mut ws_stream, _response) = connect_async(&ws_url)
        .await
        .expect("Failed to connect WebSocket");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send text input via WebSocket
    let test_text = "text message input";
    ws_stream
        .send(Message::Text(test_text.to_string()))
        .await
        .expect("Failed to send WebSocket text message");

    // Verify the input was forwarded to the channel
    let received = tokio::time::timeout(Duration::from_secs(2), input_rx.recv())
        .await
        .expect("timed out waiting for input on channel")
        .expect("channel closed unexpectedly");

    assert_eq!(String::from_utf8_lossy(&received), test_text);
}

#[tokio::test]
async fn test_websocket_bidirectional_communication() {
    // Test that WebSocket can both send and receive simultaneously
    let (input_tx, mut input_rx) = mpsc::channel(64);
    let (output_tx, _) = broadcast::channel(64);
    let state = AppState {
        input_tx,
        output_rx: output_tx.clone(),
    };
    let app = router(state);

    let addr = start_test_server(app).await;
    let ws_url = format!("ws://{}/ws/raw", addr);

    let (mut ws_stream, _response) = connect_async(&ws_url)
        .await
        .expect("Failed to connect WebSocket");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send input via WebSocket
    let test_input = b"bidirectional input";
    ws_stream
        .send(Message::Binary(test_input.to_vec()))
        .await
        .expect("Failed to send WebSocket message");

    // Simulate PTY output
    let test_output = Bytes::from("bidirectional output");
    output_tx
        .send(test_output.clone())
        .expect("Failed to send broadcast");

    // Verify input was received on the channel
    let received_input = tokio::time::timeout(Duration::from_secs(2), input_rx.recv())
        .await
        .expect("timed out waiting for input")
        .expect("channel closed");
    assert_eq!(received_input.as_ref(), test_input);

    // Verify output was received on WebSocket
    let received_output = tokio::time::timeout(Duration::from_secs(2), ws_stream.next())
        .await
        .expect("timed out waiting for WebSocket message")
        .expect("WebSocket stream ended")
        .expect("WebSocket error");

    match received_output {
        Message::Binary(data) => {
            assert_eq!(data, test_output.to_vec());
        }
        other => panic!("Expected binary message, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_websocket_multiple_outputs() {
    // Test that multiple PTY outputs are all received by WebSocket
    let (input_tx, _input_rx) = mpsc::channel(64);
    let (output_tx, _) = broadcast::channel(64);
    let state = AppState {
        input_tx,
        output_rx: output_tx.clone(),
    };
    let app = router(state);

    let addr = start_test_server(app).await;
    let ws_url = format!("ws://{}/ws/raw", addr);

    let (mut ws_stream, _response) = connect_async(&ws_url)
        .await
        .expect("Failed to connect WebSocket");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send multiple outputs
    let outputs = vec![
        Bytes::from("first output"),
        Bytes::from("second output"),
        Bytes::from("third output"),
    ];

    for output in &outputs {
        output_tx.send(output.clone()).expect("Failed to send");
    }

    // Receive all outputs
    for expected in outputs {
        let received = tokio::time::timeout(Duration::from_secs(2), ws_stream.next())
            .await
            .expect("timed out waiting for WebSocket message")
            .expect("WebSocket stream ended")
            .expect("WebSocket error");

        match received {
            Message::Binary(data) => {
                assert_eq!(data, expected.to_vec());
            }
            other => panic!("Expected binary message, got: {:?}", other),
        }
    }
}

#[tokio::test]
async fn test_nonexistent_route_returns_404() {
    let (app, _input_rx, _output_tx) = create_test_app();

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

#[tokio::test]
async fn test_input_wrong_method_returns_error() {
    let (app, _input_rx, _output_tx) = create_test_app();

    // GET on /input should fail (only POST is allowed)
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/input")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn test_health_wrong_method_returns_error() {
    let (app, _input_rx, _output_tx) = create_test_app();

    // POST on /health should fail (only GET is allowed)
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}
