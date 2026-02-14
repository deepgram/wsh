use bytes::Bytes;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use parking_lot::RwLock;
use tokio::sync::{broadcast, mpsc};
use tokio::sync::broadcast as tokio_broadcast;

use crate::activity::ActivityTracker;
use crate::input::{FocusTracker, InputBroadcaster, InputMode};
use crate::overlay::{OverlayStore, ScreenMode};
use crate::panel::PanelStore;
use crate::parser::Parser;
use crate::protocol::VisualUpdate;
use crate::pty::{Pty, PtyError, SpawnCommand};
use crate::shutdown::ShutdownCoordinator;
use crate::terminal::TerminalSize;

/// A single terminal session with all associated state.
///
/// Each `Session` owns the PTY, parser, I/O channels, and auxiliary stores
/// for one terminal session. The `SessionRegistry` manages all sessions
/// on the server.
#[derive(Clone)]
pub struct Session {
    /// Human-readable session name (displayed in UI, used in URLs).
    pub name: String,
    /// PID of the child process spawned in the PTY, if available.
    pub pid: Option<u32>,
    /// Human-readable display of the command being run (e.g. shell path or command string).
    pub command: String,
    /// Number of currently connected streaming clients (WebSocket, socket, etc.).
    pub client_count: Arc<AtomicUsize>,
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
    /// Tracks which overlay or panel currently has input focus.
    pub focus: FocusTracker,
    /// Signal to detach all streaming clients from this session.
    /// Subscribers receive `()` when `detach()` is called; the session stays alive.
    pub detach_signal: broadcast::Sender<()>,
    /// Notification channel for overlay/panel visual state changes.
    /// API handlers fire events here after mutations; the server streaming loop
    /// picks them up and sends OverlaySync/PanelSync frames to socket clients.
    pub visual_update_tx: broadcast::Sender<VisualUpdate>,
    /// Current screen mode (normal or alt). Used to tag overlays/panels and
    /// filter list results. Protected by a `parking_lot::RwLock` for cheap
    /// cloning across threads.
    pub screen_mode: Arc<RwLock<ScreenMode>>,
    /// Cancellation token that fires when this session is killed/removed.
    /// WS handlers add this to their `select!` loop to detect session death
    /// immediately rather than operating on ghost state.
    pub cancelled: tokio_util::sync::CancellationToken,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("name", &self.name)
            .field("pid", &self.pid)
            .field("command", &self.command)
            .finish_non_exhaustive()
    }
}

/// RAII guard that decrements the session client count on drop.
pub struct ClientGuard {
    counter: Arc<AtomicUsize>,
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Session {
    /// Register a new streaming client, returning an RAII guard that decrements
    /// the count when dropped.
    pub fn connect(&self) -> ClientGuard {
        self.client_count.fetch_add(1, Ordering::Relaxed);
        ClientGuard {
            counter: Arc::clone(&self.client_count),
        }
    }

    /// Return the number of currently connected streaming clients.
    pub fn clients(&self) -> usize {
        self.client_count.load(Ordering::Relaxed)
    }

    /// Signal all attached streaming clients to detach.
    ///
    /// The session remains alive — only the streaming connections are closed.
    pub fn detach(&self) {
        let _ = self.detach_signal.send(());
    }

    /// Explicitly shut down this session's background tasks.
    ///
    /// Called when a spawned session cannot be registered in the registry
    /// (e.g. due to a name conflict). Cancels the session's cancellation
    /// token and signals detach so all background tasks exit promptly.
    pub fn shutdown(&self) {
        self.cancelled.cancel();
        self.detach();
    }

    /// Send SIGKILL to the child process if we have a PID.
    ///
    /// Used as an escalation path when the child ignores SIGHUP during
    /// shutdown/drain.
    pub fn kill_child(&self) {
        if let Some(pid) = self.pid {
            if pid > i32::MAX as u32 {
                tracing::warn!(pid, "PID exceeds i32::MAX, cannot send signal");
                return;
            }
            #[cfg(unix)]
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
    }

    /// Spawn a new session with a PTY and all associated I/O tasks.
    ///
    /// The PTY reader only publishes to the broker (no stdout -- server mode).
    /// The PTY writer consumes from the input channel.
    ///
    /// Returns the session and a oneshot receiver that fires when the child
    /// process exits. If the child handle is unavailable the receiver resolves
    /// immediately.
    pub fn spawn(
        name: String,
        command: SpawnCommand,
        rows: u16,
        cols: u16,
    ) -> Result<(Self, tokio::sync::oneshot::Receiver<()>), PtyError> {
        Self::spawn_with_options(name, command, rows, cols, None, None)
    }

    /// Spawn a new session with optional cwd and environment overrides.
    pub fn spawn_with_options(
        name: String,
        command: SpawnCommand,
        rows: u16,
        cols: u16,
        cwd: Option<String>,
        env: Option<std::collections::HashMap<String, String>>,
    ) -> Result<(Self, tokio::sync::oneshot::Receiver<()>), PtyError> {
        let command_display = match &command {
            SpawnCommand::Shell { shell, .. } => {
                shell.clone().unwrap_or_else(|| {
                    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
                })
            }
            SpawnCommand::Command { command, .. } => command.clone(),
        };
        let mut cmd = Pty::build_command(&command);
        if let Some(ref dir) = cwd {
            cmd.cwd(dir);
        }
        if let Some(ref vars) = env {
            for (k, v) in vars {
                cmd.env(k, v);
            }
        }
        let mut pty = Pty::spawn_with_cmd(rows, cols, cmd)?;
        let pty_reader = pty.take_reader()?;
        let pty_writer = pty.take_writer()?;
        let pty_child = pty.take_child();
        let pid = pty_child.as_ref().and_then(|c| c.process_id());
        let pty = Arc::new(pty);

        // Monitor child exit via a oneshot channel.
        let (child_exit_tx, child_exit_rx) = tokio::sync::oneshot::channel::<()>();
        if let Some(mut child) = pty_child {
            tokio::task::spawn_blocking(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    match child.wait() {
                        Ok(status) => tracing::debug!(?status, "session child exited"),
                        Err(e) => tracing::error!(?e, "error waiting for session child"),
                    }
                }));
                if let Err(e) = result {
                    tracing::error!("child exit monitor task panicked: {:?}", e);
                }
                let _ = child_exit_tx.send(());
            });
        } else {
            // No child to wait on; signal immediately.
            let _ = child_exit_tx.send(());
        }

        let broker = crate::broker::Broker::new();
        let parser = Parser::spawn(&broker, cols as usize, rows as usize, 10_000);
        let (input_tx, input_rx) = mpsc::channel::<Bytes>(64);
        let shutdown = ShutdownCoordinator::new();
        let overlays = OverlayStore::new();
        let panels = PanelStore::new();
        let input_mode = InputMode::new();
        let input_broadcaster = InputBroadcaster::new();
        let activity = ActivityTracker::new();
        let focus = FocusTracker::new();
        let terminal_size = TerminalSize::new(rows, cols);

        // Spawn PTY reader (server mode -- no stdout, only broker)
        let broker_clone = broker.clone();
        let activity_clone = activity.clone();
        tokio::task::spawn_blocking(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                use std::io::Read;
                let mut reader = pty_reader;
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let data = Bytes::copy_from_slice(&buf[..n]);
                            broker_clone.publish(data);
                            activity_clone.touch();
                        }
                        Err(_) => break,
                    }
                }
            }));
            if let Err(e) = result {
                tracing::error!("PTY reader task panicked: {:?}", e);
            }
        });

        // Spawn PTY writer
        tokio::task::spawn_blocking(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                use std::io::Write;
                let mut writer = pty_writer;
                let mut rx = input_rx;
                while let Some(data) = rx.blocking_recv() {
                    if writer.write_all(&data).is_err() {
                        break;
                    }
                    let _ = writer.flush();
                }
            }));
            if let Err(e) = result {
                tracing::error!("PTY writer task panicked: {:?}", e);
            }
        });

        let session = Session {
            name,
            pid,
            command: command_display,
            client_count: Arc::new(AtomicUsize::new(0)),
            input_tx,
            output_rx: broker.sender(),
            shutdown,
            parser,
            overlays,
            panels,
            pty,
            terminal_size,
            input_mode,
            input_broadcaster,
            activity,
            focus,
            detach_signal: broadcast::channel::<()>(1).0,
            visual_update_tx: broadcast::channel::<VisualUpdate>(16).0,
            screen_mode: Arc::new(RwLock::new(ScreenMode::Normal)),
            cancelled: tokio_util::sync::CancellationToken::new(),
        };

        // Watch for alternate screen mode changes from the parser and
        // update session.screen_mode accordingly. This ensures overlays
        // and panels are automatically filtered by screen mode.
        {
            let screen_mode = session.screen_mode.clone();
            let visual_update_tx = session.visual_update_tx.clone();
            let parser = session.parser.clone();
            tokio::spawn(async move {
                use tokio_stream::StreamExt;
                let mut events = std::pin::pin!(parser.subscribe());
                while let Some(event) = events.next().await {
                    if let crate::parser::events::Event::Mode { alternate_active, .. } = event {
                        let new_mode = if alternate_active {
                            ScreenMode::Alt
                        } else {
                            ScreenMode::Normal
                        };
                        let changed = {
                            let mut mode = screen_mode.write();
                            if *mode != new_mode {
                                *mode = new_mode;
                                true
                            } else {
                                false
                            }
                        };
                        if changed {
                            let _ = visual_update_tx.send(VisualUpdate::OverlaysChanged);
                            let _ = visual_update_tx.send(VisualUpdate::PanelsChanged);
                        }
                    }
                }
            });
        }

        Ok((session, child_exit_rx))
    }
}

/// Server-level session lifecycle events.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    Created { name: String },
    Renamed { old_name: String, new_name: String },
    Destroyed { name: String },
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("session name already exists: {0}")]
    NameExists(String),
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("maximum number of sessions reached")]
    MaxSessionsReached,
}

struct RegistryInner {
    sessions: HashMap<String, Session>,
    next_id: u64,
    max_sessions: Option<usize>,
}

/// Manages multiple sessions by name.
#[derive(Clone)]
pub struct SessionRegistry {
    inner: Arc<RwLock<RegistryInner>>,
    events_tx: tokio_broadcast::Sender<SessionEvent>,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionRegistry {
    /// Create an empty registry with a broadcast channel for lifecycle events.
    pub fn new() -> Self {
        Self::with_max_sessions(None)
    }

    /// Create an empty registry with an optional maximum session count.
    pub fn with_max_sessions(max_sessions: Option<usize>) -> Self {
        let (events_tx, _) = tokio_broadcast::channel(64);
        Self {
            inner: Arc::new(RwLock::new(RegistryInner {
                sessions: HashMap::new(),
                next_id: 0,
                max_sessions,
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
        let mut inner = self.inner.write();

        if let Some(max) = inner.max_sessions {
            if inner.sessions.len() >= max {
                return Err(RegistryError::MaxSessionsReached);
            }
        }

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

    /// Insert a session and return both the assigned name and a clone of the
    /// session, atomically under the write lock.
    ///
    /// This avoids a TOCTOU race where a separate `get()` after `insert()`
    /// could fail if a background task (e.g. `monitor_child_exit`) removes the
    /// session between the two calls.
    pub fn insert_and_get(
        &self,
        name: Option<String>,
        mut session: Session,
    ) -> Result<(String, Session), RegistryError> {
        let mut inner = self.inner.write();

        if let Some(max) = inner.max_sessions {
            if inner.sessions.len() >= max {
                return Err(RegistryError::MaxSessionsReached);
            }
        }

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
        let cloned = session.clone();
        inner.sessions.insert(assigned_name.clone(), session);

        let _ = self.events_tx.send(SessionEvent::Created {
            name: assigned_name.clone(),
        });

        Ok((assigned_name, cloned))
    }

    /// Look up a session by name, returning a clone if found.
    pub fn get(&self, name: &str) -> Option<Session> {
        let inner = self.inner.read();
        inner.sessions.get(name).cloned()
    }

    /// Remove a session by name, returning the removed session if found.
    ///
    /// Emits a `SessionEvent::Destroyed` event when a session is removed.
    pub fn remove(&self, name: &str) -> Option<Session> {
        let mut inner = self.inner.write();
        let removed = inner.sessions.remove(name);
        if let Some(ref session) = removed {
            session.cancelled.cancel();
            let _ = self.events_tx.send(SessionEvent::Destroyed {
                name: name.to_string(),
            });
        }
        removed
    }

    /// Rename a session, returning a clone of the renamed session.
    ///
    /// Returns `RegistryError::NotFound` if `old_name` does not exist, or
    /// `RegistryError::NameExists` if `new_name` is already taken.
    /// Updates the session's `name` field to `new_name`.
    ///
    /// The clone is returned atomically under the write lock, avoiding a
    /// TOCTOU race with background tasks that may remove the session.
    pub fn rename(&self, old_name: &str, new_name: &str) -> Result<Session, RegistryError> {
        let mut inner = self.inner.write();

        if !inner.sessions.contains_key(old_name) {
            return Err(RegistryError::NotFound(old_name.to_string()));
        }
        if inner.sessions.contains_key(new_name) {
            return Err(RegistryError::NameExists(new_name.to_string()));
        }

        let mut session = inner.sessions.remove(old_name).unwrap();
        session.name = new_name.to_string();
        let cloned = session.clone();
        inner.sessions.insert(new_name.to_string(), session);

        let _ = self.events_tx.send(SessionEvent::Renamed {
            old_name: old_name.to_string(),
            new_name: new_name.to_string(),
        });

        Ok(cloned)
    }

    /// Return all session names.
    pub fn list(&self) -> Vec<String> {
        let inner = self.inner.read();
        inner.sessions.keys().cloned().collect()
    }

    /// Return the number of sessions.
    pub fn len(&self) -> usize {
        let inner = self.inner.read();
        inner.sessions.len()
    }

    /// Return true if the registry contains no sessions.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Check if a given name is available (not already in use).
    ///
    /// Returns `Ok(())` if the name is `None` (auto-assign) or the name is free.
    /// Returns `Err(RegistryError::NameExists)` if the name is taken.
    pub fn name_available(&self, name: &Option<String>) -> Result<(), RegistryError> {
        if let Some(n) = name {
            let inner = self.inner.read();
            if inner.sessions.contains_key(n) {
                return Err(RegistryError::NameExists(n.clone()));
            }
        }
        Ok(())
    }

    /// Remove all sessions, detaching streaming clients first.
    ///
    /// Called during server shutdown to ensure child processes are cleaned up
    /// promptly (dropping the Session closes PTY handles, which sends SIGHUP
    /// to the child). Returns a `JoinHandle` for the SIGKILL escalation task
    /// if any sessions were drained, so the caller can await it.
    pub fn drain(&self) -> Option<tokio::task::JoinHandle<()>> {
        let names = self.list();
        let mut sessions = Vec::new();
        for name in names {
            if let Some(session) = self.remove(&name) {
                session.detach();
                sessions.push(session);
            }
        }
        if sessions.is_empty() {
            return None;
        }
        // Give children 3 seconds to exit from SIGHUP, then escalate to SIGKILL
        Some(tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            for session in &sessions {
                session.kill_child();
            }
        }))
    }

    /// Subscribe to session lifecycle events.
    pub fn subscribe_events(&self) -> tokio_broadcast::Receiver<SessionEvent> {
        self.events_tx.subscribe()
    }

    /// Monitor a session's child process exit and remove it from the registry.
    ///
    /// Spawns a background task that waits on `child_exit_rx`. When the child
    /// exits, all streaming clients are detached (so their I/O loops exit
    /// promptly), then the session is removed from the registry (emitting a
    /// `SessionEvent::Destroyed` event). This should be called for
    /// API-created sessions where the caller would otherwise discard the
    /// exit receiver.
    pub fn monitor_child_exit(
        &self,
        name: String,
        child_exit_rx: tokio::sync::oneshot::Receiver<()>,
    ) {
        let registry = self.clone();
        tokio::spawn(async move {
            let _ = child_exit_rx.await;
            tracing::info!(session = %name, "session child process exited");
            // Signal all attached streaming clients to detach before removing
            // the session. Without this, socket streaming loops would block
            // forever on output_rx.recv() because the Session holds a
            // broadcast::Sender clone that keeps the channel open.
            if let Some(session) = registry.get(&name) {
                session.detach();
            }
            registry.remove(&name);
        });
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
            pid: None,
            command: "test".to_string(),
            client_count: Arc::new(AtomicUsize::new(0)),
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
            focus: FocusTracker::new(),
            detach_signal: broadcast::channel::<()>(1).0,
            visual_update_tx: broadcast::channel::<VisualUpdate>(16).0,
            screen_mode: Arc::new(RwLock::new(ScreenMode::Normal)),
            cancelled: tokio_util::sync::CancellationToken::new(),
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

        let session = registry.rename("old", "new").unwrap();

        assert_eq!(session.name, "new");
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

        let result = registry.rename("a", "b");
        assert!(result.is_err(), "rename to existing name should fail");
        let err = result.err().unwrap();
        assert!(
            matches!(err, RegistryError::NameExists(ref n) if n == "b"),
            "expected NameExists(\"b\"), got: {err:?}"
        );
    }

    #[tokio::test]
    async fn registry_rename_nonexistent_fails() {
        let registry = SessionRegistry::new();
        let result = registry.rename("nope", "whatever");
        assert!(result.is_err(), "rename of nonexistent session should fail");
        let err = result.err().unwrap();
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

    #[tokio::test]
    async fn registry_emits_renamed_event() {
        let registry = SessionRegistry::new();
        let mut rx = registry.subscribe_events();

        registry
            .insert(Some("old".to_string()), make_test_session("x"))
            .unwrap();
        // Drain the Created event
        let _ = rx.recv().await.unwrap();

        registry.rename("old", "new").unwrap();

        let ev = rx.recv().await.expect("should receive Renamed event");
        assert!(
            matches!(ev, SessionEvent::Renamed { ref old_name, ref new_name }
                if old_name == "old" && new_name == "new"),
            "expected Renamed {{ old_name: \"old\", new_name: \"new\" }}, got: {ev:?}"
        );
    }

    #[tokio::test]
    async fn session_spawn_creates_session_with_child_exit() {
        let (session, child_exit_rx) = Session::spawn(
            "spawned".to_string(),
            crate::pty::SpawnCommand::default(),
            24,
            80,
        )
        .expect("Session::spawn should succeed");

        assert_eq!(session.name, "spawned");

        // Send input to make the shell exit
        session
            .input_tx
            .send(bytes::Bytes::from_static(b"exit\n"))
            .await
            .expect("should send input");

        // The child exit receiver should fire
        tokio::time::timeout(std::time::Duration::from_secs(5), child_exit_rx)
            .await
            .expect("child_exit_rx should fire within timeout")
            .expect("oneshot should not be dropped");
    }

    #[tokio::test]
    async fn session_spawn_with_options_applies_env() {
        let mut env = std::collections::HashMap::new();
        env.insert("WSH_TEST_VAR".to_string(), "hello_wsh".to_string());

        let (session, _child_exit_rx) = Session::spawn_with_options(
            "env-test".to_string(),
            crate::pty::SpawnCommand::default(),
            24,
            80,
            None,
            Some(env),
        )
        .expect("Session::spawn_with_options should succeed");

        assert_eq!(session.name, "env-test");

        // Subscribe BEFORE sending input so we don't miss the output
        let mut output_rx = session.output_rx.subscribe();

        // Give the shell time to start, then send the echo command
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        session
            .input_tx
            .send(bytes::Bytes::from_static(b"echo $WSH_TEST_VAR\n"))
            .await
            .expect("should send input");

        let mut collected = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while let Ok(Ok(data)) = tokio::time::timeout_at(deadline, output_rx.recv()).await {
            collected.extend_from_slice(&data);
            if String::from_utf8_lossy(&collected).contains("hello_wsh") {
                break;
            }
        }
        let output = String::from_utf8_lossy(&collected);
        assert!(
            output.contains("hello_wsh"),
            "expected output to contain 'hello_wsh', got: {output}"
        );
    }

    #[tokio::test]
    async fn test_detach_signal_notifies_subscribers() {
        let (session, _rx) = create_test_session("detach-test");
        let mut detach_rx = session.detach_signal.subscribe();

        session.detach();

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            detach_rx.recv(),
        )
        .await;
        assert!(result.is_ok(), "detach signal should be received");
        assert!(result.unwrap().is_ok(), "detach signal should not be an error");
    }
}
