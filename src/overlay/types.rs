use serde::{Deserialize, Serialize};

/// Unique identifier for an overlay
pub type OverlayId = String;

/// Background fill style for an overlay's bounding rectangle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackgroundStyle {
    pub bg: Color,
}

/// An overlay displayed on top of terminal content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Overlay {
    pub id: OverlayId,
    pub x: u16,
    pub y: u16,
    pub z: i32,
    pub width: u16,
    pub height: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<BackgroundStyle>,
    pub spans: Vec<OverlaySpan>,
}

/// A styled text span within an overlay
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlaySpan {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<Color>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub underline: bool,
}

/// Color specification for overlay styling
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Color {
    Named(NamedColor),
    Rgb { r: u8, g: u8, b: u8 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NamedColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
}

/// Style attributes for rendering
#[derive(Debug, Clone, Default)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl From<&OverlaySpan> for Style {
    fn from(span: &OverlaySpan) -> Self {
        Style {
            fg: span.fg.clone(),
            bg: span.bg.clone(),
            bold: span.bold,
            italic: span.italic,
            underline: span.underline,
        }
    }
}
