use bytes::Bytes;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

/// Validate a session name. Names must be 1-64 chars, alphanumeric/hyphens/underscores/dots.
pub fn validate_session_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("session name must not be empty".into());
    }
    if name.len() > 64 {
        return Err(format!("session name too long ({} chars, max 64)", name.len()));
    }
    if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.') {
        return Err(format!("session name contains invalid characters: {}",
            &name[..name.len().min(64)]));
    }
    Ok(())
}

/// Validate a tag string. Tags must be 1-64 chars, alphanumeric/hyphens/underscores/dots.
pub fn validate_tag(tag: &str) -> Result<(), String> {
    if tag.is_empty() {
        return Err("tag must not be empty".to_string());
    }
    if tag.len() > 64 {
        return Err(format!("tag too long ({} chars, max 64)", tag.len()));
    }
    if !tag.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.') {
        return Err(format!("tag contains invalid characters: {tag}"));
    }
    Ok(())
}

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
    /// User-defined tags for organizing and filtering sessions.
    pub tags: Arc<RwLock<HashSet<String>>>,
    pub input_tx: mpsc::Sender<Bytes>,
    pub output_rx: broadcast::Sender<Bytes>,
    pub shutdown: ShutdownCoordinator,
    pub parser: Parser,
    pub overlays: OverlayStore,
    pub panels: PanelStore,
    pub pty: Arc<parking_lot::Mutex<Pty>>,
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
    /// Set to `true` by `monitor_child_exit` when the child process exits.
    /// Checked by `send_sighup()` and `kill_child()` to avoid signaling a
    /// potentially-recycled PID.
    pub child_exited: Arc<AtomicBool>,
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

/// Maximum number of concurrent streaming clients per session.
///
/// Prevents resource exhaustion from too many simultaneous WebSocket or
/// socket connections to a single session.
const MAX_CLIENTS_PER_SESSION: usize = 64;

/// RAII guard that decrements the session client count on drop.
pub struct ClientGuard {
    counter: Arc<AtomicUsize>,
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Release);
    }
}

impl Session {
    /// Register a new streaming client, returning an RAII guard that decrements
    /// the count when dropped.
    ///
    /// Returns `None` if the session already has [`MAX_CLIENTS_PER_SESSION`]
    /// connected clients. Uses a compare-exchange loop for race-free admission.
    pub fn connect(&self) -> Option<ClientGuard> {
        loop {
            let current = self.client_count.load(Ordering::Acquire);
            if current >= MAX_CLIENTS_PER_SESSION {
                return None;
            }
            if self
                .client_count
                .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(ClientGuard {
                    counter: Arc::clone(&self.client_count),
                });
            }
        }
    }

    /// Return the number of currently connected streaming clients.
    pub fn clients(&self) -> usize {
        self.client_count.load(Ordering::Acquire)
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

    /// Forcefully kill this session: cancel all watchers, detach all
    /// streaming clients, and send SIGKILL to the child process.
    ///
    /// Used when a session is explicitly killed via API/socket/MCP.
    /// Unlike relying on `Arc<Pty>` drop (SIGHUP), this ensures the
    /// child is terminated immediately regardless of outstanding references.
    pub fn force_kill(&self) {
        self.cancelled.cancel();
        self.detach();
        self.kill_child();
    }

    /// Send SIGHUP to the child's process group.
    ///
    /// Used during drain to explicitly request graceful termination,
    /// rather than relying on PTY fd closure (which depends on all
    /// Session Arc clones being dropped).
    ///
    /// Signals the entire process group (negative PID) so that child
    /// processes spawned by the shell also receive the signal.
    /// portable_pty calls setsid() when spawning, so the child is the
    /// leader of its own process group.
    pub fn send_sighup(&self) {
        if let Some(pid) = self.pid {
            if pid == 0 || pid > i32::MAX as u32 {
                tracing::warn!(pid, "PID is 0 or exceeds i32::MAX, cannot send signal");
                return;
            }
            if self.child_exited.load(Ordering::Acquire) {
                tracing::debug!(pid, "child already exited, skipping SIGHUP");
                return;
            }
            #[cfg(unix)]
            unsafe {
                libc::kill(-(pid as i32), libc::SIGHUP);
            }
        }
    }

    /// Send SIGKILL to the child's process group.
    ///
    /// Used as an escalation path when the child ignores SIGHUP during
    /// shutdown/drain. Signals the entire process group so sub-processes
    /// are also killed.
    ///
    /// Checks `child_exited` before signaling to avoid hitting a
    /// potentially-recycled PID. The flag is set by `monitor_child_exit`
    /// when the child process exits.
    pub fn kill_child(&self) {
        if let Some(pid) = self.pid {
            if pid == 0 || pid > i32::MAX as u32 {
                tracing::warn!(pid, "PID is 0 or exceeds i32::MAX, cannot send signal");
                return;
            }
            if self.child_exited.load(Ordering::Acquire) {
                tracing::debug!(pid, "child already exited, skipping SIGKILL");
                return;
            }
            #[cfg(unix)]
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
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
        let pty = Arc::new(parking_lot::Mutex::new(pty));

        // Monitor child exit via a oneshot channel.
        //
        // NOTE: The JoinHandles from the three spawn_blocking tasks below
        // (child exit monitor, PTY reader, PTY writer) are intentionally not
        // stored. Session derives Clone, and JoinHandle is not Clone, so
        // tracking them would require Arc<Mutex<Option<JoinHandle>>> per task.
        // This complexity is unnecessary because:
        //   1. All three tasks self-terminate when the PTY fd closes or the
        //      child exits (triggered by Session drop / drain's SIGKILL).
        //   2. The tokio runtime does not abort blocking tasks on shutdown —
        //      they run to completion on the blocking thread pool.
        //   3. drain() already ensures children are killed within 3 seconds.
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

        // ── Design decision: bounded parser channel with PTY backpressure ──
        //
        // The parser channel uses a bounded mpsc with `blocking_send()` in the
        // PTY reader thread. This is the final, opinionated design after three
        // iterations. DO NOT CHANGE without reading the full rationale below.
        //
        // ## History
        //
        // - **v1** (0fa88ad): Parser shared the broadcast channel (capacity 64).
        //   Slow parser → RecvError::Lagged → permanent VT state corruption.
        //
        // - **v2** (2e2a1a0): Dedicated bounded(4096) mpsc + try_send(). Drops
        //   data silently when full — same corruption bug, just harder to hit.
        //
        // - **v3**: Dedicated unbounded mpsc. No data loss, but `cat /dev/zero`
        //   grows the channel without bound → OOM.
        //
        // - **v4** (current): Dedicated bounded mpsc + blocking_send(). The PTY
        //   reader thread blocks when the parser can't keep up. This propagates
        //   backpressure through the kernel PTY buffer to the child process —
        //   exactly how real terminal emulators work. No data loss, no OOM.
        //
        // ## Why backpressure is correct
        //
        // The parser is the source of truth for terminal state. Every byte the
        // PTY emits MUST reach it. The only two options that preserve this
        // invariant are unbounded (v3, OOM risk) and backpressure (v4, no risk).
        //
        // Backpressure is also the natural model: a real terminal emulator
        // processes bytes at a finite rate, and the kernel PTY buffer provides
        // natural flow control. We're just making the parser part of that flow.
        //
        // ## Why this doesn't freeze the terminal or streaming clients
        //
        // The PTY reader does broadcast FIRST (non-blocking, lossy), THEN the
        // blocking parser send. So:
        //   - Local terminal passthrough: unaffected (handled before this code)
        //   - Streaming WebSocket/socket clients: unaffected (broadcast is lossy)
        //   - The only thing that slows down is the child process's write rate
        //
        // ## Capacity
        //
        // 256 slots × ~4KB typical chunk ≈ 1MB max buffered. This absorbs
        // brief parser stalls (e.g. query processing) without backpressure,
        // while capping memory for sustained floods.
        //
        // ## Do not change this to try_send or unbounded
        //
        // - try_send: silently drops data → permanent VT state corruption
        // - unbounded: `cat /dev/zero` → OOM
        // Both have been tried and reverted. This is the correct design.
        // ────────────────────────────────────────────────────────────────────
        const PARSER_CHANNEL_CAPACITY: usize = 256;
        let (parser_tx, parser_rx) = mpsc::channel::<Bytes>(PARSER_CHANNEL_CAPACITY);
        let parser = Parser::spawn(parser_rx, cols as usize, rows as usize, 10_000);

        let (input_tx, input_rx) = mpsc::channel::<Bytes>(64);
        let shutdown = ShutdownCoordinator::new();
        let overlays = OverlayStore::new();
        let panels = PanelStore::new();
        let input_mode = InputMode::new();
        let input_broadcaster = InputBroadcaster::new();
        let activity = ActivityTracker::new();
        let focus = FocusTracker::new();
        let terminal_size = TerminalSize::new(rows, cols);

        // Spawn PTY reader (server mode -- no stdout, only broker + parser)
        //
        // Order matters: broadcast first (non-blocking, lossy for streaming
        // clients), then blocking_send to parser (applies backpressure).
        // This ensures streaming clients and the local terminal are never
        // blocked by parser throughput.
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
                            // 1. Broadcast to streaming clients (non-blocking, lossy)
                            broker_clone.publish(data.clone());
                            // 2. Send to parser (blocks if channel full → PTY backpressure)
                            if parser_tx.blocking_send(data).is_err() {
                                // Parser channel closed — session is shutting down
                                break;
                            }
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
        //
        // ── REVIEWED: blocking thread pool saturation during shutdown ───
        //
        // This task blocks on `input_rx.blocking_recv()`, which occupies a
        // tokio blocking thread until the channel closes or write_all fails.
        // During drain(), Session clones held by other tasks keep input_tx
        // alive, so the channel doesn't close immediately. This has been
        // flagged as a potential blocking-thread-pool saturation risk during
        // shutdown with many sessions. It is a non-issue because:
        //
        // 1. drain() now sends explicit SIGHUP to the child (see
        //    send_sighup()). Most shells exit immediately on SIGHUP,
        //    closing the PTY slave fd. The next write_all to the PTY master
        //    then fails with EIO/EAGAIN, breaking this loop promptly.
        //
        // 2. Even if the child ignores SIGHUP, SIGKILL is sent after 3s,
        //    which unconditionally closes the PTY. The writer is therefore
        //    blocked for at most 3 seconds in the worst case.
        //
        // 3. Tokio's blocking thread pool defaults to 512 threads. Even
        //    draining 50+ sessions simultaneously leaves ample headroom.
        //    Each blocked writer consumes ~zero CPU (kernel wait state).
        //
        // 4. Adding a cancellation mechanism (e.g., select! with a token)
        //    is not possible here because this is a blocking (non-async)
        //    context. Replacing blocking_recv with a poll loop would add
        //    latency to normal input handling for negligible shutdown
        //    benefit. The current design is the right tradeoff.
        // ────────────────────────────────────────────────────────────────
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
            tags: Arc::new(RwLock::new(HashSet::new())),
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
            child_exited: Arc::new(AtomicBool::new(false)),
        };

        // Watch for alternate screen mode changes from the parser and
        // update session.screen_mode accordingly. This ensures overlays
        // and panels are automatically filtered by screen mode.
        //
        // The cancelled token ensures this task exits promptly when the
        // session is killed, rather than waiting for all Parser clones
        // to be dropped (which keeps the broadcast channel open).
        {
            let screen_mode = session.screen_mode.clone();
            let visual_update_tx = session.visual_update_tx.clone();
            let parser = session.parser.clone();
            let cancelled = session.cancelled.clone();
            tokio::spawn(async move {
                use tokio_stream::StreamExt;
                let mut events = std::pin::pin!(parser.subscribe());
                loop {
                    tokio::select! {
                        sub_event = events.next() => {
                            match sub_event {
                                Some(crate::parser::SubscriptionEvent::Event(
                                    crate::parser::events::Event::Mode { alternate_active, .. }
                                )) => {
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
                                Some(_) => {} // other events, ignore
                                None => break, // channel closed
                            }
                        }
                        _ = cancelled.cancelled() => break,
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
    TagsChanged { name: String, added: Vec<String>, removed: Vec<String> },
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("session name already exists: {0}")]
    NameExists(String),
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("maximum number of sessions reached")]
    MaxSessionsReached,
    #[error("invalid tag: {0}")]
    InvalidTag(String),
    #[error("invalid session name: {0}")]
    InvalidName(String),
}

struct RegistryInner {
    sessions: HashMap<String, Session>,
    next_id: u64,
    max_sessions: Option<usize>,
    tags_index: HashMap<String, HashSet<String>>,
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
    /// Default maximum number of sessions when no explicit limit is set.
    ///
    /// Each session costs ~2 fds (PTY pair) + 3 blocking threads + memory
    /// for parser state and scrollback. 256 provides ample headroom for
    /// typical use while preventing a runaway agent from exhausting the
    /// blocking thread pool (default 512 = ~170 sessions at 3 threads each).
    const DEFAULT_MAX_SESSIONS: usize = 256;

    /// Create an empty registry with a broadcast channel for lifecycle events.
    pub fn new() -> Self {
        Self::with_max_sessions(Some(Self::DEFAULT_MAX_SESSIONS))
    }

    /// Create an empty registry with an optional maximum session count.
    pub fn with_max_sessions(max_sessions: Option<usize>) -> Self {
        let (events_tx, _) = tokio_broadcast::channel(64);
        Self {
            inner: Arc::new(RwLock::new(RegistryInner {
                sessions: HashMap::new(),
                next_id: 0,
                max_sessions,
                tags_index: HashMap::new(),
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
                validate_session_name(&n).map_err(RegistryError::InvalidName)?;
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
        // Index initial tags
        {
            let session_tags = session.tags.read();
            for tag in session_tags.iter() {
                inner.tags_index.entry(tag.clone()).or_default().insert(assigned_name.clone());
            }
        }
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
                validate_session_name(&n).map_err(RegistryError::InvalidName)?;
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
        // Index initial tags
        {
            let session_tags = session.tags.read();
            for tag in session_tags.iter() {
                inner.tags_index.entry(tag.clone()).or_default().insert(assigned_name.clone());
            }
        }
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
    /// Also cleans up the tags_index for any tags the session had.
    pub fn remove(&self, name: &str) -> Option<Session> {
        let mut inner = self.inner.write();
        let removed = inner.sessions.remove(name);
        if let Some(ref session) = removed {
            // Clean up tags_index while still holding the write lock
            let session_tags = session.tags.read();
            for tag in session_tags.iter() {
                if let Some(set) = inner.tags_index.get_mut(tag) {
                    set.remove(name);
                    if set.is_empty() {
                        inner.tags_index.remove(tag);
                    }
                }
            }
            drop(session_tags);
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
        validate_session_name(new_name).map_err(RegistryError::InvalidName)?;
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

        // Update tags_index: replace old_name with new_name in each tag entry
        let session_tags = session.tags.read();
        for tag in session_tags.iter() {
            if let Some(set) = inner.tags_index.get_mut(tag) {
                set.remove(old_name);
                set.insert(new_name.to_string());
            }
        }
        drop(session_tags);

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

    /// **Advisory** check for whether a session name is available.
    ///
    /// Returns `Ok(())` if `name` is `None` (auto-assign) or the name is free.
    /// Returns `Err(RegistryError::NameExists)` if the name is taken.
    ///
    /// # Important: this is a fast-fail optimization, NOT a correctness guard
    ///
    /// Session creation follows a two-phase pattern:
    ///
    ///   1. `name_available()` — advisory pre-check (this method)
    ///   2. `spawn_blocking(Session::spawn_with_options(...))` — expensive fork/exec
    ///   3. `insert_and_get()` / `insert()` — **authoritative** atomic insert
    ///
    /// There is a TOCTOU window between step 1 and step 3: a concurrent request
    /// can claim the same name after the pre-check passes. This is by design.
    /// The pre-check exists solely to avoid the expensive fork/exec in step 2
    /// when the name is *obviously* already taken. The authoritative uniqueness
    /// check is `insert_and_get()` / `insert()`, which operates under a write
    /// lock. If the name was claimed between steps 1 and 3, the insert fails
    /// and the caller shuts down the just-spawned session (`session.shutdown()`).
    ///
    /// The consequence of losing the TOCTOU race is one wasted fork/exec that
    /// is immediately cleaned up — not a resource leak, not a correctness bug.
    /// This only happens under concurrent creates with the *same* name, which
    /// is a degenerate case.
    ///
    /// **Do not attempt to "fix" this TOCTOU by holding a lock across the
    /// spawn.** That would block all session operations for the duration of
    /// fork/exec (potentially hundreds of ms under memory pressure), which is
    /// far worse than the occasional wasted spawn.
    pub fn name_available(&self, name: &Option<String>) -> Result<(), RegistryError> {
        if let Some(n) = name {
            validate_session_name(n).map_err(RegistryError::InvalidName)?;
            let inner = self.inner.read();
            if inner.sessions.contains_key(n) {
                return Err(RegistryError::NameExists(n.clone()));
            }
        }
        Ok(())
    }

    /// Remove all sessions atomically, detaching streaming clients first.
    ///
    /// Called during server shutdown to ensure child processes are cleaned up
    /// promptly. Sends explicit SIGHUP to each child (rather than relying on
    /// PTY fd closure, which requires all Session Arc clones to be dropped).
    /// Returns a `JoinHandle` for the SIGKILL escalation task if any sessions
    /// were drained, so the caller can await it.
    ///
    /// Uses a single write lock for the entire operation to prevent in-flight
    /// session create requests from inserting new sessions between the
    /// snapshot and the cleanup. Without atomicity, sessions created by
    /// requests that were already past the HTTP accept layer (but not yet
    /// completed) could escape drain and leave orphaned child processes.
    pub fn drain(&self) -> Option<tokio::task::JoinHandle<()>> {
        let sessions: Vec<Session> = {
            let mut inner = self.inner.write();
            let drained: Vec<(String, Session)> = inner.sessions.drain().collect();
            inner.tags_index.clear();
            for (name, ref session) in &drained {
                session.cancelled.cancel();
                session.detach();
                session.send_sighup();
                let _ = self.events_tx.send(SessionEvent::Destroyed {
                    name: name.clone(),
                });
            }
            drained.into_iter().map(|(_, s)| s).collect()
        };
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

    /// Add tags to a session. Validates each tag, updates Session and reverse index atomically.
    pub fn add_tags(&self, name: &str, tags: &[String]) -> Result<(), RegistryError> {
        for tag in tags {
            validate_tag(tag).map_err(RegistryError::InvalidTag)?;
        }
        let mut inner = self.inner.write();
        // Clone the Arc<RwLock<HashSet>> to release the immutable borrow on inner.sessions,
        // allowing us to mutably borrow inner.tags_index in the same scope.
        let session_tags_arc = inner.sessions.get(name)
            .ok_or_else(|| RegistryError::NotFound(name.to_string()))?
            .tags.clone();
        let mut session_tags = session_tags_arc.write();
        let mut added = Vec::new();
        for tag in tags {
            if session_tags.insert(tag.clone()) {
                inner.tags_index.entry(tag.clone()).or_default().insert(name.to_string());
                added.push(tag.clone());
            }
        }
        drop(session_tags);
        drop(inner);
        if !added.is_empty() {
            let _ = self.events_tx.send(SessionEvent::TagsChanged {
                name: name.to_string(), added, removed: vec![],
            });
        }
        Ok(())
    }

    /// Remove tags from a session. Updates Session and reverse index atomically.
    pub fn remove_tags(&self, name: &str, tags: &[String]) -> Result<(), RegistryError> {
        let mut inner = self.inner.write();
        // Clone the Arc<RwLock<HashSet>> to release the immutable borrow on inner.sessions,
        // allowing us to mutably borrow inner.tags_index in the same scope.
        let session_tags_arc = inner.sessions.get(name)
            .ok_or_else(|| RegistryError::NotFound(name.to_string()))?
            .tags.clone();
        let mut session_tags = session_tags_arc.write();
        let mut removed = Vec::new();
        for tag in tags {
            if session_tags.remove(tag) {
                if let Some(set) = inner.tags_index.get_mut(tag) {
                    set.remove(name);
                    if set.is_empty() {
                        inner.tags_index.remove(tag);
                    }
                }
                removed.push(tag.clone());
            }
        }
        drop(session_tags);
        drop(inner);
        if !removed.is_empty() {
            let _ = self.events_tx.send(SessionEvent::TagsChanged {
                name: name.to_string(), added: vec![], removed,
            });
        }
        Ok(())
    }

    /// Return session names matching ANY of the given tags (union/OR semantics).
    pub fn sessions_by_tags(&self, tags: &[String]) -> Vec<String> {
        let inner = self.inner.read();
        let mut result = HashSet::new();
        for tag in tags {
            if let Some(names) = inner.tags_index.get(tag) {
                result.extend(names.iter().cloned());
            }
        }
        result.into_iter().collect()
    }

    /// Monitor a session's child process exit and remove it from the registry.
    ///
    /// Spawns a background task that waits on `child_exit_rx`. When the child
    /// exits, all streaming clients are detached (so their I/O loops exit
    /// promptly), then the session is removed from the registry (emitting a
    /// `SessionEvent::Destroyed` event). This should be called for
    /// API-created sessions where the caller would otherwise discard the
    /// exit receiver.
    ///
    /// # Identity parameter
    ///
    /// The `identity` parameter is the session's `client_count` Arc, used as
    /// a stable identity marker via `Arc::ptr_eq`. This allows the monitor to
    /// find the session's current name even after a rename.
    ///
    /// ## Design decision: why the caller passes identity explicitly
    ///
    /// This has evolved through several iterations:
    ///
    /// - **v1** (d667d66): Captured `name` by value. Worked only if the
    ///   session was never renamed between spawn and child exit.
    ///
    /// - **v2** (afdbc6e): Added `Arc::ptr_eq` identity lookup, but the
    ///   identity was captured internally via `registry.get(&name)`. This
    ///   introduced a `None` fallback path: if the lookup failed (session
    ///   not yet inserted, or removed between insert and this call), the
    ///   code silently fell back to the captured name — reintroducing the
    ///   exact stale-name bug the identity mechanism was designed to fix.
    ///   All current call sites happened to call this right after insert,
    ///   so the `None` path never executed, but it was a latent bug.
    ///
    /// - **v3** (current): The caller passes identity explicitly. There is
    ///   no `None` fallback. The caller always has the Session (they just
    ///   created it), so `session.client_count.clone()` is always available.
    ///
    /// Do not revert to internal identity lookup. The explicit parameter
    /// makes the contract enforceable by the type system.
    pub fn monitor_child_exit(
        &self,
        name: String,
        identity: Arc<AtomicUsize>,
        child_exited: Arc<AtomicBool>,
        child_exit_rx: tokio::sync::oneshot::Receiver<()>,
    ) {
        let registry = self.clone();
        tokio::spawn(async move {
            let _ = child_exit_rx.await;
            // Mark child as exited BEFORE removing from registry, so that
            // any concurrent drain/kill_child sees the flag and skips
            // signaling a potentially-recycled PID.
            child_exited.store(true, Ordering::Release);
            // ── Design decision: atomic detach + remove ──────────────
            //
            // The identity lookup, detach, and remove MUST happen under
            // a single write lock to prevent races with concurrent
            // rename() calls. Without atomicity, a rename between
            // find_name_by_identity() and remove() would orphan the
            // session in the registry (the old name is gone, the new
            // name is never removed). This was a latent bug in the
            // three-separate-lock approach.
            //
            // See also: the v1→v2→v3 evolution notes above for the
            // identity parameter rationale.
            // ─────────────────────────────────────────────────────────
            registry.detach_and_remove_by_identity(&identity, &name);
        });
    }

    /// Atomically find, detach, and remove a session by identity.
    ///
    /// Performs identity lookup (Arc::ptr_eq), detach, and remove under a
    /// single write lock to prevent races with concurrent rename() calls.
    /// The `fallback_name` is used only for logging if the session was
    /// already removed (e.g. by drain or kill) before the child exited.
    fn detach_and_remove_by_identity(
        &self,
        identity: &Arc<AtomicUsize>,
        fallback_name: &str,
    ) {
        let mut inner = self.inner.write();

        // Find the session's current name by stable identity.
        let current_name = inner
            .sessions
            .iter()
            .find(|(_, s)| Arc::ptr_eq(identity, &s.client_count))
            .map(|(n, _)| n.clone());

        match current_name {
            Some(name) => {
                tracing::info!(session = %name, "session child process exited");
                if let Some(session) = inner.sessions.remove(&name) {
                    // Clean up tags_index
                    let session_tags = session.tags.read();
                    for tag in session_tags.iter() {
                        if let Some(set) = inner.tags_index.get_mut(tag) {
                            set.remove(&name);
                            if set.is_empty() {
                                inner.tags_index.remove(tag);
                            }
                        }
                    }
                    drop(session_tags);
                    session.cancelled.cancel();
                    session.detach();
                    let _ = self.events_tx.send(SessionEvent::Destroyed { name });
                }
            }
            None => {
                // Session was already removed (e.g. by drain or kill).
                tracing::info!(session = %fallback_name, "session child process exited (already removed)");
            }
        }
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
        let (_parser_tx, parser_rx) = mpsc::channel(256);
        let parser = Parser::spawn(parser_rx, 80, 24, 1000);
        let pty = crate::pty::Pty::spawn(24, 80, crate::pty::SpawnCommand::default())
            .expect("failed to spawn PTY for test");

        let session = Session {
            name: name.to_string(),
            pid: None,
            command: "test".to_string(),
            client_count: Arc::new(AtomicUsize::new(0)),
            tags: Arc::new(RwLock::new(HashSet::new())),
            child_exited: Arc::new(AtomicBool::new(false)),
            input_tx,
            output_rx: broker.sender(),
            shutdown: ShutdownCoordinator::new(),
            parser,
            overlays: OverlayStore::new(),
            panels: PanelStore::new(),
            pty: Arc::new(parking_lot::Mutex::new(pty)),
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

    #[test]
    fn validate_tag_accepts_valid() {
        assert!(validate_tag("build").is_ok());
        assert!(validate_tag("my-tag_1.0").is_ok());
        assert!(validate_tag("a").is_ok());
        assert!(validate_tag(&"x".repeat(64)).is_ok());
    }

    #[test]
    fn validate_tag_rejects_invalid() {
        assert!(validate_tag("").is_err());
        assert!(validate_tag(" spaces ").is_err());
        assert!(validate_tag("has space").is_err());
        assert!(validate_tag(&"x".repeat(65)).is_err());
        assert!(validate_tag("special!char").is_err());
    }

    // ---- Tag operations tests ----

    #[tokio::test]
    async fn registry_add_tags() {
        let registry = SessionRegistry::new();
        registry.insert(Some("s1".into()), make_test_session("x")).unwrap();
        registry.add_tags("s1", &["build".into(), "test".into()]).unwrap();
        let s = registry.get("s1").unwrap();
        let mut tags: Vec<String> = s.tags.read().iter().cloned().collect();
        tags.sort();
        assert_eq!(tags, vec!["build", "test"]);
    }

    #[tokio::test]
    async fn registry_sessions_by_tags_union() {
        let registry = SessionRegistry::new();
        registry.insert(Some("s1".into()), make_test_session("x")).unwrap();
        registry.insert(Some("s2".into()), make_test_session("x")).unwrap();
        registry.insert(Some("s3".into()), make_test_session("x")).unwrap();
        registry.add_tags("s1", &["build".into()]).unwrap();
        registry.add_tags("s2", &["test".into()]).unwrap();
        registry.add_tags("s3", &["build".into(), "test".into()]).unwrap();

        let mut result = registry.sessions_by_tags(&["build".into()]);
        result.sort();
        assert_eq!(result, vec!["s1", "s3"]);

        let mut result = registry.sessions_by_tags(&["build".into(), "test".into()]);
        result.sort();
        assert_eq!(result, vec!["s1", "s2", "s3"]);
    }

    #[tokio::test]
    async fn registry_remove_cleans_index() {
        let registry = SessionRegistry::new();
        registry.insert(Some("s1".into()), make_test_session("x")).unwrap();
        registry.add_tags("s1", &["build".into()]).unwrap();
        registry.remove("s1");
        assert!(registry.sessions_by_tags(&["build".into()]).is_empty());
    }

    #[tokio::test]
    async fn registry_rename_updates_index() {
        let registry = SessionRegistry::new();
        registry.insert(Some("s1".into()), make_test_session("x")).unwrap();
        registry.add_tags("s1", &["build".into()]).unwrap();
        registry.rename("s1", "s2").unwrap();
        let result = registry.sessions_by_tags(&["build".into()]);
        assert!(result.contains(&"s2".to_string()));
        assert!(!result.contains(&"s1".to_string()));
    }

    #[tokio::test]
    async fn registry_remove_tags() {
        let registry = SessionRegistry::new();
        registry.insert(Some("s1".into()), make_test_session("x")).unwrap();
        registry.add_tags("s1", &["build".into(), "test".into()]).unwrap();
        registry.remove_tags("s1", &["build".into()]).unwrap();
        let s = registry.get("s1").unwrap();
        let tags: Vec<String> = s.tags.read().iter().cloned().collect();
        assert_eq!(tags, vec!["test"]);
        assert!(registry.sessions_by_tags(&["build".into()]).is_empty());
    }

    #[tokio::test]
    async fn registry_insert_with_initial_tags() {
        let registry = SessionRegistry::new();
        let s = make_test_session("x");
        *s.tags.write() = HashSet::from(["build".into(), "ci".into()]);
        registry.insert(Some("s1".into()), s).unwrap();
        let mut result = registry.sessions_by_tags(&["build".into()]);
        result.sort();
        assert_eq!(result, vec!["s1"]);
    }

    #[tokio::test]
    async fn registry_add_tags_invalid() {
        let registry = SessionRegistry::new();
        registry.insert(Some("s1".into()), make_test_session("x")).unwrap();
        assert!(registry.add_tags("s1", &["".into()]).is_err());
        assert!(registry.add_tags("s1", &["invalid tag!".into()]).is_err());
    }

    #[tokio::test]
    async fn registry_add_tags_not_found() {
        let registry = SessionRegistry::new();
        assert!(registry.add_tags("nonexistent", &["build".into()]).is_err());
    }

    #[tokio::test]
    async fn registry_add_tags_emits_event() {
        let registry = SessionRegistry::new();
        let mut rx = registry.subscribe_events();
        registry.insert(Some("s1".into()), make_test_session("x")).unwrap();
        // Drain the Created event
        let _ = rx.recv().await.unwrap();

        registry.add_tags("s1", &["build".into(), "test".into()]).unwrap();
        let ev = rx.recv().await.expect("should receive TagsChanged event");
        match ev {
            SessionEvent::TagsChanged { ref name, ref added, ref removed } => {
                assert_eq!(name, "s1");
                let mut added_sorted = added.clone();
                added_sorted.sort();
                assert_eq!(added_sorted, vec!["build", "test"]);
                assert!(removed.is_empty());
            }
            _ => panic!("expected TagsChanged, got: {ev:?}"),
        }
    }

    #[tokio::test]
    async fn registry_remove_tags_emits_event() {
        let registry = SessionRegistry::new();
        let mut rx = registry.subscribe_events();
        registry.insert(Some("s1".into()), make_test_session("x")).unwrap();
        registry.add_tags("s1", &["build".into(), "test".into()]).unwrap();
        // Drain Created + TagsChanged events
        let _ = rx.recv().await.unwrap();
        let _ = rx.recv().await.unwrap();

        registry.remove_tags("s1", &["build".into()]).unwrap();
        let ev = rx.recv().await.expect("should receive TagsChanged event");
        match ev {
            SessionEvent::TagsChanged { ref name, ref added, ref removed } => {
                assert_eq!(name, "s1");
                assert!(added.is_empty());
                assert_eq!(removed, &vec!["build".to_string()]);
            }
            _ => panic!("expected TagsChanged, got: {ev:?}"),
        }
    }

    #[tokio::test]
    async fn registry_add_tags_idempotent() {
        let registry = SessionRegistry::new();
        registry.insert(Some("s1".into()), make_test_session("x")).unwrap();
        registry.add_tags("s1", &["build".into()]).unwrap();

        let mut rx = registry.subscribe_events();
        // Adding the same tag again should not emit an event
        registry.add_tags("s1", &["build".into()]).unwrap();

        // Try to receive — should timeout since no event was emitted
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            rx.recv(),
        ).await;
        assert!(result.is_err(), "should not receive TagsChanged for duplicate add");
    }

    // ---- Session name validation tests ----

    #[test]
    fn validate_session_name_valid() {
        assert!(validate_session_name("my-session").is_ok());
        assert!(validate_session_name("test.1").is_ok());
        assert!(validate_session_name("a").is_ok());
        assert!(validate_session_name("under_score").is_ok());
        assert!(validate_session_name("123").is_ok());
    }

    #[test]
    fn validate_session_name_empty() {
        assert!(validate_session_name("").is_err());
    }

    #[test]
    fn validate_session_name_too_long() {
        let long = "a".repeat(65);
        assert!(validate_session_name(&long).is_err());
    }

    #[test]
    fn validate_session_name_max_length_ok() {
        let exact = "a".repeat(64);
        assert!(validate_session_name(&exact).is_ok());
    }

    #[test]
    fn validate_session_name_invalid_chars() {
        assert!(validate_session_name("has spaces").is_err());
        assert!(validate_session_name("../escape").is_err());
        assert!(validate_session_name("null\0byte").is_err());
        assert!(validate_session_name("semi;colon").is_err());
        assert!(validate_session_name("slash/path").is_err());
    }

    #[test]
    fn registry_insert_invalid_name_returns_error() {
        let registry = SessionRegistry::new();
        // We can't create a real Session without a PTY, but we can test name_available
        assert!(registry.name_available(&Some("../bad".to_string())).is_err());
        assert!(registry.name_available(&Some("has spaces".to_string())).is_err());
        assert!(registry.name_available(&Some("".to_string())).is_err());
        assert!(registry.name_available(&Some("valid-name".to_string())).is_ok());
        assert!(registry.name_available(&None).is_ok()); // auto-generated names bypass validation
    }
}
