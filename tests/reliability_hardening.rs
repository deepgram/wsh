//! Integration tests for reliability hardening features (Phases 3-7).
//!
//! These tests verify the defensive measures and resource limits added across
//! the reliability hardening implementation phases:
//!
//! - Phase 3b: Socket initial frame timeout
//! - Phase 4: Max session count enforcement (registry + HTTP)
//! - Phase 6: Session cancellation token on removal
//! - Phase 7b: Drain detaches and cancels sessions

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::sync::Arc;
use tower::ServiceExt;
use wsh::api::{router, AppState, RouterConfig, ServerConfig};
use wsh::session::{RegistryError, SessionRegistry};
use wsh::shutdown::ShutdownCoordinator;

// ── Phase 4: Max Session Count (registry level) ─────────────────────────────

#[tokio::test]
async fn test_max_sessions_enforced() {
    let registry = SessionRegistry::with_max_sessions(Some(2));

    // Insert first session -- should succeed
    let ts1 = common::create_test_session("sess-1");
    let name1 = registry.insert(Some("sess-1".to_string()), ts1.session);
    assert!(name1.is_ok(), "first session should be inserted");
    assert_eq!(name1.unwrap(), "sess-1");

    // Insert second session -- should succeed (at the limit)
    let ts2 = common::create_test_session("sess-2");
    let name2 = registry.insert(Some("sess-2".to_string()), ts2.session);
    assert!(name2.is_ok(), "second session should be inserted");
    assert_eq!(name2.unwrap(), "sess-2");

    // Insert third session -- should fail with MaxSessionsReached
    let ts3 = common::create_test_session("sess-3");
    let result = registry.insert(Some("sess-3".to_string()), ts3.session);
    assert!(result.is_err(), "third session should be rejected");
    assert!(
        matches!(result.unwrap_err(), RegistryError::MaxSessionsReached),
        "error should be MaxSessionsReached"
    );

    // Verify registry still has exactly 2 sessions
    assert_eq!(registry.len(), 2);
}

#[tokio::test]
async fn test_max_sessions_insert_and_get_respects_limit() {
    let registry = SessionRegistry::with_max_sessions(Some(1));

    // Insert one session via insert_and_get
    let ts1 = common::create_test_session("first");
    let result = registry.insert_and_get(Some("first".to_string()), ts1.session);
    assert!(result.is_ok(), "first session via insert_and_get should succeed");
    let (name, session) = result.unwrap();
    assert_eq!(name, "first");
    assert_eq!(session.name, "first");

    // Second insert_and_get should fail
    let ts2 = common::create_test_session("second");
    let result = registry.insert_and_get(Some("second".to_string()), ts2.session);
    assert!(result.is_err(), "second session via insert_and_get should be rejected");
    assert!(
        matches!(result.unwrap_err(), RegistryError::MaxSessionsReached),
        "error should be MaxSessionsReached"
    );
}

#[tokio::test]
async fn test_max_sessions_none_means_unlimited() {
    let registry = SessionRegistry::with_max_sessions(None);

    // Should be able to insert many sessions without limit
    for i in 0..10 {
        let name = format!("sess-{}", i);
        let ts = common::create_test_session(&name);
        let result = registry.insert(Some(name.clone()), ts.session);
        assert!(result.is_ok(), "session {} should be inserted with no limit", i);
    }
    assert_eq!(registry.len(), 10);
}

#[tokio::test]
async fn test_max_sessions_removal_frees_slot() {
    let registry = SessionRegistry::with_max_sessions(Some(1));

    // Fill the single slot
    let ts1 = common::create_test_session("first");
    registry.insert(Some("first".to_string()), ts1.session).unwrap();

    // Remove it
    let removed = registry.remove("first");
    assert!(removed.is_some());

    // Now we should be able to insert again
    let ts2 = common::create_test_session("second");
    let result = registry.insert(Some("second".to_string()), ts2.session);
    assert!(result.is_ok(), "should be able to insert after removal frees slot");
}

// ── Phase 4: Max Sessions via HTTP ──────────────────────────────────────────

#[tokio::test]
async fn test_max_sessions_http_503() {
    let registry = SessionRegistry::with_max_sessions(Some(1));
    let state = AppState {
        sessions: registry,
        shutdown: ShutdownCoordinator::new(),
        server_config: Arc::new(ServerConfig::new(false)),
            server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            ticket_store: std::sync::Arc::new(wsh::api::ticket::TicketStore::new()),
    };
    let app = router(state, RouterConfig::default());

    // Create first session -- should succeed with 201
    let body = serde_json::json!({"name": "only-one"});
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    // Create second session -- should fail with 503
    let body = serde_json::json!({"name": "too-many"});
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "second session creation should return 503"
    );

    // Verify the error body has the correct code
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "max_sessions_reached");
}

// ── Phase 6: Session Cancellation Token ─────────────────────────────────────

#[tokio::test]
async fn test_session_cancellation_on_remove() {
    let registry = SessionRegistry::new();

    let ts = common::create_test_session("cancel-me");
    registry
        .insert(Some("cancel-me".to_string()), ts.session)
        .unwrap();

    // Get the session and clone the cancellation token
    let session = registry.get("cancel-me").unwrap();
    let token = session.cancelled.clone();

    // Token should not be cancelled yet
    assert!(
        !token.is_cancelled(),
        "token should not be cancelled before removal"
    );

    // Remove the session
    let removed = registry.remove("cancel-me");
    assert!(removed.is_some());

    // Token should now be cancelled
    assert!(
        token.is_cancelled(),
        "token should be cancelled after session removal"
    );
}

#[tokio::test]
async fn test_session_cancellation_not_triggered_for_other_sessions() {
    let registry = SessionRegistry::new();

    let ts1 = common::create_test_session("keep");
    registry
        .insert(Some("keep".to_string()), ts1.session)
        .unwrap();

    let ts2 = common::create_test_session("remove");
    registry
        .insert(Some("remove".to_string()), ts2.session)
        .unwrap();

    // Get tokens for both
    let keep_token = registry.get("keep").unwrap().cancelled.clone();
    let remove_token = registry.get("remove").unwrap().cancelled.clone();

    // Remove only one session
    registry.remove("remove");

    // Only the removed session's token should be cancelled
    assert!(
        !keep_token.is_cancelled(),
        "kept session token should not be cancelled"
    );
    assert!(
        remove_token.is_cancelled(),
        "removed session token should be cancelled"
    );
}

// ── Phase 4: Scrollback Limit Cap ───────────────────────────────────────────

#[tokio::test]
async fn test_scrollback_large_limit_does_not_panic() {
    // Verify that querying scrollback with a very large limit does not
    // crash or panic. The actual cap enforcement is in the HTTP/MCP handler
    // layer, but the parser must handle large values gracefully.
    let (state, _, _, _ptx) = common::create_test_state();
    let app = router(state, RouterConfig::default());

    // Query scrollback with limit=100000 -- should succeed (capped or not)
    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions/test/scrollback?limit=100000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "scrollback query with large limit should succeed"
    );
}

// ── Phase 7b: Drain detaches and cancels sessions ───────────────────────────

#[tokio::test]
async fn test_drain_detaches_and_cancels() {
    let registry = SessionRegistry::new();

    // Insert a session
    let ts = common::create_test_session("drain-target");
    registry
        .insert(Some("drain-target".to_string()), ts.session)
        .unwrap();

    // Get references before drain
    let session = registry.get("drain-target").unwrap();
    let token = session.cancelled.clone();
    let mut detach_rx = session.detach_signal.subscribe();

    // Token and detach should not be triggered yet
    assert!(!token.is_cancelled());

    // Call drain
    registry.drain();

    // Verify detach signal was sent
    let detach_result = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        detach_rx.recv(),
    )
    .await;
    assert!(
        detach_result.is_ok(),
        "detach signal should be received after drain"
    );
    assert!(
        detach_result.unwrap().is_ok(),
        "detach signal should not be an error"
    );

    // Verify cancellation token was cancelled
    assert!(
        token.is_cancelled(),
        "cancelled token should be fired after drain"
    );

    // Verify registry is empty
    assert_eq!(registry.len(), 0, "registry should be empty after drain");
}

#[tokio::test]
async fn test_drain_multiple_sessions() {
    let registry = SessionRegistry::new();

    // Insert multiple sessions
    let mut tokens = Vec::new();
    for i in 0..3 {
        let name = format!("drain-multi-{}", i);
        let ts = common::create_test_session(&name);
        registry
            .insert(Some(name.clone()), ts.session)
            .unwrap();
        let session = registry.get(&name).unwrap();
        tokens.push(session.cancelled.clone());
    }
    assert_eq!(registry.len(), 3);

    // Drain all
    registry.drain();

    // All tokens should be cancelled
    for (i, token) in tokens.iter().enumerate() {
        assert!(
            token.is_cancelled(),
            "token for session {} should be cancelled after drain",
            i
        );
    }

    // Registry should be empty
    assert_eq!(registry.len(), 0);
}

#[tokio::test]
async fn test_drain_empty_registry_is_noop() {
    let registry = SessionRegistry::new();
    // Should not panic or error
    registry.drain();
    assert_eq!(registry.len(), 0);
}

// ── Phase 3b: Socket initial frame timeout ──────────────────────────────────

#[tokio::test]
async fn test_socket_initial_frame_timeout() {
    use tempfile::TempDir;
    use tokio::net::UnixStream;
    use wsh::protocol::Frame;

    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("test-timeout.sock");

    let sessions = SessionRegistry::new();
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();
    let path_clone = socket_path.clone();

    // Start the socket server
    tokio::spawn(async move {
        wsh::server::serve(sessions, &path_clone, cancel_clone, None, tokio_util::sync::CancellationToken::new())
            .await
            .unwrap();
    });

    // Wait for socket to appear
    for _ in 0..50 {
        if socket_path.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(socket_path.exists(), "socket should exist");

    // Connect but don't send anything
    let mut stream = UnixStream::connect(&socket_path).await.unwrap();

    // The server should close the connection after ~5 seconds (the timeout).
    // We try to read a frame; we should get an error (EOF or timeout).
    let start = tokio::time::Instant::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        Frame::read_from(&mut stream),
    )
    .await;

    let elapsed = start.elapsed();

    // The read should complete (either EOF or error) before our 8s outer timeout
    assert!(
        result.is_ok(),
        "frame read should complete before outer timeout (server should disconnect idle client)"
    );

    // The inner result should be an error (EOF / broken pipe)
    let inner = result.unwrap();
    assert!(
        inner.is_err(),
        "reading from a timed-out connection should return an error"
    );

    // It should have taken approximately 5 seconds (the server's timeout),
    // not our full 8 seconds
    assert!(
        elapsed.as_secs() >= 4 && elapsed.as_secs() <= 7,
        "timeout should occur around 5 seconds, took {:.1}s",
        elapsed.as_secs_f64()
    );

    // Clean up
    cancel.cancel();
}
