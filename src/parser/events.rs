use serde::{Deserialize, Serialize};

use super::state::{FormattedLine, ScreenResponse};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Line {
        seq: u64,
        index: usize,
        total_lines: usize,
        line: FormattedLine,
    },
    Cursor {
        seq: u64,
        row: usize,
        col: usize,
        visible: bool,
    },
    Mode {
        seq: u64,
        alternate_active: bool,
    },
    Reset {
        seq: u64,
        reason: ResetReason,
    },
    Sync {
        seq: u64,
        screen: ScreenResponse,
        scrollback_lines: usize,
    },
    Diff {
        seq: u64,
        changed_lines: Vec<usize>,
        screen: ScreenResponse,
    },
    Idle {
        seq: u64,
        generation: u64,
        screen: ScreenResponse,
        scrollback_lines: usize,
    },
    Running {
        seq: u64,
        generation: u64,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetReason {
    ClearScreen,
    ClearScrollback,
    HardReset,
    AlternateScreenEnter,
    AlternateScreenExit,
    Resize,
    /// The parser task panicked and was restarted with fresh VT state.
    /// Clients should re-query screen/scrollback as all prior state is lost.
    ParserRestart,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Subscribe {
    pub events: Vec<EventType>,
    #[serde(default = "default_interval")]
    pub interval_ms: u64,
    #[serde(default)]
    pub format: super::state::Format,
}

fn default_interval() -> u64 {
    100
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    Lines,
    Chars,
    Cursor,
    Mode,
    Diffs,
    Input,
    Overlay,
    Activity,
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::state::Cursor as CursorState;

    #[test]
    fn idle_event_serializes_correctly() {
        let event = Event::Idle {
            seq: 42,
            generation: 7,
            screen: ScreenResponse {
                epoch: 0,
                first_line_index: 0,
                total_lines: 100,
                lines: vec![],
                cursor: CursorState {
                    row: 0,
                    col: 0,
                    visible: true,
                },
                cols: 80,
                rows: 24,
                alternate_active: false,
            },
            scrollback_lines: 100,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event"], "idle");
        assert_eq!(json["seq"], 42);
        assert_eq!(json["generation"], 7);
        assert!(json["screen"].is_object());
        assert_eq!(json["scrollback_lines"], 100);
    }

    #[test]
    fn running_event_serializes_correctly() {
        let event = Event::Running {
            seq: 10,
            generation: 3,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event"], "running");
        assert_eq!(json["seq"], 10);
        assert_eq!(json["generation"], 3);
    }

    #[test]
    fn activity_event_type_deserializes() {
        let json = r#""activity""#;
        let et: EventType = serde_json::from_str(json).unwrap();
        assert_eq!(et, EventType::Activity);
    }
}
