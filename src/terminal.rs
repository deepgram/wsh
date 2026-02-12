use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};
use std::io::Write;
use std::sync::{Arc, RwLock};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TerminalError {
    #[error("failed to enable raw mode: {0}")]
    EnableRawMode(#[source] std::io::Error),

    #[error("failed to set up screen: {0}")]
    Screen(#[source] std::io::Error),
}

/// RAII guard for terminal raw mode.
///
/// When created, enables raw mode on the terminal. When dropped (even on panic),
/// restores the terminal to its previous state.
///
/// Raw mode is needed to capture all keystrokes (including Ctrl+C, etc.) and
/// forward them to the PTY instead of having the local terminal handle them.
pub struct RawModeGuard {
    _private: (),
}

impl RawModeGuard {
    pub fn new() -> Result<Self, TerminalError> {
        enable_raw_mode().map_err(TerminalError::EnableRawMode)?;
        Ok(Self { _private: () })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// The screen mode to use when wsh starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenMode {
    /// Clear the screen (default). Preserves native terminal scrollback.
    Clear,
    /// Enter the alternate screen buffer. Restores the previous screen on drop.
    AltScreen,
}

/// RAII guard for terminal screen mode.
///
/// In `Clear` mode: clears the screen on creation, no-op on drop.
/// In `AltScreen` mode: enters the alternate screen buffer on creation,
/// restores the previous screen on drop (like vim, less, etc.).
pub struct ScreenGuard {
    mode: ScreenMode,
}

impl ScreenGuard {
    pub fn new(mode: ScreenMode) -> Result<Self, TerminalError> {
        let mut stdout = std::io::stdout();
        match mode {
            ScreenMode::Clear => {
                // Clear screen and move cursor to top-left
                stdout
                    .write_all(b"\x1b[2J\x1b[H")
                    .map_err(TerminalError::Screen)?;
                stdout.flush().map_err(TerminalError::Screen)?;
            }
            ScreenMode::AltScreen => {
                // Enter alternate screen buffer (smcup)
                stdout
                    .write_all(b"\x1b[?1049h")
                    .map_err(TerminalError::Screen)?;
                stdout.flush().map_err(TerminalError::Screen)?;
            }
        }
        Ok(Self { mode })
    }
}

impl Drop for ScreenGuard {
    fn drop(&mut self) {
        if self.mode == ScreenMode::AltScreen {
            let mut stdout = std::io::stdout();
            let _ = stdout.write_all(b"\x1b[?1049l");
            let _ = stdout.flush();
        }
    }
}

/// Get the current terminal size.
///
/// Returns (rows, cols) to match PtySize convention.
/// Note: crossterm::terminal::size() returns (cols, rows), so we swap them.
pub fn terminal_size() -> anyhow::Result<(u16, u16)> {
    let (cols, rows) = size()?;
    Ok((rows, cols))
}

/// Thread-safe shared terminal dimensions.
///
/// Tracks the outer terminal's current size so that layout computation
/// and resize handlers can access it from any thread.
#[derive(Clone)]
pub struct TerminalSize {
    inner: Arc<RwLock<(u16, u16)>>,
}

impl TerminalSize {
    /// Create a new TerminalSize with initial dimensions (rows, cols).
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            inner: Arc::new(RwLock::new((rows, cols))),
        }
    }

    /// Get the current terminal size as (rows, cols).
    pub fn get(&self) -> (u16, u16) {
        *self.inner.read().unwrap()
    }

    /// Update the terminal size.
    pub fn set(&self, rows: u16, cols: u16) {
        *self.inner.write().unwrap() = (rows, cols);
    }
}
