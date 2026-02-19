//! Unix socket server for wsh client/server communication.
//!
//! Listens on a Unix domain socket and handles CLI client connections.
//! Each client sends an initial control frame (CreateSession or AttachSession),
//! then enters a streaming loop forwarding I/O between the client's terminal
//! and the server-managed PTY session.

use bytes::Bytes;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixListener;
use tracing;

use crate::panel::layout::compute_layout;
use crate::protocol::*;
use crate::pty::SpawnCommand;
use crate::session::{Session, SessionRegistry};

/// Start the Unix socket server, listening for CLI client connections.
///
/// Runs until the `cancel` token is cancelled, then stops accepting new
/// connections but lets in-flight handlers finish (they exit when sessions drain).
pub async fn serve(
    sessions: SessionRegistry,
    socket_path: &Path,
    cancel: tokio_util::sync::CancellationToken,
    token: Option<String>,
) -> io::Result<()> {
    // Remove stale socket file if it exists, but check for active server first.
    // Uses spawn_blocking to avoid blocking the tokio runtime on the connect() syscall
    // (which could hang if the socket connects to a process that is alive but unresponsive).
    if socket_path.exists() {
        let path_owned = socket_path.to_path_buf();
        let is_active = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::task::spawn_blocking(move || {
                std::os::unix::net::UnixStream::connect(&path_owned).is_ok()
            }),
        )
        .await;

        match is_active {
            Ok(Ok(true)) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("another server is already listening on {}", socket_path.display()),
                ));
            }
            _ => {
                // Socket exists but no server is listening (or check timed out) — stale, safe to remove
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
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::debug!("socket server received cancel signal, stopping accept loop");
                break;
            }
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let sessions = sessions.clone();
                        let token = token.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_client(stream, sessions, token).await {
                                tracing::debug!(?e, "client connection ended");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!(?e, "failed to accept Unix socket connection");
                        // Backoff to prevent tight loop under sustained accept errors
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }
    }
    Ok(())
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
    token: Option<String>,
) -> io::Result<()> {
    // Read initial control frame (with timeout to reject idle connections)
    let frame = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        Frame::read_from(&mut stream),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "initial frame timeout"))?
    ?;

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
        FrameType::ListSessions => {
            handle_list_sessions(&mut stream, sessions).await
        }
        FrameType::KillSession => {
            let msg: KillSessionMsg = frame.parse_json().map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, e)
            })?;
            handle_kill_session(&mut stream, sessions, msg).await
        }
        FrameType::DetachSession => {
            let msg: DetachSessionMsg = frame.parse_json().map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, e)
            })?;
            handle_detach_session(&mut stream, sessions, msg).await
        }
        FrameType::GetToken => {
            handle_get_token(&mut stream, token).await
        }
        other => {
            let err = ErrorMsg {
                code: "invalid_initial_frame".to_string(),
                message: format!(
                    "expected CreateSession, AttachSession, ListSessions, KillSession, DetachSession, or GetToken, got {:?}",
                    other
                ),
            };
            let frame = Frame::control(FrameType::Error, &err)
                .map_err(io::Error::other)?;
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

    // Advisory pre-check — see name_available() doc for TOCTOU rationale.
    // The authoritative check is insert() below.
    sessions.name_available(&msg.name).map_err(|e| {
        io::Error::new(io::ErrorKind::AlreadyExists, e.to_string())
    })?;

    let rows = msg.rows.max(1);
    let cols = msg.cols.max(1);

    let name_for_spawn = msg.name.clone().unwrap_or_default();
    let cwd = msg.cwd;
    let env = msg.env;
    let (session, child_exit_rx) = tokio::task::spawn_blocking(move || {
        Session::spawn_with_options(name_for_spawn, command, rows, cols, cwd, env)
    })
    .await
    .map_err(io::Error::other)?
    .map_err(io::Error::other)?;

    // Validate and set tags before inserting into registry
    if !msg.tags.is_empty() {
        for tag in &msg.tags {
            crate::session::validate_tag(tag).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidInput, format!("invalid tag: {}", e))
            })?;
        }
        *session.tags.write() = msg.tags.into_iter().collect();
    }

    let name = match sessions.insert(msg.name, session.clone()) {
        Ok(name) => name,
        Err(e) => {
            session.shutdown();
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, e.to_string()));
        }
    };

    sessions.monitor_child_exit(name.clone(), session.client_count.clone(), session.child_exited.clone(), child_exit_rx);

    // Send response
    let resp = CreateSessionResponseMsg {
        name: name.clone(),
        pid: session.pid,
        rows,
        cols,
    };
    let resp_frame = Frame::control(FrameType::CreateSessionResponse, &resp)
        .map_err(io::Error::other)?;
    resp_frame.write_to(stream).await?;

    tracing::info!(session = %name, "client created session");

    // Send initial visual state before streaming
    send_initial_visual_state(stream, &session).await?;

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

    // Resize session to match client terminal (clamp to min 1 to avoid zero-size PTY)
    let rows = msg.rows.max(1);
    let cols = msg.cols.max(1);
    if let Err(e) = session.pty.lock().resize(rows, cols) {
        tracing::warn!(?e, "failed to resize PTY on attach");
    }
    if let Err(e) = session.parser.resize(cols as usize, rows as usize).await {
        tracing::warn!(?e, "failed to resize parser on attach");
    }

    // Build scrollback/screen data for replay (using Styled format to
    // preserve colors and attributes for the reconnecting client).
    use crate::parser::ansi::line_to_ansi;
    use crate::parser::state::{Format, Query, QueryResponse};
    let scrollback_data = match msg.scrollback {
        ScrollbackRequest::None => Vec::new(),
        ScrollbackRequest::All | ScrollbackRequest::Lines(_) => {
            let limit = match msg.scrollback {
                ScrollbackRequest::Lines(n) => n,
                _ => usize::MAX,
            };
            match tokio::time::timeout(
                Duration::from_secs(10),
                session.parser.query(Query::Scrollback {
                    format: Format::Styled,
                    offset: 0,
                    limit,
                }),
            ).await {
                Ok(Ok(QueryResponse::Scrollback(sb))) => {
                    let mut buf = String::new();
                    for line in &sb.lines {
                        buf.push_str(&line_to_ansi(line));
                        buf.push_str("\r\n");
                    }
                    buf.into_bytes()
                }
                _ => Vec::new(),
            }
        }
    };

    let screen_data = match tokio::time::timeout(
        Duration::from_secs(10),
        session.parser.query(Query::Screen {
            format: Format::Styled,
        }),
    ).await {
        Ok(Ok(QueryResponse::Screen(screen))) => {
            let mut buf = String::new();
            // Clear screen and home cursor before replaying
            buf.push_str("\x1b[H\x1b[2J");
            for (i, line) in screen.lines.iter().enumerate() {
                buf.push_str(&line_to_ansi(line));
                if i + 1 < screen.lines.len() {
                    buf.push_str("\r\n");
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
        input_mode: session.input_mode.get(),
        screen_mode: *session.screen_mode.read(),
        focused_id: session.focus.focused(),
    };
    let resp_frame = Frame::control(FrameType::AttachSessionResponse, &resp)
        .map_err(io::Error::other)?;
    resp_frame.write_to(stream).await?;

    tracing::info!(session = %msg.name, "client attached to session");

    // Send initial visual state before streaming
    send_initial_visual_state(stream, &session).await?;

    // Enter streaming loop
    run_streaming(stream, &session).await
}

/// Handle a ListSessions request: return all session names and disconnect.
async fn handle_list_sessions<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    sessions: SessionRegistry,
) -> io::Result<()> {
    let names = sessions.list();
    let resp = ListSessionsResponseMsg {
        sessions: names
            .into_iter()
            .filter_map(|name| {
                let session = sessions.get(&name)?;
                let (rows, cols) = session.terminal_size.get();
                let mut tags: Vec<String> = session.tags.read().iter().cloned().collect();
                tags.sort();
                Some(SessionInfoMsg {
                    name,
                    pid: session.pid,
                    command: session.command.clone(),
                    rows,
                    cols,
                    clients: session.clients(),
                    tags,
                })
            })
            .collect(),
    };
    let resp_frame = Frame::control(FrameType::ListSessionsResponse, &resp)
        .map_err(io::Error::other)?;
    resp_frame.write_to(stream).await?;
    Ok(())
}

/// Handle a KillSession request: remove the session or return an error.
async fn handle_kill_session<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    sessions: SessionRegistry,
    msg: KillSessionMsg,
) -> io::Result<()> {
    match sessions.remove(&msg.name) {
        Some(session) => {
            session.force_kill();
            tracing::info!(session = %msg.name, "session killed via socket");
            let resp = KillSessionResponseMsg { name: msg.name };
            let resp_frame = Frame::control(FrameType::KillSessionResponse, &resp)
                .map_err(io::Error::other)?;
            resp_frame.write_to(stream).await?;
            Ok(())
        }
        None => {
            let err = ErrorMsg {
                code: "session_not_found".to_string(),
                message: format!("session not found: {}", msg.name),
            };
            let err_frame = Frame::control(FrameType::Error, &err)
                .map_err(io::Error::other)?;
            err_frame.write_to(stream).await?;
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("session not found: {}", msg.name),
            ))
        }
    }
}

/// Handle a DetachSession request: signal the session to detach attached clients.
async fn handle_detach_session<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    sessions: SessionRegistry,
    msg: DetachSessionMsg,
) -> io::Result<()> {
    match sessions.get(&msg.name) {
        Some(session) => {
            session.detach();
            tracing::info!(session = %msg.name, "session detached via socket");
            let resp = DetachSessionResponseMsg { name: msg.name };
            let resp_frame = Frame::control(FrameType::DetachSessionResponse, &resp)
                .map_err(io::Error::other)?;
            resp_frame.write_to(stream).await?;
            Ok(())
        }
        None => {
            let err = ErrorMsg {
                code: "session_not_found".to_string(),
                message: format!("session not found: {}", msg.name),
            };
            let err_frame = Frame::control(FrameType::Error, &err)
                .map_err(io::Error::other)?;
            err_frame.write_to(stream).await?;
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("session not found: {}", msg.name),
            ))
        }
    }
}

/// Handle a GetToken request: return the server's auth token (if configured).
async fn handle_get_token<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    token: Option<String>,
) -> io::Result<()> {
    let resp = GetTokenResponseMsg { token };
    let resp_frame = Frame::control(FrameType::GetTokenResponse, &resp)
        .map_err(io::Error::other)?;
    resp_frame.write_to(stream).await?;
    Ok(())
}

/// Send initial overlay and panel state to a newly connected client.
///
/// Called after sending CreateSessionResponse or AttachSessionResponse,
/// before entering the streaming loop.
async fn send_initial_visual_state<S: AsyncWrite + Unpin>(
    stream: &mut S,
    session: &Session,
) -> io::Result<()> {
    // Filter overlays and panels by the current screen mode so clients
    // don't receive visuals intended for a different mode (e.g. normal-mode
    // overlays shouldn't appear while vim has the alternate screen active).
    let mode = *session.screen_mode.read();

    // Send current overlay state
    let overlays = session.overlays.list_by_mode(mode);
    if !overlays.is_empty() {
        let msg = OverlaySyncMsg { overlays };
        let frame = Frame::control(FrameType::OverlaySync, &msg)
            .map_err(io::Error::other)?;
        frame.write_to(stream).await?;
    }

    // Send current panel state
    let panels = session.panels.list_by_mode(mode);
    if !panels.is_empty() {
        let (term_rows, term_cols) = session.terminal_size.get();
        let layout = compute_layout(&panels, term_rows, term_cols);
        let msg = PanelSyncMsg {
            panels,
            scroll_region_top: layout.scroll_region_top,
            scroll_region_bottom: layout.scroll_region_bottom,
        };
        let frame = Frame::control(FrameType::PanelSync, &msg)
            .map_err(io::Error::other)?;
        frame.write_to(stream).await?;
    }

    Ok(())
}

const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Write a frame with a timeout. Returns false if the write failed or timed out.
async fn write_frame_with_timeout<W: AsyncWriteExt + Unpin>(
    frame: &Frame,
    writer: &mut W,
) -> bool {
    tokio::time::timeout(WRITE_TIMEOUT, frame.write_to(writer))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
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
    let _client_guard = match session.connect() {
        Some(guard) => guard,
        None => {
            tracing::warn!(session = %session.name, "connection limit reached, rejecting socket client");
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "too many clients connected to session",
            ));
        }
    };
    let (reader, mut writer) = tokio::io::split(stream);
    // BufReader preserves partially-read bytes across select! cancellation,
    // making Frame::read_from cancellation-safe.
    let mut reader = tokio::io::BufReader::new(reader);

    // Subscribe to session output
    let mut output_rx = session.output_rx.subscribe();

    let input_tx = session.input_tx.clone();
    let pty = session.pty.clone();
    let parser = session.parser.clone();
    let activity = session.activity.clone();
    let terminal_size = session.terminal_size.clone();
    let input_mode = session.input_mode.clone();
    let input_broadcaster = session.input_broadcaster.clone();
    let focus = session.focus.clone();
    let mut detach_rx = session.detach_signal.subscribe();
    let mut visual_update_rx = session.visual_update_tx.subscribe();

    // Keepalive: server sends Ping every 30s, expects Pong within 10s.
    // Without this, idle sessions would rely on a hard read timeout to
    // detect stale connections (dead SSH, killed client, etc.).
    let mut ping_interval = tokio::time::interval(Duration::from_secs(30));
    ping_interval.tick().await; // consume the immediate first tick
    let mut ping_sent = false;
    let mut last_pong = tokio::time::Instant::now();
    let pong_timeout = Duration::from_secs(10);

    // Main loop: read from client and session output concurrently
    loop {
        tokio::select! {
            // Session was killed/removed → send Detach frame and break
            _ = session.cancelled.cancelled() => {
                tracing::debug!("session was killed, closing socket connection");
                let detach_frame = Frame::new(FrameType::Detach, Bytes::new());
                let _ = write_frame_with_timeout(&detach_frame, &mut writer).await;
                break;
            }

            // Remote detach signal → send Detach frame to client and break
            _ = detach_rx.recv() => {
                let detach_frame = Frame::new(FrameType::Detach, Bytes::new());
                let _ = write_frame_with_timeout(&detach_frame, &mut writer).await;
                break;
            }

            // Visual state changes → send OverlaySync or PanelSync frame
            result = visual_update_rx.recv() => {
                match result {
                    Ok(VisualUpdate::OverlaysChanged) => {
                        let mode = *session.screen_mode.read();
                        let msg = OverlaySyncMsg {
                            overlays: session.overlays.list_by_mode(mode),
                        };
                        if let Ok(frame) = Frame::control(FrameType::OverlaySync, &msg) {
                            if !write_frame_with_timeout(&frame, &mut writer).await {
                                break;
                            }
                        }
                    }
                    Ok(VisualUpdate::PanelsChanged) => {
                        let mode = *session.screen_mode.read();
                        let panels = session.panels.list_by_mode(mode);
                        let (term_rows, term_cols) = terminal_size.get();
                        let layout = compute_layout(&panels, term_rows, term_cols);
                        let msg = PanelSyncMsg {
                            panels,
                            scroll_region_top: layout.scroll_region_top,
                            scroll_region_bottom: layout.scroll_region_bottom,
                        };
                        if let Ok(frame) = Frame::control(FrameType::PanelSync, &msg) {
                            if !write_frame_with_timeout(&frame, &mut writer).await {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "socket client lagged on visual updates, skipping missed frames");
                    }
                }
            }

            // Output from session → client
            result = output_rx.recv() => {
                match result {
                    Ok(data) => {
                        let frame = Frame::data(FrameType::PtyOutput, data);
                        if !write_frame_with_timeout(&frame, &mut writer).await {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "socket client lagged on output, sending screen sync");
                        // ── DESIGN DECISION: lag recovery strategy ──────────────
                        //
                        // This has been deliberated across multiple review cycles.
                        // Three approaches were considered:
                        //
                        // 1. DISCONNECT (break) — force the client to reconnect
                        //    and reattach, getting full state via AttachResponse.
                        //    Pros: simple, guarantees correct state.
                        //    Cons: disrupts AI agents mid-operation; they lose
                        //    their streaming loop and must implement reconnect
                        //    logic; transient output between disconnect and
                        //    reattach is lost from the agent's perspective.
                        //    Tried in: commit afdbc6e, reverted here.
                        //
                        // 2. LOG-AND-CONTINUE (no sync) — skip the missed
                        //    messages and keep streaming.
                        //    Pros: simplest, no disruption.
                        //    Cons: client has a permanently stale/corrupt
                        //    terminal view until enough new output happens to
                        //    overwrite the screen. Unacceptable for agents that
                        //    read the screen after sending commands.
                        //    Tried in: commit d2e945b, rejected.
                        //
                        // 3. LOG + FULL SCREEN SYNC (chosen) — query the parser
                        //    for current screen state, convert to raw ANSI bytes,
                        //    and send as a PtyOutput frame. Client stays connected
                        //    with a correct view.
                        //    Pros: no disruption, correct state, matches ws_json
                        //    behavior. Cons: parser query adds brief latency.
                        //
                        // We chose (3). Do not change this to (1) or (2) without
                        // revisiting the above tradeoffs.
                        // ────────────────────────────────────────────────────────
                        use crate::parser::ansi::line_to_ansi;
                        use crate::parser::state::{Format, Query, QueryResponse};
                        if let Ok(Ok(QueryResponse::Screen(screen))) = tokio::time::timeout(
                            Duration::from_secs(5),
                            parser.query(Query::Screen { format: Format::Styled }),
                        ).await {
                            let mut buf = String::new();
                            buf.push_str("\x1b[H\x1b[2J");
                            for (i, line) in screen.lines.iter().enumerate() {
                                buf.push_str(&line_to_ansi(line));
                                if i + 1 < screen.lines.len() {
                                    buf.push_str("\r\n");
                                }
                            }
                            buf.push_str(&format!(
                                "\x1b[{};{}H",
                                screen.cursor.row + 1,
                                screen.cursor.col + 1,
                            ));
                            let sync_frame = Frame::data(FrameType::PtyOutput, Bytes::from(buf.into_bytes()));
                            if !write_frame_with_timeout(&sync_frame, &mut writer).await {
                                break;
                            }
                        }
                    }
                }
            }

            // Ping keepalive — detect stale connections (dead SSH, killed client)
            _ = ping_interval.tick() => {
                if ping_sent && last_pong.elapsed() > pong_timeout {
                    tracing::debug!("socket client unresponsive (no pong), disconnecting");
                    break;
                }
                let ping_frame = Frame::new(FrameType::Ping, Bytes::new());
                if !write_frame_with_timeout(&ping_frame, &mut writer).await {
                    break;
                }
                ping_sent = true;
            }

            // Frames from client
            result = Frame::read_from(&mut reader) => {
                match result {
                    Ok(f) => {
                        match f.frame_type {
                            FrameType::Pong => {
                                last_pong = tokio::time::Instant::now();
                                ping_sent = false;
                            }
                            FrameType::StdinInput => {
                                let data = &f.payload;
                                let mode = input_mode.get();
                                let target = focus.focused();
                                input_broadcaster.broadcast_input(data, mode, target);
                                activity.touch();

                                // Ctrl+\ toggles input capture; never forwarded to PTY
                                if crate::input::is_ctrl_backslash(data) {
                                    let new_mode = input_mode.toggle();
                                    input_broadcaster.broadcast_mode(new_mode);
                                    tracing::debug!("Ctrl+\\ pressed, toggled to {new_mode:?} mode");
                                    continue;
                                }

                                // In capture mode, don't forward to PTY
                                if mode == crate::input::Mode::Capture {
                                    continue;
                                }

                                match tokio::time::timeout(
                                    std::time::Duration::from_secs(5),
                                    input_tx.send(f.payload),
                                ).await {
                                    Ok(Ok(())) => {}
                                    Ok(Err(_)) => break, // channel closed
                                    Err(_) => {
                                        tracing::warn!("input_tx.send timed out in socket streaming");
                                        break;
                                    }
                                }
                            }
                            FrameType::Resize => {
                                if let Ok(msg) = f.parse_json::<ResizeMsg>() {
                                    let rows = msg.rows.max(1);
                                    let cols = msg.cols.max(1);
                                    terminal_size.set(rows, cols);
                                    if let Err(e) = pty.lock().resize(rows, cols) {
                                        tracing::warn!(?e, "failed to resize PTY");
                                    }
                                    if let Err(e) = parser.resize(
                                        cols as usize,
                                        rows as usize,
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

    /// Start a test server on a temporary socket and return the path and TempDir.
    /// The caller must keep the TempDir alive for the duration of the test.
    async fn start_test_server(sessions: SessionRegistry) -> (PathBuf, TempDir) {
        start_test_server_with_token(sessions, None).await
    }

    async fn start_test_server_with_token(sessions: SessionRegistry, token: Option<String>) -> (PathBuf, TempDir) {
        let dir = TempDir::new().unwrap();
        let socket_path = dir.path().join("test.sock");
        let path = socket_path.clone();

        let cancel = tokio_util::sync::CancellationToken::new();
        tokio::spawn(async move {
            serve(sessions, &socket_path, cancel, token).await.unwrap();
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
    async fn test_create_session_via_socket() {
        let sessions = SessionRegistry::new();
        let (path, _dir) = start_test_server(sessions.clone()).await;

        let mut stream = UnixStream::connect(&path).await.unwrap();

        // Send CreateSession
        let msg = CreateSessionMsg {
            name: Some("test-create".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
            tags: vec![],
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
        let identity = session.client_count.clone();
        let child_exited = session.child_exited.clone();
        sessions.insert(Some("attach-target".to_string()), session).unwrap();
        sessions.monitor_child_exit("attach-target".to_string(), identity, child_exited, child_exit_rx);

        let (path, _dir) = start_test_server(sessions.clone()).await;

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
        let (path, _dir) = start_test_server(sessions).await;

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
        let (path, _dir) = start_test_server(sessions.clone()).await;

        let mut stream = UnixStream::connect(&path).await.unwrap();

        // Create a session
        let msg = CreateSessionMsg {
            name: Some("stdin-test".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
            tags: vec![],
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
        while let Ok(Ok(frame)) = tokio::time::timeout_at(deadline, Frame::read_from(&mut stream)).await {
            if frame.frame_type == FrameType::PtyOutput {
                received_output = true;
                let output = String::from_utf8_lossy(&frame.payload);
                if output.contains("hello") {
                    break;
                }
            }
        }
        assert!(received_output, "should have received PTY output");

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_detach_ends_streaming() {
        let sessions = SessionRegistry::new();
        let (path, _dir) = start_test_server(sessions.clone()).await;

        let mut stream = UnixStream::connect(&path).await.unwrap();

        // Create session
        let msg = CreateSessionMsg {
            name: Some("detach-test".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
            tags: vec![],
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
        let (path, _dir) = start_test_server(sessions.clone()).await;

        let mut stream = UnixStream::connect(&path).await.unwrap();

        // Create session
        let msg = CreateSessionMsg {
            name: Some("resize-test".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
            tags: vec![],
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
        let (path, _dir) = start_test_server(sessions).await;

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

    #[tokio::test]
    async fn test_list_sessions_empty() {
        let sessions = SessionRegistry::new();
        let (path, _dir) = start_test_server(sessions).await;

        let mut stream = UnixStream::connect(&path).await.unwrap();

        let msg = ListSessionsMsg {};
        Frame::control(FrameType::ListSessions, &msg)
            .unwrap()
            .write_to(&mut stream)
            .await
            .unwrap();

        let resp = Frame::read_from(&mut stream).await.unwrap();
        assert_eq!(resp.frame_type, FrameType::ListSessionsResponse);
        let list: ListSessionsResponseMsg = resp.parse_json().unwrap();
        assert!(list.sessions.is_empty());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_list_sessions_with_sessions() {
        let sessions = SessionRegistry::new();

        // Pre-create two sessions
        let (session1, rx1) = Session::spawn(
            "list-a".to_string(),
            SpawnCommand::default(),
            24, 80,
        ).unwrap();
        let id1 = session1.client_count.clone();
        let child_exited1 = session1.child_exited.clone();
        sessions.insert(Some("list-a".to_string()), session1).unwrap();
        sessions.monitor_child_exit("list-a".to_string(), id1, child_exited1, rx1);

        let (session2, rx2) = Session::spawn(
            "list-b".to_string(),
            SpawnCommand::default(),
            24, 80,
        ).unwrap();
        let id2 = session2.client_count.clone();
        let child_exited2 = session2.child_exited.clone();
        sessions.insert(Some("list-b".to_string()), session2).unwrap();
        sessions.monitor_child_exit("list-b".to_string(), id2, child_exited2, rx2);

        let (path, _dir) = start_test_server(sessions).await;

        let mut stream = UnixStream::connect(&path).await.unwrap();
        Frame::control(FrameType::ListSessions, &ListSessionsMsg {})
            .unwrap()
            .write_to(&mut stream)
            .await
            .unwrap();

        let resp = Frame::read_from(&mut stream).await.unwrap();
        assert_eq!(resp.frame_type, FrameType::ListSessionsResponse);
        let list: ListSessionsResponseMsg = resp.parse_json().unwrap();
        assert_eq!(list.sessions.len(), 2);
        let names: Vec<&str> = list.sessions.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"list-a"));
        assert!(names.contains(&"list-b"));

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_kill_session_success() {
        let sessions = SessionRegistry::new();

        let (session, rx) = Session::spawn(
            "kill-me".to_string(),
            SpawnCommand::default(),
            24, 80,
        ).unwrap();
        let identity = session.client_count.clone();
        let child_exited = session.child_exited.clone();
        sessions.insert(Some("kill-me".to_string()), session).unwrap();
        sessions.monitor_child_exit("kill-me".to_string(), identity, child_exited, rx);

        let (path, _dir) = start_test_server(sessions.clone()).await;

        let mut stream = UnixStream::connect(&path).await.unwrap();
        let msg = KillSessionMsg { name: "kill-me".to_string() };
        Frame::control(FrameType::KillSession, &msg)
            .unwrap()
            .write_to(&mut stream)
            .await
            .unwrap();

        let resp = Frame::read_from(&mut stream).await.unwrap();
        assert_eq!(resp.frame_type, FrameType::KillSessionResponse);
        let kill_resp: KillSessionResponseMsg = resp.parse_json().unwrap();
        assert_eq!(kill_resp.name, "kill-me");

        // Session should be gone
        assert!(sessions.get("kill-me").is_none());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_kill_session_not_found() {
        let sessions = SessionRegistry::new();
        let (path, _dir) = start_test_server(sessions).await;

        let mut stream = UnixStream::connect(&path).await.unwrap();
        let msg = KillSessionMsg { name: "nonexistent".to_string() };
        Frame::control(FrameType::KillSession, &msg)
            .unwrap()
            .write_to(&mut stream)
            .await
            .unwrap();

        let resp = Frame::read_from(&mut stream).await.unwrap();
        assert_eq!(resp.frame_type, FrameType::Error);
        let err: ErrorMsg = resp.parse_json().unwrap();
        assert_eq!(err.code, "session_not_found");

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_get_token_returns_none() {
        let sessions = SessionRegistry::new();
        let (path, _dir) = start_test_server(sessions).await;

        let mut stream = UnixStream::connect(&path).await.unwrap();
        let msg = GetTokenMsg {};
        Frame::control(FrameType::GetToken, &msg)
            .unwrap()
            .write_to(&mut stream)
            .await
            .unwrap();

        let resp = Frame::read_from(&mut stream).await.unwrap();
        assert_eq!(resp.frame_type, FrameType::GetTokenResponse);
        let token_resp: GetTokenResponseMsg = resp.parse_json().unwrap();
        assert_eq!(token_resp.token, None);

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_get_token_returns_token() {
        let sessions = SessionRegistry::new();
        let (path, _dir) = start_test_server_with_token(
            sessions,
            Some("test-secret-token".to_string()),
        ).await;

        let mut stream = UnixStream::connect(&path).await.unwrap();
        let msg = GetTokenMsg {};
        Frame::control(FrameType::GetToken, &msg)
            .unwrap()
            .write_to(&mut stream)
            .await
            .unwrap();

        let resp = Frame::read_from(&mut stream).await.unwrap();
        assert_eq!(resp.frame_type, FrameType::GetTokenResponse);
        let token_resp: GetTokenResponseMsg = resp.parse_json().unwrap();
        assert_eq!(token_resp.token, Some("test-secret-token".to_string()));

        std::fs::remove_file(&path).ok();
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
            tags: vec![],
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
        while let Ok(Ok(_)) = tokio::time::timeout_at(deadline, Frame::read_from(&mut stream)).await {}

        stream
    }

    #[tokio::test]
    async fn test_attach_scrollback_none_returns_empty() {
        let sessions = SessionRegistry::new();
        let (path, _dir) = start_test_server(sessions.clone()).await;

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
        let (path, _dir) = start_test_server(sessions.clone()).await;

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
        let (path, _dir) = start_test_server(sessions.clone()).await;

        // Create a session and generate enough output to have scrollback
        let mut stream = UnixStream::connect(&path).await.unwrap();
        let msg = CreateSessionMsg {
            name: Some("sb-lines-test".to_string()),
            command: Some("bash".to_string()),
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
            tags: vec![],
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
        while let Ok(Ok(_)) = tokio::time::timeout_at(deadline, Frame::read_from(&mut stream)).await {}

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
        let (path, _dir) = start_test_server(sessions.clone()).await;

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
