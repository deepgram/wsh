//! ANSI escape sequence rendering for panels.
//!
//! Renders panel content into specific row ranges outside the scroll region,
//! and manages DECSTBM scroll region boundaries.

use crate::overlay::{self, OverlaySpan, RegionWrite};

use super::layout::Layout;
use super::types::Panel;

/// Returns the DECSTBM escape sequence to set the scroll region.
///
/// `top` and `bottom` are 1-indexed row numbers.
pub fn set_scroll_region(top: u16, bottom: u16) -> String {
    format!("\x1b[{};{}r", top, bottom)
}

/// Returns the escape sequence to reset the scroll region to the full terminal.
pub fn reset_scroll_region() -> &'static str {
    "\x1b[r"
}

/// Render a single panel starting at `start_row` (0-indexed terminal row).
///
/// Rendering pipeline:
/// 1. Fill background (if background is set) for all rows
/// 2. Render span content for each row (spans are split on `\n`)
/// 3. Clear remaining columns with spaces up to `terminal_cols`
/// 4. Render region writes on top of everything
pub fn render_panel(panel: &Panel, start_row: u16, terminal_cols: u16) -> String {
    let mut result = String::new();

    // Step 1: Fill background if set
    if let Some(ref background) = panel.background {
        let bg_code = render_color(&background.bg, true);
        for row_offset in 0..panel.height {
            let row = start_row.saturating_add(row_offset);
            result.push_str(&overlay::cursor_position(row, 0));
            result.push_str(&bg_code);
            for _ in 0..terminal_cols {
                result.push(' ');
            }
            result.push_str(overlay::reset());
        }
    }

    // Step 2: Flatten all span text into lines, preserving style info.
    let mut text_lines: Vec<Vec<StyledSegment>> = vec![vec![]];

    for span in &panel.spans {
        let parts: Vec<&str> = span.text.split('\n').collect();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                text_lines.push(vec![]);
            }
            if !part.is_empty() {
                text_lines.last_mut().unwrap().push(StyledSegment {
                    text: part,
                    span,
                });
            }
        }
    }

    for (row_offset, segments) in text_lines.iter().enumerate() {
        if row_offset as u16 >= panel.height {
            break; // Don't render beyond panel height
        }

        let row = start_row.saturating_add(row_offset as u16);
        result.push_str(&overlay::cursor_position(row, 0));

        let mut col = 0u16;
        for seg in segments {
            result.push_str(&render_span_style(seg.span));
            result.push_str(seg.text);
            result.push_str(overlay::reset());
            col = col.saturating_add(seg.text.len().min(u16::MAX as usize) as u16);
        }

        // Clear remaining columns
        if col < terminal_cols {
            let remaining = terminal_cols - col;
            for _ in 0..remaining {
                result.push(' ');
            }
        }
    }

    // Clear any panel rows that had no content
    let rendered_rows = text_lines.len().min(panel.height as usize);
    for row_offset in rendered_rows..panel.height as usize {
        let row = start_row.saturating_add(row_offset as u16);
        result.push_str(&overlay::cursor_position(row, 0));
        for _ in 0..terminal_cols {
            result.push(' ');
        }
    }

    // Step 3: Render region writes
    for write in &panel.region_writes {
        let abs_row = start_row.saturating_add(write.row);
        let abs_col = write.col;
        result.push_str(&overlay::cursor_position(abs_row, abs_col));
        result.push_str(&render_region_write_style(write));
        result.push_str(&write.text);
        result.push_str(overlay::reset());
    }

    result
}

/// A text segment with a reference to its parent span for styling.
struct StyledSegment<'a> {
    text: &'a str,
    span: &'a OverlaySpan,
}

/// Render style attributes for a span (same logic as overlay rendering).
fn render_span_style(span: &OverlaySpan) -> String {
    let mut result = String::new();

    if span.bold {
        result.push_str("\x1b[1m");
    }
    if span.italic {
        result.push_str("\x1b[3m");
    }
    if span.underline {
        result.push_str("\x1b[4m");
    }

    if let Some(ref fg) = span.fg {
        result.push_str(&render_color(fg, false));
    }
    if let Some(ref bg) = span.bg {
        result.push_str(&render_color(bg, true));
    }

    result
}

/// Render style attributes for a region write.
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
        result.push_str(&render_color(fg, false));
    }
    if let Some(ref bg) = write.bg {
        result.push_str(&render_color(bg, true));
    }

    result
}

/// Render a color as an ANSI escape sequence.
fn render_color(color: &crate::overlay::Color, background: bool) -> String {
    match color {
        crate::overlay::Color::Named(named) => {
            let base = if background { 40 } else { 30 };
            let code = base + match named {
                crate::overlay::NamedColor::Black => 0,
                crate::overlay::NamedColor::Red => 1,
                crate::overlay::NamedColor::Green => 2,
                crate::overlay::NamedColor::Yellow => 3,
                crate::overlay::NamedColor::Blue => 4,
                crate::overlay::NamedColor::Magenta => 5,
                crate::overlay::NamedColor::Cyan => 6,
                crate::overlay::NamedColor::White => 7,
            };
            format!("\x1b[{}m", code)
        }
        crate::overlay::Color::Rgb { r, g, b } => {
            if background {
                format!("\x1b[48;2;{};{};{}m", r, g, b)
            } else {
                format!("\x1b[38;2;{};{};{}m", r, g, b)
            }
        }
    }
}

/// Render all visible panels from a computed layout.
///
/// Top panels render from row 0 downward (highest z at the edge).
/// Bottom panels render from scroll_region_bottom + 1 downward.
/// Wraps in save/restore cursor.
pub fn render_all_panels(layout: &Layout, terminal_cols: u16) -> String {
    let mut result = String::new();
    result.push_str(overlay::save_cursor());

    // Top panels: highest z is at row 0 (closest to edge)
    let mut row = 0u16;
    for panel in &layout.top_panels {
        result.push_str(&render_panel(panel, row, terminal_cols));
        row = row.saturating_add(panel.height);
    }

    // Bottom panels: highest z is closest to bottom edge
    // scroll_region_bottom is 1-indexed, panels start at next row
    let mut row = layout.scroll_region_bottom; // already 1-indexed, this is the 0-indexed next row
    for panel in &layout.bottom_panels {
        result.push_str(&render_panel(panel, row, terminal_cols));
        row = row.saturating_add(panel.height);
    }

    result.push_str(overlay::restore_cursor());
    result
}

/// Erase all panel rows by overwriting with spaces.
///
/// Clears both top and bottom panel regions.
pub fn erase_all_panels(layout: &Layout, terminal_cols: u16) -> String {
    let mut result = String::new();
    result.push_str(overlay::save_cursor());

    // Erase top panel rows
    let top_height: u16 = layout.top_panels.iter().map(|p| p.height).sum();
    for row in 0..top_height {
        result.push_str(&overlay::cursor_position(row, 0));
        for _ in 0..terminal_cols {
            result.push(' ');
        }
    }

    // Erase bottom panel rows
    let bottom_height: u16 = layout.bottom_panels.iter().map(|p| p.height).sum();
    let bottom_start = layout.scroll_region_bottom; // 0-indexed start of bottom panels
    for row in bottom_start..bottom_start.saturating_add(bottom_height) {
        result.push_str(&overlay::cursor_position(row, 0));
        for _ in 0..terminal_cols {
            result.push(' ');
        }
    }

    result.push_str(overlay::restore_cursor());
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::{BackgroundStyle, Color, NamedColor, OverlaySpan, RegionWrite, ScreenMode};
    use crate::panel::types::Position;

    fn span(text: &str) -> OverlaySpan {
        OverlaySpan {
            text: text.to_string(),
            id: None,
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }
    }

    fn make_panel(id: &str, position: Position, height: u16, z: i32) -> Panel {
        Panel {
            id: id.to_string(),
            position,
            height,
            z,
            background: None,
            spans: vec![],
            region_writes: vec![],
            visible: true,
            focusable: false,
            screen_mode: ScreenMode::Normal,
        }
    }

    #[test]
    fn test_set_scroll_region() {
        assert_eq!(set_scroll_region(3, 22), "\x1b[3;22r");
    }

    #[test]
    fn test_reset_scroll_region() {
        assert_eq!(reset_scroll_region(), "\x1b[r");
    }

    #[test]
    fn test_render_single_row_panel() {
        let panel = Panel {
            id: "t".to_string(),
            position: Position::Bottom,
            height: 1,
            z: 0,
            background: None,
            spans: vec![span("hello")],
            region_writes: vec![],
            visible: true,
            focusable: false,
            screen_mode: ScreenMode::Normal,
        };
        let result = render_panel(&panel, 23, 10);
        // Should position at row 23, col 0 (0-indexed -> \x1b[24;1H)
        assert!(result.contains("\x1b[24;1H"));
        assert!(result.contains("hello"));
        // After "hello" + reset, remaining 5 columns filled with spaces
        assert!(result.ends_with("     "));
    }

    #[test]
    fn test_render_multi_row_panel() {
        let panel = Panel {
            id: "t".to_string(),
            position: Position::Top,
            height: 2,
            z: 0,
            background: None,
            spans: vec![span("line1\nline2")],
            region_writes: vec![],
            visible: true,
            focusable: false,
            screen_mode: ScreenMode::Normal,
        };
        let result = render_panel(&panel, 0, 10);
        // Row 0: line1
        assert!(result.contains("\x1b[1;1H"));
        assert!(result.contains("line1"));
        // Row 1: line2
        assert!(result.contains("\x1b[2;1H"));
        assert!(result.contains("line2"));
    }

    #[test]
    fn test_render_panel_clears_empty_rows() {
        let panel = Panel {
            id: "t".to_string(),
            position: Position::Top,
            height: 3,
            z: 0,
            background: None,
            spans: vec![span("only one line")],
            region_writes: vec![],
            visible: true,
            focusable: false,
            screen_mode: ScreenMode::Normal,
        };
        let result = render_panel(&panel, 0, 20);
        // Should render content on row 0, then clear rows 1 and 2
        assert!(result.contains("\x1b[2;1H")); // row 1
        assert!(result.contains("\x1b[3;1H")); // row 2
    }

    #[test]
    fn test_render_styled_panel() {
        let panel = Panel {
            id: "t".to_string(),
            position: Position::Bottom,
            height: 1,
            z: 0,
            background: None,
            spans: vec![OverlaySpan {
                text: "error".to_string(),
                id: None,
                fg: Some(Color::Named(NamedColor::Red)),
                bg: None,
                bold: true,
                italic: false,
                underline: false,
            }],
            region_writes: vec![],
            visible: true,
            focusable: false,
            screen_mode: ScreenMode::Normal,
        };
        let result = render_panel(&panel, 23, 10);
        assert!(result.contains("\x1b[1m")); // bold
        assert!(result.contains("\x1b[31m")); // red fg
        assert!(result.contains("error"));
    }

    #[test]
    fn test_render_all_panels_empty_layout() {
        let layout = Layout {
            top_panels: vec![],
            bottom_panels: vec![],
            hidden_panels: vec![],
            scroll_region_top: 1,
            scroll_region_bottom: 24,
            pty_rows: 24,
            pty_cols: 80,
        };
        let result = render_all_panels(&layout, 80);
        // Just save + restore cursor
        assert_eq!(result, "\x1b[s\x1b[u");
    }

    #[test]
    fn test_erase_all_panels() {
        let layout = Layout {
            top_panels: vec![make_panel("t", Position::Top, 2, 0)],
            bottom_panels: vec![make_panel("b", Position::Bottom, 1, 0)],
            hidden_panels: vec![],
            scroll_region_top: 3,
            scroll_region_bottom: 23,
            pty_rows: 21,
            pty_cols: 80,
        };
        let result = erase_all_panels(&layout, 5); // small cols for easier testing
        // Should erase rows 0-1 (top) and row 23 (bottom)
        assert!(result.contains("\x1b[1;1H")); // row 0
        assert!(result.contains("\x1b[2;1H")); // row 1
        assert!(result.contains("\x1b[24;1H")); // row 23
    }

    // --- Tests for background fill and region writes ---

    #[test]
    fn test_render_panel_with_background() {
        let panel = Panel {
            id: "t".to_string(),
            position: Position::Bottom,
            height: 1,
            z: 0,
            background: Some(BackgroundStyle {
                bg: Color::Named(NamedColor::Blue),
            }),
            spans: vec![span("hello")],
            visible: true,
            region_writes: vec![],
            focusable: false,
            screen_mode: ScreenMode::Normal,
        };
        let result = render_panel(&panel, 23, 10);
        assert!(result.contains("\x1b[44m")); // blue bg
    }

    #[test]
    fn test_render_panel_with_region_writes() {
        let panel = Panel {
            id: "t".to_string(),
            position: Position::Bottom,
            height: 3,
            z: 0,
            background: None,
            spans: vec![],
            visible: true,
            region_writes: vec![RegionWrite {
                row: 1,
                col: 2,
                text: "bar".to_string(),
                fg: Some(Color::Named(NamedColor::Yellow)),
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            }],
            focusable: false,
            screen_mode: ScreenMode::Normal,
        };
        let result = render_panel(&panel, 10, 20);
        // row=10+1=11, col=2, 1-indexed: \x1b[12;3H
        assert!(result.contains("\x1b[12;3H"));
        assert!(result.contains("bar"));
        assert!(result.contains("\x1b[33m")); // yellow
    }
}
