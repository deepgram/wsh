//! Graceful shutdown coordination for connected clients.
//!
//! Tracks active WebSocket connections and provides a mechanism to:
//! 1. Signal all connections to close
//! 2. Wait until all connections have actually closed

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{watch, Notify};

/// Coordinates graceful shutdown of client connections.
#[derive(Clone)]
pub struct ShutdownCoordinator {
    inner: Arc<Inner>,
}

struct Inner {
    /// Signals shutdown to all listeners
    shutdown_tx: watch::Sender<bool>,
    /// Active connection count
    active: AtomicUsize,
    /// Notified when all connections close
    all_closed: Notify,
    /// Notified when interest level changes (connection count decrements or
    /// external events that may affect shutdown decisions).
    interest_changed: Notify,
}

impl ShutdownCoordinator {
    pub fn new() -> Self {
        let (shutdown_tx, _) = watch::channel(false);
        Self {
            inner: Arc::new(Inner {
                shutdown_tx,
                active: AtomicUsize::new(0),
                all_closed: Notify::new(),
                interest_changed: Notify::new(),
            }),
        }
    }

    /// Register a new connection. Returns a guard that must be held for the
    /// connection's lifetime, and a receiver for the shutdown signal.
    pub fn register(&self) -> (ConnectionGuard, watch::Receiver<bool>) {
        self.inner.active.fetch_add(1, Ordering::SeqCst);
        let guard = ConnectionGuard {
            inner: self.inner.clone(),
        };
        let shutdown_rx = self.inner.shutdown_tx.subscribe();
        (guard, shutdown_rx)
    }

    /// Signal all connections to shut down.
    pub fn shutdown(&self) {
        let _ = self.inner.shutdown_tx.send(true);
    }

    /// Wait until all connections have closed.
    /// Returns immediately if there are no active connections.
    pub async fn wait_for_all_closed(&self) {
        loop {
            // Register the notified future BEFORE checking the count to avoid
            // a race where a guard is dropped between load() and notified().
            let notified = self.inner.all_closed.notified();
            let count = self.inner.active.load(Ordering::SeqCst);
            if count == 0 {
                return;
            }
            tracing::debug!(count, "waiting for connections to close");
            notified.await;
        }
    }

    /// Returns the current number of active connections.
    pub fn active_count(&self) -> usize {
        self.inner.active.load(Ordering::SeqCst)
    }

    /// Wake any task waiting on [`interest_changed`](Self::interest_changed).
    ///
    /// Called by external code (e.g., MCP session drop) to signal that the
    /// interest level may have changed. `ConnectionGuard::drop` already calls
    /// this automatically.
    pub fn notify_interest_changed(&self) {
        self.inner.interest_changed.notify_waiters();
    }

    /// Returns a `Notified` future that completes when interest level changes.
    ///
    /// **TOCTOU-safe usage:** register the `Notified` *before* checking state:
    /// ```ignore
    /// let notified = coordinator.interest_changed();
    /// if coordinator.active_count() == 0 { /* shut down */ }
    /// notified.await; // won't miss drops between check and wait
    /// ```
    pub fn interest_changed(&self) -> tokio::sync::futures::Notified<'_> {
        self.inner.interest_changed.notified()
    }
}

impl Default for ShutdownCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard that decrements connection count when dropped.
pub struct ConnectionGuard {
    inner: Arc<Inner>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let prev = self.inner.active.fetch_sub(1, Ordering::SeqCst);
        // Always notify interest_changed — any decrement could tip the
        // ephemeral monitor's has_interest() check (which combines
        // active_count with sessions.is_empty()).
        self.inner.interest_changed.notify_waiters();
        if prev == 1 {
            // We were the last connection, notify waiters
            self.inner.all_closed.notify_waiters();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_no_connections_returns_immediately() {
        let coord = ShutdownCoordinator::new();
        coord.shutdown();
        // Should not block
        coord.wait_for_all_closed().await;
    }

    #[tokio::test]
    async fn test_wait_for_connection_to_close() {
        let coord = ShutdownCoordinator::new();
        let (guard, mut shutdown_rx) = coord.register();

        assert_eq!(coord.active_count(), 1);

        // Signal shutdown
        coord.shutdown();
        assert!(*shutdown_rx.borrow_and_update());

        // Spawn wait task
        let coord_clone = coord.clone();
        let wait_task = tokio::spawn(async move {
            coord_clone.wait_for_all_closed().await;
        });

        // Give wait task a moment to start waiting
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!wait_task.is_finished());

        // Drop the guard (connection closed)
        drop(guard);

        // Wait should complete
        tokio::time::timeout(Duration::from_millis(100), wait_task)
            .await
            .expect("should complete")
            .expect("should not panic");

        assert_eq!(coord.active_count(), 0);
    }

    #[tokio::test]
    async fn test_multiple_connections() {
        let coord = ShutdownCoordinator::new();
        let (guard1, _) = coord.register();
        let (guard2, _) = coord.register();
        let (guard3, _) = coord.register();

        assert_eq!(coord.active_count(), 3);

        coord.shutdown();

        let coord_clone = coord.clone();
        let wait_task = tokio::spawn(async move {
            coord_clone.wait_for_all_closed().await;
        });

        // Drop connections one by one
        drop(guard1);
        assert_eq!(coord.active_count(), 2);
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(!wait_task.is_finished());

        drop(guard2);
        assert_eq!(coord.active_count(), 1);
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(!wait_task.is_finished());

        drop(guard3);
        assert_eq!(coord.active_count(), 0);

        // Now wait should complete
        tokio::time::timeout(Duration::from_millis(100), wait_task)
            .await
            .expect("should complete")
            .expect("should not panic");
    }

    #[tokio::test]
    async fn test_interest_changed_fires_on_guard_drop() {
        let coord = ShutdownCoordinator::new();
        let (guard, _) = coord.register();
        assert_eq!(coord.active_count(), 1);

        let notified = coord.interest_changed();
        drop(guard);

        // Should complete promptly
        tokio::time::timeout(Duration::from_millis(100), notified)
            .await
            .expect("interest_changed should fire on guard drop");
        assert_eq!(coord.active_count(), 0);
    }

    #[tokio::test]
    async fn test_interest_changed_fires_on_external_notify() {
        let coord = ShutdownCoordinator::new();

        let notified = coord.interest_changed();
        coord.notify_interest_changed();

        tokio::time::timeout(Duration::from_millis(100), notified)
            .await
            .expect("interest_changed should fire on external notify");
    }

    #[tokio::test]
    async fn test_interest_changed_toctou_safety() {
        // Verify that registering the Notified BEFORE checking count
        // correctly catches a guard drop that happens between check and await.
        let coord = ShutdownCoordinator::new();
        let (guard, _) = coord.register();

        // Step 1: register notified BEFORE checking state
        let notified = coord.interest_changed();

        // Step 2: check state — still 1
        assert_eq!(coord.active_count(), 1);

        // Step 3: guard drops between check and await
        drop(guard);

        // Step 4: notified must fire (not hang)
        tokio::time::timeout(Duration::from_millis(100), notified)
            .await
            .expect("TOCTOU: interest_changed must fire even when guard drops between check and await");
        assert_eq!(coord.active_count(), 0);
    }

    #[tokio::test]
    async fn test_shutdown_signal_received() {
        let coord = ShutdownCoordinator::new();
        let (_guard, mut shutdown_rx) = coord.register();

        // Initially false
        assert!(!*shutdown_rx.borrow());

        // Signal shutdown
        coord.shutdown();

        // Should receive true
        shutdown_rx.changed().await.unwrap();
        assert!(*shutdown_rx.borrow());
    }
}
