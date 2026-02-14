//! Input mode state for controlling input routing.
//!
//! Provides thread-safe state for controlling whether keyboard input
//! goes to the PTY (passthrough mode) or only to API subscribers (capture mode).
//! Tracks the owner of a capture to prevent one client from stealing
//! another client's capture.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use parking_lot::RwLock;

/// The current input routing mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Input goes to both API subscribers and PTY
    #[default]
    Passthrough,
    /// Input goes only to API subscribers
    Capture,
}

/// Error returned when a capture/release operation is rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputModeError {
    /// Another owner already holds the capture.
    AlreadyCaptured { owner: String },
    /// The caller is not the current capture owner.
    NotOwner,
}

impl std::fmt::Display for InputModeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InputModeError::AlreadyCaptured { owner } => {
                write!(f, "input already captured by {owner}")
            }
            InputModeError::NotOwner => {
                write!(f, "caller is not the current capture owner")
            }
        }
    }
}

impl std::error::Error for InputModeError {}

/// Internal state for InputMode.
struct ModeState {
    mode: Mode,
    /// Connection ID of the client that activated capture, if any.
    owner: Option<String>,
}

/// Thread-safe input mode state.
///
/// This struct provides a way to control input routing from multiple threads.
/// It defaults to passthrough mode where input flows to both API subscribers
/// and the PTY.
#[derive(Clone)]
pub struct InputMode {
    inner: Arc<RwLock<ModeState>>,
}

impl InputMode {
    /// Creates a new InputMode in the default Passthrough state.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(ModeState {
                mode: Mode::default(),
                owner: None,
            })),
        }
    }

    /// Gets the current mode.
    pub fn get(&self) -> Mode {
        self.inner.read().mode
    }

    /// Sets the mode to Capture with the given owner.
    ///
    /// Returns `Err` if already captured by a different owner.
    pub fn capture(&self, owner: &str) -> Result<(), InputModeError> {
        let mut guard = self.inner.write();
        if let Some(ref existing) = guard.owner {
            if existing != owner {
                return Err(InputModeError::AlreadyCaptured {
                    owner: existing.clone(),
                });
            }
        }
        guard.mode = Mode::Capture;
        guard.owner = Some(owner.to_string());
        Ok(())
    }

    /// Sets the mode to Passthrough, releasing the capture.
    ///
    /// Only succeeds if the caller is the current owner (or if there is no owner,
    /// for backward compatibility).
    pub fn release(&self, owner: &str) -> Result<(), InputModeError> {
        let mut guard = self.inner.write();
        if let Some(ref existing) = guard.owner {
            if existing != owner {
                return Err(InputModeError::NotOwner);
            }
        }
        guard.mode = Mode::Passthrough;
        guard.owner = None;
        Ok(())
    }

    /// Unconditionally releases the capture if the given owner holds it.
    ///
    /// Does nothing if someone else holds the capture or if already in passthrough.
    /// Used for auto-release on disconnect.
    pub fn release_if_owner(&self, owner: &str) {
        let mut guard = self.inner.write();
        if guard.owner.as_deref() == Some(owner) {
            guard.mode = Mode::Passthrough;
            guard.owner = None;
        }
    }

    /// Toggles the mode: Passthrough → Capture, Capture → Passthrough.
    ///
    /// Used by the local terminal user (Ctrl+\). Sets owner to "local".
    /// Returns the new mode after toggling.
    pub fn toggle(&self) -> Mode {
        let mut guard = self.inner.write();
        let new_mode = match guard.mode {
            Mode::Passthrough => {
                guard.owner = Some("local".to_string());
                Mode::Capture
            }
            Mode::Capture => {
                guard.owner = None;
                Mode::Passthrough
            }
        };
        guard.mode = new_mode;
        new_mode
    }

    /// Returns true if the current mode is Capture.
    pub fn is_capture(&self) -> bool {
        self.get() == Mode::Capture
    }

    /// Returns the current capture owner, if any.
    pub fn owner(&self) -> Option<String> {
        self.inner.read().owner.clone()
    }
}

impl Default for InputMode {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_mode_is_passthrough() {
        let input_mode = InputMode::new();
        assert_eq!(input_mode.get(), Mode::Passthrough);
    }

    #[test]
    fn test_capture_mode() {
        let input_mode = InputMode::new();
        input_mode.capture("agent-1").unwrap();
        assert_eq!(input_mode.get(), Mode::Capture);
    }

    #[test]
    fn test_release_mode() {
        let input_mode = InputMode::new();
        input_mode.capture("agent-1").unwrap();
        assert_eq!(input_mode.get(), Mode::Capture);
        input_mode.release("agent-1").unwrap();
        assert_eq!(input_mode.get(), Mode::Passthrough);
    }

    #[test]
    fn test_capture_rejected_if_different_owner() {
        let input_mode = InputMode::new();
        input_mode.capture("agent-1").unwrap();
        let err = input_mode.capture("agent-2").unwrap_err();
        assert_eq!(
            err,
            InputModeError::AlreadyCaptured {
                owner: "agent-1".to_string()
            }
        );
    }

    #[test]
    fn test_same_owner_can_recapture() {
        let input_mode = InputMode::new();
        input_mode.capture("agent-1").unwrap();
        input_mode.capture("agent-1").unwrap(); // should succeed
        assert_eq!(input_mode.get(), Mode::Capture);
    }

    #[test]
    fn test_release_rejected_if_not_owner() {
        let input_mode = InputMode::new();
        input_mode.capture("agent-1").unwrap();
        let err = input_mode.release("agent-2").unwrap_err();
        assert_eq!(err, InputModeError::NotOwner);
    }

    #[test]
    fn test_release_if_owner() {
        let input_mode = InputMode::new();
        input_mode.capture("agent-1").unwrap();

        // Different owner: no effect
        input_mode.release_if_owner("agent-2");
        assert_eq!(input_mode.get(), Mode::Capture);

        // Correct owner: releases
        input_mode.release_if_owner("agent-1");
        assert_eq!(input_mode.get(), Mode::Passthrough);
    }

    #[test]
    fn test_toggle() {
        let input_mode = InputMode::new();
        assert_eq!(input_mode.get(), Mode::Passthrough);

        let new_mode = input_mode.toggle();
        assert_eq!(new_mode, Mode::Capture);
        assert_eq!(input_mode.get(), Mode::Capture);
        assert_eq!(input_mode.owner(), Some("local".to_string()));

        let new_mode = input_mode.toggle();
        assert_eq!(new_mode, Mode::Passthrough);
        assert_eq!(input_mode.get(), Mode::Passthrough);
        assert_eq!(input_mode.owner(), None);
    }

    #[test]
    fn test_is_capture() {
        let input_mode = InputMode::new();
        assert!(!input_mode.is_capture());
        input_mode.capture("test").unwrap();
        assert!(input_mode.is_capture());
        input_mode.release("test").unwrap();
        assert!(!input_mode.is_capture());
    }

    #[test]
    fn test_clone_shares_state() {
        let input_mode1 = InputMode::new();
        let input_mode2 = input_mode1.clone();

        input_mode1.capture("agent-1").unwrap();
        assert_eq!(input_mode2.get(), Mode::Capture);

        input_mode2.release("agent-1").unwrap();
        assert_eq!(input_mode1.get(), Mode::Passthrough);
    }

    #[test]
    fn test_mode_default() {
        let mode = Mode::default();
        assert_eq!(mode, Mode::Passthrough);
    }

    #[test]
    fn test_mode_serialization() {
        let passthrough = Mode::Passthrough;
        let capture = Mode::Capture;

        let passthrough_json = serde_json::to_string(&passthrough).unwrap();
        let capture_json = serde_json::to_string(&capture).unwrap();

        assert_eq!(passthrough_json, "\"passthrough\"");
        assert_eq!(capture_json, "\"capture\"");
    }

    #[test]
    fn test_mode_deserialization() {
        let passthrough: Mode = serde_json::from_str("\"passthrough\"").unwrap();
        let capture: Mode = serde_json::from_str("\"capture\"").unwrap();

        assert_eq!(passthrough, Mode::Passthrough);
        assert_eq!(capture, Mode::Capture);
    }
}
