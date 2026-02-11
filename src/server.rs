//! Unix socket server for wsh client/server communication.
//!
//! Listens on a Unix domain socket and handles CLI client connections.
//! Each client sends an initial control frame (CreateSession or AttachSession),
//! then enters a streaming loop forwarding I/O between the client's terminal
//! and the server-managed PTY session.

use std::io;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::UnixListener;
use tracing;

use crate::protocol::*;
use crate::pty::SpawnCommand;
use crate::session::{Session, SessionRegistry};

/// Start the Unix socket server, listening for CLI client connections.
///
/// This function runs until the listener is shut down (e.g. by dropping the
/// `UnixListener` or receiving a shutdown signal).
pub async fn serve(
    sessions: SessionRegistry,
    socket_path: &Path,
) -> io::Result<()> {
    // Remove stale socket file if it exists, but check for active server first
    if socket_path.exists() {
        match std::os::unix::net::UnixStream::connect(socket_path) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("another server is already listening on {}", socket_path.display()),
                ));
            }
            Err(_) => {
                // Socket exists but no server is listening — stale, safe to remove
                std::fs::remove_file(socket_path)?;
            }
        }
    }

    // Ensure parent directory exists
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(socket_path)?;

    // Restrict socket permissions to owner only (0600)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    }

    tracing::info!(path = %socket_path.display(), "Unix socket server listening");

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let sessions = sessions.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream, sessions).await {
                        tracing::debug!(?e, "client connection ended");
                    }
                });
            }
            Err(e) => {
                tracing::error!(?e, "failed to accept Unix socket connection");
            }
        }
    }
}

/// Compute the default Unix socket path for this user.
pub fn default_socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/tmp/wsh-{}", whoami()));
    PathBuf::from(runtime_dir).join("wsh.sock")
}

fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Handle a single client connection.
async fn handle_client<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    sessions: SessionRegistry,
) -> io::Result<()> {
    // Read initial control frame
    let frame = Frame::read_from(&mut stream).await?;

    match frame.frame_type {
        FrameType::CreateSession => {
            let msg: CreateSessionMsg = frame.parse_json().map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, e)
            })?;
            handle_create_session(&mut stream, sessions, msg).await
        }
        FrameType::AttachSession => {
            let msg: AttachSessionMsg = frame.parse_json().map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, e)
            })?;
            handle_attach_session(&mut stream, sessions, msg).await
        }
        other => {
            let err = ErrorMsg {
                code: "invalid_initial_frame".to_string(),
                message: format!("expected CreateSession or AttachSession, got {:?}", other),
            };
            let frame = Frame::control(FrameType::Error, &err)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            frame.write_to(&mut stream).await?;
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid initial frame type",
            ))
        }
    }
}

/// Handle a CreateSession request: spawn a new session and enter streaming.
async fn handle_create_session<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    sessions: SessionRegistry,
    msg: CreateSessionMsg,
) -> io::Result<()> {
    let command = match &msg.command {
        Some(cmd) => SpawnCommand::Command {
            command: cmd.clone(),
            interactive: true,
        },
        None => SpawnCommand::default(),
    };

    let (session, child_exit_rx) = Session::spawn_with_options(
        msg.name.clone().unwrap_or_default(),
        command,
        msg.rows,
        msg.cols,
        msg.cwd,
        msg.env,
    )
    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    let name = sessions.insert(msg.name, session.clone()).map_err(|e| {
        io::Error::new(io::ErrorKind::AlreadyExists, e.to_string())
    })?;

    sessions.monitor_child_exit(name.clone(), child_exit_rx);

    // Send response
    let resp = CreateSessionResponseMsg {
        name: name.clone(),
        rows: msg.rows,
        cols: msg.cols,
    };
    let resp_frame = Frame::control(FrameType::CreateSessionResponse, &resp)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    resp_frame.write_to(stream).await?;

    tracing::info!(session = %name, "client created session");

    // Enter streaming loop
    run_streaming(stream, &session).await
}

/// Handle an AttachSession request: look up session and enter streaming.
async fn handle_attach_session<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    sessions: SessionRegistry,
    msg: AttachSessionMsg,
) -> io::Result<()> {
    let session = match sessions.get(&msg.name) {
        Some(s) => s,
        None => {
            let err = ErrorMsg {
                code: "session_not_found".to_string(),
                message: format!("session not found: {}", msg.name),
            };
            if let Ok(frame) = Frame::control(FrameType::Error, &err) {
                let _ = frame.write_to(stream).await;
            }
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("session not found: {}", msg.name),
            ));
        }
    };

    // Resize session to match client terminal
    if let Err(e) = session.pty.resize(msg.rows, msg.cols) {
        tracing::warn!(?e, "failed to resize PTY on attach");
    }
    if let Err(e) = session.parser.resize(msg.cols as usize, msg.rows as usize).await {
        tracing::warn!(?e, "failed to resize parser on attach");
    }

    // Build scrollback/screen data for replay
    use crate::parser::state::{Format, Query, QueryResponse};
    let scrollback_data = match msg.scrollback {
        ScrollbackRequest::None => Vec::new(),
        ScrollbackRequest::All | ScrollbackRequest::Lines(_) => {
            let limit = match msg.scrollback {
                ScrollbackRequest::Lines(n) => n,
                _ => usize::MAX,
            };
            match session.parser.query(Query::Scrollback {
                format: Format::Plain,
                offset: 0,
                limit,
            }).await {
                Ok(QueryResponse::Scrollback(sb)) => {
                    let mut buf = String::new();
                    for line in &sb.lines {
                        if let crate::parser::state::FormattedLine::Plain(text) = line {
                            buf.push_str(text);
                            buf.push_str("\r\n");
                        }
                    }
                    buf.into_bytes()
                }
                _ => Vec::new(),
            }
        }
    };

    let screen_data = match session.parser.query(Query::Screen {
        format: Format::Plain,
    }).await {
        Ok(QueryResponse::Screen(screen)) => {
            let mut buf = String::new();
            // Clear screen and home cursor before replaying
            buf.push_str("\x1b[H\x1b[2J");
            for (i, line) in screen.lines.iter().enumerate() {
                if let crate::parser::state::FormattedLine::Plain(text) = line {
                    buf.push_str(text);
                    if i + 1 < screen.lines.len() {
                        buf.push_str("\r\n");
                    }
                }
            }
            // Restore cursor position
            buf.push_str(&format!(
                "\x1b[{};{}H",
                screen.cursor.row + 1,
                screen.cursor.col + 1,
            ));
            buf.into_bytes()
        }
        _ => Vec::new(),
    };

    let resp = AttachSessionResponseMsg {
        name: msg.name.clone(),
        rows: msg.rows,
        cols: msg.cols,
        scrollback: scrollback_data,
        screen: screen_data,
    };
    let resp_frame = Frame::control(FrameType::AttachSessionResponse, &resp)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    resp_frame.write_to(stream).await?;

    tracing::info!(session = %msg.name, "client attached to session");

    // Enter streaming loop
    run_streaming(stream, &session).await
}

/// Main streaming loop: proxy I/O between the client and the session.
///
/// - Client → Server: StdinInput frames are forwarded to session.input_tx
/// - Server → Client: Session broker output is forwarded as PtyOutput frames
/// - Client → Server: Resize frames resize the PTY and parser
/// - Client → Server: Detach frame ends the loop cleanly
async fn run_streaming<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    session: &Session,
) -> io::Result<()> {
    let (mut reader, mut writer) = tokio::io::split(stream);

    // Subscribe to session output
    let mut output_rx = session.output_rx.subscribe();

    let input_tx = session.input_tx.clone();
    let pty = session.pty.clone();
    let parser = session.parser.clone();
    let activity = session.activity.clone();

    // Main loop: read from client and session output concurrently
    loop {
        tokio::select! {
            // Output from session → client
            result = output_rx.recv() => {
                match result {
                    Ok(data) => {
                        let frame = Frame::data(FrameType::PtyOutput, data);
                        if frame.write_to(&mut writer).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }

            // Frames from client
            result = Frame::read_from(&mut reader) => {
                match result {
                    Ok(f) => {
                        match f.frame_type {
                            FrameType::StdinInput => {
                                activity.touch();
                                if input_tx.send(f.payload).await.is_err() {
                                    break;
                                }
                            }
                            FrameType::Resize => {
                                if let Ok(msg) = f.parse_json::<ResizeMsg>() {
                                    if let Err(e) = pty.resize(msg.rows, msg.cols) {
                                        tracing::warn!(?e, "failed to resize PTY");
                                    }
                                    if let Err(e) = parser.resize(
                                        msg.cols as usize,
                                        msg.rows as usize,
                                    ).await {
                                        tracing::warn!(?e, "failed to resize parser");
                                    }
                                }
                            }
                            FrameType::Detach => {
                                tracing::debug!("client detached");
                                break;
                            }
                            _ => {
                                tracing::warn!(frame_type = ?f.frame_type, "unexpected frame type in streaming mode");
                            }
                        }
                    }
                    Err(_) => break, // client disconnected
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use tokio::net::UnixStream;
    use tempfile::TempDir;

    /// Start a test server on a temporary socket and return the path.
    /// The TempDir is leaked to keep the directory alive for the test.
    async fn start_test_server(sessions: SessionRegistry) -> PathBuf {
        let dir = TempDir::new().unwrap();
        let socket_path = dir.path().join("test.sock");
        // Leak the TempDir so it stays alive for the duration of the test
        std::mem::forget(dir);
        let path = socket_path.clone();

        tokio::spawn(async move {
            serve(sessions, &socket_path).await.unwrap();
        });

        // Wait for socket to appear
        for _ in 0..50 {
            if path.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        path
    }

    #[tokio::test]
    async fn test_create_session_via_socket() {
        let sessions = SessionRegistry::new();
        let path = start_test_server(sessions.clone()).await;

        let mut stream = UnixStream::connect(&path).await.unwrap();

        // Send CreateSession
        let msg = CreateSessionMsg {
            name: Some("test-create".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        };
        let frame = Frame::control(FrameType::CreateSession, &msg).unwrap();
        frame.write_to(&mut stream).await.unwrap();

        // Read response
        let resp_frame = Frame::read_from(&mut stream).await.unwrap();
        assert_eq!(resp_frame.frame_type, FrameType::CreateSessionResponse);
        let resp: CreateSessionResponseMsg = resp_frame.parse_json().unwrap();
        assert_eq!(resp.name, "test-create");
        assert_eq!(resp.rows, 24);
        assert_eq!(resp.cols, 80);

        // Session should exist in registry
        assert!(sessions.get("test-create").is_some());

        // Clean up
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_attach_session_via_socket() {
        let sessions = SessionRegistry::new();

        // Pre-create a session
        let (session, child_exit_rx) = Session::spawn(
            "attach-target".to_string(),
            SpawnCommand::default(),
            24,
            80,
        )
        .unwrap();
        sessions.insert(Some("attach-target".to_string()), session).unwrap();
        sessions.monitor_child_exit("attach-target".to_string(), child_exit_rx);

        let path = start_test_server(sessions.clone()).await;

        let mut stream = UnixStream::connect(&path).await.unwrap();

        // Send AttachSession
        let msg = AttachSessionMsg {
            name: "attach-target".to_string(),
            scrollback: ScrollbackRequest::None,
            rows: 30,
            cols: 120,
        };
        let frame = Frame::control(FrameType::AttachSession, &msg).unwrap();
        frame.write_to(&mut stream).await.unwrap();

        // Read response
        let resp_frame = Frame::read_from(&mut stream).await.unwrap();
        assert_eq!(resp_frame.frame_type, FrameType::AttachSessionResponse);
        let resp: AttachSessionResponseMsg = resp_frame.parse_json().unwrap();
        assert_eq!(resp.name, "attach-target");

        // Clean up
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_attach_nonexistent_session_returns_error() {
        let sessions = SessionRegistry::new();
        let path = start_test_server(sessions).await;

        let mut stream = UnixStream::connect(&path).await.unwrap();

        let msg = AttachSessionMsg {
            name: "nonexistent".to_string(),
            scrollback: ScrollbackRequest::None,
            rows: 24,
            cols: 80,
        };
        let frame = Frame::control(FrameType::AttachSession, &msg).unwrap();
        frame.write_to(&mut stream).await.unwrap();

        // Server should send an Error frame before closing
        let resp = Frame::read_from(&mut stream).await.unwrap();
        assert_eq!(resp.frame_type, FrameType::Error);
        let err: ErrorMsg = resp.parse_json().unwrap();
        assert_eq!(err.code, "session_not_found");

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_stdin_forwarding() {
        let sessions = SessionRegistry::new();
        let path = start_test_server(sessions.clone()).await;

        let mut stream = UnixStream::connect(&path).await.unwrap();

        // Create a session
        let msg = CreateSessionMsg {
            name: Some("stdin-test".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        };
        let frame = Frame::control(FrameType::CreateSession, &msg).unwrap();
        frame.write_to(&mut stream).await.unwrap();

        // Read CreateSessionResponse
        let _resp = Frame::read_from(&mut stream).await.unwrap();

        // Send stdin input
        let input_frame = Frame::data(FrameType::StdinInput, Bytes::from("echo hello\n"));
        input_frame.write_to(&mut stream).await.unwrap();

        // We should receive PtyOutput frames
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut received_output = false;
        loop {
            match tokio::time::timeout_at(deadline, Frame::read_from(&mut stream)).await {
                Ok(Ok(frame)) => {
                    if frame.frame_type == FrameType::PtyOutput {
                        received_output = true;
                        let output = String::from_utf8_lossy(&frame.payload);
                        if output.contains("hello") {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
        assert!(received_output, "should have received PTY output");

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_detach_ends_streaming() {
        let sessions = SessionRegistry::new();
        let path = start_test_server(sessions.clone()).await;

        let mut stream = UnixStream::connect(&path).await.unwrap();

        // Create session
        let msg = CreateSessionMsg {
            name: Some("detach-test".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        };
        Frame::control(FrameType::CreateSession, &msg)
            .unwrap()
            .write_to(&mut stream)
            .await
            .unwrap();
        let _resp = Frame::read_from(&mut stream).await.unwrap();

        // Send Detach
        let detach_frame = Frame::new(FrameType::Detach, Bytes::new());
        detach_frame.write_to(&mut stream).await.unwrap();

        // After detach, the server should close the connection.
        // The session should still exist in the registry.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(sessions.get("detach-test").is_some());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_resize_forwarding() {
        let sessions = SessionRegistry::new();
        let path = start_test_server(sessions.clone()).await;

        let mut stream = UnixStream::connect(&path).await.unwrap();

        // Create session
        let msg = CreateSessionMsg {
            name: Some("resize-test".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        };
        Frame::control(FrameType::CreateSession, &msg)
            .unwrap()
            .write_to(&mut stream)
            .await
            .unwrap();
        let _resp = Frame::read_from(&mut stream).await.unwrap();

        // Send Resize
        let resize_msg = ResizeMsg { rows: 40, cols: 120 };
        Frame::control(FrameType::Resize, &resize_msg)
            .unwrap()
            .write_to(&mut stream)
            .await
            .unwrap();

        // Give it time to process
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Query the parser to verify resize took effect
        let session = sessions.get("resize-test").unwrap();
        use crate::parser::state::{Format, Query, QueryResponse};
        let resp = session.parser.query(Query::Screen { format: Format::Plain }).await.unwrap();
        if let QueryResponse::Screen(screen) = resp {
            assert_eq!(screen.cols, 120);
            assert_eq!(screen.rows, 40);
        } else {
            panic!("expected Screen response");
        }

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_invalid_initial_frame() {
        let sessions = SessionRegistry::new();
        let path = start_test_server(sessions).await;

        let mut stream = UnixStream::connect(&path).await.unwrap();

        // Send a PtyOutput frame as the initial frame (invalid)
        let frame = Frame::data(FrameType::PtyOutput, Bytes::from("invalid"));
        frame.write_to(&mut stream).await.unwrap();

        // Should receive an Error frame
        let resp = Frame::read_from(&mut stream).await.unwrap();
        assert_eq!(resp.frame_type, FrameType::Error);
        let err: ErrorMsg = resp.parse_json().unwrap();
        assert_eq!(err.code, "invalid_initial_frame");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_default_socket_path() {
        let path = default_socket_path();
        assert!(path.to_str().unwrap().contains("wsh.sock"));
    }

    /// Helper: create a session via the socket, send some input to generate
    /// scrollback, wait for it to be processed, and return the socket path
    /// and session name for subsequent attach tests.
    async fn create_session_with_output(
        _sessions: &SessionRegistry,
        path: &Path,
        session_name: &str,
    ) -> UnixStream {
        let mut stream = UnixStream::connect(path).await.unwrap();

        // Create session with bash (explicit command for predictable output)
        let msg = CreateSessionMsg {
            name: Some(session_name.to_string()),
            command: Some("bash".to_string()),
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        };
        Frame::control(FrameType::CreateSession, &msg)
            .unwrap()
            .write_to(&mut stream)
            .await
            .unwrap();

        // Read CreateSessionResponse
        let resp_frame = Frame::read_from(&mut stream).await.unwrap();
        assert_eq!(resp_frame.frame_type, FrameType::CreateSessionResponse);

        // Wait for shell to start
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        // Send input to generate some scrollback lines
        let input = Frame::data(
            FrameType::StdinInput,
            Bytes::from("echo scrollback_line_1\necho scrollback_line_2\necho scrollback_line_3\n"),
        );
        input.write_to(&mut stream).await.unwrap();

        // Wait for output to be processed by the parser
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

        // Drain output frames from the creator stream so it doesn't block
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            match tokio::time::timeout_at(deadline, Frame::read_from(&mut stream)).await {
                Ok(Ok(_)) => continue,
                _ => break,
            }
        }

        stream
    }

    #[tokio::test]
    async fn test_attach_scrollback_none_returns_empty() {
        let sessions = SessionRegistry::new();
        let path = start_test_server(sessions.clone()).await;

        let _creator = create_session_with_output(&sessions, &path, "sb-none-test").await;

        // Attach with ScrollbackRequest::None
        let mut stream2 = UnixStream::connect(&path).await.unwrap();
        let attach_msg = AttachSessionMsg {
            name: "sb-none-test".to_string(),
            scrollback: ScrollbackRequest::None,
            rows: 24,
            cols: 80,
        };
        Frame::control(FrameType::AttachSession, &attach_msg)
            .unwrap()
            .write_to(&mut stream2)
            .await
            .unwrap();

        let resp_frame = Frame::read_from(&mut stream2).await.unwrap();
        assert_eq!(resp_frame.frame_type, FrameType::AttachSessionResponse);
        let resp: AttachSessionResponseMsg = resp_frame.parse_json().unwrap();

        assert_eq!(resp.name, "sb-none-test");
        assert!(
            resp.scrollback.is_empty(),
            "scrollback should be empty with ScrollbackRequest::None, got {} bytes",
            resp.scrollback.len()
        );

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_attach_scrollback_all_returns_content() {
        let sessions = SessionRegistry::new();
        let path = start_test_server(sessions.clone()).await;

        let _creator = create_session_with_output(&sessions, &path, "sb-all-test").await;

        // Attach with ScrollbackRequest::All
        let mut stream2 = UnixStream::connect(&path).await.unwrap();
        let attach_msg = AttachSessionMsg {
            name: "sb-all-test".to_string(),
            scrollback: ScrollbackRequest::All,
            rows: 24,
            cols: 80,
        };
        Frame::control(FrameType::AttachSession, &attach_msg)
            .unwrap()
            .write_to(&mut stream2)
            .await
            .unwrap();

        let resp_frame = Frame::read_from(&mut stream2).await.unwrap();
        assert_eq!(resp_frame.frame_type, FrameType::AttachSessionResponse);
        let resp: AttachSessionResponseMsg = resp_frame.parse_json().unwrap();

        assert_eq!(resp.name, "sb-all-test");

        // Scrollback should contain our echo output (it ends up in scrollback
        // because the shell prompt pushes earlier lines out of the visible screen).
        // However, on a fresh 24-row terminal with only 3 echo commands, the lines
        // may still be on the active screen rather than scrollback. Either way,
        // the combined scrollback + screen should contain our output.
        let scrollback_str = String::from_utf8_lossy(&resp.scrollback);
        let screen_str = String::from_utf8_lossy(&resp.screen);
        let combined = format!("{}{}", scrollback_str, screen_str);
        assert!(
            combined.contains("scrollback_line_1"),
            "expected to find 'scrollback_line_1' in scrollback or screen.\nScrollback ({} bytes): {:?}\nScreen ({} bytes): {:?}",
            resp.scrollback.len(),
            scrollback_str,
            resp.screen.len(),
            screen_str,
        );

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_attach_scrollback_lines_limits_output() {
        let sessions = SessionRegistry::new();
        let path = start_test_server(sessions.clone()).await;

        // Create a session and generate enough output to have scrollback
        let mut stream = UnixStream::connect(&path).await.unwrap();
        let msg = CreateSessionMsg {
            name: Some("sb-lines-test".to_string()),
            command: Some("bash".to_string()),
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        };
        Frame::control(FrameType::CreateSession, &msg)
            .unwrap()
            .write_to(&mut stream)
            .await
            .unwrap();
        let _resp = Frame::read_from(&mut stream).await.unwrap();

        // Wait for shell to start
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        // Generate many lines to force scrollback (more than 24 visible rows)
        let mut input_cmds = String::new();
        for i in 0..40 {
            input_cmds.push_str(&format!("echo line_{}\n", i));
        }
        let input = Frame::data(FrameType::StdinInput, Bytes::from(input_cmds));
        input.write_to(&mut stream).await.unwrap();

        // Wait for output processing
        tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

        // Drain creator stream
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            match tokio::time::timeout_at(deadline, Frame::read_from(&mut stream)).await {
                Ok(Ok(_)) => continue,
                _ => break,
            }
        }

        // Attach with ScrollbackRequest::Lines(5)
        let mut stream2 = UnixStream::connect(&path).await.unwrap();
        let attach_msg = AttachSessionMsg {
            name: "sb-lines-test".to_string(),
            scrollback: ScrollbackRequest::Lines(5),
            rows: 24,
            cols: 80,
        };
        Frame::control(FrameType::AttachSession, &attach_msg)
            .unwrap()
            .write_to(&mut stream2)
            .await
            .unwrap();

        let resp_frame = Frame::read_from(&mut stream2).await.unwrap();
        assert_eq!(resp_frame.frame_type, FrameType::AttachSessionResponse);
        let resp_limited: AttachSessionResponseMsg = resp_frame.parse_json().unwrap();

        // Now attach with ScrollbackRequest::All to compare
        let mut stream3 = UnixStream::connect(&path).await.unwrap();
        let attach_all_msg = AttachSessionMsg {
            name: "sb-lines-test".to_string(),
            scrollback: ScrollbackRequest::All,
            rows: 24,
            cols: 80,
        };
        Frame::control(FrameType::AttachSession, &attach_all_msg)
            .unwrap()
            .write_to(&mut stream3)
            .await
            .unwrap();

        let resp_all_frame = Frame::read_from(&mut stream3).await.unwrap();
        let resp_all: AttachSessionResponseMsg = resp_all_frame.parse_json().unwrap();

        // Lines(5) should return <= the amount that All returns
        // (if there is scrollback, the limited version should be smaller or equal)
        let limited_lines = String::from_utf8_lossy(&resp_limited.scrollback)
            .lines()
            .count();
        let all_lines = String::from_utf8_lossy(&resp_all.scrollback)
            .lines()
            .count();

        // If there is scrollback, the limited version should have fewer lines
        if all_lines > 5 {
            assert!(
                limited_lines <= 5,
                "Lines(5) should return at most 5 lines of scrollback, got {}. All had {} lines.",
                limited_lines,
                all_lines,
            );
            assert!(
                limited_lines < all_lines,
                "Lines(5) ({} lines) should return fewer lines than All ({} lines)",
                limited_lines,
                all_lines,
            );
        }

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_attach_screen_data_present() {
        let sessions = SessionRegistry::new();
        let path = start_test_server(sessions.clone()).await;

        let _creator = create_session_with_output(&sessions, &path, "screen-test").await;

        // Attach and check screen data
        let mut stream2 = UnixStream::connect(&path).await.unwrap();
        let attach_msg = AttachSessionMsg {
            name: "screen-test".to_string(),
            scrollback: ScrollbackRequest::None,
            rows: 24,
            cols: 80,
        };
        Frame::control(FrameType::AttachSession, &attach_msg)
            .unwrap()
            .write_to(&mut stream2)
            .await
            .unwrap();

        let resp_frame = Frame::read_from(&mut stream2).await.unwrap();
        assert_eq!(resp_frame.frame_type, FrameType::AttachSessionResponse);
        let resp: AttachSessionResponseMsg = resp_frame.parse_json().unwrap();

        // Screen data should be non-empty (at minimum contains cursor positioning
        // escape sequences and the active screen content)
        assert!(
            !resp.screen.is_empty(),
            "screen data should not be empty for an active session"
        );

        // Screen should contain the ANSI home/clear sequence
        let screen_str = String::from_utf8_lossy(&resp.screen);
        assert!(
            screen_str.contains("\x1b[H\x1b[2J") || screen_str.contains("\x1b["),
            "screen data should contain ANSI escape sequences, got: {:?}",
            screen_str,
        );

        std::fs::remove_file(&path).ok();
    }
}
