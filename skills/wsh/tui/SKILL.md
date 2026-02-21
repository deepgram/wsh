---
name: wsh:tui
description: >
  Use when you need to operate a full-screen terminal application (TUI)
  via wsh. Examples: "navigate vim to edit a file", "use lazygit to stage
  and commit changes", "interact with htop or k9s".
---

# wsh:tui — Operating Full-Screen Terminal Applications

Some programs take over the entire terminal — vim, htop, lazygit,
k9s, midnight commander. They use the terminal's "alternate screen
buffer," a fixed grid where the program controls every character
position. This is a fundamentally different interaction model from
command-and-response.

## Detecting Alternate Screen Mode

When a TUI is active, the screen response includes:

    "alternate_active": true

This tells you you're in grid mode. The screen is no longer a
log of output — it's a 2D canvas the program redraws at will.
Scrollback is irrelevant while alternate screen is active;
the program owns the entire display.

When the TUI exits, `alternate_active` flips back to `false`
and the normal scrollback view resumes exactly where it left
off. None of the TUI's screen content leaks into scrollback.

## Reading a 2D Grid

In a TUI, screen position matters. A line isn't just text —
it's a row in a spatial layout. Use `styled` format here;
formatting carries critical information:

- **Bold or highlighted text** often marks the selected item
- **Color differences** distinguish panes, headers, status bars
- **Inverse/reverse video** typically indicates cursor position
  or selection
- **Dim or faint text** marks inactive elements

Read the full screen and interpret it spatially. The first few
lines are often a header or menu bar. The last line or two are
often a status bar or command input. The middle is content.

## Navigation

TUI programs don't use typed commands — they use keystrokes.
Every key does something different depending on context. You
need to know the program's keybindings.

### Universal Navigation Keys

    $'\x1b[A'       # Arrow Up
    $'\x1b[B'       # Arrow Down
    $'\x1b[C'       # Arrow Right
    $'\x1b[D'       # Arrow Left
    $'\x1b[5~'      # Page Up
    $'\x1b[6~'      # Page Down
    $'\x1b[H'       # Home
    $'\x1b[F'       # End
    $'\t'           # Tab (often cycles panes or fields)
    $'\n'           # Enter (confirm / open)
    $'\x1b'         # Escape (cancel / back)

### Vim-Style Navigation
Many TUIs adopt vim conventions:

    h, j, k, l      # left, down, up, right
    g, G             # top, bottom
    /                # search
    n, N             # next/previous match
    q                # quit

### Sending Keystrokes
Send one keystroke at a time. Wait briefly between keystrokes
to let the TUI redraw — TUIs repaint the screen after each
input, and you need the updated screen to know where you are.

    send: j          # move down
    wait (500ms)
    read screen      # see what's selected now
    send: j          # move down again
    wait (500ms)
    read screen      # verify position

This is slower than blasting keys, but reliable. You're
navigating blind if you don't read between keystrokes.

## Understanding TUI Layouts

When you first enter a TUI, read the full screen and build a
mental map. Most TUIs follow common layout patterns:

### Typical Structure

    +----------------------------------+
    | Menu bar / Title bar             |  <- rows 0-1
    +----------------------------------+
    |                                  |
    | Main content area                |  <- middle rows
    | (list, editor, dashboard)        |
    |                                  |
    +----------------------------------+
    | Status bar / Help / Command line |  <- last 1-2 rows
    +----------------------------------+

### Finding Your Bearings
- **Status bar** (usually bottom): shows mode, filename,
  position, hints. Read this first — it often tells you
  everything you need to know about current state.
- **Help hints**: many TUIs show keybinding hints at the
  bottom or top. Look for text like `q:quit  j/k:navigate
  ?:help` or `^X Exit  ^O Save`.
- **The selected item**: look for inverse video, bold, or
  color-highlighted text in the content area. That's your
  cursor position.
- **Pane borders**: look for `|`, `-`, `+` characters or
  box-drawing characters. These indicate split panes. Only
  one pane is active at a time — Tab or Ctrl+W typically
  switches between them.

### Modals and Dialogs
TUIs often pop up confirmation dialogs or input fields over
the main content. These appear as a differently-styled block
in the middle of the screen. Look for:
- A bordered box that wasn't there before
- Text like "Are you sure?" or "Enter filename"
- Highlighted buttons like `[ OK ]  [ Cancel ]`

When a modal is active, navigation keys operate on the modal,
not the content behind it.

## Common Applications

You don't need to memorize every TUI's keybindings. But
knowing the basics for frequently encountered programs helps.

### Text Editors
**vim/neovim:** Starts in Normal mode. `i` to insert text,
`Esc` to return to Normal. `:w` save, `:q` quit, `:wq` both.
If lost, press `Esc Esc` then `:q!` to quit without saving.

**nano:** Simpler. Just type to edit. Keybindings shown at
bottom. `^` means Ctrl. `^X` exits, `^O` saves.

### Git TUIs
**lazygit:** Pane-based. Tab switches panes (files, branches,
commits). `j/k` navigates, Enter opens, `space` stages, `c`
commits, `p` pushes, `q` quits.

### System Monitors
**htop/top:** Shows processes. `j/k` or arrows to navigate,
`k` to kill a process, `q` to quit. `F` keys for actions
(shown at bottom).

### Kubernetes
**k9s:** Resource browser. `:` opens command mode for
resource type (`:pods`, `:deployments`). `j/k` navigates,
`Enter` drills in, `Esc` goes back, `d` describe, `l` logs.

### General Strategy for Unfamiliar TUIs
1. Read the screen — look for help hints at top or bottom
2. Try `?` or `h` — most TUIs open a help screen
3. Try `F1` (`$'\x1bOP'`) — some use function keys for help
4. Read the help, then press `q` or `Esc` to close it
5. If completely stuck: `q`, `Esc`, `:q`, `Ctrl+C`,
   `Ctrl+Q` — try these in order to exit

## Exiting a TUI

Getting out is as important as getting in. When you're done
with a TUI, you need to return to the normal shell prompt.

### Exit Strategies (in order of preference)
1. Use the program's quit command (`q`, `:q`, `Ctrl+X`)
2. Check the status bar for exit hints
3. Press `Esc` to back out of any modal or sub-mode first
4. If the program won't quit cleanly, `Ctrl+C`
5. Last resort: `Ctrl+Z` to suspend, then `kill %1`

### Confirming You're Out
After sending a quit command, wait for idle, then check:

    "alternate_active": false

If this is `false`, you're back in normal mode with your shell
prompt. If it's still `true`, the TUI is still running —
your quit command may not have worked, or the program asked
for confirmation before exiting.

## Pitfalls

### Don't type commands into a TUI
A TUI is not a shell. If you send `ls -la\n` into vim, you'll
get those characters inserted into the document. Always know
what mode you're in before sending input.

### Don't blast keystrokes
TUIs redraw the screen after each input. If you send 10 `j`
keystrokes without reading in between, you won't know where
you landed. Navigate one step at a time.

### Watch for mode changes
Many TUIs have multiple modes (vim's Normal/Insert/Visual,
lazygit's panes). The same key does different things in
different modes. Read the screen after each action to confirm
you're in the mode you expect.

### Alternate screen within alternate screen
If you launch a TUI from within a TUI (e.g., vim from within
a file manager), `alternate_active` is still just `true`. You
need to track the nesting yourself by remembering what you
launched and how many layers deep you are.
