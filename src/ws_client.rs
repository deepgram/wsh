//! WebSocket-over-UDS client for streaming terminal I/O.
//!
//! Replaces the binary socket streaming loop with a WebSocket connection
//! to the server's HTTP/WS API over a Unix domain socket. The WS JSON
//! protocol at `/sessions/{name}/ws/json` provides:
//!
//! - Binary WS frames for raw PTY output (when subscribed with `output` event)
//! - Binary WS frames sent from client are treated as raw stdin input
//! - JSON text frames for `overlay_sync` and `panel_sync` events
//! - JSON request/response for `subscribe`, `resize`, and other methods

use std::io::{self, Write};
use std::path::Path;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::net::UnixStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::client::render_panel_sync;
use crate::overlay::{self, Overlay};
use crate::panel::Panel;
use crate::parser::ansi::line_to_ansi;
use crate::parser::state::ScreenResponse;

/// Connect a WebSocket over a Unix domain socket to a session's WS endpoint.
///
/// Opens a `tokio::net::UnixStream` to the HTTP socket, performs the WS
/// handshake targeting `/sessions/{name}/ws/json`, reads the initial
/// `{"connected": true}` message, and returns the ready stream.
pub async fn connect_ws_uds(
    http_socket_path: &Path,
    session_name: &str,
) -> io::Result<WebSocketStream<UnixStream>> {
    let stream = UnixStream::connect(http_socket_path).await?;
    let url = format!("ws://localhost/sessions/{}/ws/json", session_name);

    let (ws_stream, _response) =
        tokio_tungstenite::client_async(url, stream)
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;

    Ok(ws_stream)
}

/// Read the initial `{"connected": true}` message from the WebSocket.
///
/// The server sends this immediately after the WS handshake. We consume it
/// before entering the streaming loop.
async fn read_connected_message(
    ws: &mut WebSocketStream<UnixStream>,
) -> io::Result<()> {
    match ws.next().await {
        Some(Ok(Message::Text(text))) => {
            // Parse and verify the connected message
            if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) {
                if msg.get("connected") == Some(&serde_json::Value::Bool(true)) {
                    return Ok(());
                }
            }
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected initial message: {}", text),
            ))
        }
        Some(Ok(other)) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected initial message type: {:?}", other),
        )),
        Some(Err(e)) => Err(io::Error::new(io::ErrorKind::ConnectionReset, e)),
        None => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "WebSocket closed before connected message",
        )),
    }
}

/// Subscribe to output and overlay events on the WebSocket.
///
/// Sends the subscribe request and reads the response. Returns an error
/// if the subscription fails.
async fn subscribe_events(
    ws: &mut WebSocketStream<UnixStream>,
) -> io::Result<()> {
    let subscribe_msg = serde_json::json!({
        "method": "subscribe",
        "params": {
            "events": ["output", "overlay"]
        }
    });
    ws.send(Message::Text(subscribe_msg.to_string().into()))
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e))?;

    // Read subscribe response
    match ws.next().await {
        Some(Ok(Message::Text(text))) => {
            if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) {
                if msg.get("error").is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("subscribe failed: {}", text),
                    ));
                }
                // Success response
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid subscribe response: {}", text),
                ))
            }
        }
        Some(Ok(other)) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected subscribe response type: {:?}", other),
        )),
        Some(Err(e)) => Err(io::Error::new(io::ErrorKind::ConnectionReset, e)),
        None => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "WebSocket closed before subscribe response",
        )),
    }
}

/// Resize state machine for the client-side SIGWINCH → server round-trip.
///
/// When SIGWINCH arrives, the client enters a buffering phase to avoid
/// writing stale PTY output (generated at the old dimensions) to the
/// terminal that is already at the new dimensions. The state machine
/// transitions:
///
///   Idle → AwaitingResizeAck → AwaitingScreenSync → Idle
///
/// Binary WS frames are buffered during AwaitingResizeAck and discarded
/// (the screen sync repaint replaces them). Text frames (overlays, panels)
/// continue processing normally. A 2-second timeout returns to Idle if the
/// server is unresponsive. Rapid successive SIGWINCHs coalesce by
/// restarting the state machine with new dimensions.
#[derive(Debug)]
enum ResizeState {
    /// Normal operation — forward Binary frames to stdout.
    Idle,
    /// Resize request sent, buffering output. Waiting for resize response.
    AwaitingResizeAck {
        resize_id: String,
        buffer: Vec<Bytes>,
    },
    /// Resize ack received, screen request sent. Waiting for screen response.
    AwaitingScreenSync {
        screen_id: String,
    },
}

impl ResizeState {
    fn is_idle(&self) -> bool {
        matches!(self, ResizeState::Idle)
    }
}

/// Render a full screen sync to the terminal output.
///
/// Clears the screen, writes each line with ANSI formatting, and restores
/// the cursor to the position reported by the server.
fn render_screen_sync(screen: &ScreenResponse, output: &mut impl Write) -> io::Result<()> {
    output.write_all(b"\x1b[H\x1b[2J")?; // home + clear
    for (i, line) in screen.lines.iter().enumerate() {
        output.write_all(line_to_ansi(line).as_bytes())?;
        if i + 1 < screen.lines.len() {
            output.write_all(b"\r\n")?;
        }
    }
    write!(output, "\x1b[{};{}H", screen.cursor.row + 1, screen.cursor.col + 1)?;
    output.flush()
}

/// Send a resize request over the WebSocket with a request ID.
async fn send_resize(
    ws: &mut futures::stream::SplitSink<WebSocketStream<UnixStream>, Message>,
    id: &str,
    rows: u16,
    cols: u16,
) -> io::Result<()> {
    let resize_msg = serde_json::json!({
        "id": id,
        "method": "resize",
        "params": {
            "cols": cols,
            "rows": rows
        }
    });
    ws.send(Message::Text(resize_msg.to_string().into()))
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e))
}

/// Send a get_screen request over the WebSocket with a request ID.
async fn send_get_screen(
    ws: &mut futures::stream::SplitSink<WebSocketStream<UnixStream>, Message>,
    id: &str,
) -> io::Result<()> {
    let msg = serde_json::json!({
        "id": id,
        "method": "get_screen",
        "params": { "format": "styled" }
    });
    ws.send(Message::Text(msg.to_string().into()))
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e))
}

/// Enter the WebSocket streaming I/O proxy loop.
///
/// Consumes the WS connection, spawns a stdin reader and SIGWINCH handler,
/// and runs a `tokio::select!` loop that:
/// - Reads from stdin (via `spawn_blocking`) and forwards as binary WS frames
/// - Reads WS messages from the server:
///   - Binary frames → write to stdout (raw PTY output)
///   - Text frames → parse JSON, handle overlay_sync/panel_sync for rendering
/// - Handles SIGWINCH signals and sends resize requests
/// - Ctrl+\ double-tap detection for detach
/// - Exits on stdin EOF or server disconnect
///
/// If `initial_resize` is provided, a resize request is sent after
/// subscribing but before entering the main loop (used by attach to
/// resize the session to the client's terminal size).
pub async fn run_ws_streaming(
    mut ws: WebSocketStream<UnixStream>,
    initial_resize: Option<(u16, u16)>,
) -> io::Result<()> {
    // Read the connected message
    read_connected_message(&mut ws).await?;

    // Subscribe to output and overlay events
    subscribe_events(&mut ws).await?;

    // Send initial resize if requested (for attach)
    if let Some((rows, cols)) = initial_resize {
        let resize_msg = serde_json::json!({
            "method": "resize",
            "params": {
                "cols": cols,
                "rows": rows
            }
        });
        ws.send(Message::Text(resize_msg.to_string().into()))
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e))?;
    }

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
            // Cancel pipe closed -> exit
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
    let (ws_tx, ws_rx) = ws.split();
    let result = ws_streaming_loop(
        ws_tx,
        ws_rx,
        &mut stdin_rx,
        &mut sigwinch_rx,
        &mut stdout,
    )
    .await;

    // Close the cancel pipe write end — poll() in the reader wakes
    // instantly with POLLHUP and the reader exits. Then we join it
    // to ensure it's fully stopped before the caller restores the
    // terminal.
    drop(cancel_wr);
    drop(stdin_rx);
    let _ = stdin_handle.await;

    result
}

/// The main WebSocket streaming loop, factored out for testability.
///
/// Reads stdin data from `stdin_rx`, reads WS messages from the server,
/// writes WS messages to the server, and handles resize signals from
/// `sigwinch_rx`. Terminal output (PTY data, overlays, panels) is written
/// to `output`, which is `stdout` in production and a buffer in tests.
pub(crate) async fn ws_streaming_loop(
    mut ws_tx: futures::stream::SplitSink<WebSocketStream<UnixStream>, Message>,
    mut ws_rx: futures::stream::SplitStream<WebSocketStream<UnixStream>>,
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

    // Resize state machine: buffers output during the SIGWINCH → server
    // round-trip to prevent garbled output from stale dimensions.
    let mut resize_state = ResizeState::Idle;
    let mut resize_seq: u64 = 0;
    let resize_deadline = tokio::time::sleep(std::time::Duration::from_secs(86400));
    tokio::pin!(resize_deadline);

    loop {
        tokio::select! {
            // Stdin data -> binary WS frame to server
            data = stdin_rx.recv() => {
                match data {
                    Some(data) => {
                        if crate::input::is_ctrl_backslash(&data) {
                            // Always forward immediately — server handles the toggle
                            if ws_tx.send(Message::Binary(data.clone())).await.is_err() {
                                break;
                            }

                            if pending_detach {
                                // Double-tap: detach (close WS connection)
                                let _ = ws_tx.send(Message::Close(None)).await;
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
                            if ws_tx.send(Message::Binary(data)).await.is_err() {
                                break;
                            }
                        }
                    }
                    None => {
                        // Stdin closed — detach
                        let _ = ws_tx.send(Message::Close(None)).await;
                        break;
                    }
                }
            }

            // WS messages from server -> output
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        // Raw PTY output — behavior depends on resize state
                        match &mut resize_state {
                            ResizeState::Idle => {
                                if !cached_overlays.is_empty() {
                                    let _ = output.write_all(overlay::begin_sync().as_bytes());
                                    let _ = output.write_all(overlay::erase_all_overlays(&cached_overlays).as_bytes());
                                    let _ = output.write_all(&data);
                                    let _ = output.write_all(overlay::render_all_overlays(&cached_overlays).as_bytes());
                                    let _ = output.write_all(overlay::end_sync().as_bytes());
                                } else {
                                    let _ = output.write_all(&data);
                                }
                                let _ = output.flush();
                            }
                            ResizeState::AwaitingResizeAck { buffer, .. } => {
                                // Buffer output generated at old dimensions
                                buffer.push(Bytes::from(data.to_vec()));
                            }
                            ResizeState::AwaitingScreenSync { .. } => {
                                // Post-resize output at new dimensions — write through
                                if !cached_overlays.is_empty() {
                                    let _ = output.write_all(overlay::begin_sync().as_bytes());
                                    let _ = output.write_all(overlay::erase_all_overlays(&cached_overlays).as_bytes());
                                    let _ = output.write_all(&data);
                                    let _ = output.write_all(overlay::render_all_overlays(&cached_overlays).as_bytes());
                                    let _ = output.write_all(overlay::end_sync().as_bytes());
                                } else {
                                    let _ = output.write_all(&data);
                                }
                                let _ = output.flush();
                            }
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        // Parse JSON events
                        if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) {
                            // Handle overlay_sync and panel_sync events (always processed)
                            match msg.get("type").and_then(|t| t.as_str()) {
                                Some("overlay_sync") => {
                                    if let Ok(overlays) = serde_json::from_value::<Vec<Overlay>>(
                                        msg.get("overlays").cloned().unwrap_or_default()
                                    ) {
                                        let _ = output.write_all(overlay::begin_sync().as_bytes());
                                        let _ = output.write_all(overlay::save_cursor().as_bytes());
                                        let _ = output.write_all(overlay::erase_all_overlays(&cached_overlays).as_bytes());
                                        let _ = output.write_all(overlay::render_all_overlays(&overlays).as_bytes());
                                        let _ = output.write_all(overlay::restore_cursor().as_bytes());
                                        let _ = output.write_all(overlay::end_sync().as_bytes());
                                        let _ = output.flush();
                                        cached_overlays = overlays;
                                    }
                                }
                                Some("panel_sync") => {
                                    if let Ok(panels) = serde_json::from_value::<Vec<Panel>>(
                                        msg.get("panels").cloned().unwrap_or_default()
                                    ) {
                                        let (term_rows, term_cols) = crate::terminal::terminal_size().unwrap_or((24, 80));
                                        let _ = render_panel_sync(
                                            output,
                                            &panels,
                                            &cached_panels,
                                            term_rows,
                                            term_cols,
                                        );
                                        cached_panels = panels;
                                    }
                                }
                                _ => {
                                    // Check for resize/screen response IDs from the resize state machine
                                    let msg_id = msg.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
                                    let mut handled = false;

                                    if let Some(ref id) = msg_id {
                                        match &resize_state {
                                            ResizeState::AwaitingResizeAck { resize_id, .. } if id == resize_id => {
                                                // Resize ack received — transition to screen sync
                                                resize_seq += 1;
                                                let screen_id = format!("s-{}", resize_seq);
                                                let dl = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
                                                resize_state = ResizeState::AwaitingScreenSync {
                                                    screen_id: screen_id.clone(),
                                                };
                                                resize_deadline.as_mut().reset(dl);
                                                let _ = send_get_screen(&mut ws_tx, &screen_id).await;
                                                handled = true;
                                            }
                                            ResizeState::AwaitingScreenSync { screen_id, .. } if id == screen_id => {
                                                // Screen response received — repaint and resume
                                                if let Some(result) = msg.get("result") {
                                                    if let Ok(screen) = serde_json::from_value::<ScreenResponse>(result.clone()) {
                                                        let _ = render_screen_sync(&screen, output);
                                                    }
                                                }
                                                resize_state = ResizeState::Idle;
                                                resize_deadline.as_mut().reset(
                                                    tokio::time::Instant::now() + std::time::Duration::from_secs(86400)
                                                );
                                                handled = true;
                                            }
                                            _ => {}
                                        }
                                    }

                                    if !handled {
                                        // Check for error responses
                                        if let Some(err) = msg.get("error") {
                                            if let (Some(code), Some(message)) = (
                                                err.get("code").and_then(|c| c.as_str()),
                                                err.get("message").and_then(|m| m.as_str()),
                                            ) {
                                                eprintln!("wsh: server error: {}: {}", code, message);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = ws_tx.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        // Server closed the connection
                        break;
                    }
                    _ => {
                        // Ignore other message types (Pong, Frame)
                    }
                }
            }

            // SIGWINCH -> resize request to server with buffering state machine
            size = sigwinch_rx.recv() => {
                if let Some((rows, cols)) = size {
                    // Start (or restart) the resize state machine
                    resize_seq += 1;
                    let id = format!("r-{}", resize_seq);
                    let dl = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
                    resize_state = ResizeState::AwaitingResizeAck {
                        resize_id: id.clone(),
                        buffer: Vec::new(),
                    };
                    resize_deadline.as_mut().reset(dl);
                    let _ = send_resize(&mut ws_tx, &id, rows, cols).await;
                }
            }

            // Ctrl+\ double-tap timeout expired -- no detach
            () = &mut detach_timer, if pending_detach => {
                pending_detach = false;
            }

            // Resize state machine timeout — abandon and resume normal output
            () = &mut resize_deadline, if !resize_state.is_idle() => {
                if let ResizeState::AwaitingResizeAck { buffer, .. } = &resize_state {
                    // Flush buffered output to avoid losing data
                    for chunk in buffer {
                        let _ = output.write_all(chunk);
                    }
                    let _ = output.flush();
                }
                resize_state = ResizeState::Idle;
                resize_deadline.as_mut().reset(
                    tokio::time::Instant::now() + std::time::Duration::from_secs(86400)
                );
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
            let layout = crate::panel::compute_layout(&cached_panels, term_rows, term_cols);
            let _ = output.write_all(crate::panel::erase_all_panels(&layout, term_cols).as_bytes());
            let _ = output.write_all(crate::panel::reset_scroll_region().as_bytes());
        }
        let _ = output.flush();
    }

    // Close the WS connection
    let _ = ws_tx.send(Message::Close(None)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::state::{Cursor, FormattedLine, ScreenResponse, Span, Style};

    #[test]
    fn resize_state_is_idle() {
        assert!(ResizeState::Idle.is_idle());
        assert!(!ResizeState::AwaitingResizeAck {
            resize_id: "r-1".to_string(),
            buffer: Vec::new(),
        }.is_idle());
        assert!(!ResizeState::AwaitingScreenSync {
            screen_id: "s-1".to_string(),
        }.is_idle());
    }

    #[test]
    fn render_screen_sync_empty_screen() {
        let screen = ScreenResponse {
            lines: vec![],
            cursor: Cursor { row: 0, col: 0, visible: true },
            ..Default::default()
        };
        let mut buf = Vec::new();
        render_screen_sync(&screen, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        // Should have home+clear and cursor position
        assert!(output.starts_with("\x1b[H\x1b[2J"));
        assert!(output.contains("\x1b[1;1H"));
    }

    #[test]
    fn render_screen_sync_with_lines() {
        let screen = ScreenResponse {
            lines: vec![
                FormattedLine::Plain("hello".to_string()),
                FormattedLine::Plain("world".to_string()),
            ],
            cursor: Cursor { row: 1, col: 3, visible: true },
            cols: 80,
            rows: 24,
            ..Default::default()
        };
        let mut buf = Vec::new();
        render_screen_sync(&screen, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.starts_with("\x1b[H\x1b[2J"));
        assert!(output.contains("hello"));
        assert!(output.contains("\r\n"));
        assert!(output.contains("world"));
        // Cursor at row 2 (1+1), col 4 (3+1)
        assert!(output.ends_with("\x1b[2;4H"));
    }

    #[test]
    fn render_screen_sync_with_styled_lines() {
        let screen = ScreenResponse {
            lines: vec![
                FormattedLine::Styled(vec![
                    Span {
                        text: "bold".to_string(),
                        style: Style { bold: true, ..Style::default() },
                    },
                ]),
            ],
            cursor: Cursor { row: 0, col: 4, visible: true },
            cols: 80,
            rows: 24,
            ..Default::default()
        };
        let mut buf = Vec::new();
        render_screen_sync(&screen, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\x1b[1mbold\x1b[0m"));
    }

    #[test]
    fn render_screen_sync_single_line_no_trailing_newline() {
        let screen = ScreenResponse {
            lines: vec![FormattedLine::Plain("only".to_string())],
            cursor: Cursor { row: 0, col: 0, visible: true },
            ..Default::default()
        };
        let mut buf = Vec::new();
        render_screen_sync(&screen, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        // No \r\n between lines since there's only one
        assert!(!output.contains("\r\n"));
        assert!(output.contains("only"));
    }
}
