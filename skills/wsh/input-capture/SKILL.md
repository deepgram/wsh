---
name: wsh:input-capture
description: >
  Use when you need to intercept keyboard input from the human temporarily.
  Examples: "ask the user for approval before running a command", "build a
  selection menu in the terminal", "capture text input from the user".
---

# wsh:input-capture — Intercepting Keyboard Input

Input capture lets you temporarily take over the keyboard.
While active, keystrokes from the human go to you instead
of the shell. The terminal is frozen — nothing the human
types reaches the PTY. You decide what to do with each
keystroke.

## The Mechanism

    capture input       # grab the keyboard
    # Keystrokes now go to subscribers, not the PTY
    # Do your thing — build a menu, ask a question, etc.
    release input       # give it back

While captured, the human can always press Ctrl+\ to
force-release. This is a safety valve — never disable it,
never tell the human to avoid it. It's their escape hatch.

## Reading Captured Input

Captured keystrokes arrive via WebSocket event subscription
(see the core skill for connection mechanics). Subscribe to
`input` events. Each event includes:
- `raw` — the byte sequence
- `parsed` — structured key information (key name,
  modifiers like ctrl, alt, shift)

Use `parsed` when you want to understand what key was
pressed. Use `raw` when you need to forward the exact
bytes somewhere.

## Check the Current Mode

    get input mode → "passthrough" or "capture"

Always check before capturing. If input is already
captured (by another agent or process), don't capture
again without understanding why.

## Focus Routing

When input is captured, you can direct it to a specific
overlay or panel by setting focus. The element must be
created with `focusable: true`. At most one element has
focus at a time.

Focus is a logical association — it tells the system
(and any listening clients) which UI element the
captured input belongs to. This is useful when you have
multiple overlays or panels visible and want to clarify
which one is "active."

    create overlay (focusable: true) → get id
    capture input
    set focus to overlay id

    # Input events are now associated with this overlay.
    # The element may receive visual focus indicators
    # (e.g., highlighted border) depending on the client.

    # Switch focus to a different element:
    set focus to another-element-id

    # Clear focus:
    unfocus

Focus is automatically cleared when:
- Input is released back to passthrough
- The focused element is deleted

Don't overcomplicate focus management. For a single
dialog or menu, you often don't need explicit focus —
you're the only consumer of captured input, and you
know which overlay you're updating. Focus becomes
valuable when multiple elements are visible and you
want to signal which one is "live."

## Approval Workflows

The most common use of input capture: ask the human a
yes-or-no question and wait for their answer.

### The Pattern

    1. Show the question (overlay or panel)
    2. Capture input
    3. Wait for a keystroke
    4. Interpret the keystroke
    5. Release input
    6. Remove the visual prompt
    7. Act on the answer

### Example: Confirm a Dangerous Command

    # Show the prompt (focusable for focus routing)
    create overlay (focusable: true):
      "┌─ Confirm ──────────────────────┐"
      "│ Delete 47 files from /build ?  │"
      "│         [Y]es    [N]o          │"
      "└────────────────────────────────┘"

    # Capture input and set focus
    capture input

    # Read keystroke via WebSocket
    receive input event
    if key == "y" or key == "Y":
        proceed with deletion
    else:
        cancel

    # Release and clean up
    release input
    delete overlay

### Always Provide a Way Out
Every prompt must accept a "no" or "cancel" keystroke.
Never build a prompt where the only option is "yes."
Show the available keys clearly in the prompt so the
human isn't guessing.

## Selection Menus

Let the human choose from a list of options using
arrow keys and Enter.

### The Pattern

    # Show the menu with one item highlighted (focusable for focus routing)
    create overlay (focusable: true):
      "┌─ Select environment ──────┐"
      "│   development             │"
      "│ ▸ staging                 │"
      "│   production              │"
      "└───────────────────────────┘"

    # Capture input and set focus to the menu overlay
    capture input

    # Handle navigation
    receive input events in a loop:
        Arrow Up / k   → move highlight up
        Arrow Down / j → move highlight down
        Enter          → confirm selection
        Escape / q     → cancel

    # After each navigation keystroke, update the overlay
    # to reflect the new highlight position

    # Release and clean up
    release input
    delete overlay

Track the selected index yourself. On each arrow key,
update the index, rebuild the spans with the highlight
on the new item, and update the overlay.

## Text Input

Capture free-form text from the human — a filename,
a commit message, a search query.

### The Pattern

    create overlay:
      "┌─ Session name ────────────┐"
      "│ > _                       │"
      "└───────────────────────────┘"

    capture input

    buffer = ""
    receive input events in a loop:
        printable character → append to buffer
        Backspace          → remove last character
        Enter              → confirm
        Escape             → cancel

    # After each keystroke, update the overlay to show
    # the current buffer:
    "│ > my-session_               │"

    release input
    delete overlay

You're building a tiny text editor. Handle at least:
character input, backspace, enter to confirm, escape
to cancel. Don't try to build a full readline — keep
it simple.

## Multi-Step Dialogs

Chain prompts together for workflows that need several
pieces of information:

    Step 1: Select environment  (menu)
    Step 2: Enter version tag   (text input)
    Step 3: Confirm deployment  (yes/no)

Keep input captured across all steps. Show a progress
indicator so the human knows where they are:

    "Step 2 of 3 — Enter version tag"

Use focus routing to track which dialog step currently
has input. As you advance through steps, move focus
to the overlay or panel representing the current step.
This signals to the system (and the human) which
element is active.

If the human presses Escape at any step, cancel the
entire flow and release input. Don't trap them in a
multi-step dialog they can't exit.

## Pitfalls

### Minimize Capture Duration
Every moment input is captured, the human cannot use
their terminal. This is disruptive. Capture as late as
possible, release as early as possible:

    Bad:  capture → build UI → show prompt → wait
    Good: build UI → show prompt → capture → wait

Prepare everything before you grab the keyboard. The
human should never see a captured terminal with nothing
on screen explaining why.

### Always Show What's Happening
A captured terminal with no visual explanation is
terrifying. The human types and nothing happens. They
don't know if the terminal is frozen, crashed, or
waiting. Before or simultaneously with capturing input,
always display an overlay or panel explaining what
you're asking and what keys to press.

### Handle Unexpected Input
The human may press keys you didn't anticipate. Don't
crash or behave erratically. Ignore keys you don't
handle:

    if key in expected_keys:
        handle it
    else:
        ignore, do nothing

Don't beep, flash, or scold. Just do nothing for
unrecognized keys.

### Don't Nest Captures
Input is either captured or it isn't — there's no
nesting. If you capture while already captured, you're
still in the same capture session. Design your flows
to be flat: capture once, do your multi-step dialog,
release once.

### Remember Ctrl+\
The human can force-release at any time with Ctrl+\.
Your code must handle this gracefully. If you're
mid-dialog and input is suddenly released:
- Your WebSocket will stop receiving input events
- Your overlay is still showing a stale prompt
- Clean up: remove the overlay, abandon the flow
- Don't re-capture without the human's consent

Check the input mode if you're unsure whether you
still have capture.

### Don't Capture for Information You Could Ask Differently
Input capture is the right tool for real-time keystroke
interaction — menus, approvals, text input that needs
character-by-character handling. If you just need an
answer to a question and latency doesn't matter,
consider using the conversation instead. It's less
disruptive and gives the human more room to think.
