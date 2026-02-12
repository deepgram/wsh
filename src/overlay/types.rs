use serde::{Deserialize, Serialize};

/// Which screen mode an overlay or panel belongs to.
///
/// Elements tagged with a particular mode are only visible (and returned by list
/// endpoints) when the session is in that mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScreenMode {
    #[default]
    Normal,
    Alt,
}

/// Helper for serde `skip_serializing_if` on `ScreenMode` fields.
pub fn is_normal_mode(mode: &ScreenMode) -> bool {
    matches!(mode, ScreenMode::Normal)
}

/// Unique identifier for an overlay
pub type OverlayId = String;

/// Background fill style for an overlay's bounding rectangle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackgroundStyle {
    pub bg: Color,
}

/// A styled text write at a specific (row, col) offset within an overlay or panel.
///
/// Enables freeform cell-level drawing for charts, visualizations, and other
/// non-linear content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionWrite {
    pub row: u16,
    pub col: u16,
    pub text: String,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub region_writes: Vec<RegionWrite>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub focusable: bool,
    #[serde(default, skip_serializing_if = "is_normal_mode")]
    pub screen_mode: ScreenMode,
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
