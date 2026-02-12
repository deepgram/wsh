//! wsh - The Web Shell
//!
//! A transparent PTY wrapper that exposes terminal I/O via HTTP/WebSocket API.
//!
//! ## Modes
//!
//! **Standalone mode** (default, no subcommand): Spawns a PTY, enters raw mode,
//! proxies stdin/stdout, and starts an HTTP/WS API server — all in one process.
//!
//! **Server mode** (`wsh server`): Starts a headless daemon with HTTP/WS and
//! Unix socket listeners. No PTY is spawned automatically — sessions are created
//! on demand via the API or Unix socket protocol.

use bytes::Bytes;
use clap::{Parser as ClapParser, Subcommand};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use thiserror::Error;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use wsh::{
    api, client, input, overlay, panel,
    protocol::{AttachSessionMsg, ScrollbackRequest},
    pty::SpawnCommand,
    server,
    session::{Session, SessionRegistry},
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
    },

    /// List active sessions on the server
    List {
        /// Address of the HTTP/WebSocket API server
        #[arg(long, default_value = "127.0.0.1:8080")]
        bind: SocketAddr,

        /// Authentication token
        #[arg(long, env = "WSH_TOKEN")]
        token: Option<String>,
    },

    /// Kill (destroy) a session on the server
    Kill {
        /// Session name to kill
        name: String,

        /// Address of the HTTP/WebSocket API server
        #[arg(long, default_value = "127.0.0.1:8080")]
        bind: SocketAddr,

        /// Authentication token
        #[arg(long, env = "WSH_TOKEN")]
        token: Option<String>,
    },

    /// Upgrade a running server to persistent mode (it won't shut down when sessions end)
    Persist {
        /// Address of the HTTP/WebSocket API server
        #[arg(long, default_value = "127.0.0.1:8080")]
        bind: SocketAddr,

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

    init_tracing();

    match cli.command {
        Some(Commands::Server { bind, token, socket }) => {
            run_server(bind, token, socket).await
        }
        Some(Commands::Attach { name, scrollback, socket }) => {
            run_attach(name, scrollback, socket).await
        }
        Some(Commands::List { bind, token }) => {
            run_list(bind, token).await
        }
        Some(Commands::Kill { name, bind, token }) => {
            run_kill(name, bind, token).await
        }
        Some(Commands::Persist { bind, token }) => {
            run_persist(bind, token).await
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

// ── Server mode ────────────────────────────────────────────────────

/// Run the wsh server daemon: HTTP/WS + Unix socket, no local terminal.
async fn run_server(
    bind: SocketAddr,
    token: Option<String>,
    socket: Option<PathBuf>,
) -> Result<(), WshError> {
    tracing::info!("wsh server starting");

    let token = resolve_token(&bind, &token);
    if token.is_some() {
        tracing::info!("auth token configured");
    }

    let sessions = SessionRegistry::new();
    let shutdown = ShutdownCoordinator::new();
    let server_config = std::sync::Arc::new(api::ServerConfig::new(false));
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
                            | wsh::session::SessionEvent::Exited { .. }
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

    let _ = server_shutdown_tx.send(());

    // Wait for HTTP server to stop
    if let Err(e) = http_handle.await {
        tracing::warn!(?e, "HTTP server task panicked");
    }

    // Clean up
    socket_handle.abort();
    tracing::info!("wsh server exiting");
    Ok(())
}

// ── Standalone mode ────────────────────────────────────────────────

/// Run the standalone mode: single session with local terminal I/O.
async fn run_standalone(cli: Cli) -> Result<(), WshError> {
    tracing::info!("wsh starting");

    let token = resolve_token(&cli.bind, &cli.token);
    if token.is_some() {
        tracing::info!("auth token configured");
    }

    // Enable raw mode so we receive all keystrokes (including Ctrl+C, etc.)
    let raw_guard = terminal::RawModeGuard::new()?;

    let (rows, cols) = terminal::terminal_size().unwrap_or((24, 80));
    tracing::debug!(rows, cols, "terminal size");

    // Determine what command to spawn
    let spawn_cmd = match &cli.cmd {
        Some(cmd) => SpawnCommand::Command {
            command: cmd.clone(),
            interactive: cli.interactive,
        },
        None => SpawnCommand::Shell {
            interactive: cli.interactive,
            shell: cli.shell.clone(),
        },
    };

    let session_name = cli.name.unwrap_or_else(|| "default".to_string());

    // Spawn the session
    let (mut session, child_exit_rx) =
        Session::spawn(session_name.clone(), spawn_cmd, rows, cols)?;
    session.is_local = true;
    tracing::debug!("session spawned");

    // Subscribe to session output for local terminal display
    let mut output_sub = session.output_rx.subscribe();
    let overlays_for_display = session.overlays.clone();
    tokio::spawn(async move {
        let mut stdout = std::io::stdout();
        while let Ok(data) = output_sub.recv().await {
            let overlay_list = overlays_for_display.list();
            if !overlay_list.is_empty() {
                let erase = overlay::erase_all_overlays(&overlay_list);
                let render = overlay::render_all_overlays(&overlay_list);
                let _ = stdout.write_all(overlay::begin_sync().as_bytes());
                let _ = stdout.write_all(erase.as_bytes());
                let _ = stdout.write_all(&data);
                let _ = stdout.write_all(render.as_bytes());
                let _ = stdout.write_all(overlay::end_sync().as_bytes());
            } else {
                let _ = stdout.write_all(&data);
            }
            let _ = stdout.flush();
        }
    });

    // Register session in registry
    let sessions = SessionRegistry::new();
    sessions
        .insert(Some(session_name), session.clone())
        .unwrap();

    let shutdown = ShutdownCoordinator::new();
    let server_config = std::sync::Arc::new(api::ServerConfig::new(false));
    let state = api::AppState {
        sessions,
        shutdown: shutdown.clone(),
        server_config,
    };

    // Start API server
    let app = api::router(state, token);
    tracing::info!(addr = %cli.bind, "API server listening");

    let (server_shutdown_tx, server_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let bind_addr = cli.bind;
    let server_handle = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(bind_addr).await.unwrap();
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                server_shutdown_rx.await.ok();
            })
            .await
            .unwrap();
    });

    // Spawn stdin reader
    spawn_stdin_reader(
        session.input_tx.clone(),
        session.input_mode.clone(),
        session.input_broadcaster.clone(),
        session.activity.clone(),
    );

    // SIGWINCH handler
    {
        let panels = session.panels.clone();
        let terminal_size = session.terminal_size.clone();
        let pty = session.pty.clone();
        let parser = session.parser.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigwinch =
                signal(SignalKind::window_change()).expect("failed to install SIGWINCH handler");
            loop {
                sigwinch.recv().await;
                let (new_rows, new_cols) = terminal::terminal_size()
                    .unwrap_or((terminal_size.get().0, terminal_size.get().1));
                tracing::debug!(new_rows, new_cols, "SIGWINCH received");
                terminal_size.set(new_rows, new_cols);

                if panels.list().is_empty() {
                    if let Err(e) = pty.resize(new_rows, new_cols) {
                        tracing::error!(?e, "failed to resize PTY on SIGWINCH");
                    }
                    if let Err(e) = parser.resize(new_cols as usize, new_rows as usize).await {
                        tracing::error!(?e, "failed to resize parser on SIGWINCH");
                    }
                } else {
                    panel::reconfigure_layout(&panels, &terminal_size, &pty, &parser).await;
                }
            }
        });
    }

    // Wait for exit
    wait_for_exit(child_exit_rx).await;

    // Graceful shutdown
    let active = shutdown.active_count();
    if active > 0 {
        tracing::info!(active, "signaling clients to disconnect");
        shutdown.shutdown();
        shutdown.wait_for_all_closed().await;
        tracing::debug!("all clients disconnected");
    }

    let _ = server_shutdown_tx.send(());
    if let Err(e) = server_handle.await {
        tracing::warn!(?e, "server task panicked");
    }

    tracing::info!("wsh exiting");
    drop(raw_guard);
    std::process::exit(0)
}

// ── Client subcommands ─────────────────────────────────────────────

async fn run_attach(
    name: String,
    scrollback: String,
    socket: Option<PathBuf>,
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
    drop(raw_guard);

    if let Err(e) = result {
        eprintln!("wsh attach: streaming error: {}", e);
        return Err(WshError::Io(e));
    }

    Ok(())
}

async fn run_list(bind: SocketAddr, token: Option<String>) -> Result<(), WshError> {
    let sessions = match client::Client::list_sessions(&bind, &token).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("wsh list: {}", e);
            std::process::exit(1);
        }
    };

    if sessions.is_empty() {
        println!("No active sessions.");
    } else {
        println!("{:<20}", "NAME");
        for s in &sessions {
            println!("{:<20}", s.name);
        }
    }

    Ok(())
}

async fn run_kill(name: String, bind: SocketAddr, token: Option<String>) -> Result<(), WshError> {
    if let Err(e) = client::Client::kill_session(&bind, &token, &name).await {
        eprintln!("wsh kill: {}", e);
        std::process::exit(1);
    }

    println!("Session '{}' killed.", name);
    Ok(())
}

async fn run_persist(bind: SocketAddr, token: Option<String>) -> Result<(), WshError> {
    let url = format!("http://{}/server/persist", bind);
    let client = reqwest::Client::new();
    let mut req = client.post(&url);
    if let Some(t) = &token {
        req = req.bearer_auth(t);
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            if e.is_connect() {
                eprintln!("wsh persist: could not connect to wsh server at {} — is the server running?", bind);
            } else {
                eprintln!("wsh persist: {}", e);
            }
            std::process::exit(1);
        }
    };

    if !resp.status().is_success() {
        eprintln!("wsh persist: server returned status {}", resp.status());
        std::process::exit(1);
    }

    println!("Server upgraded to persistent mode.");
    Ok(())
}

// ── Helpers ────────────────────────────────────────────────────────

/// Spawn the stdin reader task.
fn spawn_stdin_reader(
    input_tx: tokio::sync::mpsc::Sender<Bytes>,
    input_mode: input::InputMode,
    input_broadcaster: input::InputBroadcaster,
    activity: wsh::activity::ActivityTracker,
) {
    tokio::task::spawn_blocking(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 1024];

        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let data = &buf[..n];
                    let mode = input_mode.get();

                    input_broadcaster.broadcast_input(data, mode);
                    activity.touch();

                    if input::is_ctrl_backslash(data) && mode == input::Mode::Capture {
                        input_mode.release();
                        input_broadcaster.broadcast_mode(input::Mode::Passthrough);
                        tracing::debug!("Ctrl+\\ pressed, switching to passthrough mode");
                        continue;
                    }

                    if mode == input::Mode::Capture {
                        continue;
                    }

                    if input_tx.blocking_send(Bytes::copy_from_slice(data)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

/// Wait for an exit condition: child exit or Ctrl+C.
async fn wait_for_exit(child_exit_rx: tokio::sync::oneshot::Receiver<()>) {
    let shutdown = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
        tracing::info!("received Ctrl+C");
    };

    tokio::select! {
        _ = child_exit_rx => {
            tracing::debug!("child process exited");
        }
        _ = shutdown => {
            tracing::debug!("shutdown signal");
        }
    }
}
