# Generative UI: Agent-Drawn Terminal Interfaces

## Overview

AI agents need the ability to draw interactive terminal UIs directly through the
`wsh` API — no generated scripts, no intermediate programs. The agent decides
what appears on screen and draws it cell by cell, reacting to user input in real
time. The terminal is the agent's canvas.

Today's primitives (overlays, panels, input capture) enable agents to annotate
the terminal and intercept input. But they don't enable agents to draw
interactive UIs. Building a form from overlays means encoding the entire visual
as a flat span string — box-drawing characters, padding, alignment — then
recomputing and replacing the whole thing on every keystroke. Panels are limited
to full-width edge-anchored strips with the same span-based rendering.

This design extends overlays and panels with rendering enhancements, introduces
scoped input routing, adds built-in widgets for latency-sensitive interactions,
and gives agents control over alternate screen mode for full-screen UIs.

**Design principles:**

1. **The agent is the program.** The API gives agents the same rendering power
   that a TUI framework gives a compiled program.
2. **Two speeds.** Raw drawing for novel/custom UIs (agent handles each
   keystroke). Built-in widgets for common patterns (`wsh` handles keystrokes
   locally, agent gets results).
3. **Clean entry and exit.** Agents can take over part of the screen (floating
   overlays) or all of it (alternate screen mode). When done, the terminal is
   exactly as it was.
4. **No new rendering abstractions.** Overlays and panels are the right
   primitives. This design enhances them rather than replacing them.

## Rendering Enhancements

### Unified Rendering Model

Overlays and panels share the same rendering capabilities. The only differences
are positional:

- **Overlays**: positioned at `(x, y)` on the terminal grid, float on top of
  PTY content.
- **Panels**: docked to top/bottom edge, full width, resize the PTY.

Everything else — size, background, spans, named spans, region writes — works
identically for both. All coordinates in drawing operations are relative to the
overlay/panel's own origin `(0, 0)`.

### Explicit Size

Overlays require `width` and `height` at creation. This defines the owned
rectangle. (Panels already have implicit width from terminal cols and explicit
`height`.)

```json
{
  "x": 10, "y": 5,
  "width": 30, "height": 8,
  "background": {"bg": {"r": 30, "g": 30, "b": 30}},
  "spans": [...]
}
```

Size can be updated via `PATCH` without destroy/recreate — useful for responsive
layouts when the terminal resizes.

### Always-Opaque Backgrounds

Overlays and panels are always opaque. The `background` property specifies the
fill style for the entire `(width x height)` rectangle. Content (spans, widgets,
region writes) renders on top of this background.

If `background` is omitted at creation, a sensible default is used (terminal
default background or black).

The rendering pipeline is always: fill rectangle with background, then draw
content on top. No conditional transparency logic.

When a named span's text is updated to something shorter, the remaining cells
are filled with the background color automatically. Spans own their extent —
the agent never needs to pad with trailing spaces.

### Named Spans

Spans can carry an `id`. The agent can update individual spans by ID without
replacing the full array.

```json
// Creation
{"spans": [
  {"text": "CPU: "},
  {"id": "cpu", "text": "52%", "fg": "green"},
  {"text": "  MEM: "},
  {"id": "mem", "text": "3.2G", "fg": "yellow"}
]}

// Partial update — only the CPU field changes
{"update_spans": [
  {"id": "cpu", "text": "78%", "fg": "yellow"}
]}
```

Only the targeted span is re-rendered. Everything else stays untouched. If the
new text is shorter than the old text, the remaining cells are filled with the
parent element's background.

### Region Writes

For freeform drawing — charts, custom visualizations, anything that doesn't map
cleanly to a linear span sequence — the agent can write directly to `(row, col)`
offsets within the overlay or panel.

```json
{"method": "region_write", "params": {
  "id": "my-overlay-or-panel",
  "writes": [
    {"row": 3, "col": 0, "text": "████░░░░", "fg": "green"},
    {"row": 4, "col": 0, "text": "██████░░", "fg": "yellow"}
  ]
}}
```

Coordinates are always relative to the element's origin. `(0, 0)` is the
top-left of the overlay or panel, not the top-left of the terminal. The agent
doesn't need to know its absolute screen position.

### Batched Updates

Multiple operations (span updates + region writes) can be batched into a single
request and rendered atomically within a synchronized output frame (DEC 2026).
No partial redraws are visible to the user.

```json
{"method": "batch_update", "params": {
  "id": "dashboard",
  "update_spans": [
    {"id": "status", "text": "running", "fg": "green"}
  ],
  "writes": [
    {"row": 5, "col": 0, "text": "████████", "fg": "cyan"}
  ]
}}
```

## Input Routing

### Current Limitation

Today, input capture is global — `capture` or `passthrough`, all or nothing.
When captured, every keystroke goes to every WebSocket subscriber. There's no
way to direct keystrokes to a specific overlay or panel.

### Focusable Elements

Overlays and panels can be marked `focusable: true` at creation. At most one
element has focus at a time. The focused element receives keystrokes; unfocused
elements don't.

```json
{"method": "create_overlay", "params": {
  "x": 10, "y": 5, "width": 30, "height": 8,
  "background": {"bg": {"r": 30, "g": 30, "b": 30}},
  "focusable": true,
  "spans": [...]
}}
```

### Focus Requires Input Capture

Nothing can have focus unless the client is in input capture mode. If the client
is not in capture mode, all elements are inert — displayed but not interactive.
If the client leaves capture mode (explicitly or via `Ctrl+\`), all elements
lose focus immediately.

This preserves the existing input capture model as the gating mechanism. Focus
is a refinement of capture, not a replacement.

### Focus Management

Focus can be moved explicitly:

```json
{"method": "focus", "params": {"id": "other-overlay"}}
```

Or released without destroying the element:

```json
{"method": "unfocus"}
```

`unfocus` does not leave capture mode — it just means no element currently
receives keystrokes. The agent can re-focus a different element later. To fully
return input to the PTY, the agent must also release capture.

### Input Events

The focused element's subscriber receives keystrokes tagged with the element ID:

```json
{
  "event": "input",
  "target": "my-overlay-id",
  "raw": [27, 91, 65],
  "parsed": {"key": "ArrowUp", "modifiers": []}
}
```

The `target` field tells the agent which element received the input — useful
when the agent manages multiple focusable elements.

### Ctrl+\ Escape Hatch

Same as today — `Ctrl+\` always force-releases input back to passthrough,
regardless of focus state. All elements lose focus. The agent receives
notification and should clean up gracefully.

## Widgets

Widgets are built-in interactive elements that live inside overlays or panels.
They handle keystrokes locally at native speed — no round-trip to the agent.
The agent defines the widget, `wsh` handles the interaction, and the agent gets
the result.

### Why Widgets

When a user navigates a selection list, they expect instant cursor movement.
When they type into a text field, they expect instant character echo. An agent
handling each keystroke through an LLM inference call introduces 500ms–2s+ of
latency per keypress. Widgets eliminate this for common interaction patterns.

Widgets are optional. The agent can always handle keystrokes directly via raw
input events for anything custom — charts, novel visualizations, game-like
interfaces. Widgets are the fast path, not the only path.

### Widget Placement

Widgets are placed at a `(row, col)` offset within an overlay or panel, just
like region writes. They have an explicit size. They render within the parent
element's rectangle using the parent's background as their default.

```json
{"method": "create_widget", "params": {
  "parent": "my-overlay-id",
  "widget_id": "env-selector",
  "type": "select_list",
  "row": 1, "col": 1,
  "width": 28,
  "options": ["development", "staging", "production"],
  "selected": 0,
  "style": {
    "normal": {"fg": "white"},
    "highlighted": {"fg": "white", "bg": {"r": 60, "g": 60, "b": 120}},
    "indicator": "▸ "
  }
}}
```

The parent overlay/panel must be focusable. When it has focus, keystrokes are
routed to the active widget within it.

### Widget Set

| Widget          | Interactive | Description                                  |
|-----------------|-------------|----------------------------------------------|
| `text_input`    | yes         | Single-line text entry with cursor           |
| `select_list`   | yes         | Pick one from a list, closes on confirm      |
| `radio_list`    | yes         | Pick exactly one, stays open (form element)  |
| `checkbox_list` | yes         | Pick many, stays open (form element)         |
| `confirm`       | yes         | Yes/no prompt                                |
| `text_display`  | yes         | Scrollable read-only text block              |
| `progress_bar`  | no          | Agent-updated progress indicator             |

#### `text_input`

Single-line text entry. Handles character input, backspace, delete, left/right
arrow, home/end. Enter submits, Escape cancels.

```json
{
  "type": "text_input",
  "row": 2, "col": 10, "width": 20,
  "value": "",
  "placeholder": "hostname...",
  "style": {"fg": "white", "bg": {"r": 40, "g": 40, "b": 40}}
}
```

Agent receives on Enter:
```json
{"event": "widget_submit", "widget_id": "...", "value": "my-host"}
```

#### `select_list`

Navigable list with highlight. Arrow keys / j/k move, Enter confirms, Escape
cancels. The list closes (or signals the agent) on selection.

```json
{
  "type": "select_list",
  "row": 1, "col": 1, "width": 28,
  "options": ["development", "staging", "production"],
  "selected": 0,
  "style": {
    "normal": {"fg": "white"},
    "highlighted": {"fg": "white", "bg": {"r": 60, "g": 60, "b": 120}},
    "indicator": "▸ "
  }
}
```

Agent receives:
```json
{"event": "widget_submit", "widget_id": "...", "selected": 1, "value": "staging"}
```

#### `radio_list`

Like `select_list` but designed as a form element — arrow keys move the
selection, but the list stays visible and interactive. Space or Enter toggles
the selected option. The agent reads the current value when the form is
submitted.

Agent receives on toggle:
```json
{"event": "widget_change", "widget_id": "...", "selected": 2, "value": "production"}
```

#### `checkbox_list`

Multi-select list. Arrow keys navigate, Space toggles individual items, Enter
confirms the full selection.

Agent receives on confirm:
```json
{"event": "widget_submit", "widget_id": "...", "selected": [0, 2], "values": ["testing", "ci-cd"]}
```

#### `confirm`

Yes/no prompt. Responds to y/n/Enter/Escape.

```json
{
  "type": "confirm",
  "row": 5, "col": 1, "width": 28,
  "prompt": "Deploy to production?",
  "labels": ["[Y]es", "[N]o"]
}
```

Agent receives:
```json
{"event": "widget_submit", "widget_id": "...", "confirmed": true}
```

#### `text_display`

Scrollable read-only text block. Arrow keys / j/k scroll. Useful for log
output, previews, diffs.

```json
{
  "type": "text_display",
  "row": 1, "col": 1, "width": 28, "height": 10,
  "content": "...long text...",
  "wrap": true
}
```

Non-submitting — the agent updates content via the widget update API and the
user scrolls to read.

#### `progress_bar`

Non-interactive display element. The agent updates its value as work progresses.

```json
{
  "type": "progress_bar",
  "row": 7, "col": 1, "width": 28,
  "value": 0.65,
  "label": "Building...",
  "style": {"filled": "█", "empty": "░", "fg": "green"}
}
```

### Widget Focus

When a parent overlay/panel has multiple widgets, Tab/Shift-Tab moves focus
between them. The agent can also set focus programmatically:

```json
{"method": "focus_widget", "params": {"widget_id": "host-input"}}
```

### Custom Drawing + Widgets Together

An agent can mix raw drawing (spans, region writes) with widgets in the same
overlay or panel. Draw box borders, labels, and decorations with spans. Place
widgets inside for the interactive parts. The agent controls the layout;
widgets handle the interaction.

## Alternate Screen Mode

For full-screen UIs — dashboards, wizards, configuration interfaces — the agent
requests alternate screen mode. This gives the agent the entire terminal grid
without disturbing the user's shell session.

### Entering

```json
{"method": "enter_alt_screen"}
```

What happens:

1. `wsh` writes `\x1b[?1049h` to the local terminal (saves display, switches
   to alternate screen buffer).
2. The alternate screen is cleared.
3. The agent creates overlays and panels that render onto the now-blank screen.
4. PTY continues running — output is parsed and tracked, just not displayed.
5. Input capture is **not** implicit. The agent must capture input separately.

The agent doesn't draw "on the alternate screen" directly. It uses overlays and
panels — the same primitives, the same API. The alternate screen simply provides
a blank surface for them to render on.

### Exiting

```json
{"method": "exit_alt_screen"}
```

What happens:

1. All overlays and panels created during alt-screen mode are destroyed.
2. `wsh` writes `\x1b[?1049l` (restores saved display).
3. Normal-mode overlays and panels are restored (made visible again).
4. The user's shell session reappears exactly as it was.

### Screen Mode Binding (Option 3)

Every overlay and panel is tagged with the screen mode in which it was created
(`normal` or `alt`). Mode transitions follow these rules:

- **Enter alt screen:** Normal-mode elements are hidden. Alt screen starts with
  a blank canvas.
- **Exit alt screen:** Alt-screen elements are destroyed. Normal-mode elements
  are restored.

This reflects the asymmetry between normal and alt-screen UI:

- **Normal-mode UI is long-lived** — status bars, dashboards, monitoring panels
  that the agent builds up over time.
- **Alt-screen UI is ephemeral** — a wizard, a form, a configuration interface
  built for a specific moment.

The lifecycle rules match: long-lived state is preserved across mode
transitions, ephemeral state is auto-cleaned on exit.

This is also the most agent-forgiving model. An agent that crashes mid-alt-screen
gets cleaned up automatically. An agent that forgets about its normal-mode panel
still has it when it comes back.

### PTY During Alt Screen

The PTY keeps running at its original size. No resize signal, no disruption. If
the agent is monitoring a build in another session and displaying results in the
alt-screen UI, the build doesn't know anything happened.

### PTY Alt-Screen Conflicts

If the PTY's program enters alternate screen (e.g., vim resumes) while the agent
holds alt-screen mode:

- `wsh` continues parsing PTY output normally — the terminal state machine
  tracks the PTY's mode transition.
- `wsh` does **not** forward the PTY's `\x1b[?1049h` / `\x1b[?1049l` sequences
  to the local terminal. The agent owns the display.
- When the agent exits alt screen, `wsh` checks the PTY's current state. If
  the PTY is now in alternate screen (vim is active), `wsh` forwards that state
  to the terminal — the user sees vim, not their shell prompt.

The PTY's display state is always eventually consistent with the local terminal.
The agent can temporarily suppress it, but once the agent steps away, the
correct PTY state is restored.

The agent receives an event when the PTY's screen mode changes during
agent-held alt screen:

```json
{
  "event": "pty_mode_change",
  "alternate_active": true
}
```

### Nested Alt Screen

Only one alt-screen session at a time. If the agent is already in alt-screen
mode and calls `enter_alt_screen` again, it's a no-op or an error.

## Zero-Row PTY Minimum

The current 1-row minimum for PTY space is reduced to 0. Panels can consume the
entire terminal height. If there are more panels than can fit, the lowest-priority
panels (lowest z-index) are hidden, same as today.

This enables the pattern of building a full-screen UI entirely from panels
without needing alternate screen mode — the agent creates enough panels to fill
the screen.

The layout algorithm is unchanged except for the minimum:

1. Sort panels by position, then z-index descending.
2. Greedily allocate rows in priority order.
3. Once 0 rows remain for the PTY, stop. Remaining panels are hidden.
4. Hidden panels still exist and respond to API queries.
5. If space opens up, hidden panels become visible automatically.

## Usage Patterns

Three natural patterns emerge from the enhanced primitives:

### Partial UI (floating dialogs, menus)

Agent creates an opaque overlay with widgets, captures input, focuses the
overlay. The PTY continues running underneath. The overlay handles interaction
at native speed. When done, the agent destroys the overlay and releases capture.

```
Agent creates overlay (x=15, y=5, w=35, h=10, focusable=true)
Agent adds widgets: select_list, text_input, confirm button
Agent captures input, focuses the overlay
User interacts — widgets handle keystrokes locally
User confirms → agent receives widget_submit events
Agent destroys overlay, releases capture
```

### Docked UI (status dashboards, monitoring)

Agent creates panels that consume most or all rows. Agent draws content with
named spans and region writes. Panels persist across interactions. Agent updates
individual spans as data changes.

```
Agent creates bottom panel (height=5)
Agent draws chart with region_write: bar graph of CPU usage
Agent polls data source, updates specific cells via region_write
User sees a live-updating dashboard at the bottom of their terminal
```

### Full-Screen UI (wizards, configuration, rich displays)

Agent enters alt screen. Normal-mode elements are hidden. Agent creates overlays
and panels on the blank canvas — a form with multiple fields, a tabbed
interface, a rich data browser. Agent captures input. When done, agent exits alt
screen — everything is cleaned up, normal display restored.

```
Agent enters alt screen
Agent creates overlays with widgets: text_inputs, radio_lists, confirm
Agent captures input, user fills out the form
User submits → agent processes results
Agent exits alt screen → shell restored, normal-mode panels reappear
```

### Custom Drawing (charts, visualizations)

For anything the widget set doesn't cover, the agent draws directly using
region writes. A histogram, a time-series chart, a network topology diagram —
the agent computes the visual and writes characters to cells. Updates happen
on the agent's own tick (when new data arrives), not on every keystroke, so LLM
latency is acceptable.

```
Agent creates overlay (w=40, h=15)
Agent draws axes with region_write
Agent queries data, draws bars/lines with styled block characters
Agent updates on a timer — redraws changed cells only
```
