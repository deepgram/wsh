//! Panel rendering utilities for terminal clients.
//!
//! Provides `render_panel_sync` which handles the ANSI escape sequence
//! management for transitioning between panel layouts in the local terminal.

use crate::overlay;
use crate::panel::{self, Panel};

/// Render the panel sync update, writing ANSI escape sequences to `w`.
///
/// Handles scroll region transitions carefully: DECSTBM (`\x1b[r`) moves the
/// cursor to (1,1) as a side effect per VT100 spec, so we only emit it when
/// there is an actual transition between having panels and not having them.
///
/// Transition table:
/// - no panels → no panels: no scroll region change (avoids cursor jump)
/// - no panels → has panels: set scroll region
/// - has panels → has panels: update scroll region
/// - has panels → no panels: reset scroll region
pub(crate) fn render_panel_sync(
    w: &mut impl std::io::Write,
    new_panels: &[Panel],
    cached_panels: &[Panel],
    term_rows: u16,
    term_cols: u16,
) -> std::io::Result<()> {
    w.write_all(overlay::begin_sync().as_bytes())?;

    // Erase old panels using cached layout
    if !cached_panels.is_empty() {
        let old_layout = panel::compute_layout(cached_panels, term_rows, term_cols);
        w.write_all(panel::erase_all_panels(&old_layout, term_cols).as_bytes())?;
    }

    // Compute new layout
    let new_layout = panel::compute_layout(new_panels, term_rows, term_cols);

    let had_panels = !cached_panels.is_empty();
    let has_panels = !new_layout.top_panels.is_empty()
        || !new_layout.bottom_panels.is_empty();

    // Only change scroll region when transitioning between having panels and
    // not having them (or vice versa). DECSTBM (`\x1b[r` / `\x1b[t;br`)
    // moves the cursor to (1,1) as a side effect per VT100 spec, so we:
    //   1. Skip it entirely for no-panels → no-panels (the original bug fix)
    //   2. Wrap it in SCOSC save/restore when we do emit it, to preserve
    //      cursor position (safe here because erase_all_panels already
    //      completed its own save/restore cycle — no nesting)
    if has_panels {
        w.write_all(overlay::save_cursor().as_bytes())?;
        w.write_all(
            panel::set_scroll_region(new_layout.scroll_region_top, new_layout.scroll_region_bottom)
                .as_bytes(),
        )?;
        w.write_all(overlay::restore_cursor().as_bytes())?;
    } else if had_panels {
        // Transitioning from panels → no panels: reset
        w.write_all(overlay::save_cursor().as_bytes())?;
        w.write_all(panel::reset_scroll_region().as_bytes())?;
        w.write_all(overlay::restore_cursor().as_bytes())?;
    }
    // else: no panels before, no panels now — skip entirely

    if has_panels {
        w.write_all(panel::render_all_panels(&new_layout, term_cols).as_bytes())?;
    }
    w.write_all(overlay::end_sync().as_bytes())?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a minimal visible panel for testing scroll region behavior.
    fn test_panel(id: &str, position: panel::Position) -> Panel {
        Panel {
            id: id.to_string(),
            position,
            height: 1,
            z: 0,
            background: None,
            spans: vec![],
            region_writes: vec![],
            visible: true,
            focusable: false,
            screen_mode: crate::overlay::ScreenMode::Normal,
        }
    }

    #[test]
    fn test_panel_sync_no_panels_to_no_panels_skips_scroll_region() {
        // When there are no panels before and no panels after, DECSTBM (\x1b[r)
        // must NOT be emitted — it moves the cursor to (1,1) as a side effect.
        let mut buf = Vec::new();
        render_panel_sync(&mut buf, &[], &[], 24, 80).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(
            !output.contains("\x1b[r"),
            "empty→empty must not emit DECSTBM; got: {:?}",
            output,
        );
    }

    #[test]
    fn test_panel_sync_panels_to_no_panels_resets_scroll_region() {
        // Transitioning from having panels to no panels should reset the
        // scroll region so the shell uses the full terminal again.
        let old = vec![test_panel("p1", panel::Position::Bottom)];
        let mut buf = Vec::new();
        render_panel_sync(&mut buf, &[], &old, 24, 80).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(
            output.contains("\x1b[r"),
            "panels→empty must emit DECSTBM reset; got: {:?}",
            output,
        );
        // DECSTBM reset must be wrapped in cursor save/restore to avoid
        // moving the cursor to (1,1).
        assert!(
            output.contains("\x1b[s\x1b[r\x1b[u"),
            "DECSTBM reset must be wrapped in SCOSC save/restore; got: {:?}",
            output,
        );
    }

    #[test]
    fn test_panel_sync_no_panels_to_panels_sets_scroll_region() {
        // Adding the first panel should set DECSTBM to carve out panel rows.
        let new = vec![test_panel("p1", panel::Position::Bottom)];
        let mut buf = Vec::new();
        render_panel_sync(&mut buf, &new, &[], 24, 80).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // Should contain a scroll region set (e.g. \x1b[1;23r) wrapped in
        // cursor save/restore.
        assert!(
            output.contains("\x1b[s\x1b[1;23r\x1b[u"),
            "empty→panels must set scroll region with save/restore; got: {:?}",
            output,
        );
    }

    #[test]
    fn test_panel_sync_panels_to_panels_updates_scroll_region() {
        // Updating from one panel layout to another should update DECSTBM.
        let old = vec![test_panel("p1", panel::Position::Bottom)];
        let new = vec![
            test_panel("p1", panel::Position::Bottom),
            test_panel("p2", panel::Position::Top),
        ];
        let mut buf = Vec::new();
        render_panel_sync(&mut buf, &new, &old, 24, 80).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // Should contain an updated scroll region (top=2, bottom=23 → \x1b[2;23r)
        // wrapped in cursor save/restore.
        assert!(
            output.contains("\x1b[s\x1b[2;23r\x1b[u"),
            "panels→panels must update scroll region with save/restore; got: {:?}",
            output,
        );
    }
}
