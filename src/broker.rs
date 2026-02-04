use bytes::Bytes;
use tokio::sync::broadcast;

pub const BROADCAST_CAPACITY: usize = 64;

#[derive(Clone)]
pub struct Broker {
    tx: broadcast::Sender<Bytes>,
}

impl Broker {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self { tx }
    }

    pub fn publish(&self, data: Bytes) {
        // Ignore error - means no receivers
        let _ = self.tx.send(data);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Bytes> {
        self.tx.subscribe()
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
}
