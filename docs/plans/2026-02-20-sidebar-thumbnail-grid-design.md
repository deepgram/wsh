# Sidebar Thumbnail Grid Redesign

## Problem

The current sidebar fights over two visual models: mini-preview thumbnails and a text session list. Each group renders a 64px preview area AND a list of session names with status dots and "+" tag buttons. This creates clutter -- you can't actually see sessions in the group, and the name/status/title text takes too much space.

## Design

Replace both the mini-preview area and the text session list with a single **thumbnail grid** per group. Session identity (name, status, tags) is accessed through hover interactions on the thumbnails themselves.

### Group Ordering

1. Custom tag groups (sorted alphabetically)
2. "Untagged" (only if untagged sessions exist)
3. "All Sessions" (moved to last -- grows fastest, least useful for navigation)

### Collapsible Groups

Each group has a disclosure chevron:

- **Collapsed**: single line -- chevron (right), group name, colored count chips
  - Green pill with count = idle sessions
  - Amber pill with count = running sessions
  - Omit a pill if its count is 0
- **Expanded**: chevron (down), group header, then thumbnail grid below
- Collapse state persisted to localStorage per group

### Status Colors (corrected)

| State | Dot color | Label |
|-------|-----------|-------|
| Process producing output | Amber | Running |
| Process idle / quiescent | Green | Idle |

There is no "exited" state. `wsh` removes sessions immediately on process exit (`monitor_child_exit` destroys the session from the registry). The grey dot and all "exited" UI handling is dead code to be removed.

### Thumbnail Grid (expanded group)

- CSS grid: `repeat(auto-fill, minmax(0, 1fr))` with **minimum 2 columns always**
- Thumbnails maintain ~4:3 aspect ratio, sized proportionally to sidebar width
- Sessions sorted by name within each group
- Groups grow naturally -- no internal scroll cap, the sidebar itself scrolls
- Each thumbnail renders a scaled-down terminal replica (existing `transform: scale()` approach)

### Thumbnail Resting State

- Scaled terminal replica fills the cell
- Status dot (green or amber) in the **lower-right corner**, always visible
- No text overlays -- clean and minimal

### Thumbnail Hover State

- Semi-transparent bottom bar overlay (`rgba(0,0,0,0.6)`, ~20px tall) fades in
- Session name on the left side of the bar
- Status dot moves into the bar on the right
- Tag icon appears in the **upper-right corner** of the thumbnail
- **Click session name** -> inline rename (input replaces name text, Enter confirms, Escape cancels)
- **Click thumbnail** (not on name or tag icon) -> navigate to/select the session
- **Click tag icon** -> open tag popover

### Tag Popover

- Small popover anchored near the tag icon (upper-right corner)
- Shows existing tags as removable pills + text input
- **Tab**, **comma**, or **space** commits current text as a new tag and clears input
- Autocomplete suggestions from all existing tags
- **Escape** or **Enter** (on empty input) dismisses the popover and applies changes
- Click outside also dismisses

### Queue View Label Update

Queue view currently labels quiescent sessions as "pending". With the corrected color semantics:

- Quiescent sessions are labeled **"Idle"** (green), not "pending"
- Active sessions are labeled **"Running"** (amber), not "active"

## What's Removed

- Text session list (`.sidebar-session-list`) -- replaced by thumbnail grid
- Per-session "+" tag button -- replaced by hover tag icon
- Group timestamp labels ("Active", "Idle", "Exited") -- status dots convey this
- Grey/exited status dot and all "exited" UI handling -- dead code
- Fixed 64px preview area height -- replaced by natural grid sizing

## What's Unchanged

- Collapsed sidebar (40px icon stack)
- Drag-and-drop between groups
- Group click to filter main view
- All six themes
- Mobile bottom sheet (separate component)
