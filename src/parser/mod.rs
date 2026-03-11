pub mod ansi;
pub mod events;
pub mod format;
pub mod state;

mod task;

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::FutureExt;
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use bytes::Bytes;

use events::Event;
use state::{Query, QueryResponse};

/// Wrapper for parser subscription events that includes lag notifications.
#[derive(Debug, Clone)]
pub enum SubscriptionEvent {
    /// A normal parser event.
    Event(Event),
    /// The subscriber fell behind and `skipped` events were dropped.
    Lagged(u64),
}

#[derive(Error, Debug)]
pub enum ParserError {
    #[error("parser task died unexpectedly")]
    TaskDied,

    #[error("query channel full")]
    ChannelFull,

    #[error("parser query timed out")]
    QueryTimeout,

    #[error("invalid query parameters: {0}")]
    InvalidQuery(String),
}

#[derive(Clone)]
pub struct Parser {
    query_tx: mpsc::Sender<(Query, oneshot::Sender<QueryResponse>)>,
    event_tx: broadcast::Sender<Event>,
}

impl Parser {
    /// Spawn parser task that consumes raw PTY bytes from the given channel.
    ///
    /// The caller creates the bounded channel and passes only the receiver here.
    /// The sender half is held by the PTY reader thread, which uses
    /// `blocking_send()` to apply backpressure when the parser can't keep up.
    /// See the design decision comment in `Session::spawn_with_options()` for
    /// the full rationale.
    pub fn spawn(mut raw_rx: mpsc::Receiver<Bytes>, cols: usize, rows: usize, scrollback_limit: usize) -> Self {
        let (query_tx, query_rx) = mpsc::channel(32);
        let (event_tx, _) = broadcast::channel(1024);

        let event_tx_clone = event_tx.clone();

        // Shared dimension tracking for panic recovery. When the parser
        // handles a Resize query it updates these atomics, so the restart
        // loop can read the current dimensions instead of the stale values
        // from session creation time.
        let current_cols = Arc::new(AtomicUsize::new(cols));
        let current_rows = Arc::new(AtomicUsize::new(rows));
        let task_cols = current_cols.clone();
        let task_rows = current_rows.clone();

        tokio::spawn(async move {
            let mut query_rx = query_rx;
            // On first iteration use the initial dimensions; on restart
            // read the latest values from the shared atomics.
            let mut first = true;
            loop {
                let (c, r) = if first {
                    first = false;
                    (cols, rows)
                } else {
                    (
                        task_cols.load(Ordering::Acquire),
                        task_rows.load(Ordering::Acquire),
                    )
                };
                let result = AssertUnwindSafe(task::run(
                    &mut raw_rx,
                    &mut query_rx,
                    event_tx_clone.clone(),
                    c,
                    r,
                    scrollback_limit,
                    &task_cols,
                    &task_rows,
                ))
                .catch_unwind()
                .await;
                match result {
                    Ok(()) => {
                        // Normal exit: channels closed, session is shutting down.
                        tracing::debug!("parser task exited normally");
                        break;
                    }
                    Err(e) => {
                        tracing::error!("parser task panicked, restarting with fresh state: {:?}", e);
                        // Emit a reset event so clients know to re-query state.
                        // The VT state is lost, but the channels survive across
                        // the panic boundary because they're owned by this outer
                        // scope, not by the panicking task::run function.
                        let _ = event_tx_clone.send(events::Event::Reset {
                            seq: 0,
                            reason: events::ResetReason::ParserRestart,
                        });
                    }
                }
            }
        });

        Self {
            query_tx,
            event_tx,
        }
    }

    /// Query current state (hides channel creation).
    ///
    /// Returns `ParserError::QueryTimeout` if the parser task doesn't respond
    /// within 5 seconds. This prevents callers from blocking indefinitely if
    /// the parser task is stalled.
    pub async fn query(&self, query: Query) -> Result<QueryResponse, ParserError> {
        let (tx, rx) = oneshot::channel();
        self.query_tx
            .send((query, tx))
            .await
            .map_err(|_| ParserError::TaskDied)?;
        tokio::time::timeout(std::time::Duration::from_secs(5), rx)
            .await
            .map_err(|_| ParserError::QueryTimeout)?
            .map_err(|_| ParserError::TaskDied)
    }

    /// Notify parser of terminal resize
    pub async fn resize(&self, cols: usize, rows: usize) -> Result<(), ParserError> {
        self.query(Query::Resize { cols, rows }).await?;
        Ok(())
    }

    /// Subscribe to events (returns async Stream).
    ///
    /// The stream yields `SubscriptionEvent::Event` for normal events and
    /// `SubscriptionEvent::Lagged(n)` when the subscriber falls behind,
    /// allowing consumers to detect data loss and re-query state.
    pub fn subscribe(&self) -> impl Stream<Item = SubscriptionEvent> {
        BroadcastStream::new(self.event_tx.subscribe()).filter_map(|result| match result {
            Ok(event) => Some(SubscriptionEvent::Event(event)),
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                Some(SubscriptionEvent::Lagged(n))
            }
        })
    }
}

#[cfg(test)]
mod tests;
