use bytes::Bytes;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::{broadcast, mpsc};
use tokio::sync::broadcast as tokio_broadcast;

use crate::activity::ActivityTracker;
use crate::input::{InputBroadcaster, InputMode};
use crate::overlay::OverlayStore;
use crate::panel::PanelStore;
use crate::parser::Parser;
use crate::pty::Pty;
use crate::shutdown::ShutdownCoordinator;
use crate::terminal::TerminalSize;

/// A single terminal session with all associated state.
///
/// Each `Session` owns the PTY, parser, I/O channels, and auxiliary stores
/// for one terminal session. In standalone mode there is exactly one session;
/// in server mode the `SessionRegistry` manages many.
#[derive(Clone)]
pub struct Session {
    /// Human-readable session name (displayed in UI, used in URLs).
    pub name: String,
    pub input_tx: mpsc::Sender<Bytes>,
    pub output_rx: broadcast::Sender<Bytes>,
    pub shutdown: ShutdownCoordinator,
    pub parser: Parser,
    pub overlays: OverlayStore,
    pub panels: PanelStore,
    pub pty: Arc<Pty>,
    pub terminal_size: TerminalSize,
    pub input_mode: InputMode,
    pub input_broadcaster: InputBroadcaster,
    pub activity: ActivityTracker,
}

/// Server-level session lifecycle events.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    Created { name: String },
    Exited { name: String },
    Destroyed { name: String },
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("session name already exists: {0}")]
    NameExists(String),
    #[error("session not found: {0}")]
    NotFound(String),
}

struct RegistryInner {
    sessions: HashMap<String, Session>,
    next_id: u64,
}

/// Manages multiple sessions by name.
#[derive(Clone)]
pub struct SessionRegistry {
    inner: Arc<RwLock<RegistryInner>>,
    events_tx: tokio_broadcast::Sender<SessionEvent>,
}

impl SessionRegistry {
    /// Create an empty registry with a broadcast channel for lifecycle events.
    pub fn new() -> Self {
        let (events_tx, _) = tokio_broadcast::channel(64);
        Self {
            inner: Arc::new(RwLock::new(RegistryInner {
                sessions: HashMap::new(),
                next_id: 0,
            })),
            events_tx,
        }
    }

    /// Insert a session into the registry.
    ///
    /// If `name` is `None`, an auto-generated numeric name is assigned
    /// (starting from 0, skipping names already in use). If `name` is
    /// `Some` and the name is already taken, returns `RegistryError::NameExists`.
    ///
    /// The session's `name` field is updated to the assigned name before
    /// insertion, and a `SessionEvent::Created` event is emitted.
    pub fn insert(
        &self,
        name: Option<String>,
        mut session: Session,
    ) -> Result<String, RegistryError> {
        let mut inner = self.inner.write().unwrap();

        let assigned_name = match name {
            Some(n) => {
                if inner.sessions.contains_key(&n) {
                    return Err(RegistryError::NameExists(n));
                }
                n
            }
            None => {
                let mut id = inner.next_id;
                loop {
                    let candidate = id.to_string();
                    if !inner.sessions.contains_key(&candidate) {
                        inner.next_id = id + 1;
                        break candidate;
                    }
                    id += 1;
                }
            }
        };

        session.name = assigned_name.clone();
        inner.sessions.insert(assigned_name.clone(), session);

        // Send event (ignore error if there are no receivers).
        let _ = self.events_tx.send(SessionEvent::Created {
            name: assigned_name.clone(),
        });

        Ok(assigned_name)
    }

    /// Look up a session by name, returning a clone if found.
    pub fn get(&self, name: &str) -> Option<Session> {
        let inner = self.inner.read().unwrap();
        inner.sessions.get(name).cloned()
    }

    /// Remove a session by name, returning the removed session if found.
    ///
    /// Emits a `SessionEvent::Destroyed` event when a session is removed.
    pub fn remove(&self, name: &str) -> Option<Session> {
        let mut inner = self.inner.write().unwrap();
        let removed = inner.sessions.remove(name);
        if removed.is_some() {
            let _ = self.events_tx.send(SessionEvent::Destroyed {
                name: name.to_string(),
            });
        }
        removed
    }

    /// Rename a session.
    ///
    /// Returns `RegistryError::NotFound` if `old_name` does not exist, or
    /// `RegistryError::NameExists` if `new_name` is already taken.
    /// Updates the session's `name` field to `new_name`.
    pub fn rename(&self, old_name: &str, new_name: &str) -> Result<(), RegistryError> {
        let mut inner = self.inner.write().unwrap();

        if !inner.sessions.contains_key(old_name) {
            return Err(RegistryError::NotFound(old_name.to_string()));
        }
        if inner.sessions.contains_key(new_name) {
            return Err(RegistryError::NameExists(new_name.to_string()));
        }

        let mut session = inner.sessions.remove(old_name).unwrap();
        session.name = new_name.to_string();
        inner.sessions.insert(new_name.to_string(), session);
        Ok(())
    }

    /// Return all session names.
    pub fn list(&self) -> Vec<String> {
        let inner = self.inner.read().unwrap();
        inner.sessions.keys().cloned().collect()
    }

    /// Return the number of sessions.
    pub fn len(&self) -> usize {
        let inner = self.inner.read().unwrap();
        inner.sessions.len()
    }

    /// Subscribe to session lifecycle events.
    pub fn subscribe_events(&self) -> tokio_broadcast::Receiver<SessionEvent> {
        self.events_tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::Broker;

    /// Helper: build a minimal Session suitable for unit tests.
    fn create_test_session(name: &str) -> (Session, mpsc::Receiver<Bytes>) {
        let (input_tx, input_rx) = mpsc::channel(64);
        let broker = Broker::new();
        let parser = Parser::spawn(&broker, 80, 24, 1000);
        let pty = crate::pty::Pty::spawn(24, 80, crate::pty::SpawnCommand::default())
            .expect("failed to spawn PTY for test");

        let session = Session {
            name: name.to_string(),
            input_tx,
            output_rx: broker.sender(),
            shutdown: ShutdownCoordinator::new(),
            parser,
            overlays: OverlayStore::new(),
            panels: PanelStore::new(),
            pty: Arc::new(pty),
            terminal_size: TerminalSize::new(24, 80),
            input_mode: InputMode::new(),
            input_broadcaster: InputBroadcaster::new(),
            activity: ActivityTracker::new(),
        };
        (session, input_rx)
    }

    #[tokio::test]
    async fn test_session_can_be_constructed_with_name() {
        let (session, _rx) = create_test_session("my-session");
        assert_eq!(session.name, "my-session");
    }

    #[tokio::test]
    async fn test_session_is_cloneable() {
        let (session, _rx) = create_test_session("clone-me");
        let cloned = session.clone();

        // Both copies share the same name.
        assert_eq!(cloned.name, "clone-me");

        // The underlying broadcast sender is shared (same channel).
        assert_eq!(
            session.output_rx.receiver_count(),
            cloned.output_rx.receiver_count(),
        );
    }

    /// Helper: build a minimal Session for registry tests (discards the receiver).
    fn make_test_session(name: &str) -> Session {
        let (session, _rx) = create_test_session(name);
        session
    }

    // ---- SessionRegistry tests ----

    #[tokio::test]
    async fn registry_insert_with_name() {
        let registry = SessionRegistry::new();
        let session = make_test_session("placeholder");
        let name = registry
            .insert(Some("alpha".to_string()), session)
            .unwrap();
        assert_eq!(name, "alpha");

        let retrieved = registry.get("alpha").expect("session should exist");
        assert_eq!(retrieved.name, "alpha");
    }

    #[tokio::test]
    async fn registry_insert_auto_name() {
        let registry = SessionRegistry::new();

        let name0 = registry
            .insert(None, make_test_session("x"))
            .unwrap();
        assert_eq!(name0, "0");

        let name1 = registry
            .insert(None, make_test_session("x"))
            .unwrap();
        assert_eq!(name1, "1");
    }

    #[tokio::test]
    async fn registry_insert_duplicate_name_fails() {
        let registry = SessionRegistry::new();
        registry
            .insert(Some("dup".to_string()), make_test_session("x"))
            .unwrap();

        let err = registry
            .insert(Some("dup".to_string()), make_test_session("x"))
            .unwrap_err();
        assert!(
            matches!(err, RegistryError::NameExists(ref n) if n == "dup"),
            "expected NameExists(\"dup\"), got: {err:?}"
        );
    }

    #[tokio::test]
    async fn registry_remove() {
        let registry = SessionRegistry::new();
        registry
            .insert(Some("rm-me".to_string()), make_test_session("x"))
            .unwrap();

        let removed = registry.remove("rm-me");
        assert!(removed.is_some());
        assert!(registry.get("rm-me").is_none());
    }

    #[tokio::test]
    async fn registry_remove_nonexistent() {
        let registry = SessionRegistry::new();
        assert!(registry.remove("ghost").is_none());
    }

    #[tokio::test]
    async fn registry_rename() {
        let registry = SessionRegistry::new();
        registry
            .insert(Some("old".to_string()), make_test_session("x"))
            .unwrap();

        registry.rename("old", "new").unwrap();

        assert!(registry.get("old").is_none(), "old name should be gone");
        let session = registry.get("new").expect("new name should exist");
        assert_eq!(session.name, "new");
    }

    #[tokio::test]
    async fn registry_rename_to_existing_fails() {
        let registry = SessionRegistry::new();
        registry
            .insert(Some("a".to_string()), make_test_session("x"))
            .unwrap();
        registry
            .insert(Some("b".to_string()), make_test_session("x"))
            .unwrap();

        let err = registry.rename("a", "b").unwrap_err();
        assert!(
            matches!(err, RegistryError::NameExists(ref n) if n == "b"),
            "expected NameExists(\"b\"), got: {err:?}"
        );
    }

    #[tokio::test]
    async fn registry_rename_nonexistent_fails() {
        let registry = SessionRegistry::new();
        let err = registry.rename("nope", "whatever").unwrap_err();
        assert!(
            matches!(err, RegistryError::NotFound(ref n) if n == "nope"),
            "expected NotFound(\"nope\"), got: {err:?}"
        );
    }

    #[tokio::test]
    async fn registry_list() {
        let registry = SessionRegistry::new();
        registry
            .insert(Some("foo".to_string()), make_test_session("x"))
            .unwrap();
        registry
            .insert(Some("bar".to_string()), make_test_session("x"))
            .unwrap();

        let mut names = registry.list();
        names.sort();
        assert_eq!(names, vec!["bar", "foo"]);
    }

    #[tokio::test]
    async fn registry_len() {
        let registry = SessionRegistry::new();
        assert_eq!(registry.len(), 0);

        registry
            .insert(Some("a".to_string()), make_test_session("x"))
            .unwrap();
        assert_eq!(registry.len(), 1);

        registry
            .insert(Some("b".to_string()), make_test_session("x"))
            .unwrap();
        assert_eq!(registry.len(), 2);

        registry.remove("a");
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn registry_auto_name_skips_taken_names() {
        let registry = SessionRegistry::new();

        // Manually insert "0" so auto-naming must skip it.
        registry
            .insert(Some("0".to_string()), make_test_session("x"))
            .unwrap();

        let name = registry.insert(None, make_test_session("x")).unwrap();
        assert_eq!(name, "1", "auto-name should skip occupied \"0\"");
    }

    #[tokio::test]
    async fn registry_emits_events() {
        let registry = SessionRegistry::new();
        let mut rx = registry.subscribe_events();

        registry
            .insert(Some("evt".to_string()), make_test_session("x"))
            .unwrap();
        registry.remove("evt");

        let ev1 = rx.recv().await.expect("should receive Created event");
        assert!(
            matches!(ev1, SessionEvent::Created { ref name } if name == "evt"),
            "expected Created {{ name: \"evt\" }}, got: {ev1:?}"
        );

        let ev2 = rx.recv().await.expect("should receive Destroyed event");
        assert!(
            matches!(ev2, SessionEvent::Destroyed { ref name } if name == "evt"),
            "expected Destroyed {{ name: \"evt\" }}, got: {ev2:?}"
        );
    }
}
