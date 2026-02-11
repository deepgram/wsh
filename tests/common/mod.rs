#![allow(dead_code)]

use bytes::Bytes;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use wsh::activity::ActivityTracker;
use wsh::broker::Broker;
use wsh::input::{InputBroadcaster, InputMode};
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
}

/// Create a test session with default 24x80 dimensions.
pub fn create_test_session(name: &str) -> TestSession {
    create_test_session_with_size(name, 24, 80)
}

/// Create a test session with custom dimensions.
pub fn create_test_session_with_size(name: &str, rows: u16, cols: u16) -> TestSession {
    let (input_tx, input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, cols as usize, rows as usize, 1000);
    let session = Session {
        name: name.to_string(),
        input_tx,
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
        panels: PanelStore::new(),
        pty: Arc::new(
            wsh::pty::Pty::spawn(rows, cols, wsh::pty::SpawnCommand::default())
                .expect("failed to spawn PTY for test"),
        ),
        terminal_size: TerminalSize::new(rows, cols),
        activity: ActivityTracker::new(),
        is_local: false,
    };
    TestSession {
        session,
        input_rx,
        broker,
    }
}

/// Create a test AppState with a single "test" session.
pub fn create_test_state() -> (wsh::api::AppState, mpsc::Receiver<Bytes>, broadcast::Sender<Bytes>) {
    create_test_state_with_size(24, 80)
}

/// Create a test AppState with a single "test" session of custom dimensions.
pub fn create_test_state_with_size(rows: u16, cols: u16) -> (wsh::api::AppState, mpsc::Receiver<Bytes>, broadcast::Sender<Bytes>) {
    let ts = create_test_session_with_size("test", rows, cols);
    let output_tx = ts.broker.sender();
    let registry = SessionRegistry::new();
    registry.insert(Some("test".into()), ts.session).unwrap();
    let state = wsh::api::AppState {
        sessions: registry,
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(wsh::api::ServerConfig::new(false)),
    };
    (state, ts.input_rx, output_tx)
}
