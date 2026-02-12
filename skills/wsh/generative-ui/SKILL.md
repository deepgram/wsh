---
name: wsh:generative-ui
description: >
  Use when you need to build dynamic, interactive terminal experiences on the
  fly. Examples: "create a live dashboard in the terminal", "build an
  interactive file browser", "generate a custom TUI for this workflow".
---

# wsh:generative-ui — Building Dynamic Experiences

The terminal isn't just a place to run existing programs.
With wsh, you can create interactive experiences on the
fly — interfaces that didn't exist before this moment,
tailored to this specific task, this specific user,
this specific context.

## What You Have to Work With

Three layers of capability, from simple to complex:

**Layer 1: Overlays and Panels**
The wsh primitives. Position styled text anywhere on
screen, carve out panel regions, capture input. No
external programs needed. Good for: annotations,
status displays, menus, prompts, simple dashboards.

**Layer 2: Composed Experiences**
Combine overlays + panels + input capture into
cohesive interactive applications. A selection menu
with a preview panel. A dashboard with live-updating
sections. A wizard that walks through multiple steps.
Still no external programs — just wsh primitives
orchestrated together.

**Layer 3: Direct Drawing**
Use overlays and panels as 2D drawing surfaces. Opaque
overlays with explicit dimensions become windows and
dialogs. Named spans enable surgical updates to
individual elements. Region writes let you place text
at specific coordinates within an overlay or panel.
Alternate screen mode gives you a clean canvas that
vanishes when you're done. This layer turns the wsh
primitives into a full rendering engine — no external
programs, no generated scripts, just direct control
over every cell on screen.

## Choosing a Layer

Start with the simplest layer that works:
- Need to show some text? Layer 1.
- Need interaction with multiple elements? Layer 2.
- Need windows, live-updating fields, structured
  layouts, or a temporary full-screen UI? Layer 3.

Most generative UI lives in layers 1 and 2. Layer 3
is for when you need pixel-level control over what
appears where, or when you want a fully immersive
experience that cleans up after itself.

## Design Patterns

### Live Dashboard
Combine panels with periodic updates to create a
real-time display. Use named spans so you can update
individual values without redrawing the whole panel:

    panel (top, 3 lines):
      "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
      " CPU: ████░░░░ 52%  │  MEM: 3.2/8G "
      " Pods: 12 ready     │  Errs: 0     "

    named spans:
      id="cpu"  → "████░░░░ 52%"
      id="mem"  → "3.2/8G"
      id="pods" → "12 ready"
      id="errs" → "0"

When new data arrives, update just the span that
changed — e.g., update the "cpu" span to "██████░░ 75%"
without touching anything else. No flicker, no
redrawing borders or labels.

### Interactive Browser
Let the human explore structured data — files,
commits, log entries, API responses. Use an opaque
overlay as a window:

    overlay (width: 40, height: 12, background: dark):
      "┌─ Recent Commits ──────────────────┐"
      "│ ▸ 81883ad docs: rewrite README    │"
      "│   8acf8d7 feat: clear screen on   │"
      "│   55719e8 feat: add session detach │"
      "│                                   │"
      "│   ↑↓ navigate  Enter: view diff   │"
      "│   q: close                        │"
      "└───────────────────────────────────┘"

The explicit width and height create an opaque
rectangle that cleanly covers terminal content behind
it. On Enter, fetch the diff for the selected commit,
replace the overlay content with the diff view,
and add a "Back" option. You're building a mini
application from overlays and keystrokes.

### Dialog Window
Create a modal dialog using an opaque overlay.
The background fills the rectangle, giving it a
solid, window-like appearance:

    overlay (center, width: 50, height: 8, background: dark):

    region writes within the overlay:
      (1, 2)  "Confirm Deployment"        bold
      (3, 2)  "Environment:  production"
      (4, 2)  "Version:      v2.4.1"
      (5, 2)  "Containers:   12"
      (7, 8)  "[Y]es"   green
      (7, 20) "[N]o"    red

Region writes place each piece of text at a specific
(row, col) offset within the overlay. No need to
construct a single spans array with exact spacing —
just draw each element where it belongs.

### Canvas Rendering
Use region writes to build structured layouts like
tables, grids, or charts:

    overlay (width: 60, height: 15, background: dark):

    # Draw headers
    region write (0, 0):  "Service" bold
    region write (0, 20): "Status"  bold
    region write (0, 40): "Latency" bold

    # Draw separator
    region write (1, 0):  "─" × 60

    # Draw rows
    region write (2, 0):  "auth-service"
    region write (2, 20): "● healthy"   green
    region write (2, 40): "12ms"

    region write (3, 0):  "api-gateway"
    region write (3, 20): "● degraded"  yellow
    region write (3, 40): "340ms"

Each row and column is independently addressable.
Update a single cell when data changes — no need
to redraw the entire table.

### Live-Updating Status
Combine named spans with periodic updates for
elements that change independently:

    panel (bottom, 1 line):
      span id="status": "● connected"  green
      span id="sep1":   " │ "
      span id="time":   "14:32:07"
      span id="sep2":   " │ "
      span id="count":  "47 events"

Update the "time" span every few seconds. Update
"count" when events arrive. Update "status" if the
connection state changes. Each is independent — no
need to rebuild the entire status bar.

### Wizard
Walk the human through a multi-step process,
building up configuration or input along the way.
Use a panel for progress and an overlay for the
current step:

    panel (bottom, 1 line):
      " Step 2/4: Configure database ──────"

    overlay (center, width: 35, height: 6, background: dark):
      "┌─ Database Type ───────────────┐"
      "│   SQLite                      │"
      "│ ▸ PostgreSQL                  │"
      "│   MySQL                       │"
      "└───────────────────────────────┘"

Each step collects input, updates the progress
panel, and advances. At the end, summarize all
choices and ask for confirmation before acting.

### Contextual Preview
Show a preview that updates based on what the
human is doing. Combine monitoring with visual
feedback:

    # Human is editing a Markdown file
    # Panel shows a simplified rendered preview

    panel (bottom, 5 lines):
      "━━ Preview ━━━━━━━━━━━━━━━━━━━━━━"
      " # My Document                   "
      " This is the **first** paragraph "
      " - item one                      "
      " - item two                      "

Read the file periodically, render a simplified
version, update the panel. The human gets live
feedback without switching tools.

### Full-Screen Agent UI
Use alternate screen mode to take over the entire
display temporarily. Create a fully immersive
interface, then exit cleanly:

    enter alt screen

    # Now working on a clean canvas.
    # Create panels and overlays freely — they exist
    # only in alt screen mode.

    panel (top, 1 line):  "═══ Environment Setup ═══"
    panel (bottom, 1 line): " [Tab] next  [Esc] cancel "

    overlay (center, width: 60, height: 20, background: dark):
      # Your main UI content here

    # When done:
    exit alt screen
    # Everything created in alt mode is automatically
    # deleted. The human's original terminal is restored.

Alt screen mode is perfect for intensive workflows
that need the full terminal — setup wizards,
dashboards, configuration editors. The human's
terminal is completely preserved underneath.

## Composition Philosophy

The best generative UI combines layers fluidly. A
dashboard panel at the bottom. An overlay window when
the human needs to make a choice. Named spans for
live-updating values. Region writes for structured
layouts. Alt screen mode when you need a clean slate.
Each layer serves its purpose, then gets out of the way.

### Design Principles

**Fit the terminal aesthetic.** The human chose to work
in a terminal. Respect that. Use box-drawing characters,
monospace alignment, and ASCII art — not emoji-heavy
decoration. Clean, functional, information-dense.

**Build incrementally.** Start with the simplest version
that's useful. A one-line panel is better than nothing.
Add complexity only when the human needs it or asks
for it.

**Make it disposable.** Every UI element you create
should be easy to dismiss and leave no trace. The
human should never have to clean up after your
interface. When they're done, everything disappears.
Alt screen mode is the ultimate expression of this —
exit and it's as if your UI never existed.

**Adapt to the terminal size.** Read `cols` and `rows`
from the screen response. A dashboard designed for
120 columns is useless on a 80-column terminal. Scale
your layouts:
- < 80 cols: minimal, single-column, short labels
- 80-120 cols: standard layout
- > 120 cols: multi-column, more detail

**Respond to context.** A generative UI should feel
relevant to what's happening right now. A deployment
dashboard during deployment. A test results browser
after a test run. A log viewer when errors appear.
Build it when it's needed, tear it down when it's not.

## Pitfalls

### Don't Build What Already Exists
Before generating a custom file browser, check if
`ranger` or `lf` is installed. Before building a git
log viewer, try `lazygit` or `tig`. Generated UI
is for gaps — when no existing tool fits the need.
Use wsh:tui to drive existing tools when they're
available.

### Don't Over-Engineer
A simple overlay that solves the problem in seconds
is better than an elaborate multi-panel layout that
takes minutes to construct. The human is waiting.
Bias toward quick and functional.

### Don't Mix Layers Carelessly
Overlays sit on top of terminal content. If you have
multiple opaque overlays stacked, or overlays fighting
with panels for attention, the result is confusing.
Be deliberate about what's visible when. If you need
a clean canvas without worrying about layering, use
alt screen mode — it gives you a fresh surface where
you control everything.

### Test on Small Terminals
If you don't know the terminal size, design for 80x24
— the classic default. Everything you build should
be usable at this size, even if it looks better on
a larger terminal.
