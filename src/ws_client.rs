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

use std::io;
use std::path::Path;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::net::UnixStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::client::render_panel_sync;
use crate::overlay::{self, Overlay};
use crate::panel::Panel;

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

/// Send a resize request over the WebSocket.
async fn send_resize(
    ws: &mut futures::stream::SplitSink<WebSocketStream<UnixStream>, Message>,
    rows: u16,
    cols: u16,
) -> io::Result<()> {
    let resize_msg = serde_json::json!({
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
                        // Raw PTY output
                        if !cached_overlays.is_empty() {
                            // Erase overlays, write PTY output, re-render overlays
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
                    Some(Ok(Message::Text(text))) => {
                        // Parse JSON events
                        if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) {
                            match msg.get("type").and_then(|t| t.as_str()) {
                                Some("overlay_sync") => {
                                    if let Ok(overlays) = serde_json::from_value::<Vec<Overlay>>(
                                        msg.get("overlays").cloned().unwrap_or_default()
                                    ) {
                                        let _ = output.write_all(overlay::begin_sync().as_bytes());
                                        let _ = output.write_all(overlay::save_cursor().as_bytes());
                                        // Erase old overlays
                                        let _ = output.write_all(overlay::erase_all_overlays(&cached_overlays).as_bytes());
                                        // Render new overlays
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
                                    // Ignore other JSON messages (subscribe responses, Sync events, etc.)
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

            // SIGWINCH -> resize request to server
            size = sigwinch_rx.recv() => {
                if let Some((rows, cols)) = size {
                    let _ = send_resize(&mut ws_tx, rows, cols).await;
                }
            }

            // Ctrl+\ double-tap timeout expired -- no detach
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
