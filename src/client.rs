//! Unix socket client for connecting to the wsh server daemon.
//!
//! Provides a thin CLI client that connects to the server's Unix socket,
//! sends control frames (CreateSession / AttachSession), and then enters
//! a streaming I/O proxy loop forwarding stdin/stdout over the socket.

use std::io;
use std::path::Path;

use bytes::Bytes;
use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::UnixStream;

use crate::protocol::*;

/// A client connection to the wsh server daemon over a Unix socket.
pub struct Client {
    stream: UnixStream,
}

impl Client {
    /// Connect to the server's Unix domain socket.
    pub async fn connect(socket_path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(socket_path).await?;
        Ok(Self { stream })
    }

    /// Send a CreateSession control frame and read the response.
    pub async fn create_session(
        &mut self,
        msg: CreateSessionMsg,
    ) -> io::Result<CreateSessionResponseMsg> {
        let frame = Frame::control(FrameType::CreateSession, &msg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        frame.write_to(&mut self.stream).await?;

        let resp_frame = Frame::read_from(&mut self.stream).await?;
        match resp_frame.frame_type {
            FrameType::CreateSessionResponse => {
                resp_frame
                    .parse_json()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
            }
            FrameType::Error => {
                let err: ErrorMsg = resp_frame
                    .parse_json()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("{}: {}", err.code, err.message),
                ))
            }
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected response frame type: {:?}", other),
            )),
        }
    }

    /// Send an AttachSession control frame and read the response.
    pub async fn attach(
        &mut self,
        msg: AttachSessionMsg,
    ) -> io::Result<AttachSessionResponseMsg> {
        let frame = Frame::control(FrameType::AttachSession, &msg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        frame.write_to(&mut self.stream).await?;

        let resp_frame = Frame::read_from(&mut self.stream).await?;
        match resp_frame.frame_type {
            FrameType::AttachSessionResponse => {
                resp_frame
                    .parse_json()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
            }
            FrameType::Error => {
                let err: ErrorMsg = resp_frame
                    .parse_json()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("{}: {}", err.code, err.message),
                ))
            }
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected response frame type: {:?}", other),
            )),
        }
    }

    /// List sessions via the server's Unix socket.
    pub async fn list_sessions(&mut self) -> io::Result<Vec<SessionInfoMsg>> {
        let msg = ListSessionsMsg {};
        let frame = Frame::control(FrameType::ListSessions, &msg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        frame.write_to(&mut self.stream).await?;

        let resp_frame = Frame::read_from(&mut self.stream).await?;
        match resp_frame.frame_type {
            FrameType::ListSessionsResponse => {
                let resp: ListSessionsResponseMsg = resp_frame
                    .parse_json()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                Ok(resp.sessions)
            }
            FrameType::Error => {
                let err: ErrorMsg = resp_frame
                    .parse_json()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("{}: {}", err.code, err.message),
                ))
            }
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected response frame type: {:?}", other),
            )),
        }
    }

    /// Kill (destroy) a session via the server's Unix socket.
    pub async fn kill_session(&mut self, name: &str) -> io::Result<()> {
        let msg = KillSessionMsg { name: name.to_string() };
        let frame = Frame::control(FrameType::KillSession, &msg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        frame.write_to(&mut self.stream).await?;

        let resp_frame = Frame::read_from(&mut self.stream).await?;
        match resp_frame.frame_type {
            FrameType::KillSessionResponse => Ok(()),
            FrameType::Error => {
                let err: ErrorMsg = resp_frame
                    .parse_json()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("{}: {}", err.code, err.message),
                ))
            }
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected response frame type: {:?}", other),
            )),
        }
    }

    /// Detach all attached clients from a session via the server's Unix socket.
    ///
    /// Unlike `kill_session`, this keeps the session alive — it only disconnects
    /// any streaming clients currently attached.
    pub async fn detach_session(&mut self, name: &str) -> io::Result<()> {
        let msg = DetachSessionMsg { name: name.to_string() };
        let frame = Frame::control(FrameType::DetachSession, &msg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        frame.write_to(&mut self.stream).await?;

        let resp_frame = Frame::read_from(&mut self.stream).await?;
        match resp_frame.frame_type {
            FrameType::DetachSessionResponse => Ok(()),
            FrameType::Error => {
                let err: ErrorMsg = resp_frame
                    .parse_json()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("{}: {}", err.code, err.message),
                ))
            }
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected response frame type: {:?}", other),
            )),
        }
    }

    /// Enter the streaming I/O proxy loop.
    ///
    /// Consumes the client, splits the underlying stream, and runs a
    /// `tokio::select!` loop that:
    /// - Reads from stdin (via `spawn_blocking`) and forwards as StdinInput frames
    /// - Reads PtyOutput frames from the server and writes to stdout
    /// - Handles SIGWINCH signals and sends Resize frames
    /// - Exits on stdin EOF or server disconnect
    pub async fn run_streaming(self) -> io::Result<()> {
        let (reader, writer) = tokio::io::split(self.stream);

        // Channel for stdin data from the blocking reader
        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Bytes>(64);

        // Spawn stdin reader in a blocking thread
        tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 4096];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let data = Bytes::copy_from_slice(&buf[..n]);
                        if stdin_tx.blocking_send(data).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Channel for SIGWINCH signals
        let (sigwinch_tx, mut sigwinch_rx) = tokio::sync::mpsc::channel::<(u16, u16)>(4);
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigwinch = match signal(SignalKind::window_change()) {
                Ok(s) => s,
                Err(_) => return,
            };
            loop {
                sigwinch.recv().await;
                if let Ok((rows, cols)) = crate::terminal::terminal_size() {
                    if sigwinch_tx.send((rows, cols)).await.is_err() {
                        break;
                    }
                }
            }
        });

        streaming_loop(reader, writer, &mut stdin_rx, &mut sigwinch_rx).await
    }

}

/// The main streaming loop, factored out of `run_streaming` for testability.
///
/// Reads stdin data from `stdin_rx`, reads frames from the server via `reader`,
/// writes frames to the server via `writer`, and handles resize signals from
/// `sigwinch_rx`.
async fn streaming_loop(
    mut reader: ReadHalf<UnixStream>,
    mut writer: WriteHalf<UnixStream>,
    stdin_rx: &mut tokio::sync::mpsc::Receiver<Bytes>,
    sigwinch_rx: &mut tokio::sync::mpsc::Receiver<(u16, u16)>,
) -> io::Result<()> {
    // Ctrl+\ double-tap detection for detach.
    // Each Ctrl+\ is forwarded to the server immediately (the server toggles
    // input capture mode). If a second Ctrl+\ arrives within the timeout,
    // we also detach. Two rapid toggles cancel out, leaving capture mode
    // unchanged after re-attach.
    let mut pending_detach = false;
    let detach_timer = tokio::time::sleep(std::time::Duration::from_millis(500));
    tokio::pin!(detach_timer);

    loop {
        tokio::select! {
            // Stdin data → StdinInput frame to server
            data = stdin_rx.recv() => {
                match data {
                    Some(data) => {
                        if crate::input::is_ctrl_backslash(&data) {
                            // Always forward immediately — server handles the toggle
                            let frame = Frame::data(FrameType::StdinInput, data);
                            if frame.write_to(&mut writer).await.is_err() {
                                break;
                            }

                            if pending_detach {
                                // Double-tap: detach
                                let detach = Frame::new(FrameType::Detach, Bytes::new());
                                let _ = detach.write_to(&mut writer).await;
                                break;
                            } else {
                                // Start double-tap timer
                                pending_detach = true;
                                detach_timer.as_mut().reset(
                                    tokio::time::Instant::now() + std::time::Duration::from_millis(500)
                                );
                            }
                        } else {
                            pending_detach = false;
                            let frame = Frame::data(FrameType::StdinInput, data);
                            if frame.write_to(&mut writer).await.is_err() {
                                break;
                            }
                        }
                    }
                    None => {
                        // Stdin closed — detach
                        let detach = Frame::new(FrameType::Detach, Bytes::new());
                        let _ = detach.write_to(&mut writer).await;
                        break;
                    }
                }
            }

            // PtyOutput frames from server → stdout
            result = Frame::read_from(&mut reader) => {
                match result {
                    Ok(frame) => {
                        match frame.frame_type {
                            FrameType::PtyOutput => {
                                use std::io::Write;
                                let mut stdout = std::io::stdout().lock();
                                if stdout.write_all(&frame.payload).is_err() {
                                    break;
                                }
                                let _ = stdout.flush();
                            }
                            FrameType::Error => {
                                if let Ok(err) = frame.parse_json::<ErrorMsg>() {
                                    eprintln!("wsh: server error: {}: {}", err.code, err.message);
                                }
                                break;
                            }
                            FrameType::Detach => {
                                break;
                            }
                            _ => {
                                // Ignore unexpected frame types
                            }
                        }
                    }
                    Err(_) => break, // Server disconnected
                }
            }

            // SIGWINCH → Resize frame to server
            size = sigwinch_rx.recv() => {
                if let Some((rows, cols)) = size {
                    let msg = ResizeMsg { rows, cols };
                    if let Ok(frame) = Frame::control(FrameType::Resize, &msg) {
                        let _ = frame.write_to(&mut writer).await;
                    }
                }
            }

            // Ctrl+\ double-tap timeout expired — no detach
            () = &mut detach_timer, if pending_detach => {
                pending_detach = false;
            }
        }
    }

    // Ensure the writer half is cleanly shut down
    let _ = writer.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server;
    use crate::session::SessionRegistry;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use tokio::net::UnixStream as TokioUnixStream;

    /// Start a test server on a temporary socket and return the path.
    async fn start_test_server(sessions: SessionRegistry) -> PathBuf {
        let dir = TempDir::new().unwrap();
        let socket_path = dir.path().join("test.sock");
        std::mem::forget(dir);
        let path = socket_path.clone();

        tokio::spawn(async move {
            server::serve(sessions, &socket_path).await.unwrap();
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
    async fn test_client_connect_and_create_session() {
        let sessions = SessionRegistry::new();
        let path = start_test_server(sessions.clone()).await;

        let mut client = Client::connect(&path).await.unwrap();

        let msg = CreateSessionMsg {
            name: Some("client-test".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        };
        let resp = client.create_session(msg).await.unwrap();
        assert_eq!(resp.name, "client-test");
        assert_eq!(resp.rows, 24);
        assert_eq!(resp.cols, 80);

        // Verify session exists in registry
        assert!(sessions.get("client-test").is_some());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_client_connect_and_attach() {
        let sessions = SessionRegistry::new();

        // Pre-create a session
        let (session, child_exit_rx) = crate::session::Session::spawn(
            "attach-me".to_string(),
            crate::pty::SpawnCommand::default(),
            24,
            80,
        )
        .unwrap();
        sessions
            .insert(Some("attach-me".to_string()), session)
            .unwrap();
        sessions.monitor_child_exit("attach-me".to_string(), child_exit_rx);

        let path = start_test_server(sessions.clone()).await;

        let mut client = Client::connect(&path).await.unwrap();

        let msg = AttachSessionMsg {
            name: "attach-me".to_string(),
            scrollback: ScrollbackRequest::None,
            rows: 30,
            cols: 120,
        };
        let resp = client.attach(msg).await.unwrap();
        assert_eq!(resp.name, "attach-me");
        assert_eq!(resp.rows, 30);
        assert_eq!(resp.cols, 120);

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_client_connect_fails_with_bad_path() {
        let result = Client::connect(Path::new("/tmp/nonexistent-wsh-test-socket.sock")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_client_attach_nonexistent_session() {
        let sessions = SessionRegistry::new();
        let path = start_test_server(sessions).await;

        let mut client = Client::connect(&path).await.unwrap();

        let msg = AttachSessionMsg {
            name: "no-such-session".to_string(),
            scrollback: ScrollbackRequest::None,
            rows: 24,
            cols: 80,
        };
        let result = client.attach(msg).await;
        assert!(result.is_err());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_client_list_sessions_empty() {
        let sessions = SessionRegistry::new();
        let path = start_test_server(sessions).await;

        let mut client = Client::connect(&path).await.unwrap();
        let list = client.list_sessions().await.unwrap();
        assert!(list.is_empty());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_client_list_sessions_with_entries() {
        let sessions = SessionRegistry::new();

        let (s, rx) = crate::session::Session::spawn(
            "ls-test".to_string(),
            crate::pty::SpawnCommand::default(),
            24, 80,
        ).unwrap();
        sessions.insert(Some("ls-test".to_string()), s).unwrap();
        sessions.monitor_child_exit("ls-test".to_string(), rx);

        let path = start_test_server(sessions).await;

        let mut client = Client::connect(&path).await.unwrap();
        let list = client.list_sessions().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "ls-test");

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_client_kill_session() {
        let sessions = SessionRegistry::new();

        let (s, rx) = crate::session::Session::spawn(
            "kill-test".to_string(),
            crate::pty::SpawnCommand::default(),
            24, 80,
        ).unwrap();
        sessions.insert(Some("kill-test".to_string()), s).unwrap();
        sessions.monitor_child_exit("kill-test".to_string(), rx);

        let path = start_test_server(sessions.clone()).await;

        let mut client = Client::connect(&path).await.unwrap();
        client.kill_session("kill-test").await.unwrap();
        assert!(sessions.get("kill-test").is_none());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_client_kill_nonexistent_session() {
        let sessions = SessionRegistry::new();
        let path = start_test_server(sessions).await;

        let mut client = Client::connect(&path).await.unwrap();
        let result = client.kill_session("no-such").await;
        assert!(result.is_err());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_client_create_session_then_send_input() {
        let sessions = SessionRegistry::new();
        let path = start_test_server(sessions.clone()).await;

        let mut client = Client::connect(&path).await.unwrap();

        let msg = CreateSessionMsg {
            name: Some("io-test".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        };
        let _resp = client.create_session(msg).await.unwrap();

        // Send stdin input as a frame
        let input_frame = Frame::data(FrameType::StdinInput, Bytes::from("echo test\n"));
        input_frame.write_to(&mut client.stream).await.unwrap();

        // Read PtyOutput frames until we see our echo
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut received_output = false;
        loop {
            match tokio::time::timeout_at(deadline, Frame::read_from(&mut client.stream)).await {
                Ok(Ok(frame)) => {
                    if frame.frame_type == FrameType::PtyOutput {
                        received_output = true;
                        let output = String::from_utf8_lossy(&frame.payload);
                        if output.contains("test") {
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
    async fn test_streaming_loop_stdin_to_server() {
        // Set up a pair of connected Unix sockets
        let (client_stream, mut server_stream) = TokioUnixStream::pair().unwrap();

        let (reader, writer) = tokio::io::split(client_stream);
        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Bytes>(64);
        let (_sigwinch_tx, mut sigwinch_rx) = tokio::sync::mpsc::channel::<(u16, u16)>(4);

        // Spawn the streaming loop
        let loop_handle = tokio::spawn(async move {
            streaming_loop(reader, writer, &mut stdin_rx, &mut sigwinch_rx).await
        });

        // Send data through stdin channel
        stdin_tx.send(Bytes::from("hello")).await.unwrap();

        // Read the frame from the "server" side
        let frame = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Frame::read_from(&mut server_stream),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(frame.frame_type, FrameType::StdinInput);
        assert_eq!(frame.payload, Bytes::from("hello"));

        // Drop stdin sender to close the loop
        drop(stdin_tx);

        // The loop should send a Detach frame and exit
        let detach_frame = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Frame::read_from(&mut server_stream),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(detach_frame.frame_type, FrameType::Detach);

        loop_handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_streaming_loop_server_output_to_stdout() {
        // Set up a pair of connected Unix sockets
        let (client_stream, mut server_stream) = TokioUnixStream::pair().unwrap();

        let (reader, writer) = tokio::io::split(client_stream);
        let (_stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Bytes>(64);
        let (_sigwinch_tx, mut sigwinch_rx) = tokio::sync::mpsc::channel::<(u16, u16)>(4);

        // Spawn the streaming loop
        let loop_handle = tokio::spawn(async move {
            streaming_loop(reader, writer, &mut stdin_rx, &mut sigwinch_rx).await
        });

        // Send a PtyOutput frame from the "server"
        let output_frame = Frame::data(FrameType::PtyOutput, Bytes::from("server output"));
        output_frame.write_to(&mut server_stream).await.unwrap();

        // Give it a moment to process
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Close the server connection to end the loop
        drop(server_stream);

        let result = loop_handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_streaming_loop_resize_sends_frame() {
        let (client_stream, mut server_stream) = TokioUnixStream::pair().unwrap();

        let (reader, writer) = tokio::io::split(client_stream);
        let (_stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Bytes>(64);
        let (sigwinch_tx, mut sigwinch_rx) = tokio::sync::mpsc::channel::<(u16, u16)>(4);

        let loop_handle = tokio::spawn(async move {
            streaming_loop(reader, writer, &mut stdin_rx, &mut sigwinch_rx).await
        });

        // Send a resize signal
        sigwinch_tx.send((40, 120)).await.unwrap();

        // Read the resize frame from the "server" side
        let frame = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Frame::read_from(&mut server_stream),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(frame.frame_type, FrameType::Resize);
        let msg: ResizeMsg = frame.parse_json().unwrap();
        assert_eq!(msg.rows, 40);
        assert_eq!(msg.cols, 120);

        // Clean up
        drop(server_stream);
        let _ = loop_handle.await;
    }

    #[tokio::test]
    async fn test_client_detach_session() {
        let sessions = SessionRegistry::new();

        let (s, rx) = crate::session::Session::spawn(
            "detach-test".to_string(),
            crate::pty::SpawnCommand::default(),
            24, 80,
        ).unwrap();
        sessions.insert(Some("detach-test".to_string()), s).unwrap();
        sessions.monitor_child_exit("detach-test".to_string(), rx);

        let path = start_test_server(sessions.clone()).await;

        let mut client = Client::connect(&path).await.unwrap();
        client.detach_session("detach-test").await.unwrap();

        // Session should still exist (unlike kill)
        assert!(sessions.get("detach-test").is_some());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_client_detach_nonexistent_session() {
        let sessions = SessionRegistry::new();
        let path = start_test_server(sessions).await;

        let mut client = Client::connect(&path).await.unwrap();
        let result = client.detach_session("no-such").await;
        assert!(result.is_err());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_ctrl_backslash_double_tap_sends_detach() {
        let (client_stream, mut server_stream) = TokioUnixStream::pair().unwrap();

        let (reader, writer) = tokio::io::split(client_stream);
        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Bytes>(64);
        let (_sigwinch_tx, mut sigwinch_rx) = tokio::sync::mpsc::channel::<(u16, u16)>(4);

        let loop_handle = tokio::spawn(async move {
            streaming_loop(reader, writer, &mut stdin_rx, &mut sigwinch_rx).await
        });

        // Send Ctrl+\ twice in quick succession
        stdin_tx.send(Bytes::from_static(&[0x1c])).await.unwrap();
        stdin_tx.send(Bytes::from_static(&[0x1c])).await.unwrap();

        // Both Ctrl+\ are forwarded immediately as StdinInput, then Detach
        let frame1 = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Frame::read_from(&mut server_stream),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(frame1.frame_type, FrameType::StdinInput);
        assert_eq!(frame1.payload.as_ref(), &[0x1c]);

        let frame2 = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Frame::read_from(&mut server_stream),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(frame2.frame_type, FrameType::StdinInput);
        assert_eq!(frame2.payload.as_ref(), &[0x1c]);

        let frame3 = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Frame::read_from(&mut server_stream),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(frame3.frame_type, FrameType::Detach);

        loop_handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_ctrl_backslash_single_tap_forwarded_immediately() {
        let (client_stream, mut server_stream) = TokioUnixStream::pair().unwrap();

        let (reader, writer) = tokio::io::split(client_stream);
        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Bytes>(64);
        let (_sigwinch_tx, mut sigwinch_rx) = tokio::sync::mpsc::channel::<(u16, u16)>(4);

        let loop_handle = tokio::spawn(async move {
            streaming_loop(reader, writer, &mut stdin_rx, &mut sigwinch_rx).await
        });

        // Send a single Ctrl+\ — should be forwarded immediately (no delay)
        stdin_tx.send(Bytes::from_static(&[0x1c])).await.unwrap();

        let frame = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            Frame::read_from(&mut server_stream),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(frame.frame_type, FrameType::StdinInput);
        assert_eq!(frame.payload.as_ref(), &[0x1c]);

        // Clean up
        drop(stdin_tx);
        let detach = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Frame::read_from(&mut server_stream),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(detach.frame_type, FrameType::Detach);

        loop_handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_ctrl_backslash_then_other_key_forwards_both() {
        let (client_stream, mut server_stream) = TokioUnixStream::pair().unwrap();

        let (reader, writer) = tokio::io::split(client_stream);
        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Bytes>(64);
        let (_sigwinch_tx, mut sigwinch_rx) = tokio::sync::mpsc::channel::<(u16, u16)>(4);

        let loop_handle = tokio::spawn(async move {
            streaming_loop(reader, writer, &mut stdin_rx, &mut sigwinch_rx).await
        });

        // Send Ctrl+\ followed by 'a'
        stdin_tx.send(Bytes::from_static(&[0x1c])).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        stdin_tx.send(Bytes::from_static(b"a")).await.unwrap();

        // Ctrl+\ is forwarded immediately
        let frame1 = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Frame::read_from(&mut server_stream),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(frame1.frame_type, FrameType::StdinInput);
        assert_eq!(frame1.payload.as_ref(), &[0x1c]);

        // Then the 'a' (resets double-tap state)
        let frame2 = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Frame::read_from(&mut server_stream),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(frame2.frame_type, FrameType::StdinInput);
        assert_eq!(frame2.payload.as_ref(), b"a");

        // Clean up
        drop(stdin_tx);
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Frame::read_from(&mut server_stream),
        ).await;
        let _ = loop_handle.await;
    }

    #[tokio::test]
    async fn test_ctrl_backslash_then_stdin_close_sends_detach() {
        let (client_stream, mut server_stream) = TokioUnixStream::pair().unwrap();

        let (reader, writer) = tokio::io::split(client_stream);
        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Bytes>(64);
        let (_sigwinch_tx, mut sigwinch_rx) = tokio::sync::mpsc::channel::<(u16, u16)>(4);

        let loop_handle = tokio::spawn(async move {
            streaming_loop(reader, writer, &mut stdin_rx, &mut sigwinch_rx).await
        });

        // Send Ctrl+\ then immediately close stdin
        stdin_tx.send(Bytes::from_static(&[0x1c])).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        drop(stdin_tx);

        // Ctrl+\ is forwarded immediately
        let frame1 = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Frame::read_from(&mut server_stream),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(frame1.frame_type, FrameType::StdinInput);
        assert_eq!(frame1.payload.as_ref(), &[0x1c]);

        // Then the Detach frame (stdin close)
        let frame2 = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Frame::read_from(&mut server_stream),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(frame2.frame_type, FrameType::Detach);

        loop_handle.await.unwrap().unwrap();
    }
}
