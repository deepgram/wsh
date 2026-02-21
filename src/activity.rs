use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// Tracks the timestamp of the last terminal activity (PTY output or input).
///
/// Clients can wait for the terminal to become idle — a period of inactivity
/// exceeding a specified timeout — to detect when a command has finished
/// producing output.
///
/// Each activity event increments a monotonic generation counter. Clients can
/// pass back the generation from a previous idle response to avoid
/// busy-looping when the terminal is already idle (the "ETag" pattern).
#[derive(Clone)]
pub struct ActivityTracker {
    tx: Arc<watch::Sender<Instant>>,
    generation: Arc<AtomicU64>,
}

impl Default for ActivityTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivityTracker {
    /// Create a new tracker seeded with the current instant.
    pub fn new() -> Self {
        let (tx, _) = watch::channel(Instant::now());
        Self {
            tx: Arc::new(tx),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Record activity. Safe to call from blocking threads.
    pub fn touch(&self) {
        self.generation.fetch_add(1, Ordering::Release);
        self.tx.send_replace(Instant::now());
    }

    /// Current generation counter value.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Subscribe to activity changes. Returns a watch receiver that gets
    /// notified each time `touch()` is called.
    pub fn subscribe(&self) -> watch::Receiver<Instant> {
        self.tx.subscribe()
    }

    /// Return how many milliseconds have elapsed since the last activity.
    pub fn last_activity_ms(&self) -> u64 {
        self.tx.borrow().elapsed().as_millis() as u64
    }

    /// Wait until `timeout` has elapsed since the last activity.
    ///
    /// If `last_seen` is provided and matches the current generation, the
    /// method first waits for new activity before entering the idle
    /// loop. This prevents the busy-loop storm where a client repeatedly
    /// polls and gets immediate responses because nothing has changed.
    ///
    /// Returns the generation counter at the time idle was detected.
    pub async fn wait_for_idle(
        &self,
        timeout: Duration,
        last_seen: Option<u64>,
    ) -> u64 {
        let mut rx = self.tx.subscribe();

        // If the caller already saw this generation, wait for new activity
        // before entering the idle loop.
        if let Some(seen) = last_seen {
            let current = self.generation.load(Ordering::Acquire);
            if current == seen && rx.changed().await.is_err() {
                return self.generation.load(Ordering::Acquire);
            }
        }

        loop {
            let last = *rx.borrow_and_update();
            let elapsed = last.elapsed();
            if elapsed >= timeout {
                return self.generation.load(Ordering::Acquire);
            }
            let remaining = timeout - elapsed;
            tokio::select! {
                _ = tokio::time::sleep(remaining) => {
                    // Double-check: a touch may have arrived in the tiny window
                    // between sleep completing and us running.
                    let last = *rx.borrow_and_update();
                    if last.elapsed() >= timeout {
                        return self.generation.load(Ordering::Acquire);
                    }
                    // Not yet idle — loop again with fresh remaining.
                }
                res = rx.changed() => {
                    if res.is_err() {
                        // Sender dropped — treat as idle.
                        return self.generation.load(Ordering::Acquire);
                    }
                    // Activity detected — loop to recalculate remaining.
                }
            }
        }
    }

    /// Wait until `timeout` has elapsed since the last activity, but always
    /// observe at least `timeout` of real silence before returning.
    ///
    /// Unlike [`wait_for_idle`], this never returns immediately even
    /// if the terminal has been idle for longer than `timeout`. This trades
    /// latency for API simplicity — no generation tracking required.
    ///
    /// Returns the generation counter at the time idle was confirmed.
    pub async fn wait_for_fresh_idle(&self, timeout: Duration) -> u64 {
        let mut rx = self.tx.subscribe();
        loop {
            let last = *rx.borrow_and_update();
            let elapsed = last.elapsed();
            // Even if already idle, wait at least `timeout` to confirm.
            let remaining = if elapsed >= timeout {
                timeout
            } else {
                timeout - elapsed
            };
            tokio::select! {
                _ = tokio::time::sleep(remaining) => {
                    let last = *rx.borrow_and_update();
                    if last.elapsed() >= timeout {
                        return self.generation.load(Ordering::Acquire);
                    }
                    // Activity arrived during sleep — loop again.
                }
                res = rx.changed() => {
                    if res.is_err() {
                        return self.generation.load(Ordering::Acquire);
                    }
                    // Activity detected — loop with fresh remaining.
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn touch_updates_state() {
        let tracker = ActivityTracker::new();
        let before = Instant::now();
        tokio::time::sleep(Duration::from_millis(10)).await;
        tracker.touch();
        // The last activity should be after `before`.
        let mut rx = tracker.tx.subscribe();
        let last = *rx.borrow_and_update();
        assert!(last > before);
    }

    #[tokio::test]
    async fn touch_increments_generation() {
        let tracker = ActivityTracker::new();
        assert_eq!(tracker.generation(), 0);
        tracker.touch();
        assert_eq!(tracker.generation(), 1);
        tracker.touch();
        assert_eq!(tracker.generation(), 2);
    }

    #[tokio::test]
    async fn idle_fires_after_timeout() {
        let tracker = ActivityTracker::new();
        tracker.touch();
        let start = Instant::now();
        tracker.wait_for_idle(Duration::from_millis(50), None).await;
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(50));
    }

    #[tokio::test]
    async fn idle_returns_generation() {
        let tracker = ActivityTracker::new();
        tracker.touch();
        tracker.touch();
        tracker.touch();
        let gen = tracker.wait_for_idle(Duration::from_millis(50), None).await;
        assert_eq!(gen, 3);
    }

    #[tokio::test]
    async fn activity_resets_timer() {
        let tracker = ActivityTracker::new();
        tracker.touch(); // gen=1

        let t = tracker.clone();
        // Spawn a task that touches after 20ms, resetting the timer.
        // The large gap between touch delay (20ms) and timeout (150ms)
        // ensures the touch fires well before the timeout under any
        // realistic scheduler load.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            t.touch(); // gen=2
        });

        let start = Instant::now();
        let gen = tracker.wait_for_idle(Duration::from_millis(150), None).await;
        let elapsed = start.elapsed();
        // The touch at ~20ms resets the timer; total >= 20ms + 150ms = 170ms.
        assert_eq!(gen, 2, "second touch should have been observed");
        assert!(
            elapsed >= Duration::from_millis(150),
            "Expected >= 150ms (timer should have reset on activity), got {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn already_idle_returns_immediately() {
        let tracker = ActivityTracker::new();
        // Don't touch — the seed instant was set at construction time.
        // Wait long enough that the seed is stale.
        tokio::time::sleep(Duration::from_millis(60)).await;

        let start = Instant::now();
        tracker.wait_for_idle(Duration::from_millis(50), None).await;
        let elapsed = start.elapsed();
        // Should return almost immediately.
        assert!(elapsed < Duration::from_millis(10));
    }

    #[tokio::test]
    async fn last_seen_prevents_immediate_return() {
        let tracker = ActivityTracker::new();
        tracker.touch(); // generation = 1
        // Wait for idle
        tokio::time::sleep(Duration::from_millis(60)).await;

        // Without last_seen: returns immediately
        let start = Instant::now();
        let gen = tracker.wait_for_idle(Duration::from_millis(50), None).await;
        assert!(start.elapsed() < Duration::from_millis(10));
        assert_eq!(gen, 1);

        // With last_seen matching current generation: blocks until new activity
        let t = tracker.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            t.touch(); // generation = 2
        });

        let start = Instant::now();
        let gen = tracker.wait_for_idle(Duration::from_millis(30), Some(1)).await;
        let elapsed = start.elapsed();
        // Should wait ~50ms for new activity + ~30ms for idle
        assert!(elapsed >= Duration::from_millis(70));
        assert_eq!(gen, 2);
    }

    #[tokio::test]
    async fn last_seen_stale_returns_normally() {
        let tracker = ActivityTracker::new();
        tracker.touch(); // generation = 1
        tracker.touch(); // generation = 2

        // Wait for idle
        tokio::time::sleep(Duration::from_millis(60)).await;

        // last_seen=1 but current generation=2: doesn't block on new activity
        let start = Instant::now();
        let gen = tracker.wait_for_idle(Duration::from_millis(50), Some(1)).await;
        assert!(start.elapsed() < Duration::from_millis(10));
        assert_eq!(gen, 2);
    }

    #[tokio::test]
    async fn fresh_idle_always_waits() {
        let tracker = ActivityTracker::new();
        // Wait for well past the timeout so terminal is "already idle"
        tokio::time::sleep(Duration::from_millis(120)).await;

        let start = Instant::now();
        tracker.wait_for_fresh_idle(Duration::from_millis(50)).await;
        let elapsed = start.elapsed();
        // Should wait at least 50ms even though already idle
        assert!(
            elapsed >= Duration::from_millis(45),
            "Expected >= 45ms, got {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn fresh_idle_resets_on_activity() {
        let tracker = ActivityTracker::new();
        tokio::time::sleep(Duration::from_millis(200)).await;

        let t = tracker.clone();
        // Touch after 20ms. The large gap between touch delay (20ms) and
        // timeout (150ms) ensures the touch fires well before the initial
        // sleep expires under any realistic scheduler load.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            t.touch(); // gen=1
        });

        let start = Instant::now();
        let gen = tracker.wait_for_fresh_idle(Duration::from_millis(150)).await;
        let elapsed = start.elapsed();
        // The touch at ~20ms resets the timer; total >= 20ms + 150ms = 170ms.
        assert_eq!(gen, 1, "touch should have been observed");
        assert!(
            elapsed >= Duration::from_millis(150),
            "Expected >= 150ms (timer should have reset on activity), got {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn multiple_concurrent_waiters() {
        let tracker = ActivityTracker::new();
        tracker.touch();

        let t1 = tracker.clone();
        let t2 = tracker.clone();

        let (r1, r2) = tokio::join!(
            async move {
                let start = Instant::now();
                t1.wait_for_idle(Duration::from_millis(50), None).await;
                start.elapsed()
            },
            async move {
                let start = Instant::now();
                t2.wait_for_idle(Duration::from_millis(50), None).await;
                start.elapsed()
            },
        );

        assert!(r1 >= Duration::from_millis(50));
        assert!(r2 >= Duration::from_millis(50));
    }

    #[tokio::test]
    async fn last_activity_ms_zero_at_start() {
        let tracker = ActivityTracker::new();
        tokio::time::sleep(Duration::from_millis(10)).await;
        tracker.touch();
        assert!(tracker.last_activity_ms() < 10);
    }

    #[tokio::test]
    async fn last_activity_ms_grows_without_touch() {
        let tracker = ActivityTracker::new();
        tracker.touch();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(tracker.last_activity_ms() >= 50);
    }
}
