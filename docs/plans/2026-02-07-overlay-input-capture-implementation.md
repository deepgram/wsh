# Overlay & Input Capture Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add API-driven overlays and input capture to wsh, enabling agents to display visual content on top of the terminal and optionally capture user input for sidebar interactions.

**Architecture:** Two independent features sharing infrastructure. Overlays are stored in a new `OverlayStore` and composited onto stdout after PTY output. Input capture adds a mode flag that controls whether stdin also reaches the PTY. Both features integrate with the existing WebSocket subscription system.

**Tech Stack:** Rust, Axum (HTTP/WebSocket), tokio (async), crossterm (ANSI rendering), serde (JSON)

---

## Task 1: Overlay Data Types

**Files:**
- Create: `src/overlay/mod.rs`
- Create: `src/overlay/types.rs`
- Modify: `src/lib.rs:1-7`

**Step 1: Create the overlay module structure**

Create `src/overlay/mod.rs`:

```rust
pub mod types;

pub use types::{Color, Overlay, OverlaySpan, Style};
```

**Step 2: Define overlay types**

Create `src/overlay/types.rs`:

```rust
use serde::{Deserialize, Serialize};

/// Unique identifier for an overlay
pub type OverlayId = String;

/// An overlay displayed on top of terminal content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Overlay {
    pub id: OverlayId,
    pub x: u16,
    pub y: u16,
    pub z: i32,
    pub spans: Vec<OverlaySpan>,
}

/// A styled text span within an overlay
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlaySpan {
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
```

**Step 3: Export overlay module from lib.rs**

Modify `src/lib.rs` to add:

```rust
pub mod overlay;
```

**Step 4: Run tests to verify compilation**

Run: `nix develop -c sh -c "cargo check"`
Expected: Compiles without errors

**Step 5: Commit**

```bash
git add src/overlay src/lib.rs
git commit -m "$(cat <<'EOF'
feat(overlay): add overlay data types

Defines Overlay, OverlaySpan, Color, and Style types for the overlay system.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Overlay Store

**Files:**
- Create: `src/overlay/store.rs`
- Modify: `src/overlay/mod.rs`

**Step 1: Write the failing test**

Add to end of `src/overlay/store.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_overlay() {
        let store = OverlayStore::new();
        let id = store.create(0, 0, None, vec![]);
        assert!(!id.is_empty());
    }

    #[test]
    fn test_get_overlay() {
        let store = OverlayStore::new();
        let id = store.create(5, 10, Some(50), vec![]);
        let overlay = store.get(&id).unwrap();
        assert_eq!(overlay.x, 5);
        assert_eq!(overlay.y, 10);
        assert_eq!(overlay.z, 50);
    }

    #[test]
    fn test_list_overlays_sorted_by_z() {
        let store = OverlayStore::new();
        store.create(0, 0, Some(100), vec![]);
        store.create(0, 0, Some(50), vec![]);
        store.create(0, 0, Some(75), vec![]);

        let list = store.list();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].z, 50);
        assert_eq!(list[1].z, 75);
        assert_eq!(list[2].z, 100);
    }

    #[test]
    fn test_delete_overlay() {
        let store = OverlayStore::new();
        let id = store.create(0, 0, None, vec![]);
        assert!(store.delete(&id));
        assert!(store.get(&id).is_none());
    }

    #[test]
    fn test_clear_overlays() {
        let store = OverlayStore::new();
        store.create(0, 0, None, vec![]);
        store.create(0, 0, None, vec![]);
        store.clear();
        assert!(store.list().is_empty());
    }

    #[test]
    fn test_auto_increment_z() {
        let store = OverlayStore::new();
        let id1 = store.create(0, 0, None, vec![]);
        let id2 = store.create(0, 0, None, vec![]);
        let o1 = store.get(&id1).unwrap();
        let o2 = store.get(&id2).unwrap();
        assert!(o2.z > o1.z);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop -c sh -c "cargo test overlay::store"`
Expected: FAIL - module not found

**Step 3: Write the OverlayStore implementation**

Create `src/overlay/store.rs`:

```rust
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

use super::types::{Overlay, OverlayId, OverlaySpan};

/// Thread-safe store for overlays
#[derive(Clone)]
pub struct OverlayStore {
    inner: Arc<RwLock<StoreInner>>,
}

struct StoreInner {
    overlays: HashMap<OverlayId, Overlay>,
    next_z: i32,
}

impl OverlayStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(StoreInner {
                overlays: HashMap::new(),
                next_z: 0,
            })),
        }
    }

    /// Create a new overlay, returns its ID
    pub fn create(&self, x: u16, y: u16, z: Option<i32>, spans: Vec<OverlaySpan>) -> OverlayId {
        let mut inner = self.inner.write().unwrap();
        let id = Uuid::new_v4().to_string();
        let z = z.unwrap_or_else(|| {
            let z = inner.next_z;
            inner.next_z += 1;
            z
        });
        // Update next_z if explicit z is higher
        if z >= inner.next_z {
            inner.next_z = z + 1;
        }
        let overlay = Overlay { id: id.clone(), x, y, z, spans };
        inner.overlays.insert(id.clone(), overlay);
        id
    }

    /// Get an overlay by ID
    pub fn get(&self, id: &str) -> Option<Overlay> {
        let inner = self.inner.read().unwrap();
        inner.overlays.get(id).cloned()
    }

    /// List all overlays, sorted by z-index (ascending)
    pub fn list(&self) -> Vec<Overlay> {
        let inner = self.inner.read().unwrap();
        let mut overlays: Vec<_> = inner.overlays.values().cloned().collect();
        overlays.sort_by_key(|o| o.z);
        overlays
    }

    /// Update an overlay's spans (full replacement)
    pub fn update(&self, id: &str, spans: Vec<OverlaySpan>) -> bool {
        let mut inner = self.inner.write().unwrap();
        if let Some(overlay) = inner.overlays.get_mut(id) {
            overlay.spans = spans;
            true
        } else {
            false
        }
    }

    /// Move an overlay to new coordinates
    pub fn move_to(&self, id: &str, x: Option<u16>, y: Option<u16>, z: Option<i32>) -> bool {
        let mut inner = self.inner.write().unwrap();
        if let Some(overlay) = inner.overlays.get_mut(id) {
            if let Some(x) = x {
                overlay.x = x;
            }
            if let Some(y) = y {
                overlay.y = y;
            }
            if let Some(z) = z {
                overlay.z = z;
                if z >= inner.next_z {
                    inner.next_z = z + 1;
                }
            }
            true
        } else {
            false
        }
    }

    /// Delete an overlay by ID, returns true if it existed
    pub fn delete(&self, id: &str) -> bool {
        let mut inner = self.inner.write().unwrap();
        inner.overlays.remove(id).is_some()
    }

    /// Clear all overlays
    pub fn clear(&self) {
        let mut inner = self.inner.write().unwrap();
        inner.overlays.clear();
    }
}

impl Default for OverlayStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_overlay() {
        let store = OverlayStore::new();
        let id = store.create(0, 0, None, vec![]);
        assert!(!id.is_empty());
    }

    #[test]
    fn test_get_overlay() {
        let store = OverlayStore::new();
        let id = store.create(5, 10, Some(50), vec![]);
        let overlay = store.get(&id).unwrap();
        assert_eq!(overlay.x, 5);
        assert_eq!(overlay.y, 10);
        assert_eq!(overlay.z, 50);
    }

    #[test]
    fn test_list_overlays_sorted_by_z() {
        let store = OverlayStore::new();
        store.create(0, 0, Some(100), vec![]);
        store.create(0, 0, Some(50), vec![]);
        store.create(0, 0, Some(75), vec![]);

        let list = store.list();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].z, 50);
        assert_eq!(list[1].z, 75);
        assert_eq!(list[2].z, 100);
    }

    #[test]
    fn test_delete_overlay() {
        let store = OverlayStore::new();
        let id = store.create(0, 0, None, vec![]);
        assert!(store.delete(&id));
        assert!(store.get(&id).is_none());
    }

    #[test]
    fn test_clear_overlays() {
        let store = OverlayStore::new();
        store.create(0, 0, None, vec![]);
        store.create(0, 0, None, vec![]);
        store.clear();
        assert!(store.list().is_empty());
    }

    #[test]
    fn test_auto_increment_z() {
        let store = OverlayStore::new();
        let id1 = store.create(0, 0, None, vec![]);
        let id2 = store.create(0, 0, None, vec![]);
        let o1 = store.get(&id1).unwrap();
        let o2 = store.get(&id2).unwrap();
        assert!(o2.z > o1.z);
    }
}
```

**Step 4: Update mod.rs to export store**

Modify `src/overlay/mod.rs`:

```rust
pub mod store;
pub mod types;

pub use store::OverlayStore;
pub use types::{Color, NamedColor, Overlay, OverlayId, OverlaySpan, Style};
```

**Step 5: Add uuid dependency to Cargo.toml**

Run: `nix develop -c sh -c "cargo add uuid --features v4"`

**Step 6: Run tests to verify they pass**

Run: `nix develop -c sh -c "cargo test overlay::store"`
Expected: All 6 tests pass

**Step 7: Commit**

```bash
git add src/overlay Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
feat(overlay): add OverlayStore for managing overlays

Thread-safe store with create, get, list, update, move, delete, clear operations.
Auto-increments z-index for new overlays without explicit z.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Overlay ANSI Rendering

**Files:**
- Create: `src/overlay/render.rs`
- Modify: `src/overlay/mod.rs`

**Step 1: Write the failing test**

Add to `src/overlay/render.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::{Color, NamedColor, OverlaySpan};

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
            text: "Red".to_string(),
            fg: Some(Color::Named(NamedColor::Red)),
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }];
        let result = render_spans(&spans);
        assert!(result.contains("\x1b[31m")); // Red foreground
        assert!(result.contains("Red"));
    }

    #[test]
    fn test_render_bold_text() {
        let spans = vec![OverlaySpan {
            text: "Bold".to_string(),
            fg: None,
            bg: None,
            bold: true,
            italic: false,
            underline: false,
        }];
        let result = render_spans(&spans);
        assert!(result.contains("\x1b[1m")); // Bold
    }

    #[test]
    fn test_cursor_position() {
        let result = cursor_position(5, 10);
        assert_eq!(result, "\x1b[6;11H"); // 1-indexed: row 6, col 11
    }

    #[test]
    fn test_save_restore_cursor() {
        assert_eq!(save_cursor(), "\x1b[s");
        assert_eq!(restore_cursor(), "\x1b[u");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop -c sh -c "cargo test overlay::render"`
Expected: FAIL - module not found

**Step 3: Implement the render module**

Create `src/overlay/render.rs`:

```rust
use super::types::{Color, NamedColor, Overlay, OverlaySpan};

/// Save cursor position
pub fn save_cursor() -> &'static str {
    "\x1b[s"
}

/// Restore cursor position
pub fn restore_cursor() -> &'static str {
    "\x1b[u"
}

/// Move cursor to position (0-indexed input, converts to 1-indexed ANSI)
pub fn cursor_position(row: u16, col: u16) -> String {
    format!("\x1b[{};{}H", row + 1, col + 1)
}

/// Reset all attributes
pub fn reset() -> &'static str {
    "\x1b[0m"
}

/// Render a single span to ANSI escape sequences
fn render_span(span: &OverlaySpan) -> String {
    let mut result = String::new();

    // Start with SGR sequence
    let mut sgr_codes: Vec<String> = vec![];

    if span.bold {
        sgr_codes.push("1".to_string());
    }
    if span.italic {
        sgr_codes.push("3".to_string());
    }
    if span.underline {
        sgr_codes.push("4".to_string());
    }
    if let Some(ref fg) = span.fg {
        sgr_codes.push(color_to_fg_code(fg));
    }
    if let Some(ref bg) = span.bg {
        sgr_codes.push(color_to_bg_code(bg));
    }

    if !sgr_codes.is_empty() {
        result.push_str(&format!("\x1b[{}m", sgr_codes.join(";")));
    }

    result.push_str(&span.text);
    result
}

/// Render multiple spans with reset at the end
pub fn render_spans(spans: &[OverlaySpan]) -> String {
    let mut result = String::new();
    for span in spans {
        result.push_str(&render_span(span));
    }
    result.push_str(reset());
    result
}

/// Render a complete overlay to ANSI (positions cursor, renders spans)
pub fn render_overlay(overlay: &Overlay) -> String {
    let mut result = String::new();
    result.push_str(&cursor_position(overlay.y, overlay.x));

    // Handle newlines in spans - each newline moves to next row, same x
    let mut current_row = overlay.y;
    let start_col = overlay.x;

    for span in &overlay.spans {
        let lines: Vec<&str> = span.text.split('\n').collect();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                current_row += 1;
                result.push_str(&cursor_position(current_row, start_col));
            }
            if !line.is_empty() {
                let line_span = OverlaySpan {
                    text: line.to_string(),
                    fg: span.fg.clone(),
                    bg: span.bg.clone(),
                    bold: span.bold,
                    italic: span.italic,
                    underline: span.underline,
                };
                result.push_str(&render_span(&line_span));
            }
        }
    }
    result.push_str(reset());
    result
}

/// Render all overlays (sorted by z-index) with cursor save/restore
pub fn render_all_overlays(overlays: &[Overlay]) -> String {
    if overlays.is_empty() {
        return String::new();
    }

    let mut result = String::new();
    result.push_str(save_cursor());
    for overlay in overlays {
        result.push_str(&render_overlay(overlay));
    }
    result.push_str(restore_cursor());
    result
}

fn color_to_fg_code(color: &Color) -> String {
    match color {
        Color::Named(named) => match named {
            NamedColor::Black => "30".to_string(),
            NamedColor::Red => "31".to_string(),
            NamedColor::Green => "32".to_string(),
            NamedColor::Yellow => "33".to_string(),
            NamedColor::Blue => "34".to_string(),
            NamedColor::Magenta => "35".to_string(),
            NamedColor::Cyan => "36".to_string(),
            NamedColor::White => "37".to_string(),
        },
        Color::Rgb { r, g, b } => format!("38;2;{};{};{}", r, g, b),
    }
}

fn color_to_bg_code(color: &Color) -> String {
    match color {
        Color::Named(named) => match named {
            NamedColor::Black => "40".to_string(),
            NamedColor::Red => "41".to_string(),
            NamedColor::Green => "42".to_string(),
            NamedColor::Yellow => "43".to_string(),
            NamedColor::Blue => "44".to_string(),
            NamedColor::Magenta => "45".to_string(),
            NamedColor::Cyan => "46".to_string(),
            NamedColor::White => "47".to_string(),
        },
        Color::Rgb { r, g, b } => format!("48;2;{};{};{}", r, g, b),
    }
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
            text: "Red".to_string(),
            fg: Some(Color::Named(NamedColor::Red)),
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }];
        let result = render_spans(&spans);
        assert!(result.contains("\x1b[31m"));
        assert!(result.contains("Red"));
    }

    #[test]
    fn test_render_bold_text() {
        let spans = vec![OverlaySpan {
            text: "Bold".to_string(),
            fg: None,
            bg: None,
            bold: true,
            italic: false,
            underline: false,
        }];
        let result = render_spans(&spans);
        assert!(result.contains("\x1b[1m"));
    }

    #[test]
    fn test_cursor_position() {
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
            text: "RGB".to_string(),
            fg: Some(Color::Rgb { r: 255, g: 128, b: 0 }),
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }];
        let result = render_spans(&spans);
        assert!(result.contains("\x1b[38;2;255;128;0m"));
    }

    #[test]
    fn test_render_overlay_with_position() {
        let overlay = Overlay {
            id: "test".to_string(),
            x: 10,
            y: 5,
            z: 0,
            spans: vec![OverlaySpan {
                text: "Test".to_string(),
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            }],
        };
        let result = render_overlay(&overlay);
        assert!(result.starts_with("\x1b[6;11H")); // Position first
        assert!(result.contains("Test"));
    }
}
```

**Step 4: Update mod.rs**

Modify `src/overlay/mod.rs`:

```rust
pub mod render;
pub mod store;
pub mod types;

pub use render::{render_all_overlays, render_overlay};
pub use store::OverlayStore;
pub use types::{Color, NamedColor, Overlay, OverlayId, OverlaySpan, Style};
```

**Step 5: Run tests**

Run: `nix develop -c sh -c "cargo test overlay::render"`
Expected: All 7 tests pass

**Step 6: Commit**

```bash
git add src/overlay/render.rs src/overlay/mod.rs
git commit -m "$(cat <<'EOF'
feat(overlay): add ANSI rendering for overlays

Converts overlays to ANSI escape sequences with cursor positioning,
colors (named and RGB), and text attributes (bold, italic, underline).

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Input Mode State

**Files:**
- Create: `src/input/mod.rs`
- Create: `src/input/mode.rs`
- Modify: `src/lib.rs`

**Step 1: Write the failing test**

Add to `src/input/mode.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_mode_is_passthrough() {
        let mode = InputMode::new();
        assert_eq!(mode.get(), Mode::Passthrough);
    }

    #[test]
    fn test_capture_mode() {
        let mode = InputMode::new();
        mode.capture();
        assert_eq!(mode.get(), Mode::Capture);
    }

    #[test]
    fn test_release_mode() {
        let mode = InputMode::new();
        mode.capture();
        mode.release();
        assert_eq!(mode.get(), Mode::Passthrough);
    }

    #[test]
    fn test_is_capture() {
        let mode = InputMode::new();
        assert!(!mode.is_capture());
        mode.capture();
        assert!(mode.is_capture());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop -c sh -c "cargo test input::mode"`
Expected: FAIL - module not found

**Step 3: Implement input mode**

Create `src/input/mod.rs`:

```rust
pub mod mode;

pub use mode::{InputMode, Mode};
```

Create `src/input/mode.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

/// Input routing mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Input goes to both API subscribers and PTY
    Passthrough,
    /// Input goes only to API subscribers
    Capture,
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Passthrough
    }
}

/// Thread-safe input mode state
#[derive(Clone)]
pub struct InputMode {
    inner: Arc<RwLock<Mode>>,
}

impl InputMode {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Mode::Passthrough)),
        }
    }

    pub fn get(&self) -> Mode {
        *self.inner.read().unwrap()
    }

    pub fn capture(&self) {
        *self.inner.write().unwrap() = Mode::Capture;
    }

    pub fn release(&self) {
        *self.inner.write().unwrap() = Mode::Passthrough;
    }

    pub fn is_capture(&self) -> bool {
        self.get() == Mode::Capture
    }
}

impl Default for InputMode {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_mode_is_passthrough() {
        let mode = InputMode::new();
        assert_eq!(mode.get(), Mode::Passthrough);
    }

    #[test]
    fn test_capture_mode() {
        let mode = InputMode::new();
        mode.capture();
        assert_eq!(mode.get(), Mode::Capture);
    }

    #[test]
    fn test_release_mode() {
        let mode = InputMode::new();
        mode.capture();
        mode.release();
        assert_eq!(mode.get(), Mode::Passthrough);
    }

    #[test]
    fn test_is_capture() {
        let mode = InputMode::new();
        assert!(!mode.is_capture());
        mode.capture();
        assert!(mode.is_capture());
    }
}
```

**Step 4: Export from lib.rs**

Add to `src/lib.rs`:

```rust
pub mod input;
```

**Step 5: Run tests**

Run: `nix develop -c sh -c "cargo test input::mode"`
Expected: All 4 tests pass

**Step 6: Commit**

```bash
git add src/input src/lib.rs
git commit -m "$(cat <<'EOF'
feat(input): add InputMode for capture/passthrough state

Thread-safe state for controlling whether input goes to PTY or only to API.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Key Parsing

**Files:**
- Create: `src/input/keys.rs`
- Modify: `src/input/mod.rs`

**Step 1: Write the failing test**

Add to `src/input/keys.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_printable_char() {
        let parsed = parse_key(&[b'a']);
        assert_eq!(parsed.key, Some("a".to_string()));
        assert!(parsed.modifiers.is_empty());
    }

    #[test]
    fn test_parse_ctrl_c() {
        let parsed = parse_key(&[0x03]);
        assert_eq!(parsed.key, Some("c".to_string()));
        assert_eq!(parsed.modifiers, vec!["ctrl"]);
    }

    #[test]
    fn test_parse_escape() {
        let parsed = parse_key(&[0x1b]);
        assert_eq!(parsed.key, Some("Escape".to_string()));
    }

    #[test]
    fn test_parse_arrow_up() {
        let parsed = parse_key(&[0x1b, b'[', b'A']);
        assert_eq!(parsed.key, Some("ArrowUp".to_string()));
    }

    #[test]
    fn test_parse_ctrl_backslash() {
        let parsed = parse_key(&[0x1c]);
        assert_eq!(parsed.key, Some("\\".to_string()));
        assert_eq!(parsed.modifiers, vec!["ctrl"]);
    }

    #[test]
    fn test_is_ctrl_backslash() {
        assert!(is_ctrl_backslash(&[0x1c]));
        assert!(!is_ctrl_backslash(&[0x03]));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop -c sh -c "cargo test input::keys"`
Expected: FAIL - module not found

**Step 3: Implement key parsing**

Create `src/input/keys.rs`:

```rust
use serde::Serialize;

/// Parsed key information
#[derive(Debug, Clone, Serialize)]
pub struct ParsedKey {
    pub key: Option<String>,
    pub modifiers: Vec<String>,
}

/// Check if input is Ctrl+\ (the escape hatch)
pub fn is_ctrl_backslash(data: &[u8]) -> bool {
    data == [0x1c]
}

/// Parse raw input bytes into key representation
pub fn parse_key(data: &[u8]) -> ParsedKey {
    if data.is_empty() {
        return ParsedKey {
            key: None,
            modifiers: vec![],
        };
    }

    // Single byte
    if data.len() == 1 {
        let byte = data[0];
        return match byte {
            // Control characters (Ctrl+A through Ctrl+Z)
            0x01..=0x1a => ParsedKey {
                key: Some(((byte - 1 + b'a') as char).to_string()),
                modifiers: vec!["ctrl".to_string()],
            },
            // Ctrl+\ through Ctrl+_
            0x1c => ParsedKey {
                key: Some("\\".to_string()),
                modifiers: vec!["ctrl".to_string()],
            },
            0x1d => ParsedKey {
                key: Some("]".to_string()),
                modifiers: vec!["ctrl".to_string()],
            },
            0x1e => ParsedKey {
                key: Some("^".to_string()),
                modifiers: vec!["ctrl".to_string()],
            },
            0x1f => ParsedKey {
                key: Some("_".to_string()),
                modifiers: vec!["ctrl".to_string()],
            },
            // Escape
            0x1b => ParsedKey {
                key: Some("Escape".to_string()),
                modifiers: vec![],
            },
            // Tab
            0x09 => ParsedKey {
                key: Some("Tab".to_string()),
                modifiers: vec![],
            },
            // Enter/Return
            0x0d => ParsedKey {
                key: Some("Enter".to_string()),
                modifiers: vec![],
            },
            // Backspace
            0x7f => ParsedKey {
                key: Some("Backspace".to_string()),
                modifiers: vec![],
            },
            // Printable ASCII
            0x20..=0x7e => ParsedKey {
                key: Some((byte as char).to_string()),
                modifiers: vec![],
            },
            _ => ParsedKey {
                key: None,
                modifiers: vec![],
            },
        };
    }

    // Escape sequences
    if data.len() >= 2 && data[0] == 0x1b {
        // CSI sequences (ESC [)
        if data.len() >= 3 && data[1] == b'[' {
            return match data[2] {
                b'A' => ParsedKey {
                    key: Some("ArrowUp".to_string()),
                    modifiers: vec![],
                },
                b'B' => ParsedKey {
                    key: Some("ArrowDown".to_string()),
                    modifiers: vec![],
                },
                b'C' => ParsedKey {
                    key: Some("ArrowRight".to_string()),
                    modifiers: vec![],
                },
                b'D' => ParsedKey {
                    key: Some("ArrowLeft".to_string()),
                    modifiers: vec![],
                },
                b'H' => ParsedKey {
                    key: Some("Home".to_string()),
                    modifiers: vec![],
                },
                b'F' => ParsedKey {
                    key: Some("End".to_string()),
                    modifiers: vec![],
                },
                _ => ParsedKey {
                    key: None,
                    modifiers: vec![],
                },
            };
        }
    }

    // Unknown sequence
    ParsedKey {
        key: None,
        modifiers: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_printable_char() {
        let parsed = parse_key(&[b'a']);
        assert_eq!(parsed.key, Some("a".to_string()));
        assert!(parsed.modifiers.is_empty());
    }

    #[test]
    fn test_parse_ctrl_c() {
        let parsed = parse_key(&[0x03]);
        assert_eq!(parsed.key, Some("c".to_string()));
        assert_eq!(parsed.modifiers, vec!["ctrl"]);
    }

    #[test]
    fn test_parse_escape() {
        let parsed = parse_key(&[0x1b]);
        assert_eq!(parsed.key, Some("Escape".to_string()));
    }

    #[test]
    fn test_parse_arrow_up() {
        let parsed = parse_key(&[0x1b, b'[', b'A']);
        assert_eq!(parsed.key, Some("ArrowUp".to_string()));
    }

    #[test]
    fn test_parse_ctrl_backslash() {
        let parsed = parse_key(&[0x1c]);
        assert_eq!(parsed.key, Some("\\".to_string()));
        assert_eq!(parsed.modifiers, vec!["ctrl"]);
    }

    #[test]
    fn test_is_ctrl_backslash() {
        assert!(is_ctrl_backslash(&[0x1c]));
        assert!(!is_ctrl_backslash(&[0x03]));
    }
}
```

**Step 4: Update mod.rs**

Modify `src/input/mod.rs`:

```rust
pub mod keys;
pub mod mode;

pub use keys::{is_ctrl_backslash, parse_key, ParsedKey};
pub use mode::{InputMode, Mode};
```

**Step 5: Run tests**

Run: `nix develop -c sh -c "cargo test input::keys"`
Expected: All 6 tests pass

**Step 6: Commit**

```bash
git add src/input/keys.rs src/input/mod.rs
git commit -m "$(cat <<'EOF'
feat(input): add key parsing for input events

Parses raw bytes into structured key events with modifiers.
Includes detection of Ctrl+\ escape hatch.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Overlay HTTP API Endpoints

**Files:**
- Modify: `src/api.rs`

**Step 1: Write the failing test**

Add to `src/api.rs` tests module:

```rust
#[tokio::test]
async fn test_overlay_create() {
    let (state, _input_rx) = create_test_state();
    let app = router(state);

    let body = serde_json::json!({
        "x": 10,
        "y": 5,
        "spans": [{"text": "Hello"}]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/overlay")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("id").is_some());
}

#[tokio::test]
async fn test_overlay_list() {
    let (state, _input_rx) = create_test_state();
    state.overlays.create(0, 0, None, vec![]);
    let app = router(state);

    let response = app
        .oneshot(Request::builder().uri("/overlay").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.as_array().unwrap().len() >= 1);
}

#[tokio::test]
async fn test_overlay_delete() {
    let (state, _input_rx) = create_test_state();
    let id = state.overlays.create(0, 0, None, vec![]);
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/overlay/{}", id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop -c sh -c "cargo test test_overlay_create"`
Expected: FAIL - overlays field not found on AppState

**Step 3: Add OverlayStore to AppState and implement endpoints**

Modify `src/api.rs`:

Add imports at top:
```rust
use crate::overlay::{Overlay, OverlaySpan, OverlayStore};
```

Add to AppState:
```rust
#[derive(Clone)]
pub struct AppState {
    pub input_tx: mpsc::Sender<Bytes>,
    pub output_rx: broadcast::Sender<Bytes>,
    pub shutdown: ShutdownCoordinator,
    pub parser: Parser,
    pub overlays: OverlayStore,
}
```

Add request/response types:
```rust
#[derive(Deserialize)]
struct CreateOverlayRequest {
    x: u16,
    y: u16,
    z: Option<i32>,
    spans: Vec<OverlaySpan>,
}

#[derive(Serialize)]
struct CreateOverlayResponse {
    id: String,
}

#[derive(Deserialize)]
struct UpdateOverlayRequest {
    spans: Vec<OverlaySpan>,
}

#[derive(Deserialize)]
struct PatchOverlayRequest {
    x: Option<u16>,
    y: Option<u16>,
    z: Option<i32>,
}
```

Add handler functions:
```rust
async fn overlay_create(
    State(state): State<AppState>,
    Json(req): Json<CreateOverlayRequest>,
) -> impl IntoResponse {
    let id = state.overlays.create(req.x, req.y, req.z, req.spans);
    (StatusCode::CREATED, Json(CreateOverlayResponse { id }))
}

async fn overlay_list(State(state): State<AppState>) -> Json<Vec<Overlay>> {
    Json(state.overlays.list())
}

async fn overlay_get(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<Overlay>, StatusCode> {
    state.overlays.get(&id).map(Json).ok_or(StatusCode::NOT_FOUND)
}

async fn overlay_update(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<UpdateOverlayRequest>,
) -> StatusCode {
    if state.overlays.update(&id, req.spans) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn overlay_patch(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<PatchOverlayRequest>,
) -> StatusCode {
    if state.overlays.move_to(&id, req.x, req.y, req.z) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn overlay_delete(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> StatusCode {
    if state.overlays.delete(&id) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn overlay_clear(State(state): State<AppState>) -> StatusCode {
    state.overlays.clear();
    StatusCode::NO_CONTENT
}
```

Update router function:
```rust
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/input", post(input))
        .route("/ws/raw", get(ws_raw))
        .route("/ws/json", get(ws_json))
        .route("/screen", get(screen))
        .route("/scrollback", get(scrollback))
        .route("/overlay", get(overlay_list).post(overlay_create).delete(overlay_clear))
        .route("/overlay/{id}", get(overlay_get).put(overlay_update).patch(overlay_patch).delete(overlay_delete))
        .with_state(state)
}
```

Update test helper:
```rust
fn create_test_state() -> (AppState, mpsc::Receiver<Bytes>) {
    let (input_tx, input_rx) = mpsc::channel(64);
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    let state = AppState {
        input_tx,
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
    };
    (state, input_rx)
}
```

**Step 4: Update main.rs to create OverlayStore**

Add to main.rs imports:
```rust
use wsh::overlay::OverlayStore;
```

Update state creation:
```rust
let state = api::AppState {
    input_tx,
    output_rx: broker.sender(),
    shutdown: shutdown.clone(),
    parser: parser.clone(),
    overlays: OverlayStore::new(),
};
```

**Step 5: Run tests**

Run: `nix develop -c sh -c "cargo test test_overlay"`
Expected: All 3 new tests pass

**Step 6: Commit**

```bash
git add src/api.rs src/main.rs
git commit -m "$(cat <<'EOF'
feat(api): add overlay HTTP endpoints

POST /overlay - create overlay
GET /overlay - list all overlays
GET /overlay/:id - get single overlay
PUT /overlay/:id - update overlay content
PATCH /overlay/:id - move overlay
DELETE /overlay/:id - delete overlay
DELETE /overlay - clear all overlays

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Input Capture HTTP API Endpoints

**Files:**
- Modify: `src/api.rs`

**Step 1: Write the failing test**

Add to `src/api.rs` tests:

```rust
#[tokio::test]
async fn test_input_mode_default() {
    let (state, _input_rx) = create_test_state();
    let app = router(state);

    let response = app
        .oneshot(Request::builder().uri("/input/mode").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["mode"], "passthrough");
}

#[tokio::test]
async fn test_input_capture_and_release() {
    let (state, _input_rx) = create_test_state();
    let app = router(state.clone());

    // Capture
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/input/capture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify captured
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/input/mode").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["mode"], "capture");

    // Release
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/input/release")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop -c sh -c "cargo test test_input_mode"`
Expected: FAIL - route not found

**Step 3: Add InputMode to AppState and implement endpoints**

Add imports:
```rust
use crate::input::{InputMode, Mode};
```

Update AppState:
```rust
#[derive(Clone)]
pub struct AppState {
    pub input_tx: mpsc::Sender<Bytes>,
    pub output_rx: broadcast::Sender<Bytes>,
    pub shutdown: ShutdownCoordinator,
    pub parser: Parser,
    pub overlays: OverlayStore,
    pub input_mode: InputMode,
}
```

Add response type:
```rust
#[derive(Serialize)]
struct InputModeResponse {
    mode: Mode,
}
```

Add handlers:
```rust
async fn input_mode_get(State(state): State<AppState>) -> Json<InputModeResponse> {
    Json(InputModeResponse {
        mode: state.input_mode.get(),
    })
}

async fn input_capture(State(state): State<AppState>) -> StatusCode {
    state.input_mode.capture();
    StatusCode::NO_CONTENT
}

async fn input_release(State(state): State<AppState>) -> StatusCode {
    state.input_mode.release();
    StatusCode::NO_CONTENT
}
```

Update router:
```rust
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/input", post(input))
        .route("/input/mode", get(input_mode_get))
        .route("/input/capture", post(input_capture))
        .route("/input/release", post(input_release))
        .route("/ws/raw", get(ws_raw))
        .route("/ws/json", get(ws_json))
        .route("/screen", get(screen))
        .route("/scrollback", get(scrollback))
        .route("/overlay", get(overlay_list).post(overlay_create).delete(overlay_clear))
        .route("/overlay/{id}", get(overlay_get).put(overlay_update).patch(overlay_patch).delete(overlay_delete))
        .with_state(state)
}
```

Update test helper and main.rs to include InputMode::new().

**Step 4: Run tests**

Run: `nix develop -c sh -c "cargo test test_input_mode"`
Expected: All tests pass

**Step 5: Commit**

```bash
git add src/api.rs src/main.rs
git commit -m "$(cat <<'EOF'
feat(api): add input capture HTTP endpoints

GET /input/mode - get current mode
POST /input/capture - switch to capture mode
POST /input/release - switch to passthrough mode

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Overlay Compositing in PTY Reader

**Files:**
- Modify: `src/main.rs`

**Step 1: Implement overlay compositing**

The PTY reader needs to render overlays after forwarding PTY output. Modify `spawn_pty_reader` in `src/main.rs`:

```rust
fn spawn_pty_reader(
    mut reader: Box<dyn Read + Send>,
    broker: broker::Broker,
    overlays: overlay::OverlayStore,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        let mut stdout = std::io::stdout();
        let mut buf = [0u8; 4096];

        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    tracing::debug!("PTY reader: EOF");
                    break;
                }
                Ok(n) => {
                    let data = Bytes::copy_from_slice(&buf[..n]);
                    // Forward PTY output
                    let _ = stdout.write_all(&data);

                    // Render overlays on top
                    let overlay_list = overlays.list();
                    if !overlay_list.is_empty() {
                        let overlay_output = overlay::render_all_overlays(&overlay_list);
                        let _ = stdout.write_all(overlay_output.as_bytes());
                    }

                    let _ = stdout.flush();
                    broker.publish(data);
                }
                Err(e) => {
                    tracing::debug!(?e, "PTY reader: error");
                    break;
                }
            }
        }
    })
}
```

Update the call site to pass overlays:
```rust
let pty_reader_handle = spawn_pty_reader(pty_reader, broker.clone(), overlays.clone());
```

Where `overlays` is created before the AppState:
```rust
let overlays = overlay::OverlayStore::new();
```

**Step 2: Run all tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: All tests pass

**Step 3: Commit**

```bash
git add src/main.rs
git commit -m "$(cat <<'EOF'
feat(overlay): composite overlays after PTY output

Renders overlays on top of terminal content after each PTY read cycle.
Uses cursor save/restore to avoid disrupting cursor position.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Input Routing with Capture Mode

**Files:**
- Modify: `src/main.rs`

**Step 1: Implement input routing**

Modify `spawn_stdin_reader` to check capture mode and handle Ctrl+\:

```rust
fn spawn_stdin_reader(
    input_tx: mpsc::Sender<Bytes>,
    input_mode: input::InputMode,
) {
    tokio::task::spawn_blocking(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 1024];

        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let data = Bytes::copy_from_slice(&buf[..n]);

                    // Check for Ctrl+\ escape hatch
                    if input::is_ctrl_backslash(&buf[..n]) && input_mode.is_capture() {
                        input_mode.release();
                        tracing::debug!("Ctrl+\\ pressed, switching to passthrough mode");
                        continue; // Don't forward the Ctrl+\
                    }

                    // In capture mode, don't forward to PTY
                    if input_mode.is_capture() {
                        // TODO: Broadcast to input subscribers (Task 10)
                        continue;
                    }

                    // Passthrough mode: forward to PTY
                    if input_tx.blocking_send(data).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}
```

Update call site:
```rust
spawn_stdin_reader(input_tx.clone(), input_mode.clone());
```

**Step 2: Run all tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: All tests pass

**Step 3: Commit**

```bash
git add src/main.rs
git commit -m "$(cat <<'EOF'
feat(input): route stdin based on capture mode

In capture mode, stdin is not forwarded to PTY.
Ctrl+\ in capture mode switches back to passthrough mode.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Input Event Broadcasting

**Files:**
- Create: `src/input/events.rs`
- Modify: `src/input/mod.rs`
- Modify: `src/main.rs`

**Step 1: Define input event types**

Create `src/input/events.rs`:

```rust
use serde::Serialize;
use tokio::sync::broadcast;

use super::{parse_key, Mode, ParsedKey};

/// Input event broadcast to subscribers
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum InputEvent {
    Input {
        mode: Mode,
        raw: Vec<u8>,
        parsed: Option<ParsedKey>,
    },
    Mode {
        mode: Mode,
    },
}

/// Broadcaster for input events
#[derive(Clone)]
pub struct InputBroadcaster {
    tx: broadcast::Sender<InputEvent>,
}

impl InputBroadcaster {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { tx }
    }

    pub fn broadcast_input(&self, data: &[u8], mode: Mode) {
        let parsed = parse_key(data);
        let parsed = if parsed.key.is_some() {
            Some(parsed)
        } else {
            None
        };
        let _ = self.tx.send(InputEvent::Input {
            mode,
            raw: data.to_vec(),
            parsed,
        });
    }

    pub fn broadcast_mode(&self, mode: Mode) {
        let _ = self.tx.send(InputEvent::Mode { mode });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<InputEvent> {
        self.tx.subscribe()
    }
}

impl Default for InputBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 2: Update input mod.rs**

```rust
pub mod events;
pub mod keys;
pub mod mode;

pub use events::{InputBroadcaster, InputEvent};
pub use keys::{is_ctrl_backslash, parse_key, ParsedKey};
pub use mode::{InputMode, Mode};
```

**Step 3: Integrate into main.rs**

Add InputBroadcaster to spawn_stdin_reader and broadcast all input:

```rust
fn spawn_stdin_reader(
    input_tx: mpsc::Sender<Bytes>,
    input_mode: input::InputMode,
    input_broadcaster: input::InputBroadcaster,
) {
    tokio::task::spawn_blocking(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 1024];

        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let data = &buf[..n];
                    let mode = input_mode.get();

                    // Always broadcast to subscribers
                    input_broadcaster.broadcast_input(data, mode);

                    // Check for Ctrl+\ escape hatch
                    if input::is_ctrl_backslash(data) && mode == input::Mode::Capture {
                        input_mode.release();
                        input_broadcaster.broadcast_mode(input::Mode::Passthrough);
                        tracing::debug!("Ctrl+\\ pressed, switching to passthrough mode");
                        continue;
                    }

                    // In capture mode, don't forward to PTY
                    if mode == input::Mode::Capture {
                        continue;
                    }

                    // Passthrough mode: forward to PTY
                    if input_tx.blocking_send(Bytes::copy_from_slice(data)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}
```

**Step 4: Add InputBroadcaster to AppState**

Update AppState and state creation in main.rs and api.rs.

**Step 5: Run all tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: All tests pass

**Step 6: Commit**

```bash
git add src/input src/main.rs src/api.rs
git commit -m "$(cat <<'EOF'
feat(input): broadcast input events to subscribers

All input is broadcast to subscribers with mode and parsed key info.
Mode changes are also broadcast.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: WebSocket Input Subscription

**Files:**
- Modify: `src/parser/events.rs`
- Modify: `src/api.rs`

**Step 1: Add input and overlay to EventType**

Modify `src/parser/events.rs`:

```rust
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    Lines,
    Chars,
    Cursor,
    Mode,
    Diffs,
    Input,
    Overlay,
}
```

**Step 2: Update WebSocket handler to support input subscription**

In `handle_ws_json`, add input event forwarding when subscribed:

```rust
// In the main event loop, add a branch for input events
if subscribed_types.contains(&EventType::Input) {
    let mut input_rx = state.input_broadcaster.subscribe();
    // ... forward input events to WebSocket
}
```

This requires restructuring the select! loop to handle multiple event sources.

**Step 3: Run all tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: All tests pass

**Step 4: Commit**

```bash
git add src/parser/events.rs src/api.rs
git commit -m "$(cat <<'EOF'
feat(api): add input subscription to WebSocket

Clients can subscribe to 'input' events to receive all keystrokes
with mode and parsed key information.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Integration Tests

**Files:**
- Create: `tests/overlay_integration.rs`
- Create: `tests/input_capture_integration.rs`

**Step 1: Write overlay integration tests**

Create `tests/overlay_integration.rs`:

```rust
//! Integration tests for overlay API

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bytes::Bytes;
use tokio::sync::mpsc;
use tower::ServiceExt;
use wsh::api::{router, AppState};
use wsh::broker::Broker;
use wsh::input::{InputBroadcaster, InputMode};
use wsh::overlay::OverlayStore;
use wsh::parser::Parser;
use wsh::shutdown::ShutdownCoordinator;

fn create_test_state() -> AppState {
    let (input_tx, _) = mpsc::channel::<Bytes>(64);
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    AppState {
        input_tx,
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
    }
}

#[tokio::test]
async fn test_overlay_crud_flow() {
    let state = create_test_state();
    let app = router(state);

    // Create
    let body = serde_json::json!({
        "x": 10,
        "y": 5,
        "spans": [{"text": "Test overlay", "fg": "yellow", "bold": true}]
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/overlay")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let id = json["id"].as_str().unwrap();

    // Get
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/overlay/{}", id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Update
    let update_body = serde_json::json!({
        "spans": [{"text": "Updated text"}]
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/overlay/{}", id))
                .header("content-type", "application/json")
                .body(Body::from(update_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Delete
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/overlay/{}", id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify deleted
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/overlay/{}", id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
```

**Step 2: Write input capture integration tests**

Create `tests/input_capture_integration.rs`:

```rust
//! Integration tests for input capture API

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bytes::Bytes;
use tokio::sync::mpsc;
use tower::ServiceExt;
use wsh::api::{router, AppState};
use wsh::broker::Broker;
use wsh::input::{InputBroadcaster, InputMode};
use wsh::overlay::OverlayStore;
use wsh::parser::Parser;
use wsh::shutdown::ShutdownCoordinator;

fn create_test_state() -> AppState {
    let (input_tx, _) = mpsc::channel::<Bytes>(64);
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);
    AppState {
        input_tx,
        output_rx: broker.sender(),
        shutdown: ShutdownCoordinator::new(),
        parser,
        overlays: OverlayStore::new(),
        input_mode: InputMode::new(),
        input_broadcaster: InputBroadcaster::new(),
    }
}

#[tokio::test]
async fn test_input_capture_flow() {
    let state = create_test_state();
    let app = router(state);

    // Default is passthrough
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/input/mode").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["mode"], "passthrough");

    // Capture
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/input/capture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify capture
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/input/mode").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["mode"], "capture");

    // Release
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/input/release")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify passthrough
    let response = app
        .oneshot(Request::builder().uri("/input/mode").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["mode"], "passthrough");
}
```

**Step 3: Run all tests**

Run: `nix develop -c sh -c "cargo test"`
Expected: All tests pass

**Step 4: Commit**

```bash
git add tests/overlay_integration.rs tests/input_capture_integration.rs
git commit -m "$(cat <<'EOF'
test: add integration tests for overlay and input capture

Full CRUD flow tests for overlay API.
Mode switching tests for input capture API.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: Final Review and Documentation

**Files:**
- Modify: `docs/plans/2026-02-07-overlay-input-capture-design.md`

**Step 1: Mark design as implemented**

Add to top of design document:

```markdown
> **Status:** Implemented (2026-02-07)
```

**Step 2: Run full test suite**

Run: `nix develop -c sh -c "cargo test"`
Expected: All tests pass

**Step 3: Final commit**

```bash
git add docs/plans/2026-02-07-overlay-input-capture-design.md
git commit -m "$(cat <<'EOF'
docs: mark overlay and input capture design as implemented

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Summary

| Task | Description | Files |
|------|-------------|-------|
| 1 | Overlay data types | `src/overlay/{mod,types}.rs` |
| 2 | Overlay store | `src/overlay/store.rs` |
| 3 | ANSI rendering | `src/overlay/render.rs` |
| 4 | Input mode state | `src/input/{mod,mode}.rs` |
| 5 | Key parsing | `src/input/keys.rs` |
| 6 | Overlay HTTP API | `src/api.rs` |
| 7 | Input capture HTTP API | `src/api.rs` |
| 8 | Overlay compositing | `src/main.rs` |
| 9 | Input routing | `src/main.rs` |
| 10 | Input broadcasting | `src/input/events.rs` |
| 11 | WebSocket subscriptions | `src/api.rs` |
| 12 | Integration tests | `tests/*.rs` |
| 13 | Documentation | `docs/plans/*.md` |
