use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

use super::types::{BackgroundStyle, Overlay, OverlayId, OverlaySpan};

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
}
