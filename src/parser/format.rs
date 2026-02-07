use avt::{Line, Pen};

use super::state::{Color, FormattedLine, Span, Style};

/// Convert an avt Line to a FormattedLine based on format
///
/// For plain format, trailing whitespace is always trimmed.
/// For styled format, trailing whitespace is trimmed only if it has default styling
/// (no colors or attributes), preserving intentional styled whitespace like colored backgrounds.
pub fn format_line(line: &Line, styled: bool) -> FormattedLine {
    if styled {
        let mut spans = line_to_spans(line);
        trim_trailing_default_whitespace(&mut spans);
        FormattedLine::Styled(spans)
    } else {
        FormattedLine::Plain(line.text().trim_end().to_string())
    }
}

/// Convert an avt Line to styled spans
fn line_to_spans(line: &Line) -> Vec<Span> {
    let cells = line.cells();
    if cells.is_empty() {
        return vec![];
    }

    let mut spans = Vec::new();
    let mut current_text = String::new();
    let mut current_style: Option<Style> = None;

    for cell in cells {
        let ch = cell.char();
        if ch == '\0' || cell.width() == 0 {
            continue;
        }

        let style = pen_to_style(cell.pen());

        match &current_style {
            None => {
                current_style = Some(style);
                current_text.push(ch);
            }
            Some(s) if *s == style => {
                current_text.push(ch);
            }
            Some(_) => {
                // Style changed, emit current span
                if !current_text.is_empty() {
                    spans.push(Span {
                        text: std::mem::take(&mut current_text),
                        style: current_style.take().unwrap(),
                    });
                }
                current_style = Some(style);
                current_text.push(ch);
            }
        }
    }

    // Emit final span
    if !current_text.is_empty() {
        if let Some(style) = current_style {
            spans.push(Span {
                text: current_text,
                style,
            });
        }
    }

    spans
}

/// Trim trailing whitespace from spans, but only if it has default styling.
/// This preserves intentional styled whitespace (e.g., colored backgrounds)
/// while removing "empty" screen area.
fn trim_trailing_default_whitespace(spans: &mut Vec<Span>) {
    while let Some(last) = spans.last_mut() {
        if last.style.is_default() {
            // Trim trailing whitespace from this default-styled span
            let trimmed = last.text.trim_end();
            if trimmed.is_empty() {
                // Entire span was whitespace, remove it
                spans.pop();
            } else {
                // Some content remains
                last.text = trimmed.to_string();
                break;
            }
        } else {
            // Non-default style, stop trimming
            break;
        }
    }
}

fn pen_to_style(pen: &Pen) -> Style {
    Style {
        fg: pen.foreground().map(color_to_color),
        bg: pen.background().map(color_to_color),
        bold: pen.is_bold(),
        faint: pen.is_faint(),
        italic: pen.is_italic(),
        underline: pen.is_underline(),
        strikethrough: pen.is_strikethrough(),
        blink: pen.is_blink(),
        inverse: pen.is_inverse(),
    }
}

fn color_to_color(c: avt::Color) -> Color {
    match c {
        avt::Color::Indexed(i) => Color::Indexed(i),
        avt::Color::RGB(rgb) => Color::Rgb {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trim_trailing_default_whitespace_removes_all_default_spaces() {
        let mut spans = vec![
            Span {
                text: "hello".to_string(),
                style: Style::default(),
            },
            Span {
                text: "     ".to_string(),
                style: Style::default(),
            },
        ];
        trim_trailing_default_whitespace(&mut spans);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "hello");
    }

    #[test]
    fn test_trim_trailing_default_whitespace_trims_partial() {
        let mut spans = vec![Span {
            text: "hello   ".to_string(),
            style: Style::default(),
        }];
        trim_trailing_default_whitespace(&mut spans);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "hello");
    }

    #[test]
    fn test_trim_trailing_default_whitespace_preserves_styled_spaces() {
        let styled = Style {
            bg: Some(Color::Indexed(1)), // Red background
            ..Style::default()
        };
        let mut spans = vec![
            Span {
                text: "    ".to_string(), // 4 spaces with red bg
                style: styled.clone(),
            },
            Span {
                text: "     ".to_string(), // 5 spaces with default style
                style: Style::default(),
            },
        ];
        trim_trailing_default_whitespace(&mut spans);
        // Default-styled trailing spaces removed, styled spaces preserved
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "    ");
        assert_eq!(spans[0].style, styled);
    }

    #[test]
    fn test_trim_trailing_default_whitespace_stops_at_styled_span() {
        let styled = Style {
            bold: true,
            ..Style::default()
        };
        let mut spans = vec![
            Span {
                text: "normal".to_string(),
                style: Style::default(),
            },
            Span {
                text: "bold   ".to_string(), // Bold text with trailing spaces
                style: styled.clone(),
            },
        ];
        trim_trailing_default_whitespace(&mut spans);
        // Doesn't trim because last span has non-default style
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[1].text, "bold   ");
    }

    #[test]
    fn test_trim_trailing_default_whitespace_empty_input() {
        let mut spans: Vec<Span> = vec![];
        trim_trailing_default_whitespace(&mut spans);
        assert!(spans.is_empty());
    }

    #[test]
    fn test_trim_trailing_default_whitespace_all_whitespace() {
        let mut spans = vec![Span {
            text: "     ".to_string(),
            style: Style::default(),
        }];
        trim_trailing_default_whitespace(&mut spans);
        assert!(spans.is_empty());
    }

    #[test]
    fn test_style_is_default() {
        assert!(Style::default().is_default());

        let with_fg = Style {
            fg: Some(Color::Indexed(1)),
            ..Style::default()
        };
        assert!(!with_fg.is_default());

        let with_bg = Style {
            bg: Some(Color::Indexed(1)),
            ..Style::default()
        };
        assert!(!with_bg.is_default());

        let with_bold = Style {
            bold: true,
            ..Style::default()
        };
        assert!(!with_bold.is_default());
    }
}
