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

**Layer 3: Generated Programs**
Write and run a small program that produces terminal
output. A Python script that renders a table. A bash
script that draws a chart. A small TUI application
built with a framework. The generated program runs
in the terminal; you read its output and interact
with it through wsh.

## Choosing a Layer

Start with the simplest layer that works:
- Need to show some text? Layer 1.
- Need interaction with multiple elements? Layer 2.
- Need complex rendering, live data, or rich layout
  that's awkward to build from spans? Layer 3.

Most generative UI lives in layers 1 and 2. Layer 3
is for when you need something the primitives can't
do well.

## Design Patterns

### Live Dashboard
Combine panels with periodic updates to create a
real-time display. Each update replaces the panel
content:

    panel (top, 3 lines), update every few seconds:
      "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
      " CPU: ████░░░░ 52%  │  MEM: 3.2/8G "
      " Pods: 12 ready     │  Errs: 0     "

Read the data source (run a command, read a file,
call an API), format it into spans, update the
panel. The human sees a live-updating display without
any dedicated monitoring tool installed.

### Interactive Browser
Let the human explore structured data — files,
commits, log entries, API responses. Combine an
overlay list with input capture:

    overlay:
      "┌─ Recent Commits ──────────────────┐"
      "│ ▸ 81883ad docs: rewrite README    │"
      "│   8acf8d7 feat: clear screen on   │"
      "│   55719e8 feat: add session detach │"
      "│                                   │"
      "│   ↑↓ navigate  Enter: view diff   │"
      "│   q: close                        │"
      "└───────────────────────────────────┘"

On Enter, fetch the diff for the selected commit,
replace the overlay content with the diff view,
and add a "Back" option. You're building a mini
application from overlays and keystrokes.

### Wizard
Walk the human through a multi-step process,
building up configuration or input along the way.
Use a panel for progress and an overlay for the
current step:

    panel (bottom, 1 line):
      " Step 2/4: Configure database ──────"

    overlay (center):
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

## When to Generate a Program

Sometimes overlays and panels aren't enough. You need
scrolling, complex layout, rich interactivity, or live
data streams. In these cases, write a small program,
run it in the terminal, and interact with it via wsh.

### Good Candidates for Generation
- **Data tables** with sorting and filtering — hard to
  build from overlays, trivial with a script
- **Charts and graphs** — ASCII bar charts, sparklines,
  histograms
- **Log viewers** with search and highlighting
- **File browsers** with directory traversal
- **Forms** with multiple field types (text, checkbox,
  dropdown)

### Keep It Simple
Generated programs should be disposable — small,
single-purpose, written in seconds. Don't build
a framework. Write the minimum that solves the
immediate need.

Good choices for generation:
- **Bash + standard tools** — printf, column, tput.
  Available everywhere, no dependencies.
- **Python** — rich standard library, good string
  formatting, widely installed.
- **Tools like `gum`, `fzf`, `dialog`** — if installed,
  these provide polished interactive elements with
  minimal code.

### Example: Quick Data Table

Write a small script, run it, read the result:

    # Generate and run
    write /tmp/report.py:
        import json, sys
        data = json.load(open("/tmp/metrics.json"))
        print(f"{'Service':<20} {'Status':<10} {'Latency':>8}")
        print("─" * 40)
        for s in data:
            print(f"{s['name']:<20} {s['status']:<10} {s['ms']:>6}ms")

    send: python3 /tmp/report.py\n
    wait, read screen

### Example: Interactive Selection with fzf

If `fzf` is available, use it instead of building
your own menu:

    send: ls src/**/*.rs | fzf --preview 'head -20 {}'\n
    # fzf enters alternate screen
    # interact via wsh:tui patterns
    # result is printed to stdout after selection

### Clean Up After Yourself
Delete generated scripts when done. Don't leave
/tmp littered with one-off programs:

    delete /tmp/report.py

## Composition Philosophy

The best generative UI combines layers fluidly. A
dashboard panel at the bottom. An overlay menu when
the human needs to make a choice. A generated script
when you need rich output. Each layer serves its
purpose, then gets out of the way.

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
A generated script that took 30 seconds to write
and solves the problem is better than an elegant
TUI application that takes 10 minutes. The human
is waiting. Bias toward quick and functional.

### Don't Mix Layers Carelessly
If you have a generated program running in alternate
screen AND overlays displayed, the overlays will
sit on top of the TUI. This might be useful
(annotations on a running program) or confusing
(two layers of UI competing for attention). Be
deliberate about what's visible when.

### Test on Small Terminals
If you don't know the terminal size, design for 80x24
— the classic default. Everything you build should
be usable at this size, even if it looks better on
a larger terminal.
