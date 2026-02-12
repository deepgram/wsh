---
name: wsh:monitor
description: >
  Use when you need to watch, observe, or react to human terminal activity.
  Examples: "monitor the terminal for errors", "watch what the user is doing
  and provide help", "audit terminal activity for security issues".
---

# wsh:monitor — Watching and Reacting

In this mode, you're not driving the terminal — the human is.
You're watching what happens and providing value by reacting:
flagging errors, offering help, catching mistakes, maintaining
context. You're a copilot, not the pilot.

## Two Approaches

### Polling (Simple)
Periodically read the screen and react to what you see.
Good enough for most use cases:

    read screen
    analyze what changed
    respond if needed (overlay, panel, conversation)
    wait
    repeat

Polling is simple and straightforward. The downside is latency —
you're checking on an interval, so you might miss transient
output or react a few seconds late.

### Event Subscription (Real-Time)
Subscribe to real-time events via the WebSocket (see the
core skill for connection mechanics). Subscribe to the
events you care about — `lines` for output, `input` for
keystrokes — and the server pushes them as they happen.

You also get periodic `sync` snapshots when the terminal
goes quiet, giving you a natural checkpoint to analyze
the current state.

For most monitoring tasks, **start with polling**. Move to
event subscription when you need immediate reaction time.

## Pattern Detection

Monitoring is only useful if you know what to look for.
Here are the categories of patterns worth detecting.

### Errors and Failures
Read the screen and scan for:
- Compiler errors — "error[E", "SyntaxError", "TypeError"
- Command failures — "command not found", "No such file"
- Permission issues — "Permission denied", "EACCES"
- Network failures — "connection refused", "timeout"
- Stack traces — indented lines starting with "at" or "in"

When detected: show a panel or overlay with a brief
explanation and suggested fix. Don't interrupt the human's
flow — they may have already noticed.

### Dangerous Commands
Watch input events for risky patterns:
- `rm -rf` with broad paths
- `git push --force` to main/master
- `DROP TABLE`, `DELETE FROM` without WHERE
- `chmod 777`
- Credentials or tokens being pasted into commands

When detected: use input capture to intercept before
execution. Show an overlay asking for confirmation.
Release input if approved, discard if rejected.

### Opportunities to Help
Not everything is about preventing mistakes. Watch for
moments where help would be welcome:
- A command was run three times with slightly different
  flags — the human might be guessing
- A long error message just scrolled by — summarize it
- The human typed a command that has a better alternative
- A build succeeded after repeated failures — celebrate

### State Tracking
Maintain a mental model of what the human is doing:
- What directory are they in?
- What project are they working on?
- What was the last command they ran?
- Are they in a flow state or exploring?

This context makes your reactions more relevant. An `rm`
in a temp directory is different from an `rm` in the
project root.

## How to Respond

The hardest part of monitoring isn't detection — it's
calibrating your response. Too noisy and the human ignores
you. Too quiet and you're useless.

### Response Channels

**Overlays** — lightweight, transient. Best for:
- Brief warnings ("this will delete 47 files")
- Quick tips ("try --dry-run first")
- Acknowledgments ("build passed")

Position them near the relevant content. Remove them
after a few seconds or when the screen changes.

**Panels** — persistent, always visible. Best for:
- Running context summaries ("working in: /project,
  branch: feature/auth, last command: cargo test")
- Session dashboards during long workflows
- Error explanations that need to stay visible while
  the human fixes the issue

Keep panels compact. One or two lines. Update in place
rather than creating new ones.

**Input capture** — disruptive, use sparingly. Best for:
- Blocking genuinely dangerous commands
- Approval gates where the human explicitly asked for
  your oversight

Never capture input for something the human can easily
undo. Reserve it for irreversible actions.

**Conversation** — the chat with the human. Best for:
- Detailed explanations that don't fit in an overlay
- Suggestions that need discussion
- Questions that require a thoughtful answer

### Visual Structure

wsh renders spans as-is — no built-in borders, padding,
or separators. Build visual structure from text characters:

- **Borders:** use box-drawing characters (`┌─┐│└─┘`)
  for framed overlays and panels
- **Padding:** add spaces for breathing room
- **Separators:** use `│` between inline elements
- **Full-width rules:** use `━` repeated to `cols` width
  to separate panels from terminal content

See the wsh:visual-feedback skill for detailed guidance
on constructing visual elements.

### Calibration Principles

**Be quiet by default.** Only react when you have
something genuinely useful to say. The human chose to
work in a terminal — they know what they're doing most
of the time.

**Severity drives channel.** Informational → overlay.
Important → panel. Critical → input capture. Complex →
conversation.

**Don't repeat yourself.** If you flagged an error and
the human re-runs the same command, they saw your
warning and chose to proceed. Don't flag it again.

**Dissolve gracefully.** Remove overlays when they're
stale. Update panels rather than accumulating them.
Leave no visual debris.

## Monitoring Recipes

### Contextual Help
Watch what command the human is typing or has just started.
Detect the program and provide relevant, timely guidance:

    read screen
    # See: "$ parted /dev/sda"
    # The human is partitioning a disk.

    create overlay near the bottom of screen:
      "┌─ Parted Quick Ref ────────────────┐"
      "│ Common schemes:                   │"
      "│  GPT + EFI: mkpart ESP fat32      │"
      "│    1MiB 513MiB                    │"
      "│  Root: mkpart primary ext4        │"
      "│    513MiB 100%                    │"
      "│ Type 'help' for all commands      │"
      "└───────────────────────────────────┘"

This works for any tool. Detect the command, surface the
most useful information:
- `git rebase` → show the rebase commands (pick, squash,
  fixup, drop) and common flags
- `docker build` → show relevant Dockerfile tips
- `kubectl` → show the resource types and common flags
- `ffmpeg` → show common codec and format options
- `iptables` → show chain names and common patterns

**Timing matters.** Show help when the human starts
the command, not after they've already finished. Update
or remove the overlay when they move on to something else.

**Be concise.** A help overlay is a cheat sheet, not a
man page. Three to five lines of the most useful
information. If the human needs more, they'll ask.

### Error Summarizer
Long error messages scroll by and are hard to parse.
Detect them and provide a summary:

    read screen or scrollback
    # See: 47 lines of Rust compiler errors

    create panel at bottom:
      "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
      " 3 errors: missing lifetime in     "
      " auth.rs:42, type mismatch in      "
      " db.rs:108, unused import main.rs:3"

### Security Watchdog
Monitor for sensitive data in terminal output:
- API keys, tokens, passwords echoed to screen
- AWS credentials in environment variables
- Private keys displayed via cat

When detected, overlay a warning. The data is already
on screen — you can't un-show it — but you can alert the
human to rotate the credential.

### Session Journaling
Maintain a running summary panel of what's happened:

    panel at top, 2 lines:
      "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
      " Session: 14 cmds │ 2 errs │ 38 min"
      " Last: cargo test (PASS) in /wsh   "

Update after each command completes. This gives the human
(and you) a persistent sense of where things stand.

## Pitfalls

### Don't Be a Backseat Driver
The human is in control. If they run a command you'd do
differently, that's their choice. Only intervene when
something is genuinely dangerous or when they appear stuck.
"You should use --verbose" is annoying. "That rm will
delete your git repo" is helpful.

### Don't Obscure the Terminal
Overlays and panels consume screen space. On a small
terminal, a 3-line panel and two overlays can cover a
significant portion of the visible content. Be aware of
the terminal dimensions (available in the screen response)
and scale your visual elements accordingly. On a 24-row
terminal, a 1-line panel is plenty.

### Polling Frequency
If you're polling, don't hammer the API. Every request
costs a round-trip. Reasonable intervals:
- Active monitoring (security): every 1-2 seconds
- Contextual help: every 2-3 seconds
- Session journaling: every 5-10 seconds

Match the frequency to the urgency. Most monitoring
doesn't need sub-second reaction time.

### Don't Monitor What Wasn't Asked For
If the human asked you to watch for errors, don't also
start providing unsolicited style tips. Scope your
monitoring to what was requested. You can suggest
expanding scope, but don't do it silently.

### Privacy
The human may type passwords, access personal accounts,
or work on confidential material. If you're monitoring,
you see everything. Don't log, repeat, or comment on
anything that looks private unless it's directly relevant
to the monitoring task you were asked to perform.

### Know When to Stop
Monitoring is not a permanent state. When the human is
done with the task that warranted monitoring, tear down
your panels and overlays and stop polling. Ask if you
should continue rather than assuming.
