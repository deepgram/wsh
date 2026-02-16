//! Lifecycle stress tests for wsh client/server interactions.
//!
//! These tests exercise realistic user interaction sequences — creating sessions,
//! running commands, detaching, reattaching, entering alternate screen mode,
//! creating overlays — and verify that the client exits cleanly and no zombie
//! processes remain.
//!
//! All tests are #[ignore] by default. Run with:
//!   cargo test --test lifecycle_stress -- --ignored --nocapture
//!
//! Environment variables:
//!   WSH_STRESS_RUNS   — number of random walk iterations (default: 5)
//!   WSH_STRESS_STEPS  — steps per walk: "N" (exact) or "N..M" (range, default: 20..50)
//!
//! Examples:
//!   WSH_STRESS_RUNS=20 WSH_STRESS_STEPS=100 cargo test --test lifecycle_stress scenario_7 -- --ignored --nocapture
//!   WSH_STRESS_STEPS=10..30 cargo test --test lifecycle_stress scenario_6 -- --ignored --nocapture
//!
//! Tests spawn actual `wsh` binaries inside real PTYs (via portable-pty) to
//! exercise the full terminal code path including raw mode, poll()-based stdin,
//! and SIGWINCH handling.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use rand::{Rng, SeedableRng};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tempfile::TempDir;

// ── Constants ────────────────────────────────────────────────────────

const SERVER_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const CLIENT_EXIT_TIMEOUT: Duration = Duration::from_secs(10);
const SERVER_EXIT_TIMEOUT: Duration = Duration::from_secs(15);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(50);
const POST_ACTION_DELAY: Duration = Duration::from_millis(300);
const POST_COMMAND_DELAY: Duration = Duration::from_millis(500);
const SHELL_STARTUP_DELAY: Duration = Duration::from_millis(1000);

/// Maximum wall-clock time for a single scenario test before we abort.
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(120);
/// Maximum wall-clock time for the random walk stress test.
const RANDOM_WALK_TIMEOUT: Duration = Duration::from_secs(300);

/// Run a closure with a hard wall-clock timeout.
/// If the closure doesn't finish in time, the process is killed with a
/// diagnostic message. This catches hung tests that would otherwise block CI.
fn with_timeout<F: FnOnce() + Send + 'static>(name: &str, timeout: Duration, f: F) {
    let name = name.to_string();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();

    // Watchdog thread
    let watchdog_name = name.clone();
    let watchdog = std::thread::spawn(move || {
        if done_rx.recv_timeout(timeout).is_err() {
            eprintln!(
                "\n\nFATAL: Test '{}' exceeded timeout of {:?} — likely hung. Aborting process.",
                watchdog_name, timeout
            );
            std::process::exit(1);
        }
    });

    // Run the actual test
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));

    // Signal watchdog that we're done
    let _ = done_tx.send(());
    let _ = watchdog.join();

    // Re-panic if the test panicked
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

/// Unique counter for session names across tests.
static SESSION_COUNTER: AtomicU32 = AtomicU32::new(0);

fn unique_session_name(prefix: &str) -> String {
    let n = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}-{}", prefix, std::process::id(), n)
}

/// Parse WSH_STRESS_RUNS env var (number of random walk iterations).
/// Default: 5.
fn parse_walk_runs() -> usize {
    std::env::var("WSH_STRESS_RUNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5)
}

/// Parse WSH_STRESS_STEPS env var for steps per random walk.
/// Accepts "N" (exact), "N..M" (range). Default: 20..50.
fn parse_steps_range() -> (usize, usize) {
    match std::env::var("WSH_STRESS_STEPS") {
        Ok(s) => {
            if let Some((a, b)) = s.split_once("..") {
                let min = a.parse().unwrap_or(20);
                let max = b.parse().unwrap_or(50);
                (min, max)
            } else if let Ok(n) = s.parse::<usize>() {
                (n, n)
            } else {
                (20, 50)
            }
        }
        Err(_) => (20, 50),
    }
}

// ── Test Harness ─────────────────────────────────────────────────────

struct WshTestHarness {
    server_child: std::process::Child,
    server_pid: u32,
    socket_path: PathBuf,
    http_port: u16,
    _tmp_dir: TempDir,
}

impl WshTestHarness {
    /// Start a wsh server in ephemeral mode with a unique socket and HTTP port.
    fn start() -> Self {
        let tmp_dir = TempDir::new().expect("failed to create temp dir");
        let socket_path = tmp_dir.path().join("test.sock");

        // Find a free port
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let http_port = listener.local_addr().unwrap().port();
        drop(listener);

        let wsh_bin = env!("CARGO_BIN_EXE_wsh");

        let child = std::process::Command::new(wsh_bin)
            .arg("server")
            .arg("--ephemeral")
            .arg("--bind")
            .arg(format!("127.0.0.1:{}", http_port))
            .arg("--socket")
            .arg(&socket_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("failed to spawn wsh server");

        let server_pid = child.id();

        let harness = Self {
            server_child: child,
            server_pid,
            socket_path,
            http_port,
            _tmp_dir: tmp_dir,
        };

        // Wait for the server to be ready
        harness.wait_for_ready();
        harness
    }

    fn wait_for_ready(&self) {
        let url = format!("http://127.0.0.1:{}/health", self.http_port);
        let client = reqwest::blocking::Client::new();
        let deadline = Instant::now() + SERVER_STARTUP_TIMEOUT;

        while Instant::now() < deadline {
            if let Ok(resp) = client.get(&url).send() {
                if resp.status().is_success() {
                    return;
                }
            }
            std::thread::sleep(HEALTH_POLL_INTERVAL);
        }
        panic!("wsh server did not become ready within {:?}", SERVER_STARTUP_TIMEOUT);
    }

    /// Create a session via HTTP and return its name.
    fn create_session(&self, name: &str) -> String {
        let url = format!("http://127.0.0.1:{}/sessions", self.http_port);
        let client = reqwest::blocking::Client::new();
        let resp = client
            .post(&url)
            .json(&serde_json::json!({"name": name, "rows": 24, "cols": 80}))
            .send()
            .expect("session create failed");
        assert_eq!(resp.status().as_u16(), 201, "expected 201 Created, got {}", resp.status());
        let body: serde_json::Value = resp.json().unwrap();
        body["name"].as_str().unwrap().to_string()
    }

    /// Spawn a `wsh attach` client inside a real PTY.
    fn spawn_attach(&self, session_name: &str) -> WshClient {
        let wsh_bin = env!("CARGO_BIN_EXE_wsh");

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("failed to open pty");

        let mut cmd = CommandBuilder::new(wsh_bin);
        cmd.arg("attach");
        cmd.arg(session_name);
        cmd.arg("--socket");
        cmd.arg(&self.socket_path);
        cmd.arg("--scrollback");
        cmd.arg("none");
        cmd.env("TERM", "xterm-256color");

        let child = pair.slave.spawn_command(cmd).expect("failed to spawn wsh client");
        let reader = pair.master.try_clone_reader().expect("failed to get pty reader");
        let writer = pair.master.take_writer().expect("failed to get pty writer");

        WshClient::new(child, reader, writer, pair.master)
    }

    /// Create an overlay on a session via HTTP. Returns the overlay ID.
    fn create_overlay(&self, session_name: &str, content: &str) -> String {
        let url = format!(
            "http://127.0.0.1:{}/sessions/{}/overlay",
            self.http_port, session_name
        );
        let client = reqwest::blocking::Client::new();
        let resp = client
            .post(&url)
            .json(&serde_json::json!({
                "x": 1,
                "y": 1,
                "width": 40,
                "height": 1,
                "spans": [{"text": content}]
            }))
            .send()
            .expect("create overlay failed");
        assert!(
            resp.status().is_success(),
            "overlay create failed: {}",
            resp.status()
        );
        let body: serde_json::Value = resp.json().unwrap();
        body["id"].as_str().unwrap().to_string()
    }

    /// Delete an overlay from a session via HTTP.
    fn delete_overlay(&self, session_name: &str, id: &str) {
        let url = format!(
            "http://127.0.0.1:{}/sessions/{}/overlay/{}",
            self.http_port, session_name, id
        );
        let client = reqwest::blocking::Client::new();
        let resp = client.delete(&url).send().expect("delete overlay failed");
        assert!(
            resp.status().is_success(),
            "overlay delete failed: {}",
            resp.status()
        );
    }

    /// Detach a session remotely via `wsh detach`.
    fn detach_remote(&self, session_name: &str) {
        let wsh_bin = env!("CARGO_BIN_EXE_wsh");
        let output = std::process::Command::new(wsh_bin)
            .arg("detach")
            .arg(session_name)
            .arg("--socket")
            .arg(&self.socket_path)
            .output()
            .expect("wsh detach failed to execute");
        assert!(
            output.status.success(),
            "wsh detach failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// List sessions via HTTP.
    #[allow(dead_code)]
    fn list_sessions(&self) -> Vec<String> {
        let url = format!("http://127.0.0.1:{}/sessions", self.http_port);
        let client = reqwest::blocking::Client::new();
        let resp = client.get(&url).send().expect("list sessions failed");
        let body: serde_json::Value = resp.json().unwrap();
        body.as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect()
    }

    /// Wait for the server to exit (ephemeral shutdown).
    fn wait_server_exit(&mut self) {
        let deadline = Instant::now() + SERVER_EXIT_TIMEOUT;
        loop {
            match self.server_child.try_wait() {
                Ok(Some(status)) => {
                    eprintln!("  [harness] server exited: {:?}", status);
                    return;
                }
                Ok(None) => {
                    if Instant::now() > deadline {
                        eprintln!("  [harness] server did not exit in time, killing");
                        self.server_child.kill().ok();
                        self.server_child.wait().ok();
                        panic!(
                            "BUG: wsh server (pid {}) did not exit within {:?}",
                            self.server_pid, SERVER_EXIT_TIMEOUT
                        );
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => panic!("waitpid failed: {}", e),
            }
        }
    }

    /// Assert no orphan wsh processes exist that reference our socket.
    fn assert_no_orphans(&self) {
        // Give processes a moment to clean up
        std::thread::sleep(Duration::from_millis(500));

        let output = std::process::Command::new("pgrep")
            .arg("-f")
            .arg(self.socket_path.to_str().unwrap())
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                let pids = String::from_utf8_lossy(&out.stdout);
                panic!(
                    "BUG: orphan wsh processes found referencing {}:\n{}",
                    self.socket_path.display(),
                    pids.trim()
                );
            }
        }
        // pgrep returning non-zero means no matches — good
    }
}

impl Drop for WshTestHarness {
    fn drop(&mut self) {
        // Best-effort cleanup
        self.server_child.kill().ok();
        self.server_child.wait().ok();
    }
}

// ── PTY-Wrapped Client ──────────────────────────────────────────────

struct WshClient {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Receives chunks of output from the background reader thread.
    output_rx: std::sync::mpsc::Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    _reader_thread: std::thread::JoinHandle<()>,
}

impl WshClient {
    /// Construct from PTY components, spawning a background reader thread.
    fn new(
        child: Box<dyn portable_pty::Child + Send + Sync>,
        mut reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        master: Box<dyn portable_pty::MasterPty + Send>,
    ) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let reader_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break; // receiver dropped
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            child,
            output_rx: rx,
            writer,
            _master: master,
            _reader_thread: reader_thread,
        }
    }

    /// Send raw bytes to the client's stdin (via PTY master).
    fn send(&mut self, data: &[u8]) {
        self.writer.write_all(data).expect("failed to write to client pty");
        self.writer.flush().expect("failed to flush client pty");
    }

    /// Send a line of text (appends \r for PTY).
    fn send_line(&mut self, text: &str) {
        let mut buf = text.as_bytes().to_vec();
        buf.push(b'\r');
        self.send(&buf);
    }

    /// Send Ctrl+D (EOF).
    fn send_ctrl_d(&mut self) {
        self.send(&[0x04]);
    }

    /// Send Ctrl+C.
    #[allow(dead_code)]
    fn send_ctrl_c(&mut self) {
        self.send(&[0x03]);
    }

    /// Send Ctrl+\ (0x1c).
    fn send_ctrl_backslash(&mut self) {
        self.send(&[0x1c]);
    }

    /// Double-tap Ctrl+\ to detach.
    fn detach(&mut self) {
        self.send_ctrl_backslash();
        std::thread::sleep(Duration::from_millis(100));
        self.send_ctrl_backslash();
    }

    /// Drain any pending output from the background reader, non-blocking.
    fn drain(&mut self) {
        // Give a small window for output to arrive, then drain the channel
        std::thread::sleep(Duration::from_millis(50));
        while self.output_rx.try_recv().is_ok() {}
    }

    /// Read from PTY until `pattern` appears or timeout. Returns (output, found).
    #[allow(dead_code)]
    fn read_until(&mut self, pattern: &str, timeout: Duration) -> (String, bool) {
        let mut collected = Vec::new();
        let deadline = Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.output_rx.recv_timeout(remaining.min(Duration::from_millis(100))) {
                Ok(chunk) => {
                    collected.extend(chunk);
                    let output = String::from_utf8_lossy(&collected);
                    if output.contains(pattern) {
                        return (output.to_string(), true);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        let output = String::from_utf8_lossy(&collected).to_string();
        (output, false)
    }

    /// Wait for the child process to exit within the timeout.
    /// Panics if it doesn't exit in time.
    fn wait_exit(mut self, timeout: Duration) -> portable_pty::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            // Keep draining output to avoid blocking the child on a full PTY buffer
            while self.output_rx.try_recv().is_ok() {}

            match self.child.try_wait() {
                Ok(Some(status)) => return status,
                Ok(None) => {
                    if Instant::now() > deadline {
                        self.child.kill().ok();
                        panic!(
                            "BUG: wsh client did not exit within {:?} — this is the hang bug",
                            timeout
                        );
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => panic!("try_wait failed: {}", e),
            }
        }
    }

    /// Check if the child process is still running.
    #[allow(dead_code)]
    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

// ── Action Log ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
#[allow(dead_code)]
enum Action {
    SendCommand(String),
    DetachCtrlBackslash,
    DetachRemote,
    Reattach,
    AltScreenOn,
    AltScreenOff,
    CreateOverlay(String),  // content
    DeleteOverlay,
    Sleep(Duration),
    CtrlD,
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Action::SendCommand(cmd) => write!(f, "SendCommand({:?})", cmd),
            Action::DetachCtrlBackslash => write!(f, "DetachCtrlBackslash"),
            Action::DetachRemote => write!(f, "DetachRemote"),
            Action::Reattach => write!(f, "Reattach"),
            Action::AltScreenOn => write!(f, "AltScreenOn"),
            Action::AltScreenOff => write!(f, "AltScreenOff"),
            Action::CreateOverlay(content) => write!(f, "CreateOverlay({:?})", content),
            Action::DeleteOverlay => write!(f, "DeleteOverlay"),
            Action::Sleep(d) => write!(f, "Sleep({:?})", d),
            Action::CtrlD => write!(f, "CtrlD"),
        }
    }
}

/// Execute an action sequence, logging each step. Returns the final client (or panics).
#[allow(dead_code)]
fn execute_scenario(
    harness: &WshTestHarness,
    session_name: &str,
    actions: &[Action],
    log: &mut Vec<String>,
) -> WshClient {
    let mut client: Option<WshClient> = None;
    let mut overlay_id_store: Option<String> = None;

    for (i, action) in actions.iter().enumerate() {
        let step = format!("  step {}: {}", i, action);
        eprintln!("{}", &step);
        log.push(step);

        match action {
            Action::SendCommand(cmd) => {
                let c = client.as_mut().expect("no client for SendCommand");
                c.send_line(cmd);
                std::thread::sleep(POST_COMMAND_DELAY);
                c.drain();
            }
            Action::DetachCtrlBackslash => {
                let c = client.as_mut().expect("no client for Detach");
                c.detach();
                // Wait for the client to actually exit (detach)
                std::thread::sleep(Duration::from_millis(500));
                // Drain any remaining output
                if let Some(ref mut c) = client {
                    c.drain();
                }
                // Wait for the process to exit
                let c = client.take().expect("no client for Detach exit");
                let _status = c.wait_exit(CLIENT_EXIT_TIMEOUT);
            }
            Action::DetachRemote => {
                harness.detach_remote(session_name);
                std::thread::sleep(Duration::from_millis(500));
                if let Some(ref mut c) = client {
                    c.drain();
                }
                // Wait for the client to exit from the remote detach
                let c = client.take().expect("no client for RemoteDetach exit");
                let _status = c.wait_exit(CLIENT_EXIT_TIMEOUT);
            }
            Action::Reattach => {
                assert!(client.is_none(), "client still alive at Reattach");
                let mut c = harness.spawn_attach(session_name);
                std::thread::sleep(SHELL_STARTUP_DELAY);
                c.drain();
                client = Some(c);
            }
            Action::AltScreenOn => {
                let c = client.as_mut().expect("no client for AltScreenOn");
                c.send_line("printf '\\x1b[?1049h'");
                std::thread::sleep(POST_ACTION_DELAY);
                c.drain();
            }
            Action::AltScreenOff => {
                let c = client.as_mut().expect("no client for AltScreenOff");
                c.send_line("printf '\\x1b[?1049l'");
                std::thread::sleep(POST_ACTION_DELAY);
                c.drain();
            }
            Action::CreateOverlay(content) => {
                let id = harness.create_overlay(session_name, content);
                overlay_id_store = Some(id);
                std::thread::sleep(POST_ACTION_DELAY);
                if let Some(ref mut c) = client {
                    c.drain();
                }
            }
            Action::DeleteOverlay => {
                if let Some(ref id) = overlay_id_store {
                    harness.delete_overlay(session_name, id);
                    overlay_id_store = None;
                }
                std::thread::sleep(POST_ACTION_DELAY);
                if let Some(ref mut c) = client {
                    c.drain();
                }
            }
            Action::Sleep(d) => {
                std::thread::sleep(*d);
            }
            Action::CtrlD => {
                let c = client.as_mut().expect("no client for CtrlD");
                c.send_ctrl_d();
                std::thread::sleep(POST_ACTION_DELAY);
            }
        }
    }

    client.expect("scenario ended without a live client")
}

/// Run a scenario: create session, attach, execute actions, verify clean exit.
fn run_scenario(name: &str, actions: Vec<Action>) {
    eprintln!("\n{}", "=".repeat(60));
    eprintln!("SCENARIO: {}", name);
    eprintln!("{}", "=".repeat(60));

    let harness = WshTestHarness::start();
    let session_name = unique_session_name("lifecycle");

    // Create session via HTTP
    harness.create_session(&session_name);

    // Initial attach
    let mut initial_client = harness.spawn_attach(&session_name);
    std::thread::sleep(SHELL_STARTUP_DELAY);
    initial_client.drain();

    // Build full action sequence: prepend initial client
    let mut log = Vec::new();
    let mut client: Option<WshClient> = Some(initial_client);
    let mut overlay_id: Option<String> = None;

    for (i, action) in actions.iter().enumerate() {
        let step = format!("  step {}: {}", i, action);
        eprintln!("{}", &step);
        log.push(step.clone());

        match action {
            Action::SendCommand(cmd) => {
                let c = client.as_mut().expect("no client for SendCommand");
                c.send_line(cmd);
                std::thread::sleep(POST_COMMAND_DELAY);
                c.drain();
            }
            Action::DetachCtrlBackslash => {
                let c = client.as_mut().expect("no client for Detach");
                c.detach();
                std::thread::sleep(Duration::from_millis(500));
                let c = client.take().expect("no client for Detach exit");
                let _status = c.wait_exit(CLIENT_EXIT_TIMEOUT);
                eprintln!("  [info] client exited after Ctrl+\\ detach");
            }
            Action::DetachRemote => {
                harness.detach_remote(&session_name);
                std::thread::sleep(Duration::from_millis(500));
                let c = client.take().expect("no client for RemoteDetach exit");
                let _status = c.wait_exit(CLIENT_EXIT_TIMEOUT);
                eprintln!("  [info] client exited after remote detach");
            }
            Action::Reattach => {
                assert!(client.is_none(), "client still alive at Reattach step {}", i);
                let mut c = harness.spawn_attach(&session_name);
                std::thread::sleep(SHELL_STARTUP_DELAY);
                c.drain();
                client = Some(c);
                eprintln!("  [info] reattached to session");
            }
            Action::AltScreenOn => {
                let c = client.as_mut().expect("no client for AltScreenOn");
                c.send_line("printf '\\x1b[?1049h'");
                std::thread::sleep(POST_ACTION_DELAY);
                c.drain();
            }
            Action::AltScreenOff => {
                let c = client.as_mut().expect("no client for AltScreenOff");
                c.send_line("printf '\\x1b[?1049l'");
                std::thread::sleep(POST_ACTION_DELAY);
                c.drain();
            }
            Action::CreateOverlay(content) => {
                let id = harness.create_overlay(&session_name, content);
                overlay_id = Some(id);
                std::thread::sleep(POST_ACTION_DELAY);
                if let Some(ref mut c) = client {
                    c.drain();
                }
            }
            Action::DeleteOverlay => {
                if let Some(ref id) = overlay_id {
                    harness.delete_overlay(&session_name, id);
                    overlay_id = None;
                }
                std::thread::sleep(POST_ACTION_DELAY);
                if let Some(ref mut c) = client {
                    c.drain();
                }
            }
            Action::Sleep(d) => {
                std::thread::sleep(*d);
            }
            Action::CtrlD => {
                let c = client.as_mut().expect("no client for CtrlD");
                c.send_ctrl_d();
                std::thread::sleep(POST_ACTION_DELAY);
            }
        }
    }

    // The scenario should end with a live client that we now Ctrl+D
    let mut final_client = client.expect("scenario should end with a live client");
    eprintln!("  [final] sending Ctrl+D to exit");
    final_client.send_ctrl_d();

    // Assert client exits cleanly
    eprintln!("  [final] waiting for client exit...");
    let status = final_client.wait_exit(CLIENT_EXIT_TIMEOUT);
    eprintln!("  [final] client exited: {:?}", status);

    // Wait for ephemeral server to exit (session's shell should have died from Ctrl+D)
    // Give the server a moment to notice
    std::thread::sleep(Duration::from_secs(1));

    // Check if the server is gone (it should be — ephemeral mode, last session ended)
    // If not, that's also a bug but don't block forever
    let mut harness = harness;
    match harness.server_child.try_wait() {
        Ok(Some(status)) => {
            eprintln!("  [final] server exited: {:?}", status);
        }
        Ok(None) => {
            // Server still running — this might be OK if Ctrl+D didn't kill the shell
            // (e.g., shell has ignoreeof set). Try waiting a bit longer.
            eprintln!("  [final] server still running, waiting up to {:?}...", SERVER_EXIT_TIMEOUT);
            harness.wait_server_exit();
        }
        Err(e) => panic!("waitpid failed: {}", e),
    }

    // Assert no orphans
    harness.assert_no_orphans();

    eprintln!("  PASSED: {}", name);
    eprintln!();
}

// ── Scripted Scenarios ───────────────────────────────────────────────

#[test]
#[ignore]
fn lifecycle_scenario_1_extended_detach_cycles() {
    with_timeout("scenario_1_extended_detach_cycles", SCENARIO_TIMEOUT, || {
        run_scenario(
            "extended_detach_cycles",
            vec![
                Action::SendCommand("echo scenario1_a".into()),
                Action::SendCommand("date".into()),
                Action::SendCommand("ls /tmp".into()),
                Action::DetachCtrlBackslash,
                Action::Reattach,
                Action::SendCommand("echo scenario1_b".into()),
                Action::SendCommand("echo scenario1_c".into()),
                Action::DetachCtrlBackslash,
                Action::Reattach,
                Action::SendCommand("echo scenario1_d".into()),
            ],
        );
    });
}

#[test]
#[ignore]
fn lifecycle_scenario_2_alt_screen_with_commands() {
    with_timeout("scenario_2_alt_screen_with_commands", SCENARIO_TIMEOUT, || {
        run_scenario(
            "alt_screen_with_commands",
            vec![
                Action::SendCommand("echo start".into()),
                Action::AltScreenOn,
                Action::SendCommand("echo inside_alt".into()),
                Action::AltScreenOff,
                Action::SendCommand("echo back_normal".into()),
                Action::DetachCtrlBackslash,
                Action::Reattach,
                Action::AltScreenOn,
                Action::SendCommand("echo alt_again".into()),
                Action::AltScreenOff,
                Action::SendCommand("echo final".into()),
            ],
        );
    });
}

#[test]
#[ignore]
fn lifecycle_scenario_3_overlay_detach_commands() {
    with_timeout("scenario_3_overlay_detach_commands", SCENARIO_TIMEOUT, || {
        run_scenario(
            "overlay_detach_commands",
            vec![
                Action::SendCommand("echo x".into()),
                Action::CreateOverlay("test overlay".into()),
                Action::SendCommand("echo y".into()),
                Action::SendCommand("echo y2".into()),
                Action::DetachCtrlBackslash,
                Action::Reattach,
                Action::SendCommand("echo z".into()),
                Action::DeleteOverlay,
                Action::SendCommand("echo w".into()),
            ],
        );
    });
}

#[test]
#[ignore]
fn lifecycle_scenario_4_remote_detach_cycles() {
    with_timeout("scenario_4_remote_detach_cycles", SCENARIO_TIMEOUT, || {
        run_scenario(
            "remote_detach_cycles",
            vec![
                Action::SendCommand("echo one".into()),
                Action::SendCommand("echo two".into()),
                Action::DetachRemote,
                Action::Reattach,
                Action::SendCommand("echo three".into()),
                Action::SendCommand("echo four".into()),
                Action::DetachRemote,
                Action::Reattach,
                Action::SendCommand("echo five".into()),
            ],
        );
    });
}

#[test]
#[ignore]
fn lifecycle_scenario_5_kitchen_sink() {
    with_timeout("scenario_5_kitchen_sink", SCENARIO_TIMEOUT, || {
        run_scenario(
            "kitchen_sink",
            vec![
                Action::SendCommand("echo ks_a".into()),
                Action::AltScreenOn,
                Action::SendCommand("echo ks_b".into()),
                Action::AltScreenOff,
                Action::CreateOverlay("ks overlay 1".into()),
                Action::SendCommand("echo ks_c".into()),
                Action::DetachCtrlBackslash,
                Action::Reattach,
                Action::SendCommand("echo ks_d".into()),
                Action::DeleteOverlay,
                Action::SendCommand("echo ks_e".into()),
                Action::DetachRemote,
                Action::Reattach,
                Action::AltScreenOn,
                Action::SendCommand("echo ks_f".into()),
                Action::AltScreenOff,
                Action::CreateOverlay("ks overlay 2".into()),
                Action::SendCommand("echo ks_g".into()),
                Action::DeleteOverlay,
                Action::SendCommand("echo ks_h".into()),
            ],
        );
    });
}

// ── Random Walk Stress Test ─────────────────────────────────────────

fn run_random_walk() {
    let seed: u64 = rand::thread_rng().gen();
    eprintln!("\n{}", "=".repeat(60));
    eprintln!("RANDOM WALK STRESS TEST");
    eprintln!("Seed: {}", seed);
    eprintln!("{}", "=".repeat(60));

    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let harness = WshTestHarness::start();
    let session_name = unique_session_name("random");

    harness.create_session(&session_name);

    let mut client_opt: Option<WshClient> = {
        let mut c = harness.spawn_attach(&session_name);
        std::thread::sleep(SHELL_STARTUP_DELAY);
        c.drain();
        Some(c)
    };

    let mut in_alt_screen = false;
    let mut overlay_id: Option<String> = None;
    let (min_steps, max_steps) = parse_steps_range();
    let num_actions: usize = rng.gen_range(min_steps..=max_steps);
    let mut action_log: Vec<String> = Vec::new();
    let mut cmd_counter = 0u32;

    eprintln!("  Planning {} random actions", num_actions);

    for i in 0..num_actions {
        // Choose an action weighted by current state
        let has_client = client_opt.is_some();

        let action_choice: u32 = if has_client {
            rng.gen_range(0..100)
        } else {
            // Must reattach
            100
        };

        let action_desc;

        if action_choice == 100 || !has_client {
            // Reattach
            action_desc = format!("step {}: Reattach", i);
            eprintln!("  {}", action_desc);
            action_log.push(action_desc);

            let mut c = harness.spawn_attach(&session_name);
            std::thread::sleep(SHELL_STARTUP_DELAY);
            c.drain();
            client_opt = Some(c);
        } else if action_choice < 35 {
            // Send command (most common)
            cmd_counter += 1;
            let cmd = format!("echo rw_cmd_{}", cmd_counter);
            action_desc = format!("step {}: SendCommand({:?})", i, cmd);
            eprintln!("  {}", action_desc);
            action_log.push(action_desc);

            let c = client_opt.as_mut().unwrap();
            c.send_line(&cmd);
            let delay = Duration::from_millis(rng.gen_range(100..=500));
            std::thread::sleep(delay);
            c.drain();
        } else if action_choice < 50 {
            // Detach via Ctrl+\ double-tap
            action_desc = format!("step {}: DetachCtrlBackslash", i);
            eprintln!("  {}", action_desc);
            action_log.push(action_desc);

            let c = client_opt.as_mut().unwrap();
            c.detach();
            std::thread::sleep(Duration::from_millis(500));
            let c = client_opt.take().unwrap();
            let _status = c.wait_exit(CLIENT_EXIT_TIMEOUT);
            in_alt_screen = false; // reset state for next attach
        } else if action_choice < 60 {
            // Detach remotely
            action_desc = format!("step {}: DetachRemote", i);
            eprintln!("  {}", action_desc);
            action_log.push(action_desc);

            harness.detach_remote(&session_name);
            std::thread::sleep(Duration::from_millis(500));
            let c = client_opt.take().unwrap();
            let _status = c.wait_exit(CLIENT_EXIT_TIMEOUT);
            in_alt_screen = false;
        } else if action_choice < 70 && !in_alt_screen {
            // Alt screen on
            action_desc = format!("step {}: AltScreenOn", i);
            eprintln!("  {}", action_desc);
            action_log.push(action_desc);

            let c = client_opt.as_mut().unwrap();
            c.send_line("printf '\\x1b[?1049h'");
            std::thread::sleep(POST_ACTION_DELAY);
            c.drain();
            in_alt_screen = true;
        } else if action_choice < 70 && in_alt_screen {
            // Alt screen off
            action_desc = format!("step {}: AltScreenOff", i);
            eprintln!("  {}", action_desc);
            action_log.push(action_desc);

            let c = client_opt.as_mut().unwrap();
            c.send_line("printf '\\x1b[?1049l'");
            std::thread::sleep(POST_ACTION_DELAY);
            c.drain();
            in_alt_screen = false;
        } else if action_choice < 80 && overlay_id.is_none() {
            // Create overlay
            action_desc = format!("step {}: CreateOverlay", i);
            eprintln!("  {}", action_desc);
            action_log.push(action_desc);

            let id = harness.create_overlay(&session_name, "random overlay");
            overlay_id = Some(id);
            std::thread::sleep(POST_ACTION_DELAY);
            if let Some(ref mut c) = client_opt {
                c.drain();
            }
        } else if action_choice < 80 && overlay_id.is_some() {
            // Delete overlay
            action_desc = format!("step {}: DeleteOverlay", i);
            eprintln!("  {}", action_desc);
            action_log.push(action_desc);

            harness.delete_overlay(&session_name, overlay_id.as_ref().unwrap());
            overlay_id = None;
            std::thread::sleep(POST_ACTION_DELAY);
            if let Some(ref mut c) = client_opt {
                c.drain();
            }
        } else {
            // Random sleep
            let delay = Duration::from_millis(rng.gen_range(10..=200));
            action_desc = format!("step {}: Sleep({:?})", i, delay);
            eprintln!("  {}", action_desc);
            action_log.push(action_desc);
            std::thread::sleep(delay);
        }
    }

    // Clean up alt screen if needed
    if in_alt_screen {
        if let Some(ref mut c) = client_opt {
            eprintln!("  [cleanup] leaving alt screen");
            c.send_line("printf '\\x1b[?1049l'");
            std::thread::sleep(POST_ACTION_DELAY);
            c.drain();
        }
    }

    // Clean up overlay if needed
    if let Some(ref id) = overlay_id {
        eprintln!("  [cleanup] deleting overlay {}", id);
        harness.delete_overlay(&session_name, id);
        std::thread::sleep(POST_ACTION_DELAY);
    }

    // Ensure we have a client to exit from
    if client_opt.is_none() {
        eprintln!("  [cleanup] reattaching for final exit");
        let mut c = harness.spawn_attach(&session_name);
        std::thread::sleep(SHELL_STARTUP_DELAY);
        c.drain();
        client_opt = Some(c);
    }

    // Send Ctrl+D to exit
    eprintln!("  [final] sending Ctrl+D");
    let mut final_client = client_opt.take().unwrap();
    final_client.send_ctrl_d();

    eprintln!("  [final] waiting for client exit...");
    let status = final_client.wait_exit(CLIENT_EXIT_TIMEOUT);
    eprintln!("  [final] client exited: {:?}", status);

    // Wait for server
    std::thread::sleep(Duration::from_secs(1));
    let mut harness = harness;
    match harness.server_child.try_wait() {
        Ok(Some(status)) => {
            eprintln!("  [final] server exited: {:?}", status);
        }
        Ok(None) => {
            eprintln!("  [final] server still running, waiting...");
            harness.wait_server_exit();
        }
        Err(e) => panic!("waitpid failed: {}", e),
    }

    harness.assert_no_orphans();

    eprintln!("\n  PASSED: random walk (seed: {}, {} actions)", seed, num_actions);
    eprintln!("  Action log:");
    for entry in &action_log {
        eprintln!("    {}", entry);
    }
}

#[test]
#[ignore]
fn lifecycle_scenario_6_random_walk() {
    with_timeout("scenario_6_random_walk", RANDOM_WALK_TIMEOUT, run_random_walk);
}

// ── Repeated Random Walk ────────────────────────────────────────────

#[test]
#[ignore]
fn lifecycle_scenario_7_repeated_random_walks() {
    with_timeout("scenario_7_repeated_random_walks", RANDOM_WALK_TIMEOUT, || {
        let num_runs = parse_walk_runs();
        for run in 0..num_runs {
            eprintln!("\n>>> RANDOM WALK RUN {}/{}", run + 1, num_runs);
            run_random_walk();
        }
    });
}
