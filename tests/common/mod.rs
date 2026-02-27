#![allow(dead_code)]

use bytes::Bytes;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use wsh::activity::ActivityTracker;
use wsh::broker::Broker;
use wsh::input::{FocusTracker, InputBroadcaster, InputMode};
use wsh::overlay::OverlayStore;
use wsh::panel::PanelStore;
use wsh::parser::Parser;
use wsh::session::{Session, SessionRegistry};
use wsh::shutdown::ShutdownCoordinator;
use wsh::terminal::TerminalSize;

/// Components returned from test session creation.
pub struct TestSession {
    pub session: Session,
    pub input_rx: mpsc::Receiver<Bytes>,
    pub broker: Broker,
    /// Keeps the parser channel open. Send data here to feed the parser
    /// directly (instead of through a PTY).
    pub parser_tx: mpsc::Sender<Bytes>,
}

/// Create a test session with default 24x80 dimensions.
pub fn create_test_session(name: &str) -> TestSession {
    create_test_session_with_size(name, 24, 80)
}

/// Create a test session with custom dimensions.
pub fn create_test_session_with_size(name: &str, rows: u16, cols: u16) -> TestSession {
    let (input_tx, input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let (parser_tx, parser_rx) = mpsc::channel(256);
    let parser = Parser::spawn(parser_rx, cols as usize, rows as usize, 1000);
    let session = Session {
        name: name.to_string(),
        pid: None,
        command: "test".to_string(),
        client_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        tags: Arc::new(parking_lot::RwLock::new(std::collections::HashSet::new())),
        child_exited: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        input_tx,
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: PanelStore::new(),
        pty: Arc::new(parking_lot::Mutex::new(
            wsh::pty::Pty::spawn(rows, cols, wsh::pty::SpawnCommand::default())
                .expect("failed to spawn PTY for test"),
        )),
        terminal_size: TerminalSize::new(rows, cols),
        activity: ActivityTracker::new(),
        focus: FocusTracker::new(),
        detach_signal: tokio::sync::broadcast::channel::<()>(1).0,
        visual_update_tx: tokio::sync::broadcast::channel::<wsh::protocol::VisualUpdate>(16).0,
        screen_mode: std::sync::Arc::new(parking_lot::RwLock::new(wsh::overlay::ScreenMode::Normal)),
        cancelled: tokio_util::sync::CancellationToken::new(),
    };
    TestSession {
        session,
        input_rx,
        broker,
        parser_tx,
    }
}

/// Create a test AppState with a single "test" session.
pub fn create_test_state() -> (wsh::api::AppState, mpsc::Receiver<Bytes>, broadcast::Sender<Bytes>, mpsc::Sender<Bytes>) {
    create_test_state_with_size(24, 80)
}

/// Create a test AppState with a single "test" session of custom dimensions.
pub fn create_test_state_with_size(rows: u16, cols: u16) -> (wsh::api::AppState, mpsc::Receiver<Bytes>, broadcast::Sender<Bytes>, mpsc::Sender<Bytes>) {
    let ts = create_test_session_with_size("test", rows, cols);
    let output_tx = ts.broker.sender();
    let parser_tx = ts.parser_tx;
    let registry = SessionRegistry::new();
    registry.insert(Some("test".into()), ts.session).unwrap();
    let state = wsh::api::AppState {
        sessions: registry,
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(wsh::api::ServerConfig::new(false)),
        server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        ticket_store: std::sync::Arc::new(wsh::api::ticket::TicketStore::new()),
        backends: wsh::federation::registry::BackendRegistry::new(),
        hostname: "test".to_string(),
        federation_config_path: None,
        local_token: None,
        default_backend_token: None,
    };
    (state, ts.input_rx, output_tx, parser_tx)
}
