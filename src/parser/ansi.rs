//! Convert parser state types (Span, Style, Color) to raw ANSI escape sequences.
//!
//! Used by the attach handler to replay styled screen/scrollback content
//! to reconnecting clients.

use super::state::{Color, FormattedLine, Span, Style};

/// Convert a `FormattedLine` to a string containing ANSI escape sequences.
pub fn line_to_ansi(line: &FormattedLine) -> String {
    match line {
        FormattedLine::Plain(text) => text.clone(),
        FormattedLine::Styled(spans) => spans_to_ansi(spans),
    }
}

/// Convert a slice of `Span`s to an ANSI-styled string.
fn spans_to_ansi(spans: &[Span]) -> String {
    let mut buf = String::new();
    for span in spans {
        if span.style.is_default() {
            buf.push_str(&span.text);
        } else {
            let sgr = style_to_sgr(&span.style);
            buf.push_str(&format!("\x1b[{}m", sgr));
            buf.push_str(&span.text);
            buf.push_str("\x1b[0m");
        }
    }
    buf
}

/// Build the SGR parameter string from a `Style`.
///
/// Returns the semicolon-separated parameter list (without the `\x1b[` prefix
/// or `m` suffix).
fn style_to_sgr(style: &Style) -> String {
    let mut params = Vec::new();

    if style.bold {
        params.push("1".to_string());
    }
    if style.faint {
        params.push("2".to_string());
    }
    if style.italic {
        params.push("3".to_string());
    }
    if style.underline {
        params.push("4".to_string());
    }
    if style.blink {
        params.push("5".to_string());
    }
    if style.inverse {
        params.push("7".to_string());
    }
    if style.strikethrough {
        params.push("9".to_string());
    }

    if let Some(ref fg) = style.fg {
        params.push(color_to_sgr(fg, true));
    }
    if let Some(ref bg) = style.bg {
        params.push(color_to_sgr(bg, false));
    }

    params.join(";")
}

/// Convert a `Color` to its SGR parameter string.
///
/// `is_fg` selects foreground (30-37, 90-97, 38;5;N, 38;2;r;g;b) vs
/// background (40-47, 100-107, 48;5;N, 48;2;r;g;b) codes.
fn color_to_sgr(color: &Color, is_fg: bool) -> String {
    match color {
        Color::Indexed(idx) => {
            let idx = *idx;
            match idx {
                0..=7 => {
                    let base = if is_fg { 30 } else { 40 };
                    format!("{}", base + idx as u16)
                }
                8..=15 => {
                    let base = if is_fg { 90 } else { 100 };
                    format!("{}", base + (idx - 8) as u16)
                }
                _ => {
                    let prefix = if is_fg { 38 } else { 48 };
                    format!("{};5;{}", prefix, idx)
                }
            }
        }
        Color::Rgb { r, g, b } => {
            let prefix = if is_fg { 38 } else { 48 };
            format!("{};2;{};{};{}", prefix, r, g, b)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_style_produces_no_sgr() {
        let span = Span {
            text: "hello".to_string(),
            style: Style::default(),
        };
        let result = spans_to_ansi(&[span]);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_bold_attribute() {
        let style = Style { bold: true, ..Style::default() };
        let sgr = style_to_sgr(&style);
        assert_eq!(sgr, "1");
    }

    #[test]
    fn test_faint_attribute() {
        let style = Style { faint: true, ..Style::default() };
        let sgr = style_to_sgr(&style);
        assert_eq!(sgr, "2");
    }

    #[test]
    fn test_italic_attribute() {
        let style = Style { italic: true, ..Style::default() };
        let sgr = style_to_sgr(&style);
        assert_eq!(sgr, "3");
    }

    #[test]
    fn test_underline_attribute() {
        let style = Style { underline: true, ..Style::default() };
        let sgr = style_to_sgr(&style);
        assert_eq!(sgr, "4");
    }

    #[test]
    fn test_blink_attribute() {
        let style = Style { blink: true, ..Style::default() };
        let sgr = style_to_sgr(&style);
        assert_eq!(sgr, "5");
    }

    #[test]
    fn test_inverse_attribute() {
        let style = Style { inverse: true, ..Style::default() };
        let sgr = style_to_sgr(&style);
        assert_eq!(sgr, "7");
    }

    #[test]
    fn test_strikethrough_attribute() {
        let style = Style { strikethrough: true, ..Style::default() };
        let sgr = style_to_sgr(&style);
        assert_eq!(sgr, "9");
    }

    #[test]
    fn test_indexed_color_0_to_7_fg() {
        assert_eq!(color_to_sgr(&Color::Indexed(0), true), "30");
        assert_eq!(color_to_sgr(&Color::Indexed(1), true), "31");
        assert_eq!(color_to_sgr(&Color::Indexed(7), true), "37");
    }

    #[test]
    fn test_indexed_color_0_to_7_bg() {
        assert_eq!(color_to_sgr(&Color::Indexed(0), false), "40");
        assert_eq!(color_to_sgr(&Color::Indexed(7), false), "47");
    }

    #[test]
    fn test_indexed_color_8_to_15_fg() {
        assert_eq!(color_to_sgr(&Color::Indexed(8), true), "90");
        assert_eq!(color_to_sgr(&Color::Indexed(9), true), "91");
        assert_eq!(color_to_sgr(&Color::Indexed(15), true), "97");
    }

    #[test]
    fn test_indexed_color_8_to_15_bg() {
        assert_eq!(color_to_sgr(&Color::Indexed(8), false), "100");
        assert_eq!(color_to_sgr(&Color::Indexed(15), false), "107");
    }

    #[test]
    fn test_indexed_color_256_fg() {
        assert_eq!(color_to_sgr(&Color::Indexed(16), true), "38;5;16");
        assert_eq!(color_to_sgr(&Color::Indexed(255), true), "38;5;255");
    }

    #[test]
    fn test_indexed_color_256_bg() {
        assert_eq!(color_to_sgr(&Color::Indexed(128), false), "48;5;128");
    }

    #[test]
    fn test_rgb_color_fg() {
        assert_eq!(
            color_to_sgr(&Color::Rgb { r: 255, g: 128, b: 0 }, true),
            "38;2;255;128;0"
        );
    }

    #[test]
    fn test_rgb_color_bg() {
        assert_eq!(
            color_to_sgr(&Color::Rgb { r: 0, g: 0, b: 0 }, false),
            "48;2;0;0;0"
        );
    }

    #[test]
    fn test_combined_attributes() {
        let style = Style {
            bold: true,
            italic: true,
            fg: Some(Color::Indexed(1)),
            ..Style::default()
        };
        let sgr = style_to_sgr(&style);
        assert_eq!(sgr, "1;3;31");
    }

    #[test]
    fn test_line_to_ansi_plain() {
        let line = FormattedLine::Plain("plain text".to_string());
        assert_eq!(line_to_ansi(&line), "plain text");
    }

    #[test]
    fn test_line_to_ansi_styled() {
        let spans = vec![
            Span {
                text: "normal ".to_string(),
                style: Style::default(),
            },
            Span {
                text: "bold".to_string(),
                style: Style { bold: true, ..Style::default() },
            },
        ];
        let line = FormattedLine::Styled(spans);
        let result = line_to_ansi(&line);
        assert_eq!(result, "normal \x1b[1mbold\x1b[0m");
    }

    #[test]
    fn test_styled_span_with_default_style_no_sgr() {
        let spans = vec![Span {
            text: "text".to_string(),
            style: Style::default(),
        }];
        let result = spans_to_ansi(&spans);
        assert_eq!(result, "text");
    }

    #[test]
    fn test_full_round_trip() {
        let spans = vec![
            Span {
                text: "red".to_string(),
                style: Style {
                    fg: Some(Color::Indexed(1)),
                    bold: true,
                    ..Style::default()
                },
            },
            Span {
                text: " plain ".to_string(),
                style: Style::default(),
            },
            Span {
                text: "rgb".to_string(),
                style: Style {
                    fg: Some(Color::Rgb { r: 100, g: 200, b: 50 }),
                    ..Style::default()
                },
            },
        ];
        let result = spans_to_ansi(&spans);
        assert_eq!(
            result,
            "\x1b[1;31mred\x1b[0m plain \x1b[38;2;100;200;50mrgb\x1b[0m"
        );
    }
}
