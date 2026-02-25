use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rand::Rng;

/// Time-to-live for a pending ticket.
const TICKET_TTL: Duration = Duration::from_secs(30);

/// Maximum number of pending (unconsumed) tickets.
const MAX_PENDING_TICKETS: usize = 1024;

/// In-memory store of short-lived, single-use tickets for WebSocket authentication.
///
/// Browser WebSocket connections cannot set custom HTTP headers, so the
/// traditional `Authorization: Bearer <token>` flow doesn't work. Instead:
///
/// 1. Client authenticates via `POST /auth/ws-ticket` (with Bearer token)
/// 2. Server returns a single-use nonce (the "ticket")
/// 3. Client opens WebSocket with `?ticket=<nonce>`
/// 4. Server validates and consumes the ticket on upgrade
///
/// Tickets expire after 30 seconds and can only be used once.
pub struct TicketStore {
    inner: Mutex<HashMap<String, Instant>>,
}

impl TicketStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Create a new ticket. Returns the nonce on success, or `Err(())` if the
    /// maximum number of pending tickets has been reached.
    pub fn create(&self) -> Result<String, ()> {
        let mut map = self.inner.lock();

        // Prune expired tickets first
        let now = Instant::now();
        map.retain(|_, created| now.duration_since(*created) < TICKET_TTL);

        if map.len() >= MAX_PENDING_TICKETS {
            return Err(());
        }

        let nonce: String = rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(32)
            .map(char::from)
            .collect();

        map.insert(nonce.clone(), now);
        Ok(nonce)
    }

    /// Validate and consume a ticket. Returns `true` if the ticket was valid
    /// and has been consumed (removed). Returns `false` if the ticket does not
    /// exist or has expired.
    pub fn validate(&self, ticket: &str) -> bool {
        let mut map = self.inner.lock();
        match map.remove(ticket) {
            Some(created) => Instant::now().duration_since(created) < TICKET_TTL,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_returns_nonce() {
        let store = TicketStore::new();
        let ticket = store.create().unwrap();
        assert_eq!(ticket.len(), 32);
    }

    #[test]
    fn validate_consumes_ticket() {
        let store = TicketStore::new();
        let ticket = store.create().unwrap();
        assert!(store.validate(&ticket));
        // Second use should fail (single-use)
        assert!(!store.validate(&ticket));
    }

    #[test]
    fn validate_rejects_unknown() {
        let store = TicketStore::new();
        assert!(!store.validate("nonexistent"));
    }

    #[test]
    fn limit_enforced() {
        let store = TicketStore::new();
        for _ in 0..MAX_PENDING_TICKETS {
            store.create().unwrap();
        }
        assert!(store.create().is_err());
    }

    #[test]
    fn expired_tickets_pruned_on_create() {
        let store = TicketStore::new();

        // Insert a ticket with a backdated timestamp
        {
            let mut map = store.inner.lock();
            map.insert(
                "old-ticket".to_string(),
                Instant::now() - Duration::from_secs(60),
            );
        }

        // Expired ticket should not validate
        assert!(!store.validate("old-ticket"));

        // Creating new tickets should succeed (expired ones pruned)
        let ticket = store.create().unwrap();
        assert!(store.validate(&ticket));
    }
}
