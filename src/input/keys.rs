//! Key parsing for input events.
//!
//! Parses raw bytes into structured key events with modifiers.
//! Used for interpreting keyboard input from various sources.

use serde::Serialize;

/// A parsed key event with optional modifiers.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ParsedKey {
    /// The key that was pressed, if recognized.
    pub key: Option<String>,
    /// Modifiers active during the key press (e.g., "ctrl", "alt").
    pub modifiers: Vec<String>,
}

impl ParsedKey {
    /// Creates a new ParsedKey with the given key and no modifiers.
    pub fn new(key: Option<String>) -> Self {
        Self {
            key,
            modifiers: Vec::new(),
        }
    }

    /// Creates a new ParsedKey with the given key and modifiers.
    pub fn with_modifiers(key: Option<String>, modifiers: Vec<String>) -> Self {
        Self { key, modifiers }
    }
}

/// Checks if the given data represents Ctrl+\ (the escape hatch).
///
/// Ctrl+\ is represented by byte 0x1c.
pub fn is_ctrl_backslash(data: &[u8]) -> bool {
    data == [0x1c]
}

/// Parses raw bytes into a structured key event.
///
/// # Key parsing rules:
/// - Empty input -> key: None, modifiers: []
/// - Control chars 0x01-0x1a -> key: char (a-z), modifiers: ["ctrl"]
/// - 0x1c -> key: "\\", modifiers: ["ctrl"] (Ctrl+\)
/// - 0x1d -> key: "]", modifiers: ["ctrl"]
/// - 0x1e -> key: "^", modifiers: ["ctrl"]
/// - 0x1f -> key: "_", modifiers: ["ctrl"]
/// - 0x1b (single) -> key: "Escape"
/// - 0x09 -> key: "Tab"
/// - 0x0d -> key: "Enter"
/// - 0x7f -> key: "Backspace"
/// - 0x20-0x7e -> printable char
/// - ESC [ A -> "ArrowUp"
/// - ESC [ B -> "ArrowDown"
/// - ESC [ C -> "ArrowRight"
/// - ESC [ D -> "ArrowLeft"
/// - ESC [ H -> "Home"
/// - ESC [ F -> "End"
/// - Unknown -> key: None
pub fn parse_key(data: &[u8]) -> ParsedKey {
    if data.is_empty() {
        return ParsedKey::new(None);
    }

    // Check for escape sequences (ESC [ ...)
    if data.len() >= 3 && data[0] == 0x1b && data[1] == b'[' {
        let key = match data[2] {
            b'A' => Some("ArrowUp".to_string()),
            b'B' => Some("ArrowDown".to_string()),
            b'C' => Some("ArrowRight".to_string()),
            b'D' => Some("ArrowLeft".to_string()),
            b'H' => Some("Home".to_string()),
            b'F' => Some("End".to_string()),
            _ => None,
        };
        return ParsedKey::new(key);
    }

    // Single byte handling
    if data.len() == 1 {
        let byte = data[0];
        match byte {
            // Tab (0x09)
            0x09 => ParsedKey::new(Some("Tab".to_string())),
            // Enter (0x0d)
            0x0d => ParsedKey::new(Some("Enter".to_string())),
            // Escape (0x1b) - single byte only (escape sequences handled above)
            0x1b => ParsedKey::new(Some("Escape".to_string())),
            // Control characters Ctrl+A through Ctrl+Z (0x01-0x1a), excluding special cases
            0x01..=0x1a => {
                // Map control char to letter: 0x01 -> 'a', 0x03 -> 'c', etc.
                let ch = (byte - 1 + b'a') as char;
                ParsedKey::with_modifiers(Some(ch.to_string()), vec!["ctrl".to_string()])
            }
            // Ctrl+\ (0x1c)
            0x1c => {
                ParsedKey::with_modifiers(Some("\\".to_string()), vec!["ctrl".to_string()])
            }
            // Ctrl+] (0x1d)
            0x1d => {
                ParsedKey::with_modifiers(Some("]".to_string()), vec!["ctrl".to_string()])
            }
            // Ctrl+^ (0x1e)
            0x1e => {
                ParsedKey::with_modifiers(Some("^".to_string()), vec!["ctrl".to_string()])
            }
            // Ctrl+_ (0x1f)
            0x1f => {
                ParsedKey::with_modifiers(Some("_".to_string()), vec!["ctrl".to_string()])
            }
            // Backspace (0x7f)
            0x7f => ParsedKey::new(Some("Backspace".to_string())),
            // Printable ASCII (0x20-0x7e)
            0x20..=0x7e => ParsedKey::new(Some((byte as char).to_string())),
            // Unknown
            _ => ParsedKey::new(None),
        }
    } else {
        // Multi-byte sequences we don't recognize
        ParsedKey::new(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_printable_char() {
        let result = parse_key(b"a");
        assert_eq!(result.key, Some("a".to_string()));
        assert!(result.modifiers.is_empty());
    }

    #[test]
    fn test_parse_ctrl_c() {
        // Ctrl+C is 0x03
        let result = parse_key(&[0x03]);
        assert_eq!(result.key, Some("c".to_string()));
        assert_eq!(result.modifiers, vec!["ctrl".to_string()]);
    }

    #[test]
    fn test_parse_escape() {
        // Escape is 0x1b
        let result = parse_key(&[0x1b]);
        assert_eq!(result.key, Some("Escape".to_string()));
        assert!(result.modifiers.is_empty());
    }

    #[test]
    fn test_parse_arrow_up() {
        // Arrow up is ESC [ A
        let result = parse_key(&[0x1b, b'[', b'A']);
        assert_eq!(result.key, Some("ArrowUp".to_string()));
        assert!(result.modifiers.is_empty());
    }

    #[test]
    fn test_parse_ctrl_backslash() {
        // Ctrl+\ is 0x1c
        let result = parse_key(&[0x1c]);
        assert_eq!(result.key, Some("\\".to_string()));
        assert_eq!(result.modifiers, vec!["ctrl".to_string()]);
    }

    #[test]
    fn test_is_ctrl_backslash() {
        assert!(is_ctrl_backslash(&[0x1c]));
        assert!(!is_ctrl_backslash(&[0x03])); // Ctrl+C
        assert!(!is_ctrl_backslash(&[0x1c, 0x00])); // Extra byte
        assert!(!is_ctrl_backslash(&[])); // Empty
    }
}
