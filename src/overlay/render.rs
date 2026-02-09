//! ANSI escape sequence rendering for overlays.
//!
//! Converts overlays to ANSI escape sequences for terminal rendering.

use super::types::{Color, NamedColor, Overlay, OverlaySpan};

/// Returns the ANSI escape sequence to save the cursor position.
pub fn save_cursor() -> &'static str {
    "\x1b[s"
}

/// Returns the ANSI escape sequence to restore the cursor position.
pub fn restore_cursor() -> &'static str {
    "\x1b[u"
}

/// Returns the ANSI escape sequence to position the cursor.
///
/// Takes 0-indexed row and column, converts to 1-indexed ANSI format.
pub fn cursor_position(row: u16, col: u16) -> String {
    format!("\x1b[{};{}H", row + 1, col + 1)
}

/// Returns the ANSI escape sequence to reset all attributes.
pub fn reset() -> &'static str {
    "\x1b[0m"
}

/// Converts a named color to its ANSI foreground code.
fn named_color_to_fg(color: &NamedColor) -> u8 {
    match color {
        NamedColor::Black => 30,
        NamedColor::Red => 31,
        NamedColor::Green => 32,
        NamedColor::Yellow => 33,
        NamedColor::Blue => 34,
        NamedColor::Magenta => 35,
        NamedColor::Cyan => 36,
        NamedColor::White => 37,
    }
}

/// Converts a named color to its ANSI background code (fg + 10).
fn named_color_to_bg(color: &NamedColor) -> u8 {
    named_color_to_fg(color) + 10
}

/// Renders a color as an ANSI escape sequence for foreground.
fn render_fg_color(color: &Color) -> String {
    match color {
        Color::Named(named) => format!("\x1b[{}m", named_color_to_fg(named)),
        Color::Rgb { r, g, b } => format!("\x1b[38;2;{};{};{}m", r, g, b),
    }
}

/// Renders a color as an ANSI escape sequence for background.
fn render_bg_color(color: &Color) -> String {
    match color {
        Color::Named(named) => format!("\x1b[{}m", named_color_to_bg(named)),
        Color::Rgb { r, g, b } => format!("\x1b[48;2;{};{};{}m", r, g, b),
    }
}

/// Renders the style attributes for a span as ANSI escape sequences.
fn render_span_style(span: &OverlaySpan) -> String {
    let mut result = String::new();

    // Text attributes
    if span.bold {
        result.push_str("\x1b[1m");
    }
    if span.italic {
        result.push_str("\x1b[3m");
    }
    if span.underline {
        result.push_str("\x1b[4m");
    }

    // Colors
    if let Some(ref fg) = span.fg {
        result.push_str(&render_fg_color(fg));
    }
    if let Some(ref bg) = span.bg {
        result.push_str(&render_bg_color(bg));
    }

    result
}

/// Renders a slice of overlay spans to an ANSI-escaped string.
///
/// Includes style codes for colors and attributes, ends with reset.
pub fn render_spans(spans: &[OverlaySpan]) -> String {
    let mut result = String::new();

    for span in spans {
        // Render style codes
        result.push_str(&render_span_style(span));
        // Render text
        result.push_str(&span.text);
        // Reset after each span to avoid style bleed
        result.push_str(reset());
    }

    // Ensure we end with a reset if there were any spans
    if !spans.is_empty() && !result.ends_with(reset()) {
        result.push_str(reset());
    }

    // Handle empty spans case - still need to end with reset
    if spans.is_empty() {
        result.push_str(reset());
    }

    result
}

/// Renders a single overlay with cursor positioning.
///
/// Handles newlines by repositioning the cursor to the next row.
pub fn render_overlay(overlay: &Overlay) -> String {
    let mut result = String::new();
    let mut current_row = overlay.y;

    // Position cursor at overlay start
    result.push_str(&cursor_position(current_row, overlay.x));

    for span in &overlay.spans {
        // Check for newlines in the text and handle cursor repositioning
        let lines: Vec<&str> = span.text.split('\n').collect();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                // Newline encountered, move to next row
                current_row += 1;
                result.push_str(&cursor_position(current_row, overlay.x));
            }

            // Create a temporary span for this line segment
            if !line.is_empty() {
                let line_span = OverlaySpan {
                    text: line.to_string(),
                    fg: span.fg.clone(),
                    bg: span.bg.clone(),
                    bold: span.bold,
                    italic: span.italic,
                    underline: span.underline,
                };
                result.push_str(&render_span_style(&line_span));
                result.push_str(line);
                result.push_str(reset());
            }
        }
    }

    result
}

/// Renders all overlays with cursor save/restore.
///
/// Saves cursor at the start, renders all overlays, restores cursor at the end.
pub fn render_all_overlays(overlays: &[Overlay]) -> String {
    let mut result = String::new();

    // Save cursor position
    result.push_str(save_cursor());

    // Render each overlay
    for overlay in overlays {
        result.push_str(&render_overlay(overlay));
    }

    // Restore cursor position
    result.push_str(restore_cursor());

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_plain_text() {
        let spans = vec![OverlaySpan {
            text: "Hello".to_string(),
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }];
        let result = render_spans(&spans);
        assert_eq!(result, "Hello\x1b[0m");
    }

    #[test]
    fn test_render_colored_text() {
        let spans = vec![OverlaySpan {
            text: "Error".to_string(),
            fg: Some(Color::Named(NamedColor::Red)),
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }];
        let result = render_spans(&spans);
        assert!(result.contains("\x1b[31m"), "Expected red foreground code");
        assert!(result.contains("Error"));
        assert!(result.ends_with("\x1b[0m"));
    }

    #[test]
    fn test_render_bold_text() {
        let spans = vec![OverlaySpan {
            text: "Important".to_string(),
            fg: None,
            bg: None,
            bold: true,
            italic: false,
            underline: false,
        }];
        let result = render_spans(&spans);
        assert!(result.contains("\x1b[1m"), "Expected bold code");
        assert!(result.contains("Important"));
        assert!(result.ends_with("\x1b[0m"));
    }

    #[test]
    fn test_cursor_position() {
        // 0-indexed (5, 10) should become 1-indexed (6, 11)
        let result = cursor_position(5, 10);
        assert_eq!(result, "\x1b[6;11H");
    }

    #[test]
    fn test_save_restore_cursor() {
        assert_eq!(save_cursor(), "\x1b[s");
        assert_eq!(restore_cursor(), "\x1b[u");
    }

    #[test]
    fn test_render_rgb_color() {
        let spans = vec![OverlaySpan {
            text: "Orange".to_string(),
            fg: Some(Color::Rgb {
                r: 255,
                g: 128,
                b: 0,
            }),
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }];
        let result = render_spans(&spans);
        assert!(
            result.contains("\x1b[38;2;255;128;0m"),
            "Expected RGB foreground code, got: {}",
            result
        );
        assert!(result.contains("Orange"));
        assert!(result.ends_with("\x1b[0m"));
    }

    #[test]
    fn test_render_overlay_with_position() {
        let overlay = Overlay {
            id: "test".to_string(),
            x: 10,
            y: 5,
            z: 0,
            spans: vec![OverlaySpan {
                text: "Status".to_string(),
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            }],
        };
        let result = render_overlay(&overlay);
        // y=5, x=10 (0-indexed) -> row=6, col=11 (1-indexed)
        assert!(
            result.starts_with("\x1b[6;11H"),
            "Expected cursor position at start, got: {}",
            result
        );
    }

    #[test]
    fn test_render_background_color() {
        let spans = vec![OverlaySpan {
            text: "Highlight".to_string(),
            fg: None,
            bg: Some(Color::Named(NamedColor::Yellow)),
            bold: false,
            italic: false,
            underline: false,
        }];
        let result = render_spans(&spans);
        assert!(
            result.contains("\x1b[43m"),
            "Expected yellow background code (43), got: {}",
            result
        );
    }

    #[test]
    fn test_render_italic_text() {
        let spans = vec![OverlaySpan {
            text: "Emphasis".to_string(),
            fg: None,
            bg: None,
            bold: false,
            italic: true,
            underline: false,
        }];
        let result = render_spans(&spans);
        assert!(
            result.contains("\x1b[3m"),
            "Expected italic code, got: {}",
            result
        );
    }

    #[test]
    fn test_render_underline_text() {
        let spans = vec![OverlaySpan {
            text: "Link".to_string(),
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: true,
        }];
        let result = render_spans(&spans);
        assert!(
            result.contains("\x1b[4m"),
            "Expected underline code, got: {}",
            result
        );
    }

    #[test]
    fn test_render_multiple_spans() {
        let spans = vec![
            OverlaySpan {
                text: "Normal ".to_string(),
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            },
            OverlaySpan {
                text: "Bold".to_string(),
                fg: None,
                bg: None,
                bold: true,
                italic: false,
                underline: false,
            },
        ];
        let result = render_spans(&spans);
        assert!(result.contains("Normal "));
        assert!(result.contains("Bold"));
        assert!(result.ends_with("\x1b[0m"));
    }

    #[test]
    fn test_render_all_overlays() {
        let overlays = vec![Overlay {
            id: "test".to_string(),
            x: 0,
            y: 0,
            z: 0,
            spans: vec![OverlaySpan {
                text: "Test".to_string(),
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            }],
        }];
        let result = render_all_overlays(&overlays);
        assert!(
            result.starts_with("\x1b[s"),
            "Expected save cursor at start"
        );
        assert!(result.ends_with("\x1b[u"), "Expected restore cursor at end");
    }

    #[test]
    fn test_reset() {
        assert_eq!(reset(), "\x1b[0m");
    }

    #[test]
    fn test_render_rgb_background() {
        let spans = vec![OverlaySpan {
            text: "Custom".to_string(),
            fg: None,
            bg: Some(Color::Rgb {
                r: 100,
                g: 150,
                b: 200,
            }),
            bold: false,
            italic: false,
            underline: false,
        }];
        let result = render_spans(&spans);
        assert!(
            result.contains("\x1b[48;2;100;150;200m"),
            "Expected RGB background code, got: {}",
            result
        );
    }

    #[test]
    fn test_render_combined_attributes() {
        let spans = vec![OverlaySpan {
            text: "Fancy".to_string(),
            fg: Some(Color::Named(NamedColor::Green)),
            bg: Some(Color::Named(NamedColor::Black)),
            bold: true,
            italic: true,
            underline: true,
        }];
        let result = render_spans(&spans);
        // Should contain all attributes
        assert!(result.contains("\x1b[32m"), "Expected green foreground");
        assert!(result.contains("\x1b[40m"), "Expected black background");
        assert!(result.contains("\x1b[1m"), "Expected bold");
        assert!(result.contains("\x1b[3m"), "Expected italic");
        assert!(result.contains("\x1b[4m"), "Expected underline");
        assert!(result.contains("Fancy"));
        assert!(result.ends_with("\x1b[0m"));
    }
}
