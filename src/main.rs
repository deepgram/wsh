//! wsh - The Web Shell
//!
//! A transparent PTY wrapper that exposes terminal I/O via HTTP/WebSocket API.
//!
//! ## Modes
//!
//! **Default** (no subcommand): Connects to an existing server (or auto-spawns
//! an ephemeral one), creates a session, and attaches — acting as a thin
//! terminal client.
//!
//! **Server mode** (`wsh server`): Starts a headless daemon with HTTP/WS and
//! Unix socket listeners. Runs in persistent mode by default (stays alive when
//! sessions end). Use `--ephemeral` to exit when the last session ends.

use clap::{Parser as ClapParser, Subcommand};
use std::net::SocketAddr;
use std::path::PathBuf;
use thiserror::Error;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use wsh::{
    api, client, protocol,
    protocol::{AttachSessionMsg, ScrollbackRequest},
    server,
    session::SessionRegistry,
    shutdown::ShutdownCoordinator,
    terminal,
};

/// wsh - The Web Shell
///
/// A transparent PTY wrapper that exposes terminal I/O via HTTP/WebSocket API.
/// Run your shell inside wsh to access it from web browsers, agents, and other tools.
#[derive(ClapParser, Debug)]
#[command(name = "wsh", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Address to bind the HTTP/WebSocket API server
    #[arg(long, default_value = "127.0.0.1:8080")]
    bind: SocketAddr,

    /// Command string to execute (like sh -c)
    #[arg(short = 'c')]
    cmd: Option<String>,

    /// Force interactive mode
    #[arg(short = 'i')]
    interactive: bool,

    /// Authentication token for non-localhost bindings
    #[arg(long, env = "WSH_TOKEN")]
    token: Option<String>,

    /// Shell to spawn (overrides $SHELL)
    #[arg(long)]
    shell: Option<String>,

    /// Name for the initial session
    #[arg(long)]
    name: Option<String>,

    /// Use alternate screen buffer (restores previous screen on exit, but
    /// disables native terminal scrollback while wsh is running)
    #[arg(long)]
    alt_screen: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the wsh server daemon (headless, no local terminal)
    Server {
        /// Address to bind the HTTP/WebSocket API server
        #[arg(long, default_value = "127.0.0.1:8080")]
        bind: SocketAddr,

        /// Authentication token for non-localhost bindings
        #[arg(long, env = "WSH_TOKEN")]
        token: Option<String>,

        /// Path to the Unix domain socket
        #[arg(long)]
        socket: Option<PathBuf>,

        /// Run in ephemeral mode (exit when last session ends).
        /// By default, `wsh server` runs in persistent mode.
        #[arg(long)]
        ephemeral: bool,
    },

    /// Attach to an existing session on the server
    Attach {
        /// Session name to attach to
        name: String,

        /// Scrollback to replay: "all", "none", or a number of lines
        #[arg(long, default_value = "all")]
        scrollback: String,

        /// Path to the Unix domain socket
        #[arg(long)]
        socket: Option<PathBuf>,

        /// Use alternate screen buffer (restores previous screen on exit, but
        /// disables native terminal scrollback while wsh is running)
        #[arg(long)]
        alt_screen: bool,
    },

    /// List active sessions on the server
    List {
        /// Path to the Unix domain socket
        #[arg(long)]
        socket: Option<PathBuf>,
    },

    /// Kill (destroy) a session on the server
    Kill {
        /// Session name to kill
        name: String,

        /// Path to the Unix domain socket
        #[arg(long)]
        socket: Option<PathBuf>,
    },

    /// Detach all clients from a session (session stays alive)
    Detach {
        /// Session name to detach
        name: String,

        /// Path to the Unix domain socket
        #[arg(long)]
        socket: Option<PathBuf>,
    },

    /// Query or set server persistence mode.
    ///
    /// With no argument, prints the current persistence state.
    /// `wsh persist on` — server stays alive when all sessions end.
    /// `wsh persist off` — server exits when the last session ends.
    Persist {
        /// "on" or "off". Omit to query without changing.
        value: Option<String>,

        /// Address of the HTTP/WebSocket API server
        #[arg(long, default_value = "127.0.0.1:8080")]
        bind: SocketAddr,

        /// Authentication token
        #[arg(long, env = "WSH_TOKEN")]
        token: Option<String>,
    },

    /// Start an MCP server over stdio (for AI hosts like Claude Desktop)
    Mcp {
        /// Address to bind the HTTP/WebSocket API server (for auto-spawn)
        #[arg(long, default_value = "127.0.0.1:8080")]
        bind: SocketAddr,

        /// Path to the Unix domain socket
        #[arg(long)]
        socket: Option<PathBuf>,

        /// Authentication token
        #[arg(long, env = "WSH_TOKEN")]
        token: Option<String>,
    },
}

#[derive(Error, Debug)]
pub enum WshError {
    #[error("pty error: {0}")]
    Pty(#[from] wsh::pty::PtyError),

    #[error("terminal error: {0}")]
    Terminal(#[from] terminal::TerminalError),

    #[error("task join error: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

fn is_loopback(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

fn resolve_token(bind: &SocketAddr, user_token: &Option<String>) -> Option<String> {
    if is_loopback(bind) {
        return None;
    }
    match user_token {
        Some(token) => Some(token.clone()),
        None => {
            use rand::Rng;
            let token: String = rand::thread_rng()
                .sample_iter(&rand::distributions::Alphanumeric)
                .take(32)
                .map(char::from)
                .collect();
            eprintln!("wsh: API token (required for non-localhost): {}", token);
            Some(token)
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), WshError> {
    let cli = Cli::parse();

    // MCP mode: tracing must use stderr since stdout is for MCP protocol
    let is_mcp = matches!(cli.command, Some(Commands::Mcp { .. }));
    if is_mcp {
        init_tracing_stderr();
    } else {
        init_tracing();
    }

    match cli.command {
        Some(Commands::Server { bind, token, socket, ephemeral }) => {
            run_server(bind, token, socket, ephemeral).await
        }
        Some(Commands::Attach { name, scrollback, socket, alt_screen }) => {
            run_attach(name, scrollback, socket, alt_screen).await
        }
        Some(Commands::List { socket }) => {
            run_list(socket).await
        }
        Some(Commands::Kill { name, socket }) => {
            run_kill(name, socket).await
        }
        Some(Commands::Detach { name, socket }) => {
            run_detach(name, socket).await
        }
        Some(Commands::Persist { value, bind, token }) => {
            run_persist(value, bind, token).await
        }
        Some(Commands::Mcp { bind, socket, token }) => {
            run_mcp(bind, socket, token).await
        }
        None => {
            run_standalone(cli).await
        }
    }
}

fn init_tracing() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "wsh=info,tower_http=info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();
}

/// Initialize tracing with stderr output.
///
/// MCP mode uses stdout for the JSON-RPC protocol, so all tracing MUST go
/// to stderr to avoid corrupting the protocol stream.
fn init_tracing_stderr() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "wsh=info".into()),
        ))
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();
}

// ── Server mode ────────────────────────────────────────────────────

/// Run the wsh server daemon: HTTP/WS + Unix socket, no local terminal.
async fn run_server(
    bind: SocketAddr,
    token: Option<String>,
    socket: Option<PathBuf>,
    ephemeral: bool,
) -> Result<(), WshError> {
    tracing::info!("wsh server starting");

    let token = resolve_token(&bind, &token);
    if token.is_some() {
        tracing::info!("auth token configured");
    }

    let persistent = !ephemeral;
    let sessions = SessionRegistry::new();
    let shutdown = ShutdownCoordinator::new();
    let server_config = std::sync::Arc::new(api::ServerConfig::new(persistent));
    let state = api::AppState {
        sessions: sessions.clone(),
        shutdown: shutdown.clone(),
        server_config: server_config.clone(),
    };

    let app = api::router(state, token);
    tracing::info!(addr = %bind, "HTTP/WS server listening");

    // Oneshot channel for server shutdown (Ctrl+C or ephemeral exit)
    let (server_shutdown_tx, server_shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let http_handle = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                server_shutdown_rx.await.ok();
            })
            .await
            .unwrap();
    });

    // Start Unix socket server
    let socket_path = socket.unwrap_or_else(server::default_socket_path);
    let socket_path_for_cleanup = socket_path.clone();
    let socket_sessions = sessions.clone();
    let socket_handle = tokio::spawn(async move {
        if let Err(e) = server::serve(socket_sessions, &socket_path).await {
            tracing::error!(?e, "Unix socket server error");
        }
    });

    tracing::info!("wsh server ready");

    // Ephemeral shutdown monitor: when the last session exits in non-persistent
    // mode, shut down the server automatically.
    let config_for_monitor = server_config.clone();
    let sessions_for_monitor = sessions.clone();
    let ephemeral_handle = tokio::spawn(async move {
        let mut events = sessions_for_monitor.subscribe_events();
        loop {
            match events.recv().await {
                Ok(event) => {
                    let is_removal = matches!(
                        event,
                        wsh::session::SessionEvent::Destroyed { .. }
                    );
                    if is_removal
                        && !config_for_monitor.is_persistent()
                        && sessions_for_monitor.len() == 0
                    {
                        tracing::info!(
                            "last session ended, ephemeral server shutting down"
                        );
                        return true;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    });

    // Wait for either Ctrl+C or ephemeral shutdown
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("received Ctrl+C");
        }
        result = ephemeral_handle => {
            match result {
                Ok(true) => {
                    tracing::debug!("ephemeral shutdown triggered");
                }
                _ => {}
            }
        }
    }

    // Signal WebSocket handlers to send close frames
    shutdown.shutdown();
    // Give handlers a moment to flush close frames before stopping the server
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let _ = server_shutdown_tx.send(());

    // Wait for HTTP server to stop
    if let Err(e) = http_handle.await {
        tracing::warn!(?e, "HTTP server task panicked");
    }

    // Clean up
    socket_handle.abort();

    // Remove the socket file so a subsequent server can bind
    if socket_path_for_cleanup.exists() {
        let _ = std::fs::remove_file(&socket_path_for_cleanup);
        tracing::debug!(path = %socket_path_for_cleanup.display(), "removed socket file");
    }

    tracing::info!("wsh server exiting");
    Ok(())
}

// ── MCP stdio mode ─────────────────────────────────────────────────

/// Run the MCP stdio bridge: connect to (or spawn) a server, then bridge
/// stdin/stdout JSON-RPC ↔ the server's `/mcp` Streamable HTTP endpoint.
async fn run_mcp(
    bind: SocketAddr,
    socket: Option<PathBuf>,
    token: Option<String>,
) -> Result<(), WshError> {
    tracing::info!("wsh mcp stdio bridge starting");

    let socket_path = socket.unwrap_or_else(server::default_socket_path);

    // Connect to existing server or spawn one
    match client::Client::connect(&socket_path).await {
        Ok(_) => {
            tracing::debug!("connected to existing server");
        }
        Err(_) => {
            tracing::debug!("no server running, spawning daemon");
            spawn_server_daemon(&socket_path, &bind, token.as_deref())?;
            wait_for_socket(&socket_path).await?;
        }
    }

    let mcp_url = format!("http://{}/mcp", bind);
    let http_client = reqwest::Client::new();
    let mut session_id: Option<String> = None;

    let stdin = tokio::io::stdin();
    let mut reader = tokio::io::BufReader::new(stdin);
    let mut stdout = tokio::io::stdout();

    let mut line = String::new();
    loop {
        line.clear();
        let n = tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line)
            .await
            .map_err(WshError::Io)?;
        if n == 0 {
            // EOF on stdin
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Build HTTP request
        let mut req = http_client
            .post(&mcp_url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");

        if let Some(ref sid) = session_id {
            req = req.header("Mcp-Session-Id", sid);
        }
        if let Some(ref t) = token {
            req = req.bearer_auth(t);
        }

        req = req.body(trimmed.to_string());

        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(?e, "HTTP request to /mcp failed");
                // Write a JSON-RPC error to stdout
                let err_json = serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32603,
                        "message": format!("HTTP request failed: {e}")
                    },
                    "id": null
                });
                let err_line = format!("{}\n", err_json);
                tokio::io::AsyncWriteExt::write_all(&mut stdout, err_line.as_bytes())
                    .await
                    .map_err(WshError::Io)?;
                tokio::io::AsyncWriteExt::flush(&mut stdout)
                    .await
                    .map_err(WshError::Io)?;
                continue;
            }
        };

        // Capture headers before consuming the body
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        // Capture mcp-session-id from response headers
        if let Some(sid) = resp.headers().get("mcp-session-id") {
            if let Ok(s) = sid.to_str() {
                session_id = Some(s.to_string());
            }
        }

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        if !status.is_success() && !status.is_informational() {
            tracing::warn!(status = %status, "MCP endpoint returned error");
            // Try to pass through the body as-is (it may be a JSON-RPC error)
            if !body.trim().is_empty() {
                let out_line = format!("{}\n", body.trim());
                tokio::io::AsyncWriteExt::write_all(&mut stdout, out_line.as_bytes())
                    .await
                    .map_err(WshError::Io)?;
                tokio::io::AsyncWriteExt::flush(&mut stdout)
                    .await
                    .map_err(WshError::Io)?;
            }
            continue;
        }

        // Parse SSE response: look for `data:` lines in event-stream format
        // The response may be plain JSON or SSE depending on content type
        if content_type.contains("text/event-stream") || body.contains("data:") {
            // Parse as SSE
            for event in body.split("\n\n") {
                let event = event.trim();
                if event.is_empty() {
                    continue;
                }
                for event_line in event.lines() {
                    if let Some(data) = event_line.strip_prefix("data:") {
                        let json_str = data.trim();
                        if !json_str.is_empty() {
                            let out_line = format!("{}\n", json_str);
                            tokio::io::AsyncWriteExt::write_all(
                                &mut stdout,
                                out_line.as_bytes(),
                            )
                            .await
                            .map_err(WshError::Io)?;
                        }
                    }
                }
            }
        } else {
            // Plain JSON response
            let trimmed_body = body.trim();
            if !trimmed_body.is_empty() {
                let out_line = format!("{}\n", trimmed_body);
                tokio::io::AsyncWriteExt::write_all(&mut stdout, out_line.as_bytes())
                    .await
                    .map_err(WshError::Io)?;
            }
        }
        tokio::io::AsyncWriteExt::flush(&mut stdout)
            .await
            .map_err(WshError::Io)?;
    }

    tracing::info!("wsh mcp stdio bridge exiting");
    Ok(())
}

// ── Standalone mode ────────────────────────────────────────────────

/// Spawn a wsh server daemon as a background process.
///
/// The spawned server runs in ephemeral mode (exits when last session ends).
fn spawn_server_daemon(
    socket_path: &std::path::Path,
    bind: &SocketAddr,
    token: Option<&str>,
) -> Result<(), WshError> {
    let exe = std::env::current_exe().map_err(WshError::Io)?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("server")
        .arg("--ephemeral")
        .arg("--bind")
        .arg(bind.to_string())
        .arg("--socket")
        .arg(socket_path);

    if let Some(t) = token {
        cmd.arg("--token").arg(t);
    }

    // Detach from parent: redirect stdio, start new session
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    // On Unix, create a new process group so the server survives if the
    // parent exits.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    cmd.spawn().map_err(WshError::Io)?;
    tracing::debug!("spawned wsh server daemon");
    Ok(())
}

/// Wait for the Unix socket to become connectable.
async fn wait_for_socket(socket_path: &std::path::Path) -> Result<(), WshError> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if tokio::time::Instant::now() > deadline {
            return Err(WshError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "timed out waiting for server socket at {}",
                    socket_path.display()
                ),
            )));
        }
        match client::Client::connect(socket_path).await {
            Ok(_) => return Ok(()),
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }
}

/// Run the standalone mode: connect to (or spawn) a server, then attach.
async fn run_standalone(cli: Cli) -> Result<(), WshError> {
    tracing::info!("wsh starting");

    let socket_path = server::default_socket_path();

    // Try connecting to an existing server; if none, spawn one
    let mut c = match client::Client::connect(&socket_path).await {
        Ok(c) => {
            tracing::debug!("connected to existing server");
            c
        }
        Err(_) => {
            tracing::debug!("no server running, spawning daemon");
            spawn_server_daemon(&socket_path, &cli.bind, None)?;
            wait_for_socket(&socket_path).await?;
            client::Client::connect(&socket_path).await.map_err(|e| {
                eprintln!("wsh: failed to connect to server after spawn: {}", e);
                WshError::Io(e)
            })?
        }
    };

    let (rows, cols) = terminal::terminal_size().unwrap_or((24, 80));
    tracing::debug!(rows, cols, "terminal size");

    // Determine what command to pass to the server
    let command = match &cli.cmd {
        Some(cmd) => Some(cmd.clone()),
        None => cli.shell.clone(),
    };

    let msg = protocol::CreateSessionMsg {
        name: cli.name.clone(),
        command,
        cwd: None,
        env: None,
        rows,
        cols,
    };

    let resp = c.create_session(msg).await.map_err(|e| {
        eprintln!("wsh: failed to create session: {}", e);
        WshError::Io(e)
    })?;

    tracing::info!(session = %resp.name, "session created");

    // Enter raw mode for the local terminal
    let raw_guard = terminal::RawModeGuard::new()?;

    // Clear the screen (or enter alternate screen) so the local view
    // starts clean.
    let screen_mode = if cli.alt_screen {
        terminal::ScreenMode::AltScreen
    } else {
        terminal::ScreenMode::Clear
    };
    let screen_guard = terminal::ScreenGuard::new(screen_mode)?;

    // Enter the streaming I/O loop
    let result = c.run_streaming().await;

    // Restore terminal
    drop(screen_guard);
    drop(raw_guard);

    if let Err(e) = result {
        eprintln!("wsh: streaming error: {}", e);
        return Err(WshError::Io(e));
    }

    tracing::info!("wsh exiting");
    Ok(())
}

// ── Client subcommands ─────────────────────────────────────────────

async fn run_attach(
    name: String,
    scrollback: String,
    socket: Option<PathBuf>,
    alt_screen: bool,
) -> Result<(), WshError> {
    let socket_path = socket.unwrap_or_else(server::default_socket_path);

    let scrollback_req = match scrollback.as_str() {
        "none" => ScrollbackRequest::None,
        "all" => ScrollbackRequest::All,
        s => match s.parse::<usize>() {
            Ok(n) => ScrollbackRequest::Lines(n),
            Err(_) => {
                eprintln!("wsh attach: invalid scrollback value: {}", s);
                std::process::exit(1);
            }
        },
    };

    let (rows, cols) = terminal::terminal_size().unwrap_or((24, 80));

    let mut c = client::Client::connect(&socket_path).await.map_err(|e| {
        eprintln!("wsh attach: failed to connect to server at {}: {}", socket_path.display(), e);
        WshError::Io(e)
    })?;

    let msg = AttachSessionMsg {
        name: name.clone(),
        scrollback: scrollback_req,
        rows,
        cols,
    };

    let resp = c.attach(msg).await.map_err(|e| {
        eprintln!("wsh attach: {}", e);
        WshError::Io(e)
    })?;

    // Enter raw mode for the local terminal
    let raw_guard = terminal::RawModeGuard::new()?;

    // Clear the screen (or enter alternate screen) so the local view
    // starts clean before replaying scrollback.
    let screen_mode = if alt_screen {
        terminal::ScreenMode::AltScreen
    } else {
        terminal::ScreenMode::Clear
    };
    let screen_guard = terminal::ScreenGuard::new(screen_mode)?;

    // Replay scrollback and screen data before entering the streaming loop
    {
        use std::io::Write;
        let mut stdout = std::io::stdout().lock();
        if !resp.scrollback.is_empty() {
            let _ = stdout.write_all(&resp.scrollback);
        }
        if !resp.screen.is_empty() {
            let _ = stdout.write_all(&resp.screen);
        }
        let _ = stdout.flush();
    }

    // Enter the streaming I/O loop
    let result = c.run_streaming().await;

    // Restore terminal
    drop(screen_guard);
    drop(raw_guard);

    if let Err(e) = result {
        eprintln!("wsh attach: streaming error: {}", e);
        return Err(WshError::Io(e));
    }

    Ok(())
}

async fn run_list(socket: Option<PathBuf>) -> Result<(), WshError> {
    let socket_path = socket.unwrap_or_else(server::default_socket_path);
    let mut c = match client::Client::connect(&socket_path).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "wsh list: failed to connect to server at {}: {}",
                socket_path.display(),
                e
            );
            std::process::exit(1);
        }
    };

    let sessions = match c.list_sessions().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("wsh list: {}", e);
            std::process::exit(1);
        }
    };

    if sessions.is_empty() {
        println!("No active sessions.");
    } else {
        println!(
            "{:<20} {:<8} {:<20} {:<12} {}",
            "NAME", "PID", "COMMAND", "SIZE", "CLIENTS"
        );
        for s in &sessions {
            let pid_str = match s.pid {
                Some(pid) => pid.to_string(),
                None => "-".to_string(),
            };
            let size = format!("{}x{}", s.cols, s.rows);
            println!(
                "{:<20} {:<8} {:<20} {:<12} {}",
                s.name, pid_str, s.command, size, s.clients
            );
        }
    }

    Ok(())
}

async fn run_kill(name: String, socket: Option<PathBuf>) -> Result<(), WshError> {
    let socket_path = socket.unwrap_or_else(server::default_socket_path);
    let mut c = match client::Client::connect(&socket_path).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "wsh kill: failed to connect to server at {}: {}",
                socket_path.display(),
                e
            );
            std::process::exit(1);
        }
    };

    if let Err(e) = c.kill_session(&name).await {
        eprintln!("wsh kill: {}", e);
        std::process::exit(1);
    }

    println!("Session '{}' killed.", name);
    Ok(())
}

async fn run_detach(name: String, socket: Option<PathBuf>) -> Result<(), WshError> {
    let socket_path = socket.unwrap_or_else(server::default_socket_path);
    let mut c = match client::Client::connect(&socket_path).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "wsh detach: failed to connect to server at {}: {}",
                socket_path.display(),
                e
            );
            std::process::exit(1);
        }
    };

    if let Err(e) = c.detach_session(&name).await {
        eprintln!("wsh detach: {}", e);
        std::process::exit(1);
    }

    println!("Session '{}' detached.", name);
    Ok(())
}

async fn run_persist(
    value: Option<String>,
    bind: SocketAddr,
    token: Option<String>,
) -> Result<(), WshError> {
    let url = format!("http://{}/server/persist", bind);
    let client = reqwest::Client::new();

    // Determine whether to GET (query) or PUT (set)
    let persistent_value = match value.as_deref() {
        None => None,
        Some("on") => Some(true),
        Some("off") => Some(false),
        Some(other) => {
            eprintln!("wsh persist: expected 'on' or 'off', got '{}'", other);
            std::process::exit(1);
        }
    };

    let resp = match persistent_value {
        None => {
            // Query current state
            let mut req = client.get(&url);
            if let Some(t) = &token {
                req = req.bearer_auth(t);
            }
            match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    if e.is_connect() {
                        eprintln!("wsh persist: could not connect to wsh server at {} — is the server running?", bind);
                    } else {
                        eprintln!("wsh persist: {}", e);
                    }
                    std::process::exit(1);
                }
            }
        }
        Some(val) => {
            // Set new state
            let mut req = client.put(&url).json(&serde_json::json!({"persistent": val}));
            if let Some(t) = &token {
                req = req.bearer_auth(t);
            }
            match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    if e.is_connect() {
                        eprintln!("wsh persist: could not connect to wsh server at {} — is the server running?", bind);
                    } else {
                        eprintln!("wsh persist: {}", e);
                    }
                    std::process::exit(1);
                }
            }
        }
    };

    if !resp.status().is_success() {
        eprintln!("wsh persist: server returned status {}", resp.status());
        std::process::exit(1);
    }

    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    let is_persistent = body["persistent"].as_bool().unwrap_or(false);
    if is_persistent {
        println!("Server is in persistent mode (will stay alive when sessions end).");
    } else {
        println!("Server is in ephemeral mode (will exit when last session ends).");
    }
    Ok(())
}

