//! Layout coordinator for panel management.
//!
//! Provides `reconfigure_layout()` which is called after any panel mutation
//! that could change total height, and on outer terminal resize.

use std::io::Write;
use std::sync::Arc;

use crate::overlay;
use crate::parser::Parser;
use crate::pty::Pty;
use crate::terminal::TerminalSize;

use super::layout::compute_layout;
use super::render;
use super::store::PanelStore;

/// Reconfigure the terminal layout after panel changes.
///
/// This function:
/// 1. Computes the new layout from all panels and terminal size
/// 2. Updates panel visibility in the store
/// 3. Sets the DECSTBM scroll region
/// 4. Renders all visible panels
/// 5. Resizes the PTY and parser to match the new viewport
///
/// Call this after any panel create, delete, or height/position/z change.
/// Also called on outer terminal resize (SIGWINCH).
pub async fn reconfigure_layout(
    panels: &PanelStore,
    terminal_size: &TerminalSize,
    pty: &Arc<Pty>,
    parser: &Parser,
) {
    let all_panels = panels.list();
    let (term_rows, term_cols) = terminal_size.get();
    let layout = compute_layout(&all_panels, term_rows, term_cols);

    // Update visibility in the store
    for panel in &all_panels {
        let visible = !layout.hidden_panels.contains(&panel.id);
        panels.set_visible(&panel.id, visible);
    }

    // Write scroll region and panel content to stdout
    {
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        let _ = lock.write_all(overlay::begin_sync().as_bytes());

        // Erase old panel content
        let _ = lock.write_all(render::erase_all_panels(&layout, term_cols).as_bytes());

        // Set scroll region (or reset if no panels or panels consume all rows)
        if layout.top_panels.is_empty() && layout.bottom_panels.is_empty()
            || layout.pty_rows == 0
        {
            let _ = lock.write_all(render::reset_scroll_region().as_bytes());
        } else {
            let _ = lock.write_all(
                render::set_scroll_region(layout.scroll_region_top, layout.scroll_region_bottom)
                    .as_bytes(),
            );
        }

        // Render visible panels
        let _ = lock.write_all(render::render_all_panels(&layout, term_cols).as_bytes());

        let _ = lock.write_all(overlay::end_sync().as_bytes());
        let _ = lock.flush();
    }

    // Resize PTY and parser (use at least 1 row to avoid invalid resize)
    let effective_pty_rows = layout.pty_rows.max(1);
    if let Err(e) = pty.resize(effective_pty_rows, layout.pty_cols) {
        tracing::error!(?e, "failed to resize PTY");
    }
    if let Err(e) = parser
        .resize(layout.pty_cols as usize, effective_pty_rows as usize)
        .await
    {
        tracing::error!(?e, "failed to resize parser");
    }
}

/// Flush a single panel's content to stdout without changing scroll region or PTY size.
///
/// Used for span-only updates where height/position/z haven't changed.
pub fn flush_panel_content(
    panels: &PanelStore,
    panel_id: &str,
    terminal_size: &TerminalSize,
) {
    let (term_rows, term_cols) = terminal_size.get();
    let all_panels = panels.list();
    let layout = compute_layout(&all_panels, term_rows, term_cols);

    // Find the panel and its start row in the layout
    let mut row = 0u16;
    for panel in &layout.top_panels {
        if panel.id == panel_id {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            let _ = lock.write_all(overlay::begin_sync().as_bytes());
            let _ = lock.write_all(overlay::save_cursor().as_bytes());
            let _ = lock.write_all(render::render_panel(panel, row, term_cols).as_bytes());
            let _ = lock.write_all(overlay::restore_cursor().as_bytes());
            let _ = lock.write_all(overlay::end_sync().as_bytes());
            let _ = lock.flush();
            return;
        }
        row += panel.height;
    }

    // Check bottom panels
    let mut row = layout.scroll_region_bottom; // 0-indexed start of bottom panels
    for panel in &layout.bottom_panels {
        if panel.id == panel_id {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            let _ = lock.write_all(overlay::begin_sync().as_bytes());
            let _ = lock.write_all(overlay::save_cursor().as_bytes());
            let _ = lock.write_all(render::render_panel(panel, row, term_cols).as_bytes());
            let _ = lock.write_all(overlay::restore_cursor().as_bytes());
            let _ = lock.write_all(overlay::end_sync().as_bytes());
            let _ = lock.flush();
            return;
        }
        row += panel.height;
    }
}
