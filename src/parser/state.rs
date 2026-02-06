use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Plain,
    #[default]
    Styled,
}

#[derive(Debug, Clone)]
pub enum Query {
    Screen { format: Format },
    Scrollback { format: Format, offset: usize, limit: usize },
    Cursor,
    Resize { cols: usize, rows: usize },
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum QueryResponse {
    Screen(ScreenResponse),
    Scrollback(ScrollbackResponse),
    Cursor(CursorResponse),
    Ok,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScreenResponse {
    pub epoch: u64,
    pub first_line_index: usize,
    pub total_lines: usize,
    pub lines: Vec<FormattedLine>,
    pub cursor: Cursor,
    pub cols: usize,
    pub rows: usize,
    pub alternate_active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScrollbackResponse {
    pub epoch: u64,
    pub lines: Vec<FormattedLine>,
    pub total_lines: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct CursorResponse {
    pub epoch: u64,
    pub cursor: Cursor,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum FormattedLine {
    Plain(String),
    Styled(Vec<Span>),
}

#[derive(Debug, Clone, Serialize)]
pub struct Span {
    pub text: String,
    #[serde(flatten)]
    pub style: Style,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Style {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<Color>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<Color>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub faint: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub underline: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub strikethrough: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub blink: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub inverse: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    Indexed(u8),
    Rgb { r: u8, g: u8, b: u8 },
}
