//! ANSI escape sequence rendering for overlays.
//!
//! Converts overlays to ANSI escape sequences for terminal rendering.

use super::types::{Color, NamedColor, Overlay, OverlaySpan, RegionWrite};

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
    format!("\x1b[{};{}H", row.saturating_add(1), col.saturating_add(1))
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

/// Renders the style attributes for a region write as ANSI escape sequences.
fn render_region_write_style(write: &RegionWrite) -> String {
    let mut result = String::new();

    if write.bold {
        result.push_str("\x1b[1m");
    }
    if write.italic {
        result.push_str("\x1b[3m");
    }
    if write.underline {
        result.push_str("\x1b[4m");
    }

    if let Some(ref fg) = write.fg {
        result.push_str(&render_fg_color(fg));
    }
    if let Some(ref bg) = write.bg {
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
/// Rendering pipeline:
/// 1. Fill background rectangle (if background is set)
/// 2. Render spans on top
/// 3. Render region writes on top of everything
pub fn render_overlay(overlay: &Overlay) -> String {
    let mut result = String::new();

    // Step 1: Fill background rectangle if set
    if let Some(ref background) = overlay.background {
        let bg_code = render_bg_color(&background.bg);
        for row_offset in 0..overlay.height {
            result.push_str(&cursor_position(overlay.y.saturating_add(row_offset), overlay.x));
            result.push_str(&bg_code);
            for _ in 0..overlay.width {
                result.push(' ');
            }
            result.push_str(reset());
        }
    }

    // Step 2: Render spans
    let mut current_row = overlay.y;
    result.push_str(&cursor_position(current_row, overlay.x));

    for span in &overlay.spans {
        // Check for newlines in the text and handle cursor repositioning
        let lines: Vec<&str> = span.text.split('\n').collect();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                // Newline encountered, move to next row
                current_row = current_row.saturating_add(1);
                result.push_str(&cursor_position(current_row, overlay.x));
            }

            // Create a temporary span for this line segment
            if !line.is_empty() {
                let line_span = OverlaySpan {
                    text: line.to_string(),
                    id: None,
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

    // Step 3: Render region writes
    for write in &overlay.region_writes {
        let abs_row = overlay.y.saturating_add(write.row);
        let abs_col = overlay.x.saturating_add(write.col);
        result.push_str(&cursor_position(abs_row, abs_col));
        result.push_str(&render_region_write_style(write));
        result.push_str(&write.text);
        result.push_str(reset());
    }

    result
}

/// Returns the synchronized output begin sequence (DEC private mode 2026).
///
/// Tells the terminal to buffer subsequent output until `end_sync()` is received,
/// then apply it atomically. Prevents visual tearing during erase+render cycles.
pub fn begin_sync() -> &'static str {
    "\x1b[?2026h"
}

/// Returns the synchronized output end sequence (DEC private mode 2026).
pub fn end_sync() -> &'static str {
    "\x1b[?2026l"
}

/// Returns `(row, col, width)` for each visual line of an overlay.
///
/// Replicates the newline-splitting logic from `render_overlay` but only computes
/// geometry. Uses `len()` for width (ASCII approximation).
pub fn overlay_line_extents(overlay: &Overlay) -> Vec<(u16, u16, u16)> {
    let mut extents = Vec::new();
    let mut current_row = overlay.y;
    let mut current_width: u16 = 0;
    let mut line_started = false;

    for span in &overlay.spans {
        let lines: Vec<&str> = span.text.split('\n').collect();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                // Newline boundary: flush the current line
                if line_started {
                    extents.push((current_row, overlay.x, current_width));
                }
                current_row = current_row.saturating_add(1);
                current_width = 0;
                line_started = false;
            }
            if !line.is_empty() {
                current_width = current_width.saturating_add(line.len().min(u16::MAX as usize) as u16);
                line_started = true;
            }
        }
    }

    // Flush the last line
    if line_started {
        extents.push((current_row, overlay.x, current_width));
    }

    extents
}

/// Generates ANSI sequences to erase a single overlay by overwriting with spaces.
///
/// Uses the overlay's explicit `width` and `height` dimensions to erase the full
/// bounding rectangle, since backgrounds and region writes can extend beyond spans.
pub fn erase_overlay(overlay: &Overlay) -> String {
    let mut result = String::new();
    let w = overlay.width as usize;
    let spaces: String = " ".repeat(w);
    for row_offset in 0..overlay.height {
        result.push_str(&cursor_position(overlay.y.saturating_add(row_offset), overlay.x));
        result.push_str(&spaces);
    }
    result
}

/// Generates ANSI sequences to erase all overlays, wrapped in cursor save/restore.
pub fn erase_all_overlays(overlays: &[Overlay]) -> String {
    let mut result = String::new();
    result.push_str(save_cursor());
    for overlay in overlays {
        result.push_str(&erase_overlay(overlay));
    }
    result.push_str(restore_cursor());
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
    use super::super::types::{BackgroundStyle, ScreenMode};

    #[test]
    fn test_render_plain_text() {
        let spans = vec![OverlaySpan {
            text: "Hello".to_string(),
            id: None,
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
            id: None,
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
            id: None,
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
            id: None,
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
            width: 80,
            height: 1,
            background: None,
            spans: vec![OverlaySpan {
                text: "Status".to_string(),
                id: None,
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            }],
            region_writes: vec![],
            focusable: false,
            screen_mode: ScreenMode::Normal,
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
            id: None,
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
            id: None,
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
            id: None,
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
                id: None,
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            },
            OverlaySpan {
                text: "Bold".to_string(),
                id: None,
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
            width: 80,
            height: 1,
            background: None,
            spans: vec![OverlaySpan {
                text: "Test".to_string(),
                id: None,
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            }],
            region_writes: vec![],
            focusable: false,
            screen_mode: ScreenMode::Normal,
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
            id: None,
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
            id: None,
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

    // --- Tests for new sync/erase functions ---

    #[test]
    fn test_begin_sync() {
        assert_eq!(begin_sync(), "\x1b[?2026h");
    }

    #[test]
    fn test_end_sync() {
        assert_eq!(end_sync(), "\x1b[?2026l");
    }

    #[test]
    fn test_overlay_line_extents_single_line() {
        let overlay = Overlay {
            id: "t".to_string(),
            x: 5,
            y: 3,
            z: 0,
            width: 80,
            height: 1,
            background: None,
            spans: vec![OverlaySpan {
                text: "hello".to_string(),
                id: None,
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            }],
            region_writes: vec![],
            focusable: false,
            screen_mode: ScreenMode::Normal,
        };
        let extents = overlay_line_extents(&overlay);
        assert_eq!(extents, vec![(3, 5, 5)]);
    }

    #[test]
    fn test_overlay_line_extents_multiline() {
        let overlay = Overlay {
            id: "t".to_string(),
            x: 0,
            y: 0,
            z: 0,
            width: 80,
            height: 3,
            background: None,
            spans: vec![OverlaySpan {
                text: "ab\ncde\nf".to_string(),
                id: None,
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            }],
            region_writes: vec![],
            focusable: false,
            screen_mode: ScreenMode::Normal,
        };
        let extents = overlay_line_extents(&overlay);
        assert_eq!(extents, vec![(0, 0, 2), (1, 0, 3), (2, 0, 1)]);
    }

    #[test]
    fn test_overlay_line_extents_multi_span() {
        let overlay = Overlay {
            id: "t".to_string(),
            x: 2,
            y: 1,
            z: 0,
            width: 80,
            height: 1,
            background: None,
            spans: vec![
                OverlaySpan {
                    text: "ab".to_string(),
                    id: None,
                    fg: None,
                    bg: None,
                    bold: false,
                    italic: false,
                    underline: false,
                },
                OverlaySpan {
                    text: "cd".to_string(),
                    id: None,
                    fg: None,
                    bg: None,
                    bold: false,
                    italic: false,
                    underline: false,
                },
            ],
            region_writes: vec![],
            focusable: false,
            screen_mode: ScreenMode::Normal,
        };
        let extents = overlay_line_extents(&overlay);
        // Two spans on same line: width = 2 + 2 = 4
        assert_eq!(extents, vec![(1, 2, 4)]);
    }

    #[test]
    fn test_overlay_line_extents_newline_at_span_boundary() {
        let overlay = Overlay {
            id: "t".to_string(),
            x: 0,
            y: 0,
            z: 0,
            width: 80,
            height: 2,
            background: None,
            spans: vec![
                OverlaySpan {
                    text: "ab\n".to_string(),
                    id: None,
                    fg: None,
                    bg: None,
                    bold: false,
                    italic: false,
                    underline: false,
                },
                OverlaySpan {
                    text: "cd".to_string(),
                    id: None,
                    fg: None,
                    bg: None,
                    bold: false,
                    italic: false,
                    underline: false,
                },
            ],
            region_writes: vec![],
            focusable: false,
            screen_mode: ScreenMode::Normal,
        };
        let extents = overlay_line_extents(&overlay);
        // First span: "ab\n" -> line "ab" (width 2), then newline
        // Second span: "cd" -> continues on the new row (width 2)
        assert_eq!(extents, vec![(0, 0, 2), (1, 0, 2)]);
    }

    #[test]
    fn test_erase_overlay_output() {
        let overlay = Overlay {
            id: "t".to_string(),
            x: 5,
            y: 3,
            z: 0,
            width: 10,
            height: 1,
            background: None,
            spans: vec![OverlaySpan {
                text: "hello".to_string(),
                id: None,
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            }],
            region_writes: vec![],
            focusable: false,
            screen_mode: ScreenMode::Normal,
        };
        let result = erase_overlay(&overlay);
        // Should erase full width=10 rectangle at (3,5) -> \x1b[4;6H then 10 spaces
        assert_eq!(result, format!("\x1b[4;6H{}", " ".repeat(10)));
    }

    #[test]
    fn test_erase_overlay_multiline() {
        let overlay = Overlay {
            id: "t".to_string(),
            x: 0,
            y: 0,
            z: 0,
            width: 20,
            height: 2,
            background: None,
            spans: vec![OverlaySpan {
                text: "ab\ncde".to_string(),
                id: None,
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            }],
            region_writes: vec![],
            focusable: false,
            screen_mode: ScreenMode::Normal,
        };
        let result = erase_overlay(&overlay);
        let spaces = " ".repeat(20);
        // Row 0 and row 1 both erased with full width=20
        assert_eq!(result, format!("\x1b[1;1H{spaces}\x1b[2;1H{spaces}"));
    }

    #[test]
    fn test_erase_all_overlays_empty() {
        let result = erase_all_overlays(&[]);
        // Just save + restore cursor, no erase sequences
        assert_eq!(result, "\x1b[s\x1b[u");
    }

    // --- Tests for background fill and region writes ---

    #[test]
    fn test_render_overlay_with_background_fills_rectangle() {
        let overlay = Overlay {
            id: "t".to_string(),
            x: 0,
            y: 0,
            z: 0,
            width: 5,
            height: 2,
            background: Some(BackgroundStyle {
                bg: Color::Rgb {
                    r: 30,
                    g: 30,
                    b: 30,
                },
            }),
            spans: vec![],
            region_writes: vec![],
            focusable: false,
            screen_mode: ScreenMode::Normal,
        };
        let result = render_overlay(&overlay);
        // Should contain background color
        assert!(result.contains("\x1b[48;2;30;30;30m"));
        // Should position cursor for row 0 and row 1
        assert!(result.contains("\x1b[1;1H"));
        assert!(result.contains("\x1b[2;1H"));
    }

    #[test]
    fn test_render_overlay_with_region_writes() {
        let overlay = Overlay {
            id: "t".to_string(),
            x: 10,
            y: 5,
            z: 0,
            width: 20,
            height: 3,
            background: None,
            spans: vec![],
            region_writes: vec![RegionWrite {
                row: 1,
                col: 5,
                text: "hello".to_string(),
                fg: Some(Color::Named(NamedColor::Green)),
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            }],
            focusable: false,
            screen_mode: ScreenMode::Normal,
        };
        let result = render_overlay(&overlay);
        // Region write at (1, 5) within overlay at (10, 5)
        // Absolute: row=5+1=6, col=10+5=15, 1-indexed: \x1b[7;16H
        assert!(result.contains("\x1b[7;16H"));
        assert!(result.contains("hello"));
        assert!(result.contains("\x1b[32m")); // green fg
    }
}
