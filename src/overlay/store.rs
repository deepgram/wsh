use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

use super::types::{BackgroundStyle, Overlay, OverlayId, OverlaySpan, RegionWrite};

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
        let overlay = Overlay {
            id: id.clone(),
            x,
            y,
            z,
            width,
            height,
            background,
            spans,
            region_writes: vec![],
        };
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
    pub fn move_to(
        &self,
        id: &str,
        x: Option<u16>,
        y: Option<u16>,
        z: Option<i32>,
        width: Option<u16>,
        height: Option<u16>,
    ) -> bool {
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
            }
            if let Some(width) = width {
                overlay.width = width;
            }
            if let Some(height) = height {
                overlay.height = height;
            }
            true
        } else {
            return false;
        };
        // Update next_z outside the overlay borrow
        if let Some(z) = z {
            if z >= inner.next_z {
                inner.next_z = z + 1;
            }
        }
        true
    }

    /// Update specific spans by their `id` field.
    ///
    /// For each span in `updates`, find the span with matching `id` in the overlay
    /// and replace its text, colors, and attributes. Returns false if overlay not found.
    pub fn update_spans(&self, overlay_id: &str, updates: &[OverlaySpan]) -> bool {
        let mut inner = self.inner.write().unwrap();
        if let Some(overlay) = inner.overlays.get_mut(overlay_id) {
            for update in updates {
                if let Some(ref update_id) = update.id {
                    for span in &mut overlay.spans {
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

    /// Replace the stored region writes for an overlay.
    ///
    /// Returns false if the overlay does not exist.
    pub fn region_write(&self, id: &str, writes: Vec<RegionWrite>) -> bool {
        let mut inner = self.inner.write().unwrap();
        if let Some(overlay) = inner.overlays.get_mut(id) {
            overlay.region_writes = writes;
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
    use crate::overlay::types::{Color, NamedColor};

    #[test]
    fn test_create_overlay() {
        let store = OverlayStore::new();
        let id = store.create(0, 0, None, 80, 1, None, vec![]);
        assert!(!id.is_empty());
    }

    #[test]
    fn test_get_overlay() {
        let store = OverlayStore::new();
        let id = store.create(5, 10, Some(50), 80, 1, None, vec![]);
        let overlay = store.get(&id).unwrap();
        assert_eq!(overlay.x, 5);
        assert_eq!(overlay.y, 10);
        assert_eq!(overlay.z, 50);
    }

    #[test]
    fn test_list_overlays_sorted_by_z() {
        let store = OverlayStore::new();
        store.create(0, 0, Some(100), 80, 1, None, vec![]);
        store.create(0, 0, Some(50), 80, 1, None, vec![]);
        store.create(0, 0, Some(75), 80, 1, None, vec![]);

        let list = store.list();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].z, 50);
        assert_eq!(list[1].z, 75);
        assert_eq!(list[2].z, 100);
    }

    #[test]
    fn test_delete_overlay() {
        let store = OverlayStore::new();
        let id = store.create(0, 0, None, 80, 1, None, vec![]);
        assert!(store.delete(&id));
        assert!(store.get(&id).is_none());
    }

    #[test]
    fn test_clear_overlays() {
        let store = OverlayStore::new();
        store.create(0, 0, None, 80, 1, None, vec![]);
        store.create(0, 0, None, 80, 1, None, vec![]);
        store.clear();
        assert!(store.list().is_empty());
    }

    #[test]
    fn test_auto_increment_z() {
        let store = OverlayStore::new();
        let id1 = store.create(0, 0, None, 80, 1, None, vec![]);
        let id2 = store.create(0, 0, None, 80, 1, None, vec![]);
        let o1 = store.get(&id1).unwrap();
        let o2 = store.get(&id2).unwrap();
        assert!(o2.z > o1.z);
    }

    #[test]
    fn test_update_spans_by_id() {
        let store = OverlayStore::new();
        let spans = vec![
            OverlaySpan {
                id: Some("label".to_string()),
                text: "Progress: ".to_string(),
                fg: None,
                bg: None,
                bold: true,
                italic: false,
                underline: false,
            },
            OverlaySpan {
                id: Some("value".to_string()),
                text: "52%".to_string(),
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            },
        ];
        let oid = store.create(0, 0, None, 80, 1, None, spans);

        // Update only the "value" span
        let updates = vec![OverlaySpan {
            id: Some("value".to_string()),
            text: "78%".to_string(),
            fg: Some(Color::Named(NamedColor::Green)),
            bg: None,
            bold: true,
            italic: false,
            underline: false,
        }];
        assert!(store.update_spans(&oid, &updates));

        let overlay = store.get(&oid).unwrap();
        // "label" span should be unchanged
        assert_eq!(overlay.spans[0].text, "Progress: ");
        assert!(overlay.spans[0].bold);
        assert_eq!(overlay.spans[0].fg, None);
        // "value" span should be updated
        assert_eq!(overlay.spans[1].text, "78%");
        assert_eq!(overlay.spans[1].fg, Some(Color::Named(NamedColor::Green)));
        assert!(overlay.spans[1].bold);
    }

    #[test]
    fn test_update_spans_nonexistent_overlay() {
        let store = OverlayStore::new();
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
        let store = OverlayStore::new();
        let spans = vec![OverlaySpan {
            id: Some("label".to_string()),
            text: "Hello".to_string(),
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }];
        let oid = store.create(0, 0, None, 80, 1, None, spans);

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
        assert!(store.update_spans(&oid, &updates));

        // Original span should be unchanged
        let overlay = store.get(&oid).unwrap();
        assert_eq!(overlay.spans[0].text, "Hello");
    }

    #[test]
    fn test_region_write_stores_writes() {
        let store = OverlayStore::new();
        let oid = store.create(0, 0, None, 80, 10, None, vec![]);

        let writes = vec![
            RegionWrite {
                row: 0,
                col: 5,
                text: "Hello".to_string(),
                fg: Some(Color::Named(NamedColor::Green)),
                bg: None,
                bold: true,
                italic: false,
                underline: false,
            },
            RegionWrite {
                row: 1,
                col: 0,
                text: "World".to_string(),
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            },
        ];
        assert!(store.region_write(&oid, writes));

        let overlay = store.get(&oid).unwrap();
        assert_eq!(overlay.region_writes.len(), 2);
        assert_eq!(overlay.region_writes[0].text, "Hello");
        assert_eq!(overlay.region_writes[0].row, 0);
        assert_eq!(overlay.region_writes[0].col, 5);
        assert!(overlay.region_writes[0].bold);
        assert_eq!(overlay.region_writes[1].text, "World");
        assert_eq!(overlay.region_writes[1].row, 1);
    }

    #[test]
    fn test_region_write_replaces_previous() {
        let store = OverlayStore::new();
        let oid = store.create(0, 0, None, 80, 10, None, vec![]);

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
        assert!(store.region_write(&oid, writes1));
        assert_eq!(store.get(&oid).unwrap().region_writes.len(), 1);

        let writes2 = vec![
            RegionWrite {
                row: 0,
                col: 0,
                text: "A".to_string(),
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            },
            RegionWrite {
                row: 1,
                col: 0,
                text: "B".to_string(),
                fg: None,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            },
        ];
        assert!(store.region_write(&oid, writes2));

        let overlay = store.get(&oid).unwrap();
        assert_eq!(overlay.region_writes.len(), 2);
        assert_eq!(overlay.region_writes[0].text, "A");
        assert_eq!(overlay.region_writes[1].text, "B");
    }

    #[test]
    fn test_region_write_nonexistent_overlay() {
        let store = OverlayStore::new();
        assert!(!store.region_write("nonexistent", vec![]));
    }
}
