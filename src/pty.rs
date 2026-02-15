use portable_pty::{native_pty_system, CommandBuilder, PtyPair, PtySize};
use std::io::{Read, Write};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PtyError {
    #[error("failed to open pty: {0}")]
    OpenPty(#[source] anyhow::Error),

    #[error("failed to spawn command: {0}")]
    SpawnCommand(#[source] anyhow::Error),

    #[error("failed to clone reader: {0}")]
    CloneReader(#[source] anyhow::Error),

    #[error("failed to take writer: {0}")]
    TakeWriter(#[source] anyhow::Error),

    #[error("failed to resize pty: {0}")]
    Resize(#[source] anyhow::Error),

    #[error("failed to wait for child: {0}")]
    Wait(#[from] std::io::Error),
}

/// Configuration for what command to spawn in the PTY.
#[derive(Debug, Clone)]
pub enum SpawnCommand {
    /// Spawn the user's shell ($SHELL or /bin/sh fallback).
    /// The bool indicates whether to force interactive mode (-i flag).
    /// An optional shell path overrides $SHELL.
    Shell { interactive: bool, shell: Option<String> },
    /// Spawn a command via `sh -c 'command'`.
    /// The bool indicates whether to force interactive mode (-i flag).
    Command { command: String, interactive: bool },
}

impl Default for SpawnCommand {
    fn default() -> Self {
        Self::Shell { interactive: false, shell: None }
    }
}

pub struct Pty {
    pair: PtyPair,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
}


impl Pty {
    /// Spawn a PTY with the given dimensions and command configuration.
    pub fn spawn(rows: u16, cols: u16, spawn_cmd: SpawnCommand) -> Result<Self, PtyError> {
        let cmd = Self::build_command(&spawn_cmd);
        Self::spawn_with_cmd(rows, cols, cmd)
    }

    /// Spawn a PTY with the given dimensions and a pre-built CommandBuilder.
    ///
    /// Use this when you need to customize the command (e.g. set cwd or env)
    /// before spawning.
    pub fn spawn_with_cmd(rows: u16, cols: u16, cmd: CommandBuilder) -> Result<Self, PtyError> {
        let pty_system = native_pty_system();

        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = pty_system.openpty(size).map_err(PtyError::OpenPty)?;

        let child = pair.slave.spawn_command(cmd).map_err(PtyError::SpawnCommand)?;

        Ok(Self { pair, child: Some(child) })
    }

    /// Build a CommandBuilder from the spawn configuration.
    pub fn build_command(spawn_cmd: &SpawnCommand) -> CommandBuilder {
        let term = std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_string());

        let mut cmd = match spawn_cmd {
            SpawnCommand::Shell { interactive, shell } => {
                let shell_path = match shell {
                    Some(s) => s.clone(),
                    None => std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
                };
                let mut cmd = CommandBuilder::new(&shell_path);
                if *interactive {
                    cmd.arg("-i");
                }
                cmd
            }
            SpawnCommand::Command { command, interactive } => {
                let mut cmd = CommandBuilder::new("/bin/sh");
                if *interactive {
                    cmd.arg("-ic");
                } else {
                    cmd.arg("-c");
                }
                cmd.arg(command);
                cmd
            }
        };

        cmd.env("TERM", term);
        cmd
    }

    pub fn take_reader(&self) -> Result<Box<dyn Read + Send>, PtyError> {
        self.pair.master.try_clone_reader().map_err(PtyError::CloneReader)
    }

    pub fn take_writer(&self) -> Result<Box<dyn Write + Send>, PtyError> {
        self.pair.master.take_writer().map_err(PtyError::TakeWriter)
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), PtyError> {
        self.pair.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        }).map_err(PtyError::Resize)
    }

    pub fn take_child(&mut self) -> Option<Box<dyn portable_pty::Child + Send + Sync>> {
        self.child.take()
    }

    pub fn wait(&mut self) -> Result<portable_pty::ExitStatus, PtyError> {
        match &mut self.child {
            Some(child) => Ok(child.wait()?),
            None => Err(PtyError::Wait(std::io::Error::other("child already taken"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    /// Helper to read from PTY with a timeout to avoid blocking forever.
    /// Returns the bytes read, or an empty vec if timeout occurred.
    fn read_with_timeout(
        mut reader: Box<dyn Read + Send>,
        timeout: Duration,
    ) -> Vec<u8> {
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let mut buf = vec![0u8; 4096];
            let mut collected = Vec::new();

            // Read in a loop until we get some data or error
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        collected.extend_from_slice(&buf[..n]);
                        // Send what we have so far
                        let _ = tx.send(collected.clone());
                        // Keep reading a bit more in case there's more output
                        thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => break,
                }
            }
        });

        // Wait for data with timeout
        rx.recv_timeout(timeout).unwrap_or_default()
    }

    #[test]
    fn test_spawn_creates_pty_with_shell() {
        let pty = Pty::spawn(24, 80, SpawnCommand::default());
        assert!(pty.is_ok(), "Failed to spawn PTY: {:?}", pty.err());
    }

    #[test]
    fn test_spawn_creates_pty_with_command() {
        let pty = Pty::spawn(24, 80, SpawnCommand::Command {
            command: "echo hello".to_string(),
            interactive: false,
        });
        assert!(pty.is_ok(), "Failed to spawn PTY with command: {:?}", pty.err());
    }

    #[test]
    fn test_spawn_interactive_shell() {
        let pty = Pty::spawn(24, 80, SpawnCommand::Shell { interactive: true, shell: None });
        assert!(pty.is_ok(), "Failed to spawn interactive shell: {:?}", pty.err());
    }

    #[test]
    fn test_take_reader_returns_handle() {
        let pty = Pty::spawn(24, 80, SpawnCommand::default()).expect("Failed to spawn PTY");
        let reader = pty.take_reader();
        assert!(reader.is_ok(), "Failed to get reader: {:?}", reader.err());
    }

    #[test]
    fn test_take_writer_returns_handle() {
        let pty = Pty::spawn(24, 80, SpawnCommand::default()).expect("Failed to spawn PTY");
        let writer = pty.take_writer();
        assert!(writer.is_ok(), "Failed to get writer: {:?}", writer.err());
    }

    #[test]
    fn test_write_and_read_roundtrip() {
        let pty = Pty::spawn(24, 80, SpawnCommand::default()).expect("Failed to spawn PTY");
        let mut writer = pty.take_writer().expect("Failed to get writer");
        let reader = pty.take_reader().expect("Failed to get reader");

        // Write a simple echo command
        // Use a unique marker to identify our output
        let marker = "WSH_TEST_12345";
        let cmd = format!("echo {}\n", marker);
        writer.write_all(cmd.as_bytes()).expect("Write failed");
        writer.flush().expect("Flush failed");

        // Read with timeout
        let output = read_with_timeout(reader, Duration::from_secs(2));

        // Convert to string and check for our marker
        let output_str = String::from_utf8_lossy(&output);
        assert!(
            output_str.contains(marker),
            "Expected output to contain '{}', but got: {}",
            marker,
            output_str
        );
    }

    #[test]
    fn test_command_execution() {
        // Test that SpawnCommand::Command actually runs the command
        let marker = "COMMAND_TEST_67890";
        let pty = Pty::spawn(24, 80, SpawnCommand::Command {
            command: format!("echo {}", marker),
            interactive: false,
        }).expect("Failed to spawn PTY with command");

        let reader = pty.take_reader().expect("Failed to get reader");
        let output = read_with_timeout(reader, Duration::from_secs(2));
        let output_str = String::from_utf8_lossy(&output);

        assert!(
            output_str.contains(marker),
            "Expected command output to contain '{}', but got: {}",
            marker,
            output_str
        );
    }

    #[test]
    fn test_resize_succeeds() {
        let pty = Pty::spawn(24, 80, SpawnCommand::default()).expect("Failed to spawn PTY");

        // Resize to different dimensions
        let result = pty.resize(40, 120);
        assert!(result.is_ok(), "Failed to resize PTY: {:?}", result.err());

        // Resize again to confirm it works multiple times
        let result = pty.resize(25, 100);
        assert!(result.is_ok(), "Failed to resize PTY second time: {:?}", result.err());
    }

    #[test]
    fn test_multiple_readers_can_be_cloned() {
        let pty = Pty::spawn(24, 80, SpawnCommand::default()).expect("Failed to spawn PTY");

        // Should be able to clone multiple readers
        let reader1 = pty.take_reader();
        let reader2 = pty.take_reader();

        assert!(reader1.is_ok(), "Failed to get first reader");
        assert!(reader2.is_ok(), "Failed to get second reader");
    }

    #[test]
    fn test_spawn_with_various_dimensions() {
        // Test with minimum dimensions
        let pty_small = Pty::spawn(1, 1, SpawnCommand::default());
        assert!(pty_small.is_ok(), "Failed to spawn PTY with 1x1 dimensions");

        // Test with larger dimensions
        let pty_large = Pty::spawn(100, 200, SpawnCommand::default());
        assert!(pty_large.is_ok(), "Failed to spawn PTY with 100x200 dimensions");
    }
}
