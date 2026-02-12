//! End-to-end tests for the wsh Unix socket client/server flow.
//!
//! These tests start a real server, connect real clients, and verify
//! the complete session lifecycle including creation, attach, I/O,
//! resize, detach/reattach, and multi-client scenarios.

use bytes::Bytes;
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::net::UnixStream;

use wsh::client::Client;
use wsh::parser::state::{Format, Query, QueryResponse};
use wsh::protocol::*;
use wsh::session::{SessionEvent, SessionRegistry};

/// Start a test server on a temporary socket and return the path and session registry.
///
/// The TempDir is intentionally leaked so the socket file survives the function scope.
async fn start_test_server() -> (PathBuf, SessionRegistry) {
    let sessions = SessionRegistry::new();
    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("test.sock");
    std::mem::forget(dir);
    let path = socket_path.clone();
    let sessions_clone = sessions.clone();

    tokio::spawn(async move {
        wsh::server::serve(sessions_clone, &socket_path)
            .await
            .unwrap();
    });

    // Wait for socket to appear
    for _ in 0..50 {
        if path.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(path.exists(), "server socket should exist at {:?}", path);

    (path, sessions)
}

// ── Test 1: Create session via Client ──────────────────────────────

#[tokio::test]
async fn test_create_session_via_client() {
    let (path, sessions) = start_test_server().await;

    let mut client = Client::connect(&path).await.unwrap();

    let msg = CreateSessionMsg {
        name: Some("e2e-create".to_string()),
        command: None,
        cwd: None,
        env: None,
        rows: 24,
        cols: 80,
    };
    let resp = client.create_session(msg).await.unwrap();

    assert_eq!(resp.name, "e2e-create");
    assert_eq!(resp.rows, 24);
    assert_eq!(resp.cols, 80);

    // Verify the session actually exists in the registry
    assert!(
        sessions.get("e2e-create").is_some(),
        "session should exist in registry after creation"
    );

    std::fs::remove_file(&path).ok();
}

// ── Test 2: Two sessions are independent ───────────────────────────

#[tokio::test]
async fn test_create_two_sessions_isolation() {
    let (path, sessions) = start_test_server().await;

    // Create first session
    let mut client1 = Client::connect(&path).await.unwrap();
    let resp1 = client1
        .create_session(CreateSessionMsg {
            name: Some("session-alpha".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        })
        .await
        .unwrap();

    // Create second session
    let mut client2 = Client::connect(&path).await.unwrap();
    let resp2 = client2
        .create_session(CreateSessionMsg {
            name: Some("session-beta".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 30,
            cols: 120,
        })
        .await
        .unwrap();

    assert_eq!(resp1.name, "session-alpha");
    assert_eq!(resp2.name, "session-beta");
    assert_ne!(resp1.name, resp2.name);

    // Both exist independently
    assert!(sessions.get("session-alpha").is_some());
    assert!(sessions.get("session-beta").is_some());
    assert_eq!(sessions.len(), 2);

    // They have different dimensions (confirming independent configs)
    assert_eq!(resp1.cols, 80);
    assert_eq!(resp2.cols, 120);

    std::fs::remove_file(&path).ok();
}

// ── Test 3: Attach to an existing session ──────────────────────────

#[tokio::test]
async fn test_attach_to_session() {
    let (path, _sessions) = start_test_server().await;

    // Create a session first
    let mut creator = Client::connect(&path).await.unwrap();
    let create_resp = creator
        .create_session(CreateSessionMsg {
            name: Some("attach-target".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        })
        .await
        .unwrap();
    assert_eq!(create_resp.name, "attach-target");

    // Attach from a second client
    let mut attacher = Client::connect(&path).await.unwrap();
    let attach_resp = attacher
        .attach(AttachSessionMsg {
            name: "attach-target".to_string(),
            scrollback: ScrollbackRequest::None,
            rows: 30,
            cols: 120,
        })
        .await
        .unwrap();

    assert_eq!(attach_resp.name, "attach-target");
    assert_eq!(attach_resp.rows, 30);
    assert_eq!(attach_resp.cols, 120);

    std::fs::remove_file(&path).ok();
}

// ── Test 4: Detach and reattach ────────────────────────────────────

#[tokio::test]
async fn test_detach_and_reattach() {
    let (path, sessions) = start_test_server().await;

    // Create a session via raw socket (so we can send Detach frame)
    let mut stream = UnixStream::connect(&path).await.unwrap();
    let create_msg = CreateSessionMsg {
        name: Some("detach-reattach".to_string()),
        command: None,
        cwd: None,
        env: None,
        rows: 24,
        cols: 80,
    };
    Frame::control(FrameType::CreateSession, &create_msg)
        .unwrap()
        .write_to(&mut stream)
        .await
        .unwrap();

    let resp_frame = Frame::read_from(&mut stream).await.unwrap();
    assert_eq!(resp_frame.frame_type, FrameType::CreateSessionResponse);

    // Send Detach frame to cleanly disconnect
    let detach = Frame::new(FrameType::Detach, Bytes::new());
    detach.write_to(&mut stream).await.unwrap();

    // Give server time to process detach
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Session should still be alive in the registry
    assert!(
        sessions.get("detach-reattach").is_some(),
        "session should survive client detach"
    );

    // Drop the old stream
    drop(stream);

    // Reattach with a new client
    let mut new_client = Client::connect(&path).await.unwrap();
    let attach_resp = new_client
        .attach(AttachSessionMsg {
            name: "detach-reattach".to_string(),
            scrollback: ScrollbackRequest::None,
            rows: 40,
            cols: 100,
        })
        .await
        .unwrap();

    assert_eq!(attach_resp.name, "detach-reattach");
    assert_eq!(attach_resp.rows, 40);
    assert_eq!(attach_resp.cols, 100);

    std::fs::remove_file(&path).ok();
}

// ── Test 5: Multiple clients on the same session ───────────────────

#[tokio::test]
async fn test_multiple_clients_same_session() {
    let (path, sessions) = start_test_server().await;

    // Client 1 creates the session
    let mut stream1 = UnixStream::connect(&path).await.unwrap();
    Frame::control(
        FrameType::CreateSession,
        &CreateSessionMsg {
            name: Some("multi-client".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        },
    )
    .unwrap()
    .write_to(&mut stream1)
    .await
    .unwrap();

    let resp1 = Frame::read_from(&mut stream1).await.unwrap();
    assert_eq!(resp1.frame_type, FrameType::CreateSessionResponse);

    // Client 2 attaches to the same session
    let mut stream2 = UnixStream::connect(&path).await.unwrap();
    Frame::control(
        FrameType::AttachSession,
        &AttachSessionMsg {
            name: "multi-client".to_string(),
            scrollback: ScrollbackRequest::None,
            rows: 24,
            cols: 80,
        },
    )
    .unwrap()
    .write_to(&mut stream2)
    .await
    .unwrap();

    let resp2 = Frame::read_from(&mut stream2).await.unwrap();
    assert_eq!(resp2.frame_type, FrameType::AttachSessionResponse);

    // Both clients are connected. Send input from client 1.
    let input = Frame::data(FrameType::StdinInput, Bytes::from("echo multi\n"));
    input.write_to(&mut stream1).await.unwrap();

    // Client 2 should receive PtyOutput with the echoed text
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut found = false;
    loop {
        match tokio::time::timeout_at(deadline, Frame::read_from(&mut stream2)).await {
            Ok(Ok(frame)) => {
                if frame.frame_type == FrameType::PtyOutput {
                    let output = String::from_utf8_lossy(&frame.payload);
                    if output.contains("multi") {
                        found = true;
                        break;
                    }
                }
            }
            _ => break,
        }
    }

    assert!(
        found,
        "client 2 should see output from input sent by client 1"
    );

    // The session should still be present
    assert!(sessions.get("multi-client").is_some());

    std::fs::remove_file(&path).ok();
}

// ── Test 6: Input/output round trip ────────────────────────────────

#[tokio::test]
async fn test_session_input_output_round_trip() {
    let (path, _sessions) = start_test_server().await;

    let mut stream = UnixStream::connect(&path).await.unwrap();

    // Create session with explicit bash command
    Frame::control(
        FrameType::CreateSession,
        &CreateSessionMsg {
            name: Some("io-roundtrip".to_string()),
            command: Some("bash".to_string()),
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        },
    )
    .unwrap()
    .write_to(&mut stream)
    .await
    .unwrap();

    let resp = Frame::read_from(&mut stream).await.unwrap();
    assert_eq!(resp.frame_type, FrameType::CreateSessionResponse);

    // Wait for the shell to start
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Send a command
    let input = Frame::data(
        FrameType::StdinInput,
        Bytes::from("echo roundtrip_test_marker\n"),
    );
    input.write_to(&mut stream).await.unwrap();

    // Read output frames until we see our marker
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut collected = String::new();
    let mut found = false;
    loop {
        match tokio::time::timeout_at(deadline, Frame::read_from(&mut stream)).await {
            Ok(Ok(frame)) => {
                if frame.frame_type == FrameType::PtyOutput {
                    collected.push_str(&String::from_utf8_lossy(&frame.payload));
                    if collected.contains("roundtrip_test_marker") {
                        found = true;
                        break;
                    }
                }
            }
            _ => break,
        }
    }

    assert!(
        found,
        "should find 'roundtrip_test_marker' in PTY output. Collected: {:?}",
        collected
    );

    std::fs::remove_file(&path).ok();
}

// ── Test 7: Resize via client ──────────────────────────────────────

#[tokio::test]
async fn test_client_resize() {
    let (path, sessions) = start_test_server().await;

    let mut stream = UnixStream::connect(&path).await.unwrap();

    // Create session
    Frame::control(
        FrameType::CreateSession,
        &CreateSessionMsg {
            name: Some("resize-e2e".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        },
    )
    .unwrap()
    .write_to(&mut stream)
    .await
    .unwrap();

    let resp = Frame::read_from(&mut stream).await.unwrap();
    assert_eq!(resp.frame_type, FrameType::CreateSessionResponse);

    // Send Resize frame
    let resize_msg = ResizeMsg {
        rows: 50,
        cols: 160,
    };
    Frame::control(FrameType::Resize, &resize_msg)
        .unwrap()
        .write_to(&mut stream)
        .await
        .unwrap();

    // Give time for resize to propagate
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Verify resize via parser state
    let session = sessions.get("resize-e2e").unwrap();
    let resp = session
        .parser
        .query(Query::Screen {
            format: Format::Plain,
        })
        .await
        .unwrap();
    if let QueryResponse::Screen(screen) = resp {
        assert_eq!(screen.cols, 160, "parser should report new cols");
        assert_eq!(screen.rows, 50, "parser should report new rows");
    } else {
        panic!("expected Screen response from parser query");
    }

    std::fs::remove_file(&path).ok();
}

// ── Test 8: Attach to nonexistent session ──────────────────────────

#[tokio::test]
async fn test_attach_nonexistent_session() {
    let (path, _sessions) = start_test_server().await;

    let mut client = Client::connect(&path).await.unwrap();

    let result = client
        .attach(AttachSessionMsg {
            name: "does-not-exist".to_string(),
            scrollback: ScrollbackRequest::None,
            rows: 24,
            cols: 80,
        })
        .await;

    // The server should close the connection (NotFound error),
    // and the Client should surface this as an io::Error.
    assert!(
        result.is_err(),
        "attaching to a nonexistent session should fail"
    );

    std::fs::remove_file(&path).ok();
}

// ── Test 9: Session survives client disconnect ─────────────────────

#[tokio::test]
async fn test_session_registry_after_client_disconnect() {
    let (path, sessions) = start_test_server().await;

    // Create session
    let mut client = Client::connect(&path).await.unwrap();
    let resp = client
        .create_session(CreateSessionMsg {
            name: Some("survive-disconnect".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        })
        .await
        .unwrap();
    assert_eq!(resp.name, "survive-disconnect");

    // Abruptly drop the client (simulates disconnect)
    drop(client);

    // Give server time to notice the disconnection
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Session should still be in the registry
    assert!(
        sessions.get("survive-disconnect").is_some(),
        "session should survive after client disconnects"
    );

    // We can even attach to it with a new client
    let mut new_client = Client::connect(&path).await.unwrap();
    let attach_resp = new_client
        .attach(AttachSessionMsg {
            name: "survive-disconnect".to_string(),
            scrollback: ScrollbackRequest::None,
            rows: 24,
            cols: 80,
        })
        .await
        .unwrap();
    assert_eq!(attach_resp.name, "survive-disconnect");

    std::fs::remove_file(&path).ok();
}

// ── Test 10: List sessions (empty) ──────────────────────────────────

#[tokio::test]
async fn test_list_sessions_empty() {
    let (path, _sessions) = start_test_server().await;

    let mut client = Client::connect(&path).await.unwrap();
    let list = client.list_sessions().await.unwrap();
    assert!(list.is_empty(), "no sessions should exist initially");

    std::fs::remove_file(&path).ok();
}

// ── Test 11: List sessions with entries ─────────────────────────────

#[tokio::test]
async fn test_list_sessions_with_entries() {
    let (path, _sessions) = start_test_server().await;

    // Create two sessions via the socket
    let mut c1 = Client::connect(&path).await.unwrap();
    c1.create_session(CreateSessionMsg {
        name: Some("list-alpha".to_string()),
        command: None,
        cwd: None,
        env: None,
        rows: 24,
        cols: 80,
    })
    .await
    .unwrap();

    let mut c2 = Client::connect(&path).await.unwrap();
    c2.create_session(CreateSessionMsg {
        name: Some("list-beta".to_string()),
        command: None,
        cwd: None,
        env: None,
        rows: 24,
        cols: 80,
    })
    .await
    .unwrap();

    // List sessions from a fresh client
    let mut lister = Client::connect(&path).await.unwrap();
    let list = lister.list_sessions().await.unwrap();

    assert_eq!(list.len(), 2);
    let names: Vec<&str> = list.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"list-alpha"), "should contain list-alpha");
    assert!(names.contains(&"list-beta"), "should contain list-beta");

    std::fs::remove_file(&path).ok();
}

// ── Test 12: Kill a session ─────────────────────────────────────────

#[tokio::test]
async fn test_kill_session_via_client() {
    let (path, sessions) = start_test_server().await;

    // Create a session
    let mut creator = Client::connect(&path).await.unwrap();
    creator
        .create_session(CreateSessionMsg {
            name: Some("kill-target".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        })
        .await
        .unwrap();

    assert!(sessions.get("kill-target").is_some());

    // Kill it from a different client
    let mut killer = Client::connect(&path).await.unwrap();
    killer.kill_session("kill-target").await.unwrap();

    // Verify it's gone
    assert!(
        sessions.get("kill-target").is_none(),
        "session should be removed after kill"
    );

    // List should be empty
    let mut lister = Client::connect(&path).await.unwrap();
    let list = lister.list_sessions().await.unwrap();
    assert!(list.is_empty(), "no sessions should remain after kill");

    std::fs::remove_file(&path).ok();
}

// ── Test 13: Kill nonexistent session ───────────────────────────────

#[tokio::test]
async fn test_kill_nonexistent_session() {
    let (path, _sessions) = start_test_server().await;

    let mut client = Client::connect(&path).await.unwrap();
    let result = client.kill_session("does-not-exist").await;

    assert!(
        result.is_err(),
        "killing a nonexistent session should fail"
    );

    std::fs::remove_file(&path).ok();
}

// ── Test 14: Session events are emitted ────────────────────────────

#[tokio::test]
async fn test_ephemeral_server_watches_sessions() {
    let (path, sessions) = start_test_server().await;

    // Subscribe to session events before creating
    let mut events_rx = sessions.subscribe_events();

    // Create a session via the socket
    let mut client = Client::connect(&path).await.unwrap();
    let resp = client
        .create_session(CreateSessionMsg {
            name: Some("event-watch".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        })
        .await
        .unwrap();
    assert_eq!(resp.name, "event-watch");

    // We should receive a Created event
    let event = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        events_rx.recv(),
    )
    .await
    .expect("should receive event within timeout")
    .expect("event channel should not be closed");

    assert!(
        matches!(event, SessionEvent::Created { ref name } if name == "event-watch"),
        "expected Created event for 'event-watch', got: {:?}",
        event
    );

    // Now remove it from the registry (simulating session exit cleanup)
    sessions.remove("event-watch");

    // We should receive a Destroyed event
    let event2 = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        events_rx.recv(),
    )
    .await
    .expect("should receive event within timeout")
    .expect("event channel should not be closed");

    // May receive an Exited event first (from child process), then Destroyed.
    // Drain until we see Destroyed.
    let mut saw_destroyed = matches!(
        event2,
        SessionEvent::Destroyed { ref name } if name == "event-watch"
    );

    if !saw_destroyed {
        // Try one more event
        if let Ok(Ok(event3)) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            events_rx.recv(),
        )
        .await
        {
            saw_destroyed = matches!(
                event3,
                SessionEvent::Destroyed { ref name } if name == "event-watch"
            );
        }
    }

    assert!(
        saw_destroyed,
        "should have received a Destroyed event after removing session"
    );

    std::fs::remove_file(&path).ok();
}
