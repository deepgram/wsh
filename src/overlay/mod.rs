pub mod render;
pub mod store;
pub mod types;

pub use render::{
    begin_sync, cursor_position, end_sync, erase_all_overlays, erase_overlay,
    overlay_line_extents, render_all_overlays, render_overlay, render_spans, reset,
    restore_cursor, save_cursor,
};
pub use store::OverlayStore;
pub use types::{BackgroundStyle, Color, NamedColor, Overlay, OverlayId, OverlaySpan, RegionWrite, Style};
