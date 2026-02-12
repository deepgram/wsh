use serde::{Deserialize, Serialize};

use crate::overlay::OverlaySpan;

/// Unique identifier for a panel
pub type PanelId = String;

/// Edge of the terminal where a panel is anchored
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    Top,
    Bottom,
}

/// A panel that carves out dedicated rows at the top or bottom of the terminal.
///
/// Unlike overlays (which draw on top of PTY content), panels shrink the PTY
/// viewport so that programs never write into panel space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Panel {
    pub id: PanelId,
    pub position: Position,
    pub height: u16,
    pub z: i32,
    pub spans: Vec<OverlaySpan>,
    pub visible: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_serialization() {
        assert_eq!(serde_json::to_string(&Position::Top).unwrap(), "\"top\"");
        assert_eq!(
            serde_json::to_string(&Position::Bottom).unwrap(),
            "\"bottom\""
        );
    }

    #[test]
    fn test_position_deserialization() {
        let top: Position = serde_json::from_str("\"top\"").unwrap();
        assert_eq!(top, Position::Top);
        let bottom: Position = serde_json::from_str("\"bottom\"").unwrap();
        assert_eq!(bottom, Position::Bottom);
    }

    #[test]
    fn test_panel_serde_round_trip() {
        let panel = Panel {
            id: "test-id".to_string(),
            position: Position::Bottom,
            height: 2,
            z: 5,
            spans: vec![OverlaySpan {
                text: "status".to_string(),
                id: None,
                fg: None,
                bg: None,
                bold: true,
                italic: false,
                underline: false,
            }],
            visible: true,
        };
        let json = serde_json::to_string(&panel).unwrap();
        let deserialized: Panel = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "test-id");
        assert_eq!(deserialized.position, Position::Bottom);
        assert_eq!(deserialized.height, 2);
        assert_eq!(deserialized.z, 5);
        assert!(deserialized.visible);
        assert_eq!(deserialized.spans.len(), 1);
        assert_eq!(deserialized.spans[0].text, "status");
    }

    #[test]
    fn test_panel_visible_always_serialized() {
        let panel = Panel {
            id: "t".to_string(),
            position: Position::Top,
            height: 1,
            z: 0,
            spans: vec![],
            visible: false,
        };
        let json = serde_json::to_string(&panel).unwrap();
        assert!(json.contains("\"visible\":false"));
    }
}
