//! Layout coordinator for panel management.
//!
//! Provides `reconfigure_layout()` which is called after any panel mutation
//! that could change total height, and on outer terminal resize.

use std::sync::Arc;

use crate::parser::Parser;
use crate::pty::Pty;
use crate::terminal::TerminalSize;

use super::layout::compute_layout;
use super::store::PanelStore;

/// Reconfigure the terminal layout after panel changes.
///
/// This function:
/// 1. Computes the new layout from all panels and terminal size
/// 2. Updates panel visibility in the store
/// 3. Resizes the PTY and parser to match the new viewport
///
/// Call this after any panel create, delete, or height/position/z change.
/// Also called on outer terminal resize (SIGWINCH).
///
/// Note: Visual rendering (scroll region, panel content) is handled by
/// socket clients via PanelSync frames, not by the server.
pub async fn reconfigure_layout(
    panels: &PanelStore,
    terminal_size: &TerminalSize,
    pty: &Arc<parking_lot::Mutex<Pty>>,
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

    // Resize PTY and parser (use at least 1 row to avoid invalid resize)
    let effective_pty_rows = layout.pty_rows.max(1);
    if let Err(e) = pty.lock().resize(effective_pty_rows, layout.pty_cols) {
        tracing::error!(?e, "failed to resize PTY");
    }
    if let Err(e) = parser
        .resize(layout.pty_cols as usize, effective_pty_rows as usize)
        .await
    {
        tracing::error!(?e, "failed to resize parser");
    }
}

/// Notify that a single panel's content has changed (spans only).
///
/// This is a no-op on the server. Visual rendering of panel content is
/// handled by socket clients via PanelSync frames.
pub fn flush_panel_content(
    _panels: &PanelStore,
    _panel_id: &str,
    _terminal_size: &TerminalSize,
) {
    // No-op: clients render panels from PanelSync frames
}
