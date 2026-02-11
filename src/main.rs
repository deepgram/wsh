//! wsh - The Web Shell
//!
//! A transparent PTY wrapper that exposes terminal I/O via HTTP/WebSocket API.
//!
//! Architecture:
//! - Session::spawn() creates the PTY, broker, parser, and I/O tasks
//! - stdin reader: Dedicated thread reading from stdin, sends to input channel
//! - stdout writer: Subscribes to the session output broker, writes to stdout
//! - HTTP/WebSocket server: Async, receives input via API, sends to input channel
//! - Child monitor: Watches for shell process exit (via Session::spawn)
//!
//! Shutdown: When the shell exits (detected via child_exit_rx or broker closing),
//! we restore terminal state and call process::exit(). This is necessary because
//! the stdin reader thread is blocked on read() and cannot be cancelled.

use bytes::Bytes;
use clap::Parser as ClapParser;
use std::io::{Read, Write};
use std::net::SocketAddr;
use thiserror::Error;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use wsh::{api, input, overlay, panel, pty::SpawnCommand, session::{Session, SessionRegistry}, shutdown::ShutdownCoordinator, terminal};

/// wsh - The Web Shell
///
/// A transparent PTY wrapper that exposes terminal I/O via HTTP/WebSocket API.
/// Run your shell inside wsh to access it from web browsers, agents, and other tools.
#[derive(ClapParser, Debug)]
#[command(name = "wsh", version, about, long_about = None)]
struct Args {
    /// Address to bind the HTTP/WebSocket API server
    #[arg(long, default_value = "127.0.0.1:8080")]
    bind: SocketAddr,

    /// Command string to execute (like sh -c)
    #[arg(short = 'c')]
    command: Option<String>,

    /// Force interactive mode
    #[arg(short = 'i')]
    interactive: bool,

    /// Authentication token for non-localhost bindings
    #[arg(long, env = "WSH_TOKEN")]
    token: Option<String>,

    /// Shell to spawn (overrides $SHELL)
    #[arg(long)]
    shell: Option<String>,
}

#[derive(Error, Debug)]
pub enum WshError {
    #[error("pty error: {0}")]
    Pty(#[from] wsh::pty::PtyError),

    #[error("terminal error: {0}")]
    Terminal(#[from] terminal::TerminalError),

    #[error("task join error: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),
}

fn is_loopback(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

fn resolve_token(args: &Args) -> Option<String> {
    if is_loopback(&args.bind) {
        return None;
    }
    match &args.token {
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
    let args = Args::parse();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "wsh=info,tower_http=info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("wsh starting");

    let token = resolve_token(&args);
    if let Some(ref _t) = token {
        tracing::info!("auth token configured");
    }

    // Enable raw mode so we receive all keystrokes (including Ctrl+C, etc.)
    // The guard restores normal mode when dropped
    let raw_guard = terminal::RawModeGuard::new()?;

    let (rows, cols) = terminal::terminal_size().unwrap_or((24, 80));
    tracing::debug!(rows, cols, "terminal size");

    // Determine what command to spawn based on CLI args
    let spawn_cmd = match &args.command {
        Some(cmd) => SpawnCommand::Command {
            command: cmd.clone(),
            interactive: args.interactive,
        },
        None => SpawnCommand::Shell {
            interactive: args.interactive,
            shell: args.shell.clone(),
        },
    };

    // Spawn the session: this creates the PTY, broker, parser, and I/O tasks
    let (session, child_exit_rx) = Session::spawn("default".to_string(), spawn_cmd, rows, cols)?;
    tracing::debug!("session spawned");

    // Subscribe to the session output for local terminal display (stdout passthrough)
    let mut output_sub = session.output_rx.subscribe();
    let overlays_for_display = session.overlays.clone();
    tokio::spawn(async move {
        let mut stdout = std::io::stdout();
        while let Ok(data) = output_sub.recv().await {
            // Render overlays around PTY data to prevent scrollback smearing
            let overlay_list = overlays_for_display.list();
            if !overlay_list.is_empty() {
                let erase = overlay::erase_all_overlays(&overlay_list);
                let render = overlay::render_all_overlays(&overlay_list);
                // Use synchronized output so terminal applies atomically
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

    // Register the session in the registry
    let sessions = SessionRegistry::new();
    sessions.insert(Some("default".into()), session.clone()).unwrap();

    // Build the global shutdown coordinator and app state
    let shutdown = ShutdownCoordinator::new();
    let state = api::AppState {
        sessions,
        shutdown: shutdown.clone(),
    };

    // Start API server with graceful shutdown support
    let app = api::router(state, token);
    tracing::info!(addr = %args.bind, "API server listening");

    // Channel to signal the server to begin graceful shutdown
    let (server_shutdown_tx, server_shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let bind_addr = args.bind;
    let server_handle = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(bind_addr).await.unwrap();
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                server_shutdown_rx.await.ok();
            })
            .await
            .unwrap();
    });

    // Spawn stdin reader (reads from process stdin, sends to session input)
    spawn_stdin_reader(
        session.input_tx.clone(),
        session.input_mode.clone(),
        session.input_broadcaster.clone(),
        session.activity.clone(),
    );

    // SIGWINCH handler: reconfigure layout when terminal is resized
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
                let (new_rows, new_cols) =
                    terminal::terminal_size().unwrap_or((terminal_size.get().0, terminal_size.get().1));
                tracing::debug!(new_rows, new_cols, "SIGWINCH received");
                terminal_size.set(new_rows, new_cols);

                if panels.list().is_empty() {
                    // No panels: just resize PTY and parser directly
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

    // Wait for exit condition
    wait_for_exit(child_exit_rx).await;

    // Gracefully shut down: signal WebSocket handlers to close, then wait for them
    let active = shutdown.active_count();
    if active > 0 {
        tracing::info!(active, "signaling clients to disconnect");
        shutdown.shutdown();
        shutdown.wait_for_all_closed().await;
        tracing::debug!("all clients disconnected");
    }

    // Signal server to stop accepting connections and wait for it to finish
    let _ = server_shutdown_tx.send(());
    if let Err(e) = server_handle.await {
        tracing::warn!(?e, "server task panicked");
    }

    // Restore terminal and exit
    // Note: We use process::exit() because the stdin reader thread is blocked
    // on read() and cannot be cancelled. This is standard for terminal applications.
    tracing::info!("wsh exiting");
    drop(raw_guard);
    std::process::exit(0)
}

/// Spawn the stdin reader task.
/// Reads from stdin and sends to the input channel.
///
/// All input is broadcast to subscribers with mode and parsed key info.
/// In capture mode, stdin is not forwarded to the PTY.
/// Ctrl+\ in capture mode switches back to passthrough mode.
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

                    // Get mode once at start of loop iteration
                    let mode = input_mode.get();

                    // Always broadcast input first
                    input_broadcaster.broadcast_input(data, mode);

                    // Any input resets the quiescence timer
                    activity.touch();

                    // Check for Ctrl+\ escape hatch in capture mode
                    if input::is_ctrl_backslash(data) && mode == input::Mode::Capture {
                        input_mode.release();
                        input_broadcaster.broadcast_mode(input::Mode::Passthrough);
                        tracing::debug!("Ctrl+\\ pressed, switching to passthrough mode");
                        continue; // Don't forward the Ctrl+\
                    }

                    // In capture mode, don't forward to PTY
                    if mode == input::Mode::Capture {
                        continue;
                    }

                    // Passthrough mode: forward to PTY
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
async fn wait_for_exit(
    child_exit_rx: tokio::sync::oneshot::Receiver<()>,
) {
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
