use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;
use uuid::Uuid;

use crate::overlay::{BackgroundStyle, OverlaySpan, RegionWrite, ScreenMode};

use super::types::{Panel, PanelId, Position};

const MAX_PANELS: usize = 256;
const MAX_SPANS_PER_PANEL: usize = 4096;

/// Thread-safe store for panels
#[derive(Clone)]
pub struct PanelStore {
    inner: Arc<RwLock<StoreInner>>,
}

struct StoreInner {
    panels: HashMap<PanelId, Panel>,
    next_z: i32,
}

impl PanelStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(StoreInner {
                panels: HashMap::new(),
                next_z: 0,
            })),
        }
    }

    /// Create a new panel, returns its ID or an error if limits are exceeded.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &self,
        position: Position,
        height: u16,
        z: Option<i32>,
        background: Option<BackgroundStyle>,
        spans: Vec<OverlaySpan>,
        focusable: bool,
        screen_mode: ScreenMode,
    ) -> Result<PanelId, &'static str> {
        let mut inner = self.inner.write();
        if inner.panels.len() >= MAX_PANELS {
            return Err("maximum panel count reached");
        }
        if spans.len() > MAX_SPANS_PER_PANEL {
            return Err("too many spans");
        }
        let id = Uuid::new_v4().to_string();
        let z = z.unwrap_or_else(|| {
            let z = inner.next_z;
            inner.next_z = inner.next_z.saturating_add(1);
            z
        });
        if z >= inner.next_z {
            inner.next_z = z.saturating_add(1);
        }
        let panel = Panel {
            id: id.clone(),
            position,
            height,
            z,
            background,
            spans,
            region_writes: vec![],
            visible: true,
            focusable,
            screen_mode,
        };
        inner.panels.insert(id.clone(), panel);
        Ok(id)
    }

    /// Get a panel by ID
    pub fn get(&self, id: &str) -> Option<Panel> {
        let inner = self.inner.read();
        inner.panels.get(id).cloned()
    }

    /// List all panels, sorted by position (Top first) then z descending
    pub fn list(&self) -> Vec<Panel> {
        let inner = self.inner.read();
        let mut panels: Vec<_> = inner.panels.values().cloned().collect();
        panels.sort_by(|a, b| {
            let pos_ord = match (&a.position, &b.position) {
                (Position::Top, Position::Bottom) => std::cmp::Ordering::Less,
                (Position::Bottom, Position::Top) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            };
            pos_ord.then(b.z.cmp(&a.z))
        });
        panels
    }

    /// Update a panel's spans (full replacement)
    pub fn update(&self, id: &str, spans: Vec<OverlaySpan>) -> bool {
        let mut inner = self.inner.write();
        if let Some(panel) = inner.panels.get_mut(id) {
            panel.spans = spans;
            true
        } else {
            false
        }
    }

    /// Patch a panel's properties (partial update)
    ///
    /// Returns true if the panel was found and updated.
    pub fn patch(
        &self,
        id: &str,
        position: Option<Position>,
        height: Option<u16>,
        z: Option<i32>,
        background: Option<BackgroundStyle>,
        spans: Option<Vec<OverlaySpan>>,
    ) -> bool {
        let mut inner = self.inner.write();
        if !inner.panels.contains_key(id) {
            return false;
        }
        if let Some(z) = z {
            if z >= inner.next_z {
                inner.next_z = z.saturating_add(1);
            }
        }
        let panel = inner.panels.get_mut(id).unwrap();
        if let Some(position) = position {
            panel.position = position;
        }
        if let Some(height) = height {
            panel.height = height;
        }
        if let Some(z) = z {
            panel.z = z;
        }
        if let Some(background) = background {
            panel.background = Some(background);
        }
        if let Some(spans) = spans {
            panel.spans = spans;
        }
        true
    }

    /// Update specific spans by their `id` field.
    ///
    /// For each span in `updates`, find the span with matching `id` in the panel
    /// and replace its text, colors, and attributes. Returns false if panel not found.
    pub fn update_spans(&self, panel_id: &str, updates: &[OverlaySpan]) -> bool {
        let mut inner = self.inner.write();
        if let Some(panel) = inner.panels.get_mut(panel_id) {
            for update in updates {
                if let Some(ref update_id) = update.id {
                    for span in &mut panel.spans {
                        if span.id.as_deref() == Some(update_id) {
                            span.text = update.text.clone();
                            span.fg = update.fg.clone();
                            span.bg = update.bg.clone();
                            span.bold = update.bold;
                            span.italic = update.italic;
                            span.underline = update.underline;
                        }
                    }
                }
            }
            true
        } else {
            false
        }
    }

    /// Replace the stored region writes for a panel.
    ///
    /// Returns false if the panel does not exist.
    pub fn region_write(&self, id: &str, writes: Vec<RegionWrite>) -> bool {
        let mut inner = self.inner.write();
        if let Some(panel) = inner.panels.get_mut(id) {
            panel.region_writes = writes;
            true
        } else {
            false
        }
    }

    /// Set visibility for a panel (called by layout engine)
    pub fn set_visible(&self, id: &str, visible: bool) {
        let mut inner = self.inner.write();
        if let Some(panel) = inner.panels.get_mut(id) {
            panel.visible = visible;
        }
    }

    /// Delete a panel by ID, returns true if it existed
    pub fn delete(&self, id: &str) -> bool {
        let mut inner = self.inner.write();
        inner.panels.remove(id).is_some()
    }

    /// List panels for a specific screen mode, sorted by position then z descending
    pub fn list_by_mode(&self, mode: ScreenMode) -> Vec<Panel> {
        let inner = self.inner.read();
        let mut panels: Vec<_> = inner
            .panels
            .values()
            .filter(|p| p.screen_mode == mode)
            .cloned()
            .collect();
        panels.sort_by(|a, b| {
            let pos_ord = match (&a.position, &b.position) {
                (Position::Top, Position::Bottom) => std::cmp::Ordering::Less,
                (Position::Bottom, Position::Top) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            };
            pos_ord.then(b.z.cmp(&a.z))
        });
        panels
    }

    /// Delete all panels for a specific screen mode
    pub fn delete_by_mode(&self, mode: ScreenMode) {
        let mut inner = self.inner.write();
        inner.panels.retain(|_, p| p.screen_mode != mode);
    }

    /// Clear all panels
    pub fn clear(&self) {
        let mut inner = self.inner.write();
        inner.panels.clear();
    }
}

impl Default for PanelStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_panel() {
        let store = PanelStore::new();
        let id = store.create(Position::Bottom, 1, None, None, vec![], false, ScreenMode::Normal).unwrap();
        assert!(!id.is_empty());
    }

    #[test]
    fn test_get_panel() {
        let store = PanelStore::new();
        let id = store.create(Position::Top, 2, Some(10), None, vec![], false, ScreenMode::Normal).unwrap();
        let panel = store.get(&id).unwrap();
        assert_eq!(panel.position, Position::Top);
        assert_eq!(panel.height, 2);
        assert_eq!(panel.z, 10);
        assert!(panel.visible);
    }

    #[test]
    fn test_list_sorted_by_position_then_z_desc() {
        let store = PanelStore::new();
        store.create(Position::Bottom, 1, Some(5), None, vec![], false, ScreenMode::Normal).unwrap();
        store.create(Position::Top, 1, Some(3), None, vec![], false, ScreenMode::Normal).unwrap();
        store.create(Position::Top, 1, Some(10), None, vec![], false, ScreenMode::Normal).unwrap();
        store.create(Position::Bottom, 1, Some(20), None, vec![], false, ScreenMode::Normal).unwrap();

        let list = store.list();
        assert_eq!(list.len(), 4);
        // Top panels first, z descending
        assert_eq!(list[0].position, Position::Top);
        assert_eq!(list[0].z, 10);
        assert_eq!(list[1].position, Position::Top);
        assert_eq!(list[1].z, 3);
        // Bottom panels next, z descending
        assert_eq!(list[2].position, Position::Bottom);
        assert_eq!(list[2].z, 20);
        assert_eq!(list[3].position, Position::Bottom);
        assert_eq!(list[3].z, 5);
    }

    #[test]
    fn test_update_spans() {
        let store = PanelStore::new();
        let id = store.create(Position::Bottom, 1, None, None, vec![], false, ScreenMode::Normal).unwrap();
        let new_spans = vec![OverlaySpan {
            text: "updated".to_string(),
            id: None,
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }];
        assert!(store.update(&id, new_spans));
        let panel = store.get(&id).unwrap();
        assert_eq!(panel.spans[0].text, "updated");
    }

    #[test]
    fn test_patch_partial() {
        let store = PanelStore::new();
        let id = store.create(Position::Bottom, 1, Some(0), None, vec![], false, ScreenMode::Normal).unwrap();

        // Patch only height
        assert!(store.patch(&id, None, Some(3), None, None, None));
        let panel = store.get(&id).unwrap();
        assert_eq!(panel.height, 3);
        assert_eq!(panel.position, Position::Bottom);
        assert_eq!(panel.z, 0);

        // Patch position and z
        assert!(store.patch(&id, Some(Position::Top), None, Some(99), None, None));
        let panel = store.get(&id).unwrap();
        assert_eq!(panel.position, Position::Top);
        assert_eq!(panel.z, 99);
        assert_eq!(panel.height, 3);
    }

    #[test]
    fn test_delete_panel() {
        let store = PanelStore::new();
        let id = store.create(Position::Top, 1, None, None, vec![], false, ScreenMode::Normal).unwrap();
        assert!(store.delete(&id));
        assert!(store.get(&id).is_none());
    }

    #[test]
    fn test_delete_nonexistent() {
        let store = PanelStore::new();
        assert!(!store.delete("nonexistent"));
    }

    #[test]
    fn test_clear_panels() {
        let store = PanelStore::new();
        store.create(Position::Top, 1, None, None, vec![], false, ScreenMode::Normal).unwrap();
        store.create(Position::Bottom, 1, None, None, vec![], false, ScreenMode::Normal).unwrap();
        store.clear();
        assert!(store.list().is_empty());
    }

    #[test]
    fn test_auto_increment_z() {
        let store = PanelStore::new();
        let id1 = store.create(Position::Bottom, 1, None, None, vec![], false, ScreenMode::Normal).unwrap();
        let id2 = store.create(Position::Bottom, 1, None, None, vec![], false, ScreenMode::Normal).unwrap();
        let p1 = store.get(&id1).unwrap();
        let p2 = store.get(&id2).unwrap();
        assert!(p2.z > p1.z);
    }

    #[test]
    fn test_set_visible() {
        let store = PanelStore::new();
        let id = store.create(Position::Top, 1, None, None, vec![], false, ScreenMode::Normal).unwrap();
        assert!(store.get(&id).unwrap().visible);
        store.set_visible(&id, false);
        assert!(!store.get(&id).unwrap().visible);
        store.set_visible(&id, true);
        assert!(store.get(&id).unwrap().visible);
    }

    #[test]
    fn test_update_spans_by_id() {
        use crate::overlay::types::{Color, NamedColor};

        let store = PanelStore::new();
        let spans = vec![
            OverlaySpan {
                id: Some("label".to_string()),
                text: "Status: ".to_string(),
                fg: None,
                bg: None,
                bold: true,
                italic: false,
                underline: false,
            },
            OverlaySpan {
                id: Some("status".to_string()),
                text: "pending".to_string(),
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            },
        ];
        let pid = store.create(Position::Bottom, 1, None, None, spans, false, ScreenMode::Normal).unwrap();

        // Update only the "status" span
        let updates = vec![OverlaySpan {
            id: Some("status".to_string()),
            text: "complete".to_string(),
            fg: Some(Color::Named(NamedColor::Green)),
            bg: None,
            bold: false,
            italic: false,
            underline: true,
        }];
        assert!(store.update_spans(&pid, &updates));

        let panel = store.get(&pid).unwrap();
        // "label" span should be unchanged
        assert_eq!(panel.spans[0].text, "Status: ");
        assert!(panel.spans[0].bold);
        assert_eq!(panel.spans[0].fg, None);
        // "status" span should be updated
        assert_eq!(panel.spans[1].text, "complete");
        assert_eq!(panel.spans[1].fg, Some(Color::Named(NamedColor::Green)));
        assert!(panel.spans[1].underline);
    }

    #[test]
    fn test_update_spans_nonexistent_panel() {
        let store = PanelStore::new();
        let updates = vec![OverlaySpan {
            id: Some("value".to_string()),
            text: "new".to_string(),
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }];
        assert!(!store.update_spans("nonexistent", &updates));
    }

    #[test]
    fn test_update_spans_no_matching_span_id() {
        let store = PanelStore::new();
        let spans = vec![OverlaySpan {
            id: Some("label".to_string()),
            text: "Hello".to_string(),
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }];
        let pid = store.create(Position::Bottom, 1, None, None, spans, false, ScreenMode::Normal).unwrap();

        // Update with a span ID that doesn't match anything
        let updates = vec![OverlaySpan {
            id: Some("nonexistent_span".to_string()),
            text: "Goodbye".to_string(),
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }];
        assert!(store.update_spans(&pid, &updates));

        // Original span should be unchanged
        let panel = store.get(&pid).unwrap();
        assert_eq!(panel.spans[0].text, "Hello");
    }

    #[test]
    fn test_region_write_stores_writes() {
        use crate::overlay::types::{Color, NamedColor};

        let store = PanelStore::new();
        let pid = store.create(Position::Bottom, 3, None, None, vec![], false, ScreenMode::Normal).unwrap();

        let writes = vec![
            RegionWrite {
                row: 0,
                col: 0,
                text: "A".to_string(),
                fg: Some(Color::Named(NamedColor::Red)),
                bg: None,
                bold: true,
                italic: false,
                underline: false,
            },
            RegionWrite {
                row: 1,
                col: 5,
                text: "B".to_string(),
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            },
        ];
        assert!(store.region_write(&pid, writes));

        let panel = store.get(&pid).unwrap();
        assert_eq!(panel.region_writes.len(), 2);
        assert_eq!(panel.region_writes[0].text, "A");
        assert_eq!(panel.region_writes[0].row, 0);
        assert!(panel.region_writes[0].bold);
        assert_eq!(panel.region_writes[1].text, "B");
        assert_eq!(panel.region_writes[1].col, 5);
    }

    #[test]
    fn test_region_write_replaces_previous() {
        let store = PanelStore::new();
        let pid = store.create(Position::Top, 2, None, None, vec![], false, ScreenMode::Normal).unwrap();

        let writes1 = vec![RegionWrite {
            row: 0,
            col: 0,
            text: "First".to_string(),
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }];
        assert!(store.region_write(&pid, writes1));
        assert_eq!(store.get(&pid).unwrap().region_writes.len(), 1);

        // Replace with new writes
        let writes2 = vec![
            RegionWrite {
                row: 0,
                col: 0,
                text: "X".to_string(),
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            },
            RegionWrite {
                row: 0,
                col: 1,
                text: "Y".to_string(),
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            },
        ];
        assert!(store.region_write(&pid, writes2));

        let panel = store.get(&pid).unwrap();
        assert_eq!(panel.region_writes.len(), 2);
        assert_eq!(panel.region_writes[0].text, "X");
        assert_eq!(panel.region_writes[1].text, "Y");
    }

    #[test]
    fn test_region_write_nonexistent_panel() {
        let store = PanelStore::new();
        assert!(!store.region_write("nonexistent", vec![]));
    }

    #[test]
    fn test_list_by_mode_filters_correctly() {
        let store = PanelStore::new();
        store.create(Position::Bottom, 1, None, None, vec![], false, ScreenMode::Normal).unwrap();
        store.create(Position::Top, 1, None, None, vec![], false, ScreenMode::Normal).unwrap();
        store.create(Position::Bottom, 1, None, None, vec![], false, ScreenMode::Alt).unwrap();

        let normal = store.list_by_mode(ScreenMode::Normal);
        let alt = store.list_by_mode(ScreenMode::Alt);
        assert_eq!(normal.len(), 2);
        assert_eq!(alt.len(), 1);
    }

    #[test]
    fn test_delete_by_mode_removes_only_matching() {
        let store = PanelStore::new();
        store.create(Position::Bottom, 1, None, None, vec![], false, ScreenMode::Normal).unwrap();
        store.create(Position::Top, 1, None, None, vec![], false, ScreenMode::Alt).unwrap();
        store.create(Position::Bottom, 1, None, None, vec![], false, ScreenMode::Alt).unwrap();

        store.delete_by_mode(ScreenMode::Alt);
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].screen_mode, ScreenMode::Normal);
    }

    #[test]
    fn test_create_with_background() {
        use crate::overlay::types::{Color, NamedColor};

        let store = PanelStore::new();
        let bg = BackgroundStyle {
            bg: Color::Named(NamedColor::Blue),
        };
        let id = store.create(Position::Bottom, 2, None, Some(bg), vec![], false, ScreenMode::Normal).unwrap();
        let panel = store.get(&id).unwrap();
        assert!(panel.background.is_some());
        assert_eq!(panel.background.unwrap().bg, Color::Named(NamedColor::Blue));
    }
}
