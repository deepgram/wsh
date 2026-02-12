---
name: wsh:visual-feedback
description: >
  Use when you need to communicate with the human visually through the terminal.
  Examples: "show a status panel", "display an overlay notification",
  "build a visual dashboard in the terminal".
---

# wsh:visual-feedback — Communicating Visually

You can place text on the terminal screen to communicate
with the human. You have two tools: overlays and panels.
Choosing the right one matters.

## Overlays

Floating text positioned at specific screen coordinates.
They sit on top of terminal content without affecting it.

**Characteristics:**
- Positioned at (x, y) — you choose exactly where
- Can overlap terminal content and each other
- Don't affect the PTY or its layout
- Best for transient, contextual information
- Can have explicit `width`, `height`, and `background`
  making them opaque rectangles — not just floating text

**Use for:** tooltips, warnings, quick tips, annotations,
notifications, contextual help near relevant output.
With explicit dimensions: windows, cards, dialogs,
modal panels — anything that needs a solid backdrop.

## Panels

Dedicated screen regions at the top or bottom edge. They
**shrink the PTY** — the terminal gets fewer rows, and
programs adapt to the reduced space.

**Characteristics:**
- Fixed to top or bottom
- Have a height in rows
- Carve out permanent space
- Best for persistent information

**Use for:** status bars, progress displays, context
summaries, dashboards, error explanations.

Panels also support `background` fill, named spans with
`id` fields for targeted updates, and region writes for
placing text at specific (row, col) offsets. These work
the same way as they do for overlays.

## Choosing Between Them

Ask two questions:

1. **Is it tied to a screen position?** If the information
   relates to a specific line or region of output, use an
   overlay positioned near it. If it's ambient context,
   use a panel.

2. **How long should it live?** If it's relevant for a few
   seconds or until the screen changes, use an overlay.
   If it should persist across screen updates, use a panel.

## Designing Effective Overlays

### Positioning
Screen coordinates are (x, y) where (0, 0) is the top-left
corner. x is the column, y is the row.

Place overlays near the content they relate to:
- Error annotation → same row as the error, offset to the
  right so it doesn't obscure the error text
- Command hint → just below or above the command line
- Notification → top-right corner, out of the way

Read the screen dimensions from the screen response
(`cols`, `rows`) to avoid placing overlays off-screen.

### Sizing
Keep overlay text short. One to three lines, under half
the screen width. If you need more space, use a panel or
the conversation instead.

Overlays with explicit `width` and `height` create an
opaque bounding box. The `background` color fills the
entire rectangle, and spans and region writes render on
top of it. This turns overlays into window-like elements
with solid backgrounds — useful for dialogs, cards, and
any UI that needs to cleanly cover terminal content.

### Styling
Spans support formatting attributes. Use them for
visual hierarchy:

    {"spans": [
      {"text": "Warning: ", "bold": true, "fg": {"indexed": 3}},
      {"text": "this deletes 47 files"}
    ]}

- **Bold** for labels, keywords, emphasis
- **Color** for severity (red = danger, yellow = warning,
  green = success, cyan = informational)
- **Faint/dim** for secondary information

Use the indexed color palette (0-7 for standard, 8-15 for
bright) for broad terminal compatibility. Use RGB colors
only when you're confident the terminal supports them.

### Named Spans
Spans can have an `id` field. This lets you update a
single span by its id instead of replacing all content
in the overlay or panel.

    {"spans": [
      {"id": "label", "text": "Status: ", "bold": true},
      {"id": "value", "text": "building", "fg": {"indexed": 3}}
    ]}

Later, update just the "value" span:

    update span "value" → {"text": "complete", "fg": {"indexed": 2}}

The "label" span stays untouched. This is good for:
- Live-updating status fields that change frequently
- Counters, timestamps, or progress percentages
- Labels that change independently of surrounding text
- Any element where you want to avoid redrawing
  everything just to change one piece

### Region Writes
Write styled text at specific (row, col) offsets within
an overlay or panel. This turns them into 2D drawable
surfaces — not just a flat list of spans, but a canvas
where you can place text at exact coordinates.

    overlay (width: 40, height: 5, background: dark):

    write at (0, 1):  "Name"     bold
    write at (0, 15): "Status"   bold
    write at (1, 1):  "auth"
    write at (1, 15): "● ok"     green
    write at (2, 1):  "api"
    write at (2, 15): "● slow"   yellow

Region writes are good for:
- Tables with aligned columns
- Grids and structured layouts
- Charts and diagrams
- Any content where relative positioning matters

Region writes and spans coexist. Spans flow as inline
content; region writes place content at absolute
positions within the element. Use whichever model fits
the content.

### Z-Order
When overlays overlap, higher `z` values render on top.
Default is 0. Use z-order to layer related elements:

    background context:  z=0
    primary information:  z=1
    urgent alert:         z=2

### Lifecycle
Overlays don't disappear on their own. You must manage them:
- Store the ID returned on creation
- Update content when information changes
- Delete individual overlays when no longer relevant
- Clear all overlays when cleaning up entirely

## Designing Effective Panels

### Layout
Panels are rows of styled text at the top or bottom of the
terminal. A 2-line panel at the bottom makes a natural
status bar. A 3-line panel at the top works for context
summaries.

    create panel (bottom, height: 2):
      spans: [
        {text: " Status: ", bold},
        {text: "building", fg: yellow},
        {text: "  |  "},
        {text: "Branch: ", bold},
        {text: "feature/auth"}
      ]

### Updating In Place
Panels persist. When information changes, update the
existing panel rather than deleting and recreating.
Use the panel's ID (returned on creation) to update
its content in place.

This prevents flicker and keeps the panel stable.

### Height Budget
Every row of panel height is a row stolen from the
terminal. Be miserly:
- 1 line for simple status
- 2 lines for status + detail
- 3 lines maximum in most cases

On a 24-row terminal, a 3-line panel costs 12% of the
visible area. Respect the human's screen space.

## Visual Structure: Borders and Padding

wsh renders your spans as-is. There are no built-in
borders, padding, margins, or separators. If you want
visual structure, you build it from text characters.

### Borders
Use box-drawing characters for framed overlays:

    {"spans": [
      {"text": "┌─ Warning ────────────┐\n"},
      {"text": "│ This will delete the │\n"},
      {"text": "│ entire build cache.  │\n"},
      {"text": "└──────────────────────┘"}
    ]}

### Padding
Add spaces for breathing room:

    {"text": "  Status: running  "}
              ^^              ^^  padding

### Separators
Use `│` or `|` between inline elements:

    {"spans": [
      {"text": " build: "},
      {"text": "ok", "fg": {"indexed": 2}},
      {"text": " │ tests: "},
      {"text": "3 failed", "fg": {"indexed": 1}},
      {"text": " │ lint: "},
      {"text": "clean", "fg": {"indexed": 2}},
      {"text": " "}
    ]}

### Panel Separators
A panel has no visible border separating it from the
terminal content. Add your own with a full-width line:

    {"text": "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n"}

Read the terminal's `cols` value to size separators
to the full width.

## Composition Patterns

Individual overlays and panels are building blocks.
Combining them creates richer experiences.

### Status Bar + Contextual Annotations
A persistent panel tracks overall state. Overlays
provide momentary context:

    panel (bottom, 1 line):
      " ● monitoring  │  12 commands  │  2 errors "

    overlay (near error on row 15):
      "┌─ Suggestion ──────────────────┐"
      "│ Try: cargo build --release    │"
      "└───────────────────────────────┘"

The panel stays. The overlay appears when relevant
and disappears when the screen moves on.

### Layered Overlays
Stack overlays with z-order for progressive detail.
A compact label at z=0, expanded detail at z=1
shown only when the human triggers it (via input
capture or a follow-up question):

    z=0: " ⚠ 3 warnings "
    z=1: "┌─ Warnings ──────────────────┐"
         "│ auth.rs:42  unused import   │"
         "│ db.rs:17    deprecated call │"
         "│ main.rs:3   dead code       │"
         "└────────────────────────────-┘"

### Multi-Panel Dashboard
Use top and bottom panels together. Top for context,
bottom for status:

    panel (top, 2 lines):
      "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
      " Project: wsh  Branch: master "

    panel (bottom, 1 line):
      " ● 3 sessions  │  build: ok  │  tests: running "

Be conservative. Two panels means the terminal loses
3+ rows. Only use both when the information justifies it.

### Temporary Overlays
For notifications that should appear briefly, create
the overlay and then delete it after a delay. You'll
need to manage the timing yourself:

    create overlay → get id
    (continue working)
    after a few interaction cycles, delete the overlay

There's no built-in timer. The overlay lives until you
remove it. Tie cleanup to your next interaction with
the terminal — when you next read the screen, check
if any overlays are stale and remove them.

## Pitfalls

### Visual Clutter
Every overlay and panel competes for attention. If
everything is highlighted, nothing is. Apply a strict
budget:
- At most 1-2 panels active
- At most 2-3 overlays visible simultaneously
- If you need to show a new overlay and you're at your
  limit, remove the least relevant one first

### Stale Elements
Overlays and panels don't expire. If you create an
overlay saying "build started" and never remove it,
it's still there 20 minutes later when the human has
moved on to something else entirely. Track what you've
created and clean up proactively.

Keep a mental inventory of your active visual elements.
Before creating a new one, ask: are any existing ones
stale? Remove them first.

### Overlapping Content
An overlay at (0, 5) that's 40 characters wide will
cover whatever is on row 5. If that row contains
important terminal output, you've hidden it. Strategies:
- Position overlays at the right edge of the screen,
  past where output typically ends
- Use short overlays that don't span the full width
- Prefer panels for information that shouldn't compete
  with terminal content

### Color Assumptions
Not all terminals use the same color scheme. Indexed
color 1 is "red" but the exact shade depends on the
terminal's theme. More importantly, the terminal might
have a light or dark background. Don't rely on color
alone to convey meaning — pair it with text labels
or symbols:

    Good:  {"text": "✗ FAIL", "fg": {"indexed": 1}}
    Bad:   {"text": "●", "fg": {"indexed": 1}}

The word "FAIL" communicates even without color. A
red dot is meaningless on a terminal where red is
hard to see.

### Coordinate Drift
Screen content scrolls. An overlay pinned to row 10
made sense when the error was on row 10, but after
the human runs another command, row 10 is different
content. Contextual overlays should be removed when
the screen changes significantly. Use the `epoch`
field in screen responses — if the epoch has changed
since you placed the overlay, re-evaluate whether it's
still positioned correctly.
