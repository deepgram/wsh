//! Unix socket client for connecting to the wsh server daemon.
//!
//! Provides a thin CLI client that connects to the server's Unix socket,
//! sends control frames (CreateSession / AttachSession), and then enters
//! a streaming I/O proxy loop forwarding stdin/stdout over the socket.

use std::io;
use std::net::SocketAddr;
use std::path::Path;

use bytes::Bytes;
use serde::Deserialize;
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

    /// List sessions via the server's HTTP API.
    ///
    /// This is an associated function (not a method) since it uses HTTP,
    /// not the Unix socket.
    pub async fn list_sessions(
        bind: &SocketAddr,
        token: &Option<String>,
    ) -> io::Result<Vec<SessionListEntry>> {
        let url = format!("http://{}/sessions", bind);
        let client = reqwest::Client::new();
        let mut req = client.get(&url);
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| reqwest_to_io_error(bind, e))?;

        if !resp.status().is_success() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("server returned status {}", resp.status()),
            ));
        }

        let sessions: Vec<SessionListEntry> = resp
            .json()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        Ok(sessions)
    }

    /// Kill (destroy) a session via the server's HTTP API.
    ///
    /// This is an associated function (not a method) since it uses HTTP,
    /// not the Unix socket.
    pub async fn kill_session(
        bind: &SocketAddr,
        token: &Option<String>,
        name: &str,
    ) -> io::Result<()> {
        let url = format!("http://{}/sessions/{}", bind, url_encode_name(name));
        let client = reqwest::Client::new();
        let mut req = client.delete(&url);
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| reqwest_to_io_error(bind, e))?;

        if resp.status().as_u16() == 404 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("session not found: {}", name),
            ));
        }

        if !resp.status().is_success() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("server returned status {}", resp.status()),
            ));
        }

        Ok(())
    }
}

/// Convert a reqwest error into a human-friendly `io::Error`.
fn reqwest_to_io_error(bind: &SocketAddr, e: reqwest::Error) -> io::Error {
    if e.is_connect() {
        io::Error::new(
            io::ErrorKind::ConnectionRefused,
            format!("could not connect to wsh server at {} — is the server running?", bind),
        )
    } else if e.is_timeout() {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("connection to wsh server at {} timed out", bind),
        )
    } else {
        io::Error::new(io::ErrorKind::Other, e)
    }
}

/// Percent-encode a session name for use in URL paths.
fn url_encode_name(name: &str) -> String {
    let mut encoded = String::with_capacity(name.len());
    for c in name.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => encoded.push(c),
            _ => {
                for b in c.to_string().as_bytes() {
                    encoded.push_str(&format!("%{:02X}", b));
                }
            }
        }
    }
    encoded
}

/// Session info returned by the list endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionListEntry {
    pub name: String,
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
    loop {
        tokio::select! {
            // Stdin data → StdinInput frame to server
            data = stdin_rx.recv() => {
                match data {
                    Some(data) => {
                        let frame = Frame::data(FrameType::StdinInput, data);
                        if frame.write_to(&mut writer).await.is_err() {
                            break;
                        }
                    }
                    None => {
                        // Stdin closed — send Detach and exit
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
}
