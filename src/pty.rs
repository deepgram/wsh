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

pub struct Pty {
    pair: PtyPair,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl Pty {
    pub fn spawn(rows: u16, cols: u16) -> Result<Self, PtyError> {
        let pty_system = native_pty_system();

        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = pty_system.openpty(size).map_err(PtyError::OpenPty)?;

        // Use $SHELL or fall back to /bin/sh
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.env("TERM", std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_string()));

        let child = pair.slave.spawn_command(cmd).map_err(PtyError::SpawnCommand)?;

        Ok(Self { pair, child })
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

    pub fn wait(&mut self) -> Result<portable_pty::ExitStatus, PtyError> {
        Ok(self.child.wait()?)
    }
}
