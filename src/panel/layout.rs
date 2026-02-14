use super::types::{Panel, PanelId, Position};

/// Computed screen layout based on active panels and terminal dimensions.
#[derive(Debug, Clone)]
pub struct Layout {
    /// Visible top panels, ordered from edge toward content (highest z first)
    pub top_panels: Vec<Panel>,
    /// Visible bottom panels, ordered from edge toward content (highest z first)
    pub bottom_panels: Vec<Panel>,
    /// IDs of panels that are hidden due to insufficient space
    pub hidden_panels: Vec<PanelId>,
    /// First PTY row (1-indexed, for DECSTBM)
    pub scroll_region_top: u16,
    /// Last PTY row (1-indexed, for DECSTBM)
    pub scroll_region_bottom: u16,
    /// Number of rows available for the PTY
    pub pty_rows: u16,
    /// Number of columns (unchanged from terminal)
    pub pty_cols: u16,
}

/// Compute the screen layout given all panels and the terminal dimensions.
///
/// Panels are allocated greedily by z-index (highest first = highest priority).
/// Panels that don't fit in the remaining space are hidden.
pub fn compute_layout(panels: &[Panel], terminal_rows: u16, terminal_cols: u16) -> Layout {
    let mut top_panels: Vec<Panel> = panels
        .iter()
        .filter(|p| p.position == Position::Top)
        .cloned()
        .collect();
    let mut bottom_panels: Vec<Panel> = panels
        .iter()
        .filter(|p| p.position == Position::Bottom)
        .cloned()
        .collect();

    // Sort by z descending (highest priority first)
    top_panels.sort_by(|a, b| b.z.cmp(&a.z));
    bottom_panels.sort_by(|a, b| b.z.cmp(&a.z));

    let mut remaining_rows = terminal_rows;
    let mut visible_top: Vec<Panel> = Vec::new();
    let mut visible_bottom: Vec<Panel> = Vec::new();
    let mut hidden: Vec<PanelId> = Vec::new();

    // Interleave top and bottom panels by priority (highest z first across both)
    // We process all panels in z-descending order regardless of position
    let mut all_panels: Vec<Panel> = top_panels
        .iter()
        .chain(bottom_panels.iter())
        .cloned()
        .collect();
    all_panels.sort_by(|a, b| b.z.cmp(&a.z));

    for panel in &all_panels {
        if remaining_rows == 0 {
            hidden.push(panel.id.clone());
            continue;
        }

        if panel.height <= remaining_rows {
            remaining_rows -= panel.height;
            let mut visible_panel = panel.clone();
            visible_panel.visible = true;
            match panel.position {
                Position::Top => visible_top.push(visible_panel),
                Position::Bottom => visible_bottom.push(visible_panel),
            }
        } else {
            // Panel doesn't fit even partially -- hide it
            hidden.push(panel.id.clone());
        }
    }

    // Re-sort visible panels: top by z descending, bottom by z descending
    visible_top.sort_by(|a, b| b.z.cmp(&a.z));
    visible_bottom.sort_by(|a, b| b.z.cmp(&a.z));

    let top_height: u16 = visible_top.iter().map(|p| p.height).sum();
    let bottom_height: u16 = visible_bottom.iter().map(|p| p.height).sum();
    let pty_rows = terminal_rows - top_height - bottom_height;

    // DECSTBM uses 1-indexed rows
    let scroll_region_top = top_height + 1;
    let scroll_region_bottom = terminal_rows - bottom_height;

    Layout {
        top_panels: visible_top,
        bottom_panels: visible_bottom,
        hidden_panels: hidden,
        scroll_region_top,
        scroll_region_bottom,
        pty_rows,
        pty_cols: terminal_cols,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::{OverlaySpan, ScreenMode};

    fn make_panel(id: &str, position: Position, height: u16, z: i32) -> Panel {
        Panel {
            id: id.to_string(),
            position,
            height,
            z,
            background: None,
            spans: vec![],
            region_writes: vec![],
            visible: true,
            focusable: false,
            screen_mode: ScreenMode::Normal,
        }
    }

    fn span(text: &str) -> OverlaySpan {
        OverlaySpan {
            text: text.to_string(),
            id: None,
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }
    }

    #[test]
    fn test_no_panels() {
        let layout = compute_layout(&[], 24, 80);
        assert!(layout.top_panels.is_empty());
        assert!(layout.bottom_panels.is_empty());
        assert!(layout.hidden_panels.is_empty());
        assert_eq!(layout.pty_rows, 24);
        assert_eq!(layout.pty_cols, 80);
        assert_eq!(layout.scroll_region_top, 1);
        assert_eq!(layout.scroll_region_bottom, 24);
    }

    #[test]
    fn test_single_top_panel() {
        let panels = vec![make_panel("a", Position::Top, 2, 0)];
        let layout = compute_layout(&panels, 24, 80);
        assert_eq!(layout.top_panels.len(), 1);
        assert!(layout.bottom_panels.is_empty());
        assert_eq!(layout.pty_rows, 22);
        assert_eq!(layout.scroll_region_top, 3); // rows 1-2 are panel, PTY starts at 3
        assert_eq!(layout.scroll_region_bottom, 24);
    }

    #[test]
    fn test_single_bottom_panel() {
        let panels = vec![make_panel("a", Position::Bottom, 1, 0)];
        let layout = compute_layout(&panels, 24, 80);
        assert!(layout.top_panels.is_empty());
        assert_eq!(layout.bottom_panels.len(), 1);
        assert_eq!(layout.pty_rows, 23);
        assert_eq!(layout.scroll_region_top, 1);
        assert_eq!(layout.scroll_region_bottom, 23); // row 24 is panel
    }

    #[test]
    fn test_top_and_bottom_panels() {
        let panels = vec![
            make_panel("top", Position::Top, 2, 0),
            make_panel("bot", Position::Bottom, 1, 0),
        ];
        let layout = compute_layout(&panels, 24, 80);
        assert_eq!(layout.top_panels.len(), 1);
        assert_eq!(layout.bottom_panels.len(), 1);
        assert_eq!(layout.pty_rows, 21);
        assert_eq!(layout.scroll_region_top, 3);
        assert_eq!(layout.scroll_region_bottom, 23);
    }

    #[test]
    fn test_panels_exceeding_height_hides_lowest_z() {
        // 5-row terminal, try to fit 3 panels of 2 rows each
        let panels = vec![
            make_panel("high", Position::Top, 2, 10),
            make_panel("mid", Position::Bottom, 2, 5),
            make_panel("low", Position::Top, 2, 1),
        ];
        let layout = compute_layout(&panels, 5, 80);

        // high (z=10) takes 2 rows, mid (z=5) takes 2 rows = 4 used, 1 PTY row
        // low (z=1) can't fit -- hidden
        assert_eq!(layout.pty_rows, 1);
        assert_eq!(layout.hidden_panels, vec!["low"]);
        assert_eq!(layout.top_panels.len(), 1);
        assert_eq!(layout.top_panels[0].id, "high");
        assert_eq!(layout.bottom_panels.len(), 1);
        assert_eq!(layout.bottom_panels[0].id, "mid");
    }

    #[test]
    fn test_exactly_one_pty_row_remaining() {
        let panels = vec![make_panel("a", Position::Top, 23, 0)];
        let layout = compute_layout(&panels, 24, 80);
        assert_eq!(layout.pty_rows, 1);
        assert!(layout.hidden_panels.is_empty());
    }

    #[test]
    fn test_terminal_one_row_panels_consume_all() {
        let panels = vec![make_panel("a", Position::Top, 1, 0)];
        let layout = compute_layout(&panels, 1, 80);
        // Panels can consume all rows, leaving zero for the PTY
        assert_eq!(layout.pty_rows, 0);
        assert_eq!(layout.top_panels.len(), 1);
        assert!(layout.hidden_panels.is_empty());
    }

    #[test]
    fn test_multiple_panels_same_position_z_ordering() {
        let panels = vec![
            make_panel("low", Position::Bottom, 1, 1),
            make_panel("high", Position::Bottom, 1, 10),
            make_panel("mid", Position::Bottom, 1, 5),
        ];
        let layout = compute_layout(&panels, 24, 80);
        assert_eq!(layout.bottom_panels.len(), 3);
        // Ordered edge->content: highest z first
        assert_eq!(layout.bottom_panels[0].id, "high");
        assert_eq!(layout.bottom_panels[1].id, "mid");
        assert_eq!(layout.bottom_panels[2].id, "low");
    }

    #[test]
    fn test_z_determines_which_panels_hidden() {
        // 4-row terminal, three 1-row bottom panels, one must be hidden
        let panels = vec![
            make_panel("z1", Position::Bottom, 1, 1),
            make_panel("z3", Position::Bottom, 1, 3),
            make_panel("z2", Position::Bottom, 1, 2),
        ];
        let layout = compute_layout(&panels, 4, 80);
        // z3 (1 row) + z2 (1 row) + z1 would need 3 rows, leaving 1 PTY row
        // All three fit: 3 panel rows + 1 PTY row = 4
        assert_eq!(layout.bottom_panels.len(), 3);
        assert_eq!(layout.pty_rows, 1);
        assert!(layout.hidden_panels.is_empty());
    }

    #[test]
    fn test_large_panel_hidden_when_no_fit() {
        // 5-row terminal, high-z panel takes 3 rows, low-z panel takes 3 rows
        let panels = vec![
            make_panel("big_low", Position::Top, 3, 1),
            make_panel("big_high", Position::Top, 3, 10),
        ];
        let layout = compute_layout(&panels, 5, 80);
        // big_high (z=10) takes 3 rows, 2 remaining, need 1 for PTY -> 1 available for panels
        // big_low (z=1) needs 3 but only 1 available -> hidden
        assert_eq!(layout.top_panels.len(), 1);
        assert_eq!(layout.top_panels[0].id, "big_high");
        assert_eq!(layout.hidden_panels, vec!["big_low"]);
        assert_eq!(layout.pty_rows, 2);
    }

    #[test]
    fn test_panels_can_consume_all_rows() {
        let panels = vec![make_panel("a", Position::Top, 24, 0)];
        let layout = compute_layout(&panels, 24, 80);
        assert_eq!(layout.pty_rows, 0);
        assert!(layout.hidden_panels.is_empty());
    }

    #[test]
    fn test_panel_with_spans() {
        let panels = vec![Panel {
            id: "s".to_string(),
            position: Position::Bottom,
            height: 1,
            z: 0,
            background: None,
            spans: vec![span("hello")],
            region_writes: vec![],
            visible: true,
            focusable: false,
            screen_mode: ScreenMode::Normal,
        }];
        let layout = compute_layout(&panels, 24, 80);
        assert_eq!(layout.bottom_panels[0].spans[0].text, "hello");
    }
}
