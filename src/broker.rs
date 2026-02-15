use std::sync::{Arc, Mutex};

use bytes::Bytes;
use tokio::sync::{broadcast, mpsc};

pub const BROADCAST_CAPACITY: usize = 64;

/// Capacity for the dedicated parser channel. Each message is typically
/// a small PTY chunk (~4 KiB), so 4096 messages ≈ 16 MiB max buffered.
const PARSER_CHANNEL_CAPACITY: usize = 4096;

#[derive(Clone)]
pub struct Broker {
    tx: broadcast::Sender<Bytes>,
    parser_tx: mpsc::Sender<Bytes>,
    parser_rx: Arc<Mutex<Option<mpsc::Receiver<Bytes>>>>,
}

impl Broker {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (parser_tx, parser_rx) = mpsc::channel(PARSER_CHANNEL_CAPACITY);
        Self {
            tx,
            parser_tx,
            parser_rx: Arc::new(Mutex::new(Some(parser_rx))),
        }
    }

    pub fn publish(&self, data: Bytes) {
        match self.parser_tx.try_send(data.clone()) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!("parser channel full, dropping data (parser may be stalled)");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::debug!("parser channel closed (parser task exited)");
            }
        }
        // Ignore error - means no receivers
        let _ = self.tx.send(data);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Bytes> {
        self.tx.subscribe()
    }

    /// Take the dedicated parser channel out of the broker.
    ///
    /// The parser is singular -- this method panics if called more than once.
    /// Returns both halves: the sender (so the caller can keep the channel alive)
    /// and the receiver.
    pub fn subscribe_parser(&self) -> (mpsc::Sender<Bytes>, mpsc::Receiver<Bytes>) {
        let rx = self.parser_rx
            .lock()
            .expect("parser_rx mutex poisoned")
            .take()
            .expect("subscribe_parser() called more than once");
        (self.parser_tx.clone(), rx)
    }

    pub fn sender(&self) -> broadcast::Sender<Bytes> {
        self.tx.clone()
    }
}

impl Default for Broker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_new_creates_broker() {
        let broker = Broker::new();
        // Can subscribe without error
        let _rx = broker.subscribe();
    }

    #[tokio::test]
    async fn test_publish_with_no_subscribers() {
        let broker = Broker::new();
        // Should not panic even with no subscribers
        broker.publish(Bytes::from("hello"));
    }

    #[tokio::test]
    async fn test_single_subscriber_receives() {
        let broker = Broker::new();
        let mut rx = broker.subscribe();

        broker.publish(Bytes::from("hello"));

        let received = rx.recv().await.expect("should receive message");
        assert_eq!(received, Bytes::from("hello"));
    }

    #[tokio::test]
    async fn test_multiple_subscribers_receive() {
        let broker = Broker::new();
        let mut rx1 = broker.subscribe();
        let mut rx2 = broker.subscribe();
        let mut rx3 = broker.subscribe();

        broker.publish(Bytes::from("broadcast"));

        let received1 = rx1.recv().await.expect("rx1 should receive message");
        let received2 = rx2.recv().await.expect("rx2 should receive message");
        let received3 = rx3.recv().await.expect("rx3 should receive message");

        assert_eq!(received1, Bytes::from("broadcast"));
        assert_eq!(received2, Bytes::from("broadcast"));
        assert_eq!(received3, Bytes::from("broadcast"));
    }

    #[tokio::test]
    async fn test_subscriber_receives_multiple_messages() {
        let broker = Broker::new();
        let mut rx = broker.subscribe();

        broker.publish(Bytes::from("first"));
        broker.publish(Bytes::from("second"));
        broker.publish(Bytes::from("third"));

        let msg1 = rx.recv().await.expect("should receive first message");
        let msg2 = rx.recv().await.expect("should receive second message");
        let msg3 = rx.recv().await.expect("should receive third message");

        assert_eq!(msg1, Bytes::from("first"));
        assert_eq!(msg2, Bytes::from("second"));
        assert_eq!(msg3, Bytes::from("third"));
    }

    #[tokio::test]
    async fn test_default_creates_broker() {
        let broker = Broker::default();
        // Can subscribe without error
        let _rx = broker.subscribe();
    }

    #[tokio::test]
    async fn test_sender_can_be_used_directly() {
        let broker = Broker::new();
        let mut rx = broker.subscribe();
        let sender = broker.sender();

        // Send via the sender directly
        sender.send(Bytes::from("via sender")).expect("send should succeed");

        let received = rx.recv().await.expect("should receive message");
        assert_eq!(received, Bytes::from("via sender"));
    }

    #[tokio::test]
    async fn test_clone_shares_channel() {
        let broker1 = Broker::new();
        let broker2 = broker1.clone();

        let mut rx = broker1.subscribe();

        // Publish via the clone
        broker2.publish(Bytes::from("from clone"));

        let received = rx.recv().await.expect("should receive message from clone");
        assert_eq!(received, Bytes::from("from clone"));
    }

    #[tokio::test]
    async fn test_parser_channel_receives_independently() {
        let broker = Broker::new();
        let (_parser_tx, mut parser_rx) = broker.subscribe_parser();
        let mut broadcast_rx = broker.subscribe();

        broker.publish(Bytes::from("hello"));

        let parser_msg = parser_rx.recv().await.expect("parser should receive");
        assert_eq!(parser_msg, Bytes::from("hello"));

        let broadcast_msg = broadcast_rx.recv().await.expect("broadcast should receive");
        assert_eq!(broadcast_msg, Bytes::from("hello"));
    }

    #[tokio::test]
    async fn test_parser_channel_does_not_lag() {
        let broker = Broker::new();
        let (_parser_tx, mut parser_rx) = broker.subscribe_parser();

        for i in 0..200 {
            broker.publish(Bytes::from(format!("msg-{i}")));
        }

        for i in 0..200 {
            let msg = parser_rx.recv().await.expect("parser should not lose data");
            assert_eq!(msg, Bytes::from(format!("msg-{i}")));
        }
    }

    #[tokio::test]
    #[should_panic(expected = "subscribe_parser() called more than once")]
    async fn test_subscribe_parser_panics_on_second_call() {
        let broker = Broker::new();
        let _rx1 = broker.subscribe_parser(); // takes (Sender, Receiver)
        let _rx2 = broker.subscribe_parser(); // should panic
    }
}
