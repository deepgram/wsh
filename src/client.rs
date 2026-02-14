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

use crate::overlay::{self, Overlay};
use crate::panel::{self, Panel};
use crate::protocol::*;

/// Render the panel sync update, writing ANSI escape sequences to `w`.
///
/// Handles scroll region transitions carefully: DECSTBM (`\x1b[r`) moves the
/// cursor to (1,1) as a side effect per VT100 spec, so we only emit it when
/// there is an actual transition between having panels and not having them.
///
/// Transition table:
/// - no panels → no panels: no scroll region change (avoids cursor jump)
/// - no panels → has panels: set scroll region
/// - has panels → has panels: update scroll region
/// - has panels → no panels: reset scroll region
fn render_panel_sync(
    w: &mut impl std::io::Write,
    new_panels: &[Panel],
    cached_panels: &[Panel],
    term_rows: u16,
    term_cols: u16,
) -> std::io::Result<()> {
    w.write_all(overlay::begin_sync().as_bytes())?;

    // Erase old panels using cached layout
    if !cached_panels.is_empty() {
        let old_layout = panel::compute_layout(cached_panels, term_rows, term_cols);
        w.write_all(panel::erase_all_panels(&old_layout, term_cols).as_bytes())?;
    }

    // Compute new layout
    let new_layout = panel::compute_layout(new_panels, term_rows, term_cols);

    let had_panels = !cached_panels.is_empty();
    let has_panels = !new_layout.top_panels.is_empty()
        || !new_layout.bottom_panels.is_empty();

    // Only change scroll region when transitioning between having panels and
    // not having them (or vice versa). DECSTBM (`\x1b[r` / `\x1b[t;br`)
    // moves the cursor to (1,1) as a side effect per VT100 spec, so we:
    //   1. Skip it entirely for no-panels → no-panels (the original bug fix)
    //   2. Wrap it in SCOSC save/restore when we do emit it, to preserve
    //      cursor position (safe here because erase_all_panels already
    //      completed its own save/restore cycle — no nesting)
    if has_panels {
        w.write_all(overlay::save_cursor().as_bytes())?;
        w.write_all(
            panel::set_scroll_region(new_layout.scroll_region_top, new_layout.scroll_region_bottom)
                .as_bytes(),
        )?;
        w.write_all(overlay::restore_cursor().as_bytes())?;
    } else if had_panels {
        // Transitioning from panels → no panels: reset
        w.write_all(overlay::save_cursor().as_bytes())?;
        w.write_all(panel::reset_scroll_region().as_bytes())?;
        w.write_all(overlay::restore_cursor().as_bytes())?;
    }
    // else: no panels before, no panels now — skip entirely

    if has_panels {
        w.write_all(panel::render_all_panels(&new_layout, term_cols).as_bytes())?;
    }
    w.write_all(overlay::end_sync().as_bytes())?;
    w.flush()
}

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
                Err(io::Error::other(format!("{}: {}", err.code, err.message)))
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
                Err(io::Error::other(format!("{}: {}", err.code, err.message)))
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
                Err(io::Error::other(format!("{}: {}", err.code, err.message)))
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
                Err(io::Error::other(format!("{}: {}", err.code, err.message)))
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
                Err(io::Error::other(format!("{}: {}", err.code, err.message)))
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

        // Self-pipe for stdin reader cancellation. poll() blocks on both
        // stdin and the read end of this pipe. To cancel, we drop the write
        // end — poll() wakes instantly with POLLHUP. No timeout needed.
        let (cancel_rd, cancel_wr) = {
            let mut fds = [0i32; 2];
            if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            unsafe {
                use std::os::unix::io::FromRawFd;
                (
                    std::os::unix::io::OwnedFd::from_raw_fd(fds[0]),
                    std::os::unix::io::OwnedFd::from_raw_fd(fds[1]),
                )
            }
        };
        let cancel_rd_raw = std::os::unix::io::AsRawFd::as_raw_fd(&cancel_rd);

        // Spawn stdin reader in a blocking thread.
        let stdin_handle = tokio::task::spawn_blocking(move || {
            use std::io::Read;
            use std::os::unix::io::AsRawFd;

            let _cancel_rd = cancel_rd; // keep alive; closed on exit
            let stdin = std::io::stdin();
            let stdin_fd = stdin.as_raw_fd();
            let mut buf = [0u8; 4096];
            loop {
                let mut pfds = [
                    libc::pollfd { fd: stdin_fd, events: libc::POLLIN, revents: 0 },
                    libc::pollfd { fd: cancel_rd_raw, events: libc::POLLIN, revents: 0 },
                ];
                let ret = unsafe { libc::poll(pfds.as_mut_ptr(), 2, -1) };
                if ret < 0 {
                    break;
                }
                // Cancel pipe closed → exit
                if pfds[1].revents != 0 {
                    break;
                }
                if pfds[0].revents & libc::POLLIN == 0 {
                    continue;
                }
                match stdin.lock().read(&mut buf) {
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

        let mut stdout = std::io::stdout();
        let result = streaming_loop(reader, writer, &mut stdin_rx, &mut sigwinch_rx, &mut stdout).await;

        // Close the cancel pipe write end — poll() in the reader wakes
        // instantly with POLLHUP and the reader exits. Then we join it
        // to ensure it's fully stopped before the caller restores the
        // terminal.
        drop(cancel_wr);
        drop(stdin_rx);
        let _ = stdin_handle.await;

        result
    }

}

/// The main streaming loop, factored out of `run_streaming` for testability.
///
/// Reads stdin data from `stdin_rx`, reads frames from the server via `reader`,
/// writes frames to the server via `writer`, and handles resize signals from
/// `sigwinch_rx`. Terminal output (PTY data, overlays, panels) is written to
/// `output`, which is `stdout` in production and a buffer in tests.
async fn streaming_loop(
    mut reader: ReadHalf<UnixStream>,
    mut writer: WriteHalf<UnixStream>,
    stdin_rx: &mut tokio::sync::mpsc::Receiver<Bytes>,
    sigwinch_rx: &mut tokio::sync::mpsc::Receiver<(u16, u16)>,
    output: &mut impl std::io::Write,
) -> io::Result<()> {
    // Ctrl+\ double-tap detection for detach.
    // Each Ctrl+\ is forwarded to the server immediately (the server toggles
    // input capture mode). If a second Ctrl+\ arrives within the timeout,
    // we also detach. Two rapid toggles cancel out, leaving capture mode
    // unchanged after re-attach.
    let mut pending_detach = false;
    let detach_timer = tokio::time::sleep(std::time::Duration::from_millis(500));
    tokio::pin!(detach_timer);

    // Local caches of visual state for erase-before-render
    let mut cached_overlays: Vec<Overlay> = Vec::new();
    let mut cached_panels: Vec<Panel> = Vec::new();

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

            // Frames from server → output
            result = Frame::read_from(&mut reader) => {
                match result {
                    Ok(frame) => {
                        match frame.frame_type {
                            FrameType::PtyOutput => {
                                if !cached_overlays.is_empty() {
                                    // Erase overlays, write PTY output, re-render overlays
                                    let _ = output.write_all(overlay::begin_sync().as_bytes());
                                    let _ = output.write_all(overlay::erase_all_overlays(&cached_overlays).as_bytes());
                                    let _ = output.write_all(&frame.payload);
                                    let _ = output.write_all(overlay::render_all_overlays(&cached_overlays).as_bytes());
                                    let _ = output.write_all(overlay::end_sync().as_bytes());
                                } else {
                                    let _ = output.write_all(&frame.payload);
                                }
                                let _ = output.flush();
                            }
                            FrameType::OverlaySync => {
                                if let Ok(msg) = frame.parse_json::<OverlaySyncMsg>() {
                                    let _ = output.write_all(overlay::begin_sync().as_bytes());
                                    let _ = output.write_all(overlay::save_cursor().as_bytes());
                                    // Erase old overlays
                                    let _ = output.write_all(overlay::erase_all_overlays(&cached_overlays).as_bytes());
                                    // Render new overlays
                                    let _ = output.write_all(overlay::render_all_overlays(&msg.overlays).as_bytes());
                                    let _ = output.write_all(overlay::restore_cursor().as_bytes());
                                    let _ = output.write_all(overlay::end_sync().as_bytes());
                                    let _ = output.flush();
                                    cached_overlays = msg.overlays;
                                }
                            }
                            FrameType::PanelSync => {
                                if let Ok(msg) = frame.parse_json::<PanelSyncMsg>() {
                                    let (term_rows, term_cols) = crate::terminal::terminal_size().unwrap_or((24, 80));
                                    let _ = render_panel_sync(
                                        output,
                                        &msg.panels,
                                        &cached_panels,
                                        term_rows,
                                        term_cols,
                                    );
                                    cached_panels = msg.panels;
                                }
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

    // Clean up visual state before exiting
    {
        if !cached_overlays.is_empty() {
            let _ = output.write_all(overlay::erase_all_overlays(&cached_overlays).as_bytes());
        }
        if !cached_panels.is_empty() {
            let (term_rows, term_cols) = crate::terminal::terminal_size().unwrap_or((24, 80));
            let layout = panel::compute_layout(&cached_panels, term_rows, term_cols);
            let _ = output.write_all(panel::erase_all_panels(&layout, term_cols).as_bytes());
            let _ = output.write_all(panel::reset_scroll_region().as_bytes());
        }
        let _ = output.flush();
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

    /// Start a test server on a temporary socket and return the path and TempDir.
    /// The caller must keep the TempDir alive for the duration of the test.
    async fn start_test_server(sessions: SessionRegistry) -> (PathBuf, TempDir) {
        let dir = TempDir::new().unwrap();
        let socket_path = dir.path().join("test.sock");
        let path = socket_path.clone();

        tokio::spawn(async move {
            let cancel = tokio_util::sync::CancellationToken::new();
            server::serve(sessions, &socket_path, cancel).await.unwrap();
        });

        // Wait for socket to appear
        for _ in 0..50 {
            if path.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        (path, dir)
    }

    #[tokio::test]
    async fn test_client_connect_and_create_session() {
        let sessions = SessionRegistry::new();
        let (path, _dir) = start_test_server(sessions.clone()).await;

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

        let (path, _dir) = start_test_server(sessions.clone()).await;

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
        let (path, _dir) = start_test_server(sessions).await;

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
        let (path, _dir) = start_test_server(sessions).await;

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

        let (path, _dir) = start_test_server(sessions).await;

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

        let (path, _dir) = start_test_server(sessions.clone()).await;

        let mut client = Client::connect(&path).await.unwrap();
        client.kill_session("kill-test").await.unwrap();
        assert!(sessions.get("kill-test").is_none());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_client_kill_nonexistent_session() {
        let sessions = SessionRegistry::new();
        let (path, _dir) = start_test_server(sessions).await;

        let mut client = Client::connect(&path).await.unwrap();
        let result = client.kill_session("no-such").await;
        assert!(result.is_err());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_client_create_session_then_send_input() {
        let sessions = SessionRegistry::new();
        let (path, _dir) = start_test_server(sessions.clone()).await;

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
        while let Ok(Ok(frame)) = tokio::time::timeout_at(deadline, Frame::read_from(&mut client.stream)).await {
            if frame.frame_type == FrameType::PtyOutput {
                received_output = true;
                let output = String::from_utf8_lossy(&frame.payload);
                if output.contains("test") {
                    break;
                }
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
            streaming_loop(reader, writer, &mut stdin_rx, &mut sigwinch_rx, &mut std::io::sink()).await
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
            streaming_loop(reader, writer, &mut stdin_rx, &mut sigwinch_rx, &mut std::io::sink()).await
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
            streaming_loop(reader, writer, &mut stdin_rx, &mut sigwinch_rx, &mut std::io::sink()).await
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

        let (path, _dir) = start_test_server(sessions.clone()).await;

        let mut client = Client::connect(&path).await.unwrap();
        client.detach_session("detach-test").await.unwrap();

        // Session should still exist (unlike kill)
        assert!(sessions.get("detach-test").is_some());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_client_detach_nonexistent_session() {
        let sessions = SessionRegistry::new();
        let (path, _dir) = start_test_server(sessions).await;

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
            streaming_loop(reader, writer, &mut stdin_rx, &mut sigwinch_rx, &mut std::io::sink()).await
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
            streaming_loop(reader, writer, &mut stdin_rx, &mut sigwinch_rx, &mut std::io::sink()).await
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
            streaming_loop(reader, writer, &mut stdin_rx, &mut sigwinch_rx, &mut std::io::sink()).await
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
            streaming_loop(reader, writer, &mut stdin_rx, &mut sigwinch_rx, &mut std::io::sink()).await
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

    /// Helper: create a minimal visible panel for testing scroll region behavior.
    fn test_panel(id: &str, position: panel::Position) -> Panel {
        Panel {
            id: id.to_string(),
            position,
            height: 1,
            z: 0,
            background: None,
            spans: vec![],
            region_writes: vec![],
            visible: true,
            focusable: false,
            screen_mode: crate::overlay::ScreenMode::Normal,
        }
    }

    #[test]
    fn test_panel_sync_no_panels_to_no_panels_skips_scroll_region() {
        // When there are no panels before and no panels after, DECSTBM (\x1b[r)
        // must NOT be emitted — it moves the cursor to (1,1) as a side effect.
        let mut buf = Vec::new();
        render_panel_sync(&mut buf, &[], &[], 24, 80).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(
            !output.contains("\x1b[r"),
            "empty→empty must not emit DECSTBM; got: {:?}",
            output,
        );
    }

    #[test]
    fn test_panel_sync_panels_to_no_panels_resets_scroll_region() {
        // Transitioning from having panels to no panels should reset the
        // scroll region so the shell uses the full terminal again.
        let old = vec![test_panel("p1", panel::Position::Bottom)];
        let mut buf = Vec::new();
        render_panel_sync(&mut buf, &[], &old, 24, 80).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(
            output.contains("\x1b[r"),
            "panels→empty must emit DECSTBM reset; got: {:?}",
            output,
        );
        // DECSTBM reset must be wrapped in cursor save/restore to avoid
        // moving the cursor to (1,1).
        assert!(
            output.contains("\x1b[s\x1b[r\x1b[u"),
            "DECSTBM reset must be wrapped in SCOSC save/restore; got: {:?}",
            output,
        );
    }

    #[test]
    fn test_panel_sync_no_panels_to_panels_sets_scroll_region() {
        // Adding the first panel should set DECSTBM to carve out panel rows.
        let new = vec![test_panel("p1", panel::Position::Bottom)];
        let mut buf = Vec::new();
        render_panel_sync(&mut buf, &new, &[], 24, 80).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // Should contain a scroll region set (e.g. \x1b[1;23r) wrapped in
        // cursor save/restore.
        assert!(
            output.contains("\x1b[s\x1b[1;23r\x1b[u"),
            "empty→panels must set scroll region with save/restore; got: {:?}",
            output,
        );
    }

    #[test]
    fn test_panel_sync_panels_to_panels_updates_scroll_region() {
        // Updating from one panel layout to another should update DECSTBM.
        let old = vec![test_panel("p1", panel::Position::Bottom)];
        let new = vec![
            test_panel("p1", panel::Position::Bottom),
            test_panel("p2", panel::Position::Top),
        ];
        let mut buf = Vec::new();
        render_panel_sync(&mut buf, &new, &old, 24, 80).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // Should contain an updated scroll region (top=2, bottom=23 → \x1b[2;23r)
        // wrapped in cursor save/restore.
        assert!(
            output.contains("\x1b[s\x1b[2;23r\x1b[u"),
            "panels→panels must update scroll region with save/restore; got: {:?}",
            output,
        );
    }

    /// Integration test: send a PanelSync frame with empty panels through the
    /// socket and verify the streaming loop does NOT emit DECSTBM (`\x1b[r`).
    ///
    /// This is the exact scenario that caused cursor corruption after alternate
    /// screen exit: the server sends a PanelSync with empty panels (triggered by
    /// screen_mode change), and the client must not emit `\x1b[r` since it moves
    /// the cursor to (1,1).
    #[tokio::test]
    async fn test_streaming_loop_empty_panel_sync_no_decstbm() {
        use std::sync::{Arc, Mutex};

        /// A `Write` impl that appends to a shared buffer.
        #[derive(Clone)]
        struct SharedBuf(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for SharedBuf {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let (client_stream, mut server_stream) = TokioUnixStream::pair().unwrap();

        let (reader, writer) = tokio::io::split(client_stream);
        let (_stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Bytes>(64);
        let (_sigwinch_tx, mut sigwinch_rx) = tokio::sync::mpsc::channel::<(u16, u16)>(4);

        let output_buf = SharedBuf(Arc::new(Mutex::new(Vec::new())));
        let output_buf_clone = output_buf.clone();

        let loop_handle = tokio::spawn(async move {
            let mut out = output_buf_clone;
            streaming_loop(reader, writer, &mut stdin_rx, &mut sigwinch_rx, &mut out).await
        });

        // Send a PanelSync frame with empty panels (simulates server visual
        // update after alternate screen exit with no active panels)
        let panel_sync_msg = PanelSyncMsg {
            panels: vec![],
            scroll_region_top: 1,
            scroll_region_bottom: 24,
        };
        let frame = Frame::control(FrameType::PanelSync, &panel_sync_msg).unwrap();
        frame.write_to(&mut server_stream).await.unwrap();

        // Give the loop time to process the frame
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Close the server connection to end the loop
        drop(server_stream);
        let _ = loop_handle.await;

        let bytes = output_buf.0.lock().unwrap();
        let output = String::from_utf8_lossy(&bytes);

        assert!(
            !output.contains("\x1b[r"),
            "empty PanelSync must not emit DECSTBM (\\x1b[r); got: {:?}",
            output,
        );
    }
}
