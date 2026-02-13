# Generative UI Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enhance overlays and panels to support agent-drawn interactive terminal UIs — opaque backgrounds, explicit sizing, named spans, region writes, focusable input routing, built-in widgets, and alternate screen mode.

**Architecture:** Extend existing overlay/panel data model with `width`, `height`, `background`, and `screen_mode` fields. Add named span IDs for partial updates and a `region_write` API for cell-level drawing. Introduce focus tracking in the input module tied to overlay/panel IDs. Add a widget subsystem that renders into parent overlays/panels and handles keystrokes locally. Add session-level alt-screen mode that hides/restores elements by screen mode tag.

**Tech Stack:** Rust, tokio, axum, serde, uuid. Nix-wrapped cargo. All commands: `nix develop -c sh -c "cargo ..."`.

---

## Phase 1: Overlay Data Model — Size, Background, Named Spans

### Task 1: Add `width`, `height`, `background` to Overlay struct

**Files:**
- Modify: `src/overlay/types.rs:7-14` (Overlay struct)
- Test: `src/overlay/store.rs` (existing unit tests, update to compile)

**Step 1: Write the failing test**

Add to `src/overlay/store.rs` tests:

```rust
#[test]
fn test_create_overlay_with_size_and_background() {
    let store = OverlayStore::new();
    let bg = BackgroundStyle { bg: Color::Rgb { r: 30, g: 30, b: 30 } };
    let id = store.create(0, 0, None, 10, 5, Some(bg.clone()), vec![]);
    let overlay = store.get(&id).unwrap();
    assert_eq!(overlay.width, 10);
    assert_eq!(overlay.height, 5);
    assert_eq!(overlay.background.as_ref().unwrap().bg, bg.bg);
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop -c sh -c "cargo test overlay::store::tests::test_create_overlay_with_size_and_background -- --nocapture"`
Expected: FAIL — `BackgroundStyle` not defined, `create()` signature mismatch

**Step 3: Update Overlay struct and BackgroundStyle**

In `src/overlay/types.rs`, add:

```rust
/// Background fill style for overlays (always opaque)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundStyle {
    pub bg: Color,
}
```

Update the `Overlay` struct:

```rust
pub struct Overlay {
    pub id: OverlayId,
    pub x: u16,
    pub y: u16,
    pub z: i32,
    pub width: u16,
    pub height: u16,
    pub background: Option<BackgroundStyle>,
    pub spans: Vec<OverlaySpan>,
}
```

**Step 4: Update `OverlayStore::create()` signature**

In `src/overlay/store.rs`, update `create()` to accept `width`, `height`, `background`:

```rust
pub fn create(
    &self,
    x: u16,
    y: u16,
    z: Option<i32>,
    width: u16,
    height: u16,
    background: Option<BackgroundStyle>,
    spans: Vec<OverlaySpan>,
) -> OverlayId {
    // ... same body but include new fields in Overlay construction
    let overlay = Overlay { id: id.clone(), x, y, z, width, height, background, spans };
    // ...
}
```

**Step 5: Update `OverlayStore::move_to()` to accept `width` and `height`**

Add optional `width` and `height` parameters to `move_to()`:

```rust
pub fn move_to(
    &self,
    id: &str,
    x: Option<u16>,
    y: Option<u16>,
    z: Option<i32>,
    width: Option<u16>,
    height: Option<u16>,
) -> bool {
    // ... existing logic plus:
    if let Some(w) = width { overlay.width = w; }
    if let Some(h) = height { overlay.height = h; }
}
```

**Step 6: Fix all existing overlay tests in store.rs**

Every call to `store.create(x, y, z, spans)` needs `width, height, background` added. Use defaults: `width: 80, height: 1, background: None`.

Every call to `store.move_to(id, x, y, z)` needs `width: None, height: None` appended.

**Step 7: Fix all compilation errors across codebase**

The `create()` and `move_to()` signature changes will break:
- `src/api/handlers.rs` — `overlay_create()`, `overlay_patch()`
- `src/api/ws_methods.rs` — `create_overlay`, `patch_overlay` dispatch arms
- `src/overlay/render.rs` — tests that construct Overlay structs
- `src/panel/layout.rs` — tests that import OverlaySpan (no change needed)
- `tests/overlay_integration.rs` — integration tests
- `tests/ws_json_methods.rs` — if overlay methods are tested
- `tests/api_integration.rs` — if overlay endpoints are tested

For each, add default `width: 80, height: 1, background: None` to maintain existing behavior. The HTTP/WS request types need `width`, `height`, and `background` added.

**Step 8: Run full test suite**

Run: `nix develop -c sh -c "cargo test"`
Expected: ALL PASS

**Step 9: Commit**

```bash
git add -A && git commit -m "feat: add width, height, background to Overlay struct"
```

---

### Task 2: Add `id` field to OverlaySpan (named spans)

**Files:**
- Modify: `src/overlay/types.rs:17-30` (OverlaySpan struct)

**Step 1: Write the failing test**

Add to `src/overlay/store.rs` tests:

```rust
#[test]
fn test_create_overlay_with_named_spans() {
    let store = OverlayStore::new();
    let spans = vec![
        OverlaySpan {
            id: None,
            text: "CPU: ".to_string(),
            fg: None, bg: None, bold: false, italic: false, underline: false,
        },
        OverlaySpan {
            id: Some("cpu".to_string()),
            text: "52%".to_string(),
            fg: None, bg: None, bold: false, italic: false, underline: false,
        },
    ];
    let oid = store.create(0, 0, None, 80, 1, None, spans);
    let overlay = store.get(&oid).unwrap();
    assert_eq!(overlay.spans[0].id, None);
    assert_eq!(overlay.spans[1].id, Some("cpu".to_string()));
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop -c sh -c "cargo test overlay::store::tests::test_create_overlay_with_named_spans -- --nocapture"`
Expected: FAIL — `OverlaySpan` has no `id` field

**Step 3: Add `id` to OverlaySpan**

In `src/overlay/types.rs`:

```rust
pub struct OverlaySpan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub text: String,
    // ... rest unchanged
}
```

**Step 4: Fix all existing OverlaySpan constructions**

Every place that constructs `OverlaySpan` needs `id: None` added:
- `src/overlay/render.rs` — tests and the `render_overlay()` temp span
- `src/panel/layout.rs` — test helper `span()`
- `src/panel/render.rs` — test helper `span()`
- `tests/overlay_integration.rs`
- `tests/panel_integration.rs`
- Any other test files constructing spans

**Step 5: Run full test suite**

Run: `nix develop -c sh -c "cargo test"`
Expected: ALL PASS

**Step 6: Commit**

```bash
git add -A && git commit -m "feat: add optional id field to OverlaySpan for named spans"
```

---

### Task 3: Implement `update_spans()` — partial span update by ID

**Files:**
- Modify: `src/overlay/store.rs` (add method)
- Test: `src/overlay/store.rs` (unit test)

**Step 1: Write the failing test**

```rust
#[test]
fn test_update_spans_by_id() {
    let store = OverlayStore::new();
    let spans = vec![
        OverlaySpan {
            id: None, text: "CPU: ".to_string(),
            fg: None, bg: None, bold: false, italic: false, underline: false,
        },
        OverlaySpan {
            id: Some("cpu".to_string()), text: "52%".to_string(),
            fg: Some(Color::Named(NamedColor::Green)),
            bg: None, bold: false, italic: false, underline: false,
        },
    ];
    let oid = store.create(0, 0, None, 80, 1, None, spans);

    // Partial update: change the "cpu" span only
    let updates = vec![OverlaySpan {
        id: Some("cpu".to_string()), text: "78%".to_string(),
        fg: Some(Color::Named(NamedColor::Yellow)),
        bg: None, bold: false, italic: false, underline: false,
    }];
    assert!(store.update_spans(&oid, &updates));

    let overlay = store.get(&oid).unwrap();
    assert_eq!(overlay.spans[0].text, "CPU: "); // unchanged
    assert_eq!(overlay.spans[1].text, "78%");   // updated
    assert_eq!(overlay.spans[1].fg, Some(Color::Named(NamedColor::Yellow)));
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop -c sh -c "cargo test overlay::store::tests::test_update_spans_by_id -- --nocapture"`
Expected: FAIL — `update_spans` method doesn't exist

**Step 3: Implement `update_spans()`**

In `src/overlay/store.rs`:

```rust
/// Update specific spans by their `id` field.
///
/// For each span in `updates`, finds the span with matching `id` in the
/// overlay and replaces its text, colors, and attributes. Spans without
/// an `id` in `updates` are skipped. Returns false if overlay not found.
pub fn update_spans(&self, overlay_id: &str, updates: &[OverlaySpan]) -> bool {
    let mut inner = self.inner.write().unwrap();
    let overlay = match inner.overlays.get_mut(overlay_id) {
        Some(o) => o,
        None => return false,
    };
    for update in updates {
        if let Some(ref update_id) = update.id {
            if let Some(span) = overlay.spans.iter_mut().find(|s| s.id.as_deref() == Some(update_id)) {
                span.text = update.text.clone();
                span.fg = update.fg.clone();
                span.bg = update.bg.clone();
                span.bold = update.bold;
                span.italic = update.italic;
                span.underline = update.underline;
            }
        }
    }
    true
}
```

**Step 4: Run test to verify it passes**

Run: `nix develop -c sh -c "cargo test overlay::store::tests::test_update_spans_by_id -- --nocapture"`
Expected: PASS

**Step 5: Commit**

```bash
git add -A && git commit -m "feat: add update_spans() for partial span update by ID"
```

---

### Task 4: Add `background` to Panel struct

**Files:**
- Modify: `src/panel/types.rs:17-28` (Panel struct)
- Modify: `src/panel/store.rs` (update `create()` signature)

**Step 1: Write the failing test**

Add to `src/panel/store.rs` tests:

```rust
#[test]
fn test_create_panel_with_background() {
    let store = PanelStore::new();
    let bg = crate::overlay::BackgroundStyle { bg: crate::overlay::Color::Rgb { r: 30, g: 30, b: 30 } };
    let id = store.create(Position::Bottom, 1, None, Some(bg.clone()), vec![]);
    let panel = store.get(&id).unwrap();
    assert!(panel.background.is_some());
}
```

**Step 2: Run test to verify it fails**

Expected: FAIL — Panel has no `background` field, `create()` signature mismatch

**Step 3: Add `background` to Panel struct and update PanelStore**

In `src/panel/types.rs`:

```rust
pub struct Panel {
    pub id: PanelId,
    pub position: Position,
    pub height: u16,
    pub z: i32,
    pub background: Option<BackgroundStyle>,
    pub spans: Vec<OverlaySpan>,
    pub visible: bool,
}
```

Update `PanelStore::create()` to accept `background`:

```rust
pub fn create(
    &self,
    position: Position,
    height: u16,
    z: Option<i32>,
    background: Option<BackgroundStyle>,
    spans: Vec<OverlaySpan>,
) -> PanelId {
    // ... include background in Panel construction
}
```

Update `PanelStore::patch()` to accept optional `background`:

```rust
pub fn patch(
    &self,
    id: &str,
    position: Option<Position>,
    height: Option<u16>,
    z: Option<i32>,
    background: Option<Option<BackgroundStyle>>,
    spans: Option<Vec<OverlaySpan>>,
) -> bool { ... }
```

**Step 4: Fix all Panel constructions across codebase**

Add `background: None` to every `Panel` struct construction and `store.create()` call:
- `src/panel/store.rs` — tests
- `src/panel/layout.rs` — `make_panel()` test helper
- `src/panel/render.rs` — `make_panel()` test helper
- `src/panel/coordinator.rs` — if any constructions exist
- `src/api/handlers.rs` — `panel_create()`, `CreatePanelRequest`, `PatchPanelRequest`
- `src/api/ws_methods.rs` — `CreatePanelParams`, panel dispatch arms
- `tests/panel_integration.rs`
- `tests/common/mod.rs` — if panels are constructed

**Step 5: Run full test suite**

Run: `nix develop -c sh -c "cargo test"`
Expected: ALL PASS

**Step 6: Commit**

```bash
git add -A && git commit -m "feat: add background field to Panel struct"
```

---

### Task 5: Add `update_spans()` to PanelStore

Mirror the overlay `update_spans()` method for panels.

**Files:**
- Modify: `src/panel/store.rs` (add method)

**Step 1: Write the failing test**

```rust
#[test]
fn test_update_spans_by_id() {
    let store = PanelStore::new();
    let spans = vec![
        OverlaySpan {
            id: Some("status".to_string()), text: "running".to_string(),
            fg: None, bg: None, bold: false, italic: false, underline: false,
        },
    ];
    let pid = store.create(Position::Bottom, 1, None, None, spans);
    let updates = vec![OverlaySpan {
        id: Some("status".to_string()), text: "done".to_string(),
        fg: Some(Color::Named(NamedColor::Green)),
        bg: None, bold: false, italic: false, underline: false,
    }];
    assert!(store.update_spans(&pid, &updates));
    let panel = store.get(&pid).unwrap();
    assert_eq!(panel.spans[0].text, "done");
}
```

**Step 2: Run test to verify it fails**

Expected: FAIL — `update_spans` doesn't exist on `PanelStore`

**Step 3: Implement (same logic as OverlayStore)**

**Step 4: Run test, verify pass**

**Step 5: Commit**

```bash
git add -A && git commit -m "feat: add update_spans() to PanelStore for partial span update"
```

---

## Phase 2: Rendering — Background Fill, Region Writes

### Task 6: Render overlay background fill

**Files:**
- Modify: `src/overlay/render.rs` — `render_overlay()` function

**Step 1: Write the failing test**

```rust
#[test]
fn test_render_overlay_with_background_fills_rectangle() {
    let overlay = Overlay {
        id: "t".to_string(),
        x: 0, y: 0, z: 0,
        width: 5, height: 2,
        background: Some(BackgroundStyle { bg: Color::Rgb { r: 30, g: 30, b: 30 } }),
        spans: vec![],
    };
    let result = render_overlay(&overlay);
    // Should contain cursor positioning for rows 0 and 1
    assert!(result.contains("\x1b[1;1H")); // row 0
    assert!(result.contains("\x1b[2;1H")); // row 1
    // Should contain background color code
    assert!(result.contains("\x1b[48;2;30;30;30m"));
    // Should fill 5 cells per row with spaces
    // (background color + 5 spaces per row)
}
```

**Step 2: Run test to verify it fails**

**Step 3: Update `render_overlay()` to fill background first**

Before rendering spans, iterate over the overlay's `(width x height)` rectangle and fill with the background color. For each row, position cursor and write `width` spaces with the background style.

**Step 4: Run test, verify pass**

**Step 5: Run full test suite (existing render tests may need Overlay struct updates)**

**Step 6: Commit**

```bash
git add -A && git commit -m "feat: render opaque background fill for overlays"
```

---

### Task 7: Render panel background fill

**Files:**
- Modify: `src/panel/render.rs` — `render_panel()` function

**Step 1: Write the failing test**

```rust
#[test]
fn test_render_panel_with_background() {
    let panel = Panel {
        id: "t".to_string(),
        position: Position::Bottom,
        height: 2,
        z: 0,
        background: Some(BackgroundStyle { bg: Color::Named(NamedColor::Blue) }),
        spans: vec![span("hello")],
        visible: true,
    };
    let result = render_panel(&panel, 22, 10);
    // Should contain blue background code
    assert!(result.contains("\x1b[44m"));
}
```

**Step 2: Run test to verify it fails**

**Step 3: Update `render_panel()` to use background when filling empty space**

Currently, empty space is filled with plain spaces. When `background` is set, fill with background-styled spaces instead.

**Step 4: Run tests, verify pass**

**Step 5: Commit**

```bash
git add -A && git commit -m "feat: render background fill for panels"
```

---

### Task 8: Implement region_write for overlays and panels

Region writes let agents write styled text at specific `(row, col)` offsets within an overlay or panel. This is a new rendering concept — we need to store region write data and render it.

**Files:**
- Modify: `src/overlay/types.rs` — add `RegionWrite` struct
- Modify: `src/overlay/store.rs` — add `region_write()` method
- Modify: `src/panel/store.rs` — add `region_write()` method

**Design decision:** Region writes are stored as a separate `Vec<RegionWrite>` on the Overlay/Panel structs. During rendering, spans are rendered first, then region writes are rendered on top (they overwrite). This keeps the span system intact while adding cell-level control.

**Step 1: Define RegionWrite type**

In `src/overlay/types.rs`:

```rust
/// A styled text write at a specific (row, col) offset within an overlay/panel.
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
```

**Step 2: Add `region_writes` field to Overlay and Panel**

```rust
pub struct Overlay {
    // ... existing fields ...
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub region_writes: Vec<RegionWrite>,
}
```

Same for `Panel`.

**Step 3: Add `region_write()` method to OverlayStore and PanelStore**

```rust
/// Apply region writes to an overlay. Replaces the stored region writes.
pub fn region_write(&self, id: &str, writes: Vec<RegionWrite>) -> bool {
    let mut inner = self.inner.write().unwrap();
    if let Some(overlay) = inner.overlays.get_mut(id) {
        overlay.region_writes = writes;
        true
    } else {
        false
    }
}
```

**Step 4: Write unit tests for region_write storage**

**Step 5: Fix all Overlay and Panel struct constructions (add `region_writes: vec![]`)**

**Step 6: Run full test suite**

**Step 7: Commit**

```bash
git add -A && git commit -m "feat: add region_write storage for overlays and panels"
```

---

### Task 9: Render region writes

**Files:**
- Modify: `src/overlay/render.rs` — update `render_overlay()` to render region writes
- Modify: `src/panel/render.rs` — update `render_panel()` to render region writes

**Step 1: Write the failing test**

```rust
#[test]
fn test_render_overlay_with_region_writes() {
    let overlay = Overlay {
        id: "t".to_string(),
        x: 10, y: 5, z: 0,
        width: 20, height: 3,
        background: None,
        spans: vec![],
        region_writes: vec![
            RegionWrite {
                row: 1, col: 5, text: "hello".to_string(),
                fg: Some(Color::Named(NamedColor::Green)),
                bg: None, bold: false, italic: false, underline: false,
            },
        ],
    };
    let result = render_overlay(&overlay);
    // Region write at (1, 5) within overlay at (10, 5)
    // Absolute position: row = 5+1 = 6, col = 10+5 = 15
    // 1-indexed: \x1b[7;16H
    assert!(result.contains("\x1b[7;16H"));
    assert!(result.contains("hello"));
    assert!(result.contains("\x1b[32m")); // green
}
```

**Step 2: Run test to verify it fails**

**Step 3: Implement — after rendering spans, iterate `region_writes` and position cursor + render each**

For overlays: absolute position = `(overlay.y + write.row, overlay.x + write.col)`.
For panels: absolute position = `(panel_start_row + write.row, write.col)`.

**Step 4: Run tests, verify pass**

**Step 5: Commit**

```bash
git add -A && git commit -m "feat: render region writes for overlays and panels"
```

---

### Task 10: Zero-row PTY minimum

**Files:**
- Modify: `src/panel/layout.rs:23` — change `MIN_PTY_ROWS` from 1 to 0

**Step 1: Write the failing test**

```rust
#[test]
fn test_panels_can_consume_all_rows() {
    let panels = vec![make_panel("a", Position::Top, 24, 0)];
    let layout = compute_layout(&panels, 24, 80);
    assert_eq!(layout.pty_rows, 0);
    assert!(layout.hidden_panels.is_empty());
}
```

**Step 2: Run test to verify it fails**

Expected: FAIL — panel is hidden because MIN_PTY_ROWS = 1

**Step 3: Change `MIN_PTY_ROWS` to 0**

```rust
const MIN_PTY_ROWS: u16 = 0;
```

**Step 4: Update existing tests that assert MIN_PTY_ROWS = 1 behavior**

The test `test_terminal_one_row_no_panels_possible` currently expects panels to be hidden on a 1-row terminal. With MIN_PTY_ROWS = 0, the panel should now fit. Update this test.

The test `test_panels_exceeding_height_hides_lowest_z` and `test_exactly_one_pty_row_remaining` may need adjustments.

**Step 5: Run full test suite**

Run: `nix develop -c sh -c "cargo test"`
Expected: ALL PASS

**Step 6: Commit**

```bash
git add -A && git commit -m "feat: allow zero PTY rows — panels can consume entire screen"
```

---

## Phase 3: API Layer — HTTP & WebSocket Updates

### Task 11: Update HTTP overlay endpoints for new fields

**Files:**
- Modify: `src/api/handlers.rs` — `CreateOverlayRequest`, `PatchOverlayRequest`, overlay handlers
- Modify: `src/api/ws_methods.rs` — `CreateOverlayParams`, overlay WS methods

**Step 1: Update `CreateOverlayRequest`**

```rust
pub(super) struct CreateOverlayRequest {
    x: u16,
    y: u16,
    z: Option<i32>,
    width: u16,
    height: u16,
    background: Option<BackgroundStyle>,
    #[serde(default)]
    spans: Vec<OverlaySpan>,
}
```

**Step 2: Update `PatchOverlayRequest` to accept width, height, background**

```rust
pub(super) struct PatchOverlayRequest {
    x: Option<u16>,
    y: Option<u16>,
    z: Option<i32>,
    width: Option<u16>,
    height: Option<u16>,
    background: Option<BackgroundStyle>,
}
```

**Step 3: Add `update_spans` HTTP endpoint and WS method**

New endpoint: `POST /sessions/{name}/overlay/{id}/spans` with body `{"spans": [...]}`
New WS method: `update_overlay_spans`

These call `store.update_spans()`.

**Step 4: Add `region_write` HTTP endpoint and WS method**

New endpoint: `POST /sessions/{name}/overlay/{id}/write` with body `{"writes": [...]}`
New WS method: `overlay_region_write`

Same for panels: `POST /sessions/{name}/panel/{id}/write`

**Step 5: Add `batch_update` WS method**

New WS method: `batch_update` that accepts `id`, optional `update_spans`, optional `writes`, and applies both atomically.

**Step 6: Update all handler functions to pass new fields through**

**Step 7: Run full test suite**

**Step 8: Commit**

```bash
git add -A && git commit -m "feat: update HTTP/WS API for overlay size, background, named spans, region writes"
```

---

### Task 12: Update HTTP panel endpoints for new fields

**Files:**
- Modify: `src/api/handlers.rs` — `CreatePanelRequest`, `PatchPanelRequest`
- Modify: `src/api/ws_methods.rs` — `CreatePanelParams`

Same pattern as Task 11 but for panels. Add background to create/patch requests,
add `update_panel_spans` and `panel_region_write` WS methods.

**Step 1-6: Mirror Task 11 for panels**

**Step 7: Commit**

```bash
git add -A && git commit -m "feat: update panel HTTP/WS API for background, named spans, region writes"
```

---

## Phase 4: Input Routing — Focus Tracking

### Task 13: Add focus tracking to Session

**Files:**
- Create: `src/input/focus.rs`
- Modify: `src/input/mod.rs` — export new module
- Modify: `src/session.rs` — add `FocusTracker` field

**Step 1: Write the failing test**

```rust
#[test]
fn test_focus_tracker_basics() {
    let tracker = FocusTracker::new();
    assert_eq!(tracker.focused(), None);

    tracker.focus("overlay-1".to_string());
    assert_eq!(tracker.focused(), Some("overlay-1".to_string()));

    tracker.unfocus();
    assert_eq!(tracker.focused(), None);
}
```

**Step 2: Run test to verify it fails**

**Step 3: Implement FocusTracker**

```rust
/// Tracks which overlay/panel currently has input focus.
///
/// At most one element has focus at a time. Focus requires input capture
/// mode to be active — the FocusTracker doesn't enforce this itself;
/// the API layer checks capture mode before routing input.
#[derive(Clone)]
pub struct FocusTracker {
    inner: Arc<RwLock<Option<String>>>,
}

impl FocusTracker {
    pub fn new() -> Self { ... }
    pub fn focus(&self, id: String) { ... }
    pub fn unfocus(&self) { ... }
    pub fn focused(&self) -> Option<String> { ... }
    pub fn clear_if_focused(&self, id: &str) { ... } // unfocus only if this id has focus
}
```

**Step 4: Add FocusTracker to Session struct**

In `src/session.rs`, add `pub focus: FocusTracker` to `Session`.

**Step 5: Fix all Session constructions**

Add `focus: FocusTracker::new()` to:
- `src/session.rs` — `spawn_with_options()`, test helpers
- `tests/common/mod.rs` — `create_test_session_with_size()`
- Anywhere else Session is constructed

**Step 6: Run full test suite**

**Step 7: Commit**

```bash
git add -A && git commit -m "feat: add FocusTracker for scoped input routing"
```

---

### Task 14: Add `focusable` field to Overlay and Panel

**Files:**
- Modify: `src/overlay/types.rs` — add `focusable: bool` to Overlay
- Modify: `src/panel/types.rs` — add `focusable: bool` to Panel

**Step 1: Add `focusable` field with `#[serde(default)]`**

**Step 2: Fix all struct constructions (add `focusable: false`)**

**Step 3: Run full test suite**

**Step 4: Commit**

```bash
git add -A && git commit -m "feat: add focusable field to Overlay and Panel"
```

---

### Task 15: Wire focus into input event routing

**Files:**
- Modify: `src/input/events.rs` — add `target` field to InputEvent::Input

**Step 1: Add `target: Option<String>` to InputEvent::Input variant**

When input is captured and an element has focus, the target is the focused element's ID. When no element is focused, target is None.

**Step 2: Update `broadcast_input()` to accept target**

**Step 3: Update all callers of `broadcast_input()`**

**Step 4: Add focus/unfocus HTTP endpoints and WS methods**

- `POST /sessions/{name}/input/focus` — body `{"id": "overlay-id"}`
- `POST /sessions/{name}/input/unfocus`
- WS methods: `focus`, `unfocus`

**Step 5: Wire focus clearing into `input_release()` handler**

When input is released (passthrough mode), clear the focus tracker.

**Step 6: Wire focus clearing into overlay/panel delete handlers**

When an overlay or panel is deleted, call `focus.clear_if_focused(&id)`.

**Step 7: Run full test suite**

**Step 8: Commit**

```bash
git add -A && git commit -m "feat: wire focus tracking into input event routing"
```

---

## Phase 5: Alternate Screen Mode

### Task 16: Add screen mode tagging to overlays and panels

**Files:**
- Modify: `src/overlay/types.rs` — add `screen_mode: ScreenMode` to Overlay
- Modify: `src/panel/types.rs` — add `screen_mode: ScreenMode` to Panel

**Step 1: Define ScreenMode enum**

In `src/overlay/types.rs` (shared, since panels already import from overlay):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenMode {
    Normal,
    Alt,
}
```

**Step 2: Add `screen_mode` to Overlay and Panel structs**

Default to `Normal`. This field is set automatically at creation time based on the session's current screen mode — not specified by the agent.

**Step 3: Add `screen_mode` field to Session**

```rust
pub screen_mode: Arc<RwLock<ScreenMode>>,
```

**Step 4: Fix all constructions**

**Step 5: Run full test suite**

**Step 6: Commit**

```bash
git add -A && git commit -m "feat: add screen mode tagging to overlays, panels, and sessions"
```

---

### Task 17: Implement enter/exit alt screen

**Files:**
- Modify: `src/api/handlers.rs` — add `enter_alt_screen`, `exit_alt_screen` handlers
- Modify: `src/api/ws_methods.rs` — add WS methods
- Modify: `src/overlay/store.rs` — add `list_by_mode()`, `delete_by_mode()`
- Modify: `src/panel/store.rs` — add `list_by_mode()`, `delete_by_mode()`

**Step 1: Add store methods for mode-based operations**

```rust
/// List overlays for a specific screen mode
pub fn list_by_mode(&self, mode: ScreenMode) -> Vec<Overlay> { ... }

/// Delete all overlays for a specific screen mode
pub fn delete_by_mode(&self, mode: ScreenMode) { ... }
```

**Step 2: Implement `enter_alt_screen` handler**

1. Check session isn't already in alt mode (return error if so)
2. Set session.screen_mode to Alt
3. Write `\x1b[?1049h` to stdout (if local session)
4. Normal-mode overlays/panels are hidden (not rendered, but still in store)

**Step 3: Implement `exit_alt_screen` handler**

1. Check session is in alt mode
2. Delete all alt-mode overlays and panels
3. Set session.screen_mode to Normal
4. Write `\x1b[?1049l` to stdout (if local session)
5. Reconfigure panel layout (restores normal-mode panels)
6. Re-render normal-mode overlays

**Step 4: Wire overlay/panel creation to tag with current screen mode**

In `overlay_create()` and `panel_create()` handlers, read the session's current screen mode and set it on the new element.

**Step 5: Add HTTP endpoints**

- `POST /sessions/{name}/screen/alt` — enter alt screen
- `DELETE /sessions/{name}/screen/alt` — exit alt screen
- `GET /sessions/{name}/screen` — get current screen mode

**Step 6: Add WS methods**: `enter_alt_screen`, `exit_alt_screen`, `get_screen_mode`

**Step 7: Write integration tests**

**Step 8: Run full test suite**

**Step 9: Commit**

```bash
git add -A && git commit -m "feat: implement enter/exit alternate screen mode with element lifecycle"
```

---

## Phase 6: Widgets (future — separate plan)

Widgets are a significant subsystem. They should be designed and implemented in a separate plan after Phases 1-5 are complete and validated. The widget system includes:

- New `src/widget/` module with types, store, rendering, and keystroke handling
- Widget-to-parent binding (widgets live inside overlays/panels)
- Local keystroke processing (no round-trip to agent)
- Widget event emission via WebSocket
- Seven widget types: `text_input`, `select_list`, `radio_list`, `checkbox_list`, `confirm`, `text_display`, `progress_bar`
- Tab/Shift-Tab focus navigation between widgets within a parent
- HTTP and WebSocket API for widget CRUD

**Recommendation:** Implement Phases 1-5 first. Validate with real agent usage. Then design widgets based on actual interaction patterns observed.

---

## Phase 7: Documentation Updates

### Task 18: Update API documentation

**Files:**
- Modify: `docs/api/overlays.md` — add `width`, `height`, `background`, named spans, `update_spans`, `region_write`
- Modify: `docs/api/panels.md` — add `background`, named spans, `update_spans`, `region_write`
- Modify: `docs/api/input-capture.md` — add focus tracking, `focus`/`unfocus` endpoints
- Modify: `docs/api/README.md` — add new endpoints to overview, add alt screen section
- Create: `docs/api/alt-screen.md` — alternate screen mode documentation
- Modify: `docs/api/websocket.md` — add new WS methods
- Modify: `docs/api/openapi.yaml` — add all new schemas and endpoints

For each doc file:
1. Read the current content
2. Add new sections for each new capability
3. Update existing sections where behavior changed (overlay creation now requires width/height)
4. Add examples showing the new fields

**Commit:**

```bash
git add -A && git commit -m "docs: update API documentation for generative UI features"
```

---

### Task 19: Update skills

**Files:**
- Modify: `skills/wsh/generative-ui/SKILL.md` — major rewrite: replace "Layer 3: Generated Programs" with direct drawing capabilities, add opaque overlays, named spans, region writes, alt screen patterns
- Modify: `skills/wsh/visual-feedback/SKILL.md` — update overlay section for `width`, `height`, `background`; update panel section for `background`; add named spans and region writes to rendering guidance
- Modify: `skills/wsh/input-capture/SKILL.md` — add focus routing section, update examples to use focusable overlays
- Modify: `skills/wsh/core/SKILL.md` — update API overview with new endpoints; add alt screen mode to capabilities list

For each skill:
1. Read the current SKILL.md
2. Identify sections that reference old behavior (e.g., transparent overlays, span-only rendering)
3. Update to reflect new capabilities
4. Keep the skill focused on "what" (content/patterns), not "how" (protocol details), per CLAUDE.md instructions
5. Ensure no skill refers to specific API endpoints — that's the core skill's job

**Commit:**

```bash
git add -A && git commit -m "docs: update skills for generative UI capabilities"
```

---

## Phase 8: Integration Tests

### Task 20: Integration tests for overlay enhancements

**Files:**
- Modify: `tests/overlay_integration.rs`

Test:
- Create overlay with width, height, background via HTTP
- Create overlay with named spans, update individual spans via HTTP
- Region write via HTTP
- Verify JSON responses include new fields

### Task 21: Integration tests for panel enhancements

**Files:**
- Modify: `tests/panel_integration.rs`

Test:
- Create panel with background via HTTP
- Named span updates on panels
- Zero-row PTY behavior (panels consuming all rows)

### Task 22: Integration tests for focus and alt screen

**Files:**
- Modify: `tests/input_capture_integration.rs`
- Create: `tests/alt_screen_integration.rs`

Test:
- Focus/unfocus via HTTP
- Focus cleared on input release
- Focus cleared on element deletion
- Alt screen enter/exit via HTTP
- Alt-screen elements destroyed on exit
- Normal-mode elements restored on exit

**Commit after all integration tests pass:**

```bash
git add -A && git commit -m "test: integration tests for generative UI features"
```

---

## Summary of Changes by File

| File | Changes |
|------|---------|
| `src/overlay/types.rs` | Add `width`, `height`, `background`, `region_writes` to Overlay; add `id` to OverlaySpan; add `BackgroundStyle`, `RegionWrite`, `ScreenMode` types |
| `src/overlay/store.rs` | Update `create()` signature; add `update_spans()`, `region_write()`, `list_by_mode()`, `delete_by_mode()` |
| `src/overlay/render.rs` | Background fill rendering; region write rendering |
| `src/overlay/mod.rs` | Export new types |
| `src/panel/types.rs` | Add `background`, `focusable`, `screen_mode`, `region_writes` to Panel |
| `src/panel/store.rs` | Update `create()` and `patch()` signatures; add `update_spans()`, `region_write()`, mode methods |
| `src/panel/render.rs` | Background fill rendering; region write rendering |
| `src/panel/layout.rs` | Change `MIN_PTY_ROWS` from 1 to 0 |
| `src/input/mod.rs` | Export `focus` module |
| `src/input/focus.rs` | New: `FocusTracker` |
| `src/input/events.rs` | Add `target` to InputEvent::Input |
| `src/session.rs` | Add `focus`, `screen_mode` fields |
| `src/api/handlers.rs` | Update request types; add focus, alt screen, region_write, update_spans endpoints |
| `src/api/ws_methods.rs` | Add WS methods for all new features |
| `src/api/mod.rs` | Add routes for new endpoints |
| `tests/common/mod.rs` | Update Session construction |
| `tests/overlay_integration.rs` | New tests for enhanced overlays |
| `tests/panel_integration.rs` | New tests for enhanced panels |
| `tests/input_capture_integration.rs` | New tests for focus |
| `tests/alt_screen_integration.rs` | New: alt screen integration tests |
| `docs/api/overlays.md` | Document new fields and endpoints |
| `docs/api/panels.md` | Document new fields and endpoints |
| `docs/api/input-capture.md` | Document focus routing |
| `docs/api/alt-screen.md` | New: alt screen documentation |
| `docs/api/README.md` | Add new endpoints overview |
| `docs/api/websocket.md` | Add new WS methods |
| `docs/api/openapi.yaml` | Add all new schemas and paths |
| `skills/wsh/generative-ui/SKILL.md` | Major rewrite for direct drawing |
| `skills/wsh/visual-feedback/SKILL.md` | Update for opacity, named spans |
| `skills/wsh/input-capture/SKILL.md` | Add focus routing |
| `skills/wsh/core/SKILL.md` | Add new capabilities overview |
