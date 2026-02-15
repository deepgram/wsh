pub mod ansi;
pub mod events;
pub mod format;
pub mod state;

mod task;

use std::panic::AssertUnwindSafe;

use futures::FutureExt;
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use bytes::Bytes;

use crate::broker::Broker;
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
    /// Holds the parser's dedicated mpsc sender alive. As long as
    /// the `Parser` (or any clone) exists, the channel stays open
    /// and the parser task will not exit due to a closed channel.
    _raw_tx: mpsc::Sender<Bytes>,
}

impl Parser {
    /// Spawn parser task, subscribing to raw byte broker
    pub fn spawn(raw_broker: &Broker, cols: usize, rows: usize, scrollback_limit: usize) -> Self {
        let (query_tx, query_rx) = mpsc::channel(32);
        let (event_tx, _) = broadcast::channel(256);

        let (raw_tx, raw_rx) = raw_broker.subscribe_parser();
        let event_tx_clone = event_tx.clone();

        tokio::spawn(async move {
            let result = AssertUnwindSafe(task::run(
                raw_rx,
                query_rx,
                event_tx_clone,
                cols,
                rows,
                scrollback_limit,
            ))
            .catch_unwind()
            .await;
            match result {
                Ok(()) => tracing::warn!("parser task exited unexpectedly"),
                Err(e) => tracing::error!("parser task panicked: {:?}", e),
            }
        });

        Self {
            query_tx,
            event_tx,
            _raw_tx: raw_tx,
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
