mod api;
mod broker;
mod pty;
mod terminal;

use bytes::Bytes;
use std::io::{Read, Write};
use std::net::SocketAddr;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

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
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "wsh=debug,tower_http=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("wsh starting");

    // Enable raw mode - guard restores on drop
    let _raw_guard = terminal::RawModeGuard::new()?;

    let pty = pty::Pty::spawn()?;
    tracing::info!("PTY spawned");

    let mut pty_reader = pty.take_reader()?;
    let mut pty_writer = pty.take_writer()?;

    let broker = broker::Broker::new();
    let broker_clone = broker.clone();

    // Channel for input from all sources -> PTY writer
    let (input_tx, mut input_rx) = mpsc::channel::<Bytes>(64);

    // PTY reader task: read from PTY, write to stdout, broadcast
    let mut pty_reader_handle = tokio::task::spawn_blocking(move || {
        let mut stdout = std::io::stdout();
        let mut buf = [0u8; 4096];

        loop {
            match pty_reader.read(&mut buf) {
                Ok(0) => {
                    tracing::debug!("PTY reader: EOF");
                    break;
                }
                Ok(n) => {
                    let data = Bytes::copy_from_slice(&buf[..n]);
                    // Write to stdout
                    let _ = stdout.write_all(&data);
                    let _ = stdout.flush();
                    // Broadcast to subscribers
                    broker_clone.publish(data);
                }
                Err(e) => {
                    tracing::error!(?e, "PTY read error");
                    break;
                }
            }
        }
    });

    // PTY writer task: receive from channel, write to PTY
    let pty_writer_handle = tokio::task::spawn_blocking(move || {
        while let Some(data) = input_rx.blocking_recv() {
            if let Err(e) = pty_writer.write_all(&data) {
                tracing::error!(?e, "PTY write error");
                break;
            }
            let _ = pty_writer.flush();
        }
        tracing::debug!("PTY writer: channel closed");
    });

    // Stdin reader task: read from stdin, send to PTY writer channel
    let stdin_tx = input_tx.clone();
    let stdin_handle = tokio::task::spawn_blocking(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 1024];

        loop {
            match stdin.read(&mut buf) {
                Ok(0) => {
                    tracing::debug!("stdin: EOF");
                    break;
                }
                Ok(n) => {
                    let data = Bytes::copy_from_slice(&buf[..n]);
                    if stdin_tx.blocking_send(data).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!(?e, "stdin read error");
                    break;
                }
            }
        }
    });

    // Axum server
    let state = api::AppState {
        input_tx: input_tx.clone(),
        output_rx: broker.sender(),
    };
    let app = api::router(state);
    let addr: SocketAddr = "127.0.0.1:8080".parse().expect("valid socket address");
    tracing::info!(%addr, "API server listening");

    let server_handle = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        axum::serve(listener, app).await.unwrap();
    });

    // Signal handling for graceful shutdown
    let shutdown = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
        tracing::info!("Received Ctrl+C, shutting down");
    };

    // Wait for either: PTY reader to finish (shell exited) OR shutdown signal
    tokio::select! {
        result = &mut pty_reader_handle => {
            match result {
                Ok(()) => tracing::info!("Shell exited"),
                Err(e) => tracing::error!(?e, "PTY reader task failed"),
            }
        }
        _ = shutdown => {
            tracing::info!("Shutdown signal received");
            pty_reader_handle.abort();
        }
    }

    // Clean up all tasks
    drop(input_tx);
    let _ = pty_writer_handle.await;
    stdin_handle.abort();
    server_handle.abort();

    tracing::info!("wsh exiting");
    Ok(())
}
