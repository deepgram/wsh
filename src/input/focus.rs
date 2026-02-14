use std::sync::Arc;
use parking_lot::RwLock;

/// Tracks which overlay/panel currently has input focus.
///
/// At most one element has focus at a time. Focus requires input capture
/// mode to be active -- the FocusTracker doesn't enforce this itself;
/// the API layer checks capture mode before routing input.
#[derive(Clone)]
pub struct FocusTracker {
    inner: Arc<RwLock<Option<String>>>,
}

impl FocusTracker {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
        }
    }

    /// Set focus to a specific element by ID.
    pub fn focus(&self, id: String) {
        let mut inner = self.inner.write();
        *inner = Some(id);
    }

    /// Remove focus from any element.
    pub fn unfocus(&self) {
        let mut inner = self.inner.write();
        *inner = None;
    }

    /// Get the currently focused element's ID, if any.
    pub fn focused(&self) -> Option<String> {
        let inner = self.inner.read();
        inner.clone()
    }

    /// Clear focus only if the given ID currently has focus.
    /// Used when an element is deleted -- only unfocus if it was the focused one.
    pub fn clear_if_focused(&self, id: &str) {
        let mut inner = self.inner.write();
        if inner.as_deref() == Some(id) {
            *inner = None;
        }
    }
}

impl Default for FocusTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_has_no_focus() {
        let tracker = FocusTracker::new();
        assert!(tracker.focused().is_none());
    }

    #[test]
    fn test_focus_sets_element() {
        let tracker = FocusTracker::new();
        tracker.focus("overlay-1".to_string());
        assert_eq!(tracker.focused(), Some("overlay-1".to_string()));
    }

    #[test]
    fn test_focus_replaces_previous() {
        let tracker = FocusTracker::new();
        tracker.focus("overlay-1".to_string());
        tracker.focus("panel-2".to_string());
        assert_eq!(tracker.focused(), Some("panel-2".to_string()));
    }

    #[test]
    fn test_unfocus_clears_focus() {
        let tracker = FocusTracker::new();
        tracker.focus("overlay-1".to_string());
        tracker.unfocus();
        assert!(tracker.focused().is_none());
    }

    #[test]
    fn test_unfocus_when_already_none() {
        let tracker = FocusTracker::new();
        tracker.unfocus();
        assert!(tracker.focused().is_none());
    }

    #[test]
    fn test_clear_if_focused_clears_matching() {
        let tracker = FocusTracker::new();
        tracker.focus("overlay-1".to_string());
        tracker.clear_if_focused("overlay-1");
        assert!(tracker.focused().is_none());
    }

    #[test]
    fn test_clear_if_focused_ignores_non_matching() {
        let tracker = FocusTracker::new();
        tracker.focus("overlay-1".to_string());
        tracker.clear_if_focused("panel-2");
        assert_eq!(tracker.focused(), Some("overlay-1".to_string()));
    }

    #[test]
    fn test_clear_if_focused_when_none() {
        let tracker = FocusTracker::new();
        tracker.clear_if_focused("overlay-1");
        assert!(tracker.focused().is_none());
    }

    #[test]
    fn test_clone_shares_state() {
        let tracker = FocusTracker::new();
        let cloned = tracker.clone();
        tracker.focus("overlay-1".to_string());
        assert_eq!(cloned.focused(), Some("overlay-1".to_string()));
    }

    #[test]
    fn test_default_has_no_focus() {
        let tracker = FocusTracker::default();
        assert!(tracker.focused().is_none());
    }
}
