//! wsh - The Web Shell
//!
//! A transparent PTY wrapper that exposes terminal I/O via HTTP/WebSocket API.
//!
//! Architecture:
//! - stdin reader: Dedicated thread reading from stdin, sends to input channel
//! - PTY writer: Dedicated thread receiving from input channel, writes to PTY
//! - PTY reader: Dedicated thread reading from PTY, writes to stdout and broadcasts
//! - HTTP/WebSocket server: Async, receives input via API, sends to input channel
//! - Child monitor: Watches for shell process exit
//!
//! Shutdown: When the shell exits (detected via child monitor or PTY reader EOF),
//! we restore terminal state and call process::exit(). This is necessary because
//! the stdin reader thread is blocked on read() and cannot be cancelled.

use bytes::Bytes;
use clap::Parser as ClapParser;
use std::io::{Read, Write};
use std::net::SocketAddr;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use wsh::{api, broker, parser::Parser, pty, shutdown::ShutdownCoordinator, terminal};

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
}

#[derive(Error, Debug)]
pub enum WshError {
    #[error("pty error: {0}")]
    Pty(#[from] pty::PtyError),

    #[error("terminal error: {0}")]
    Terminal(#[from] terminal::TerminalError),

    #[error("task join error: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),
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

    // Enable raw mode so we receive all keystrokes (including Ctrl+C, etc.)
    // The guard restores normal mode when dropped
    let raw_guard = terminal::RawModeGuard::new()?;

    let (rows, cols) = terminal::terminal_size().unwrap_or((24, 80));
    tracing::debug!(rows, cols, "terminal size");

    let mut pty = pty::Pty::spawn(rows, cols)?;
    tracing::debug!("PTY spawned");

    let pty_reader = pty.take_reader()?;
    let pty_writer = pty.take_writer()?;
    let mut pty_child = pty.take_child().expect("child process");

    // Channel to signal when child process exits
    let (child_exit_tx, child_exit_rx) = tokio::sync::oneshot::channel::<()>();

    // Child process monitor: detects when shell exits
    tokio::task::spawn_blocking(move || {
        match pty_child.wait() {
            Ok(status) => tracing::debug!(?status, "shell exited"),
            Err(e) => tracing::error!(?e, "error waiting for shell"),
        }
        let _ = child_exit_tx.send(());
    });

    let broker = broker::Broker::new();

    // Create parser for terminal state tracking
    let parser = Parser::spawn(&broker, cols as usize, rows as usize, 10_000);

    // Channel for input from all sources (stdin, HTTP, WebSocket) -> PTY writer
    let (input_tx, input_rx) = mpsc::channel::<Bytes>(64);

    // Shutdown coordinator for graceful client disconnection
    let shutdown = ShutdownCoordinator::new();

    // Spawn I/O tasks
    let pty_reader_handle = spawn_pty_reader(pty_reader, broker.clone());
    spawn_pty_writer(pty_writer, input_rx);
    spawn_stdin_reader(input_tx.clone());

    // Start API server with graceful shutdown support
    let state = api::AppState {
        input_tx,
        output_rx: broker.sender(),
        shutdown: shutdown.clone(),
        parser: parser.clone(),
    };
    let app = api::router(state);
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

    // Wait for exit condition
    wait_for_exit(child_exit_rx, pty_reader_handle).await;

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

/// Spawn the PTY reader task.
/// Reads from PTY, writes to stdout, and broadcasts to subscribers.
fn spawn_pty_reader(
    mut reader: Box<dyn Read + Send>,
    broker: broker::Broker,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        let mut stdout = std::io::stdout();
        let mut buf = [0u8; 4096];

        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    tracing::debug!("PTY reader: EOF");
                    break;
                }
                Ok(n) => {
                    let data = Bytes::copy_from_slice(&buf[..n]);
                    let _ = stdout.write_all(&data);
                    let _ = stdout.flush();
                    broker.publish(data);
                }
                Err(e) => {
                    tracing::debug!(?e, "PTY reader: error");
                    break;
                }
            }
        }
    })
}

/// Spawn the PTY writer task.
/// Receives input from channel and writes to PTY.
fn spawn_pty_writer(mut writer: Box<dyn Write + Send>, mut input_rx: mpsc::Receiver<Bytes>) {
    tokio::task::spawn_blocking(move || {
        while let Some(data) = input_rx.blocking_recv() {
            if let Err(e) = writer.write_all(&data) {
                tracing::debug!(?e, "PTY writer: error");
                break;
            }
            let _ = writer.flush();
        }
    });
}

/// Spawn the stdin reader task.
/// Reads from stdin and sends to the input channel.
fn spawn_stdin_reader(input_tx: mpsc::Sender<Bytes>) {
    tokio::task::spawn_blocking(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 1024];

        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let data = Bytes::copy_from_slice(&buf[..n]);
                    if input_tx.blocking_send(data).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

/// Wait for an exit condition: child exit, PTY reader EOF, or Ctrl+C.
async fn wait_for_exit(
    mut child_exit_rx: tokio::sync::oneshot::Receiver<()>,
    mut pty_reader_handle: tokio::task::JoinHandle<()>,
) {
    let shutdown = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
        tracing::info!("received Ctrl+C");
    };

    tokio::select! {
        _ = &mut child_exit_rx => {
            tracing::debug!("child process exited");
        }
        _ = &mut pty_reader_handle => {
            tracing::debug!("PTY reader finished");
        }
        _ = shutdown => {
            tracing::debug!("shutdown signal");
        }
    }
}
