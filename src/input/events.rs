//! Input event broadcasting for subscribers.
//!
//! Provides a broadcast channel for input events, allowing subscribers
//! to receive input from stdin in real-time.

use serde::Serialize;
use tokio::sync::broadcast;

use super::{parse_key, Mode, ParsedKey};

/// Input event broadcast to subscribers
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum InputEvent {
    Input {
        mode: Mode,
        raw: Vec<u8>,
        parsed: Option<ParsedKey>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target: Option<String>,
    },
    Mode {
        mode: Mode,
    },
}

/// Broadcaster for input events
#[derive(Clone)]
pub struct InputBroadcaster {
    tx: broadcast::Sender<InputEvent>,
}

impl InputBroadcaster {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { tx }
    }

    pub fn broadcast_input(&self, data: &[u8], mode: Mode, target: Option<String>) {
        let parsed = parse_key(data);
        let parsed = if parsed.key.is_some() {
            Some(parsed)
        } else {
            None
        };
        let _ = self.tx.send(InputEvent::Input {
            mode,
            raw: data.to_vec(),
            parsed,
            target,
        });
    }

    pub fn broadcast_mode(&self, mode: Mode) {
        let _ = self.tx.send(InputEvent::Mode { mode });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<InputEvent> {
        self.tx.subscribe()
    }
}

impl Default for InputBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_broadcaster_new() {
        let broadcaster = InputBroadcaster::new();
        let _rx = broadcaster.subscribe();
    }

    #[test]
    fn test_broadcast_input_with_parsed_key() {
        let broadcaster = InputBroadcaster::new();
        let mut rx = broadcaster.subscribe();

        broadcaster.broadcast_input(b"a", Mode::Passthrough, None);

        let event = rx.try_recv().unwrap();
        match event {
            InputEvent::Input { mode, raw, parsed, target } => {
                assert_eq!(mode, Mode::Passthrough);
                assert_eq!(raw, vec![b'a']);
                assert!(parsed.is_some());
                let parsed = parsed.unwrap();
                assert_eq!(parsed.key, Some("a".to_string()));
                assert!(target.is_none());
            }
            _ => panic!("Expected Input event"),
        }
    }

    #[test]
    fn test_broadcast_input_without_parsed_key() {
        let broadcaster = InputBroadcaster::new();
        let mut rx = broadcaster.subscribe();

        // Unknown sequence
        broadcaster.broadcast_input(&[0x80, 0x81], Mode::Capture, None);

        let event = rx.try_recv().unwrap();
        match event {
            InputEvent::Input { mode, raw, parsed, target } => {
                assert_eq!(mode, Mode::Capture);
                assert_eq!(raw, vec![0x80, 0x81]);
                assert!(parsed.is_none());
                assert!(target.is_none());
            }
            _ => panic!("Expected Input event"),
        }
    }

    #[test]
    fn test_broadcast_mode() {
        let broadcaster = InputBroadcaster::new();
        let mut rx = broadcaster.subscribe();

        broadcaster.broadcast_mode(Mode::Capture);

        let event = rx.try_recv().unwrap();
        match event {
            InputEvent::Mode { mode } => {
                assert_eq!(mode, Mode::Capture);
            }
            _ => panic!("Expected Mode event"),
        }
    }

    #[test]
    fn test_input_event_serialization() {
        let event = InputEvent::Input {
            mode: Mode::Passthrough,
            raw: vec![b'a'],
            parsed: Some(ParsedKey::new(Some("a".to_string()))),
            target: None,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event\":\"input\""));
        assert!(json.contains("\"mode\":\"passthrough\""));
        assert!(json.contains("\"raw\":[97]"));
        assert!(json.contains("\"parsed\""));
    }

    #[test]
    fn test_mode_event_serialization() {
        let event = InputEvent::Mode {
            mode: Mode::Capture,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event\":\"mode\""));
        assert!(json.contains("\"mode\":\"capture\""));
    }

    #[test]
    fn test_broadcaster_clone_shares_channel() {
        let broadcaster1 = InputBroadcaster::new();
        let broadcaster2 = broadcaster1.clone();
        let mut rx = broadcaster1.subscribe();

        broadcaster2.broadcast_mode(Mode::Capture);

        let event = rx.try_recv().unwrap();
        match event {
            InputEvent::Mode { mode } => {
                assert_eq!(mode, Mode::Capture);
            }
            _ => panic!("Expected Mode event"),
        }
    }
}
