---
name: wsh:drive-process
description: >
  Use when you need to drive a CLI program through command-and-response
  interaction via wsh. Examples: "run a build command and check the output",
  "interact with an installer that asks questions", "execute a sequence of
  shell commands and handle errors".
---

# wsh:drive-process — Driving CLI Programs

You're operating a terminal programmatically. You send input, wait
for output to settle, read the screen, and decide what to do next.
This skill teaches you the patterns and pitfalls.

## The Loop

Every interaction follows the same shape:

1. **Send input** — a command, a response to a prompt, a keystroke
2. **Wait for quiescence** — output settles, suggesting the program
   may be idle. Choose your timeout based on what you expect:
   - Fast commands (ls, cat, echo): 500-1000ms
   - Build/install commands: 3000-5000ms
   - Network operations: 2000-3000ms
   Quiescence is a *hint*, not a guarantee. The program may still
   be working — it just hasn't produced output recently.
3. **Read the screen** — see what happened
4. **Decide** — did the command succeed? Is there a prompt waiting
   for input? Did something go wrong? Act accordingly.

When re-polling quiescence (e.g., the command isn't done yet), pass
back the `generation` from the previous response as `last_generation`
to avoid busy-loop storms. Or use `fresh=true` for simplicity.

## Sending a Command

Always include a newline to "press Enter":

    send input: npm install\n

Without the trailing `\n`, you've typed the text but haven't
submitted it. Sometimes that's what you want (e.g., building up
a command before sending), but usually you want the newline.

## Reading the Result

After waiting for quiescence, read the screen. Prefer `plain`
format when you just need text content. Use `styled` when
formatting matters (e.g., distinguishing error output highlighted
in red).

If the output is long, it may have scrolled off screen. Use
scrollback to get the full history.

## Handling Interactive Prompts

Many programs ask questions and wait for a response. After reading
the screen, look for patterns like:

- `[Y/n]` or `[y/N]` — yes/no confirmation
- `Password:` or `Enter passphrase:` — credential prompts
- `>` or `?` — interactive selection (fzf, inquirer, etc.)
- `(yes/no)` — full-word confirmation (e.g., SSH host verification)
- `Press any key to continue`

Respond naturally — send the appropriate input:

    send input: y\n
    send input: yes\n

For password prompts, note that the terminal will not echo your
input back. The screen will look unchanged after you type. Wait
for quiescence after sending — the program will advance.

## Control Characters

These are your emergency exits and special actions:

    $'\x03'         # Ctrl+C  — interrupt / cancel
    $'\x04'         # Ctrl+D  — EOF / exit shell
    $'\x1a'         # Ctrl+Z  — suspend process
    $'\x0c'         # Ctrl+L  — clear screen
    $'\x01'         # Ctrl+A  — beginning of line
    $'\x05'         # Ctrl+E  — end of line
    $'\x15'         # Ctrl+U  — clear line
    $'\x1b'         # Escape

If a command hangs or you need to bail out, Ctrl+C is your first
resort. If the process doesn't respond to Ctrl+C, Ctrl+D or
Ctrl+Z may work. Read the screen after each attempt to see if
it had effect.

## Detecting Success and Failure

After reading the screen, look for signals:

**Success indicators:**
- A fresh shell prompt (`$`, `#`, `>`) on the last line
- Explicit success messages ("done", "completed", "ok")
- Exit code 0 if visible

**Failure indicators:**
- Words like "error", "failed", "fatal", "denied", "not found"
- Stack traces or tracebacks
- A shell prompt after unexpectedly short output
- Non-zero exit codes

When in doubt, check the exit code explicitly:

    send input: echo $?\n

A `0` means the previous command succeeded. Anything else is
a failure.

## Long-Running Commands

Some commands run for minutes or longer — builds, downloads,
test suites. Waiting for quiescence will return when output
pauses, but the command may not be done.

Strategies:

**Poll in a loop.** Wait for quiescence, read the screen, check
if a shell prompt has returned. If not, wait again:

    wait for quiescence (timeout: 5000ms)
    read screen
    # No prompt yet? Wait again.

**Use scrollback for full output.** Long commands produce output
that scrolls off screen. After the command finishes, read
scrollback to get everything:

    read scrollback (offset: 0, limit: 500)

**Don't set unreasonably long quiescence timeouts.** A
`timeout_ms=30000` means you'll wait 30 seconds of silence
before getting a response. Prefer shorter timeouts with
repeated polls — it lets you observe intermediate progress
and react if something goes wrong.

## Common Patterns

### Chained Commands
When you need to run several commands in sequence, you have two
options. Run them as separate send/wait/read cycles when you need
to inspect output between steps:

    # Step 1
    send: cd /project
    wait, read — verify directory exists

    # Step 2
    send: npm install
    wait, read — check for errors

    # Step 3
    send: npm test
    wait, read — check results

Or chain with `&&` when intermediate output doesn't matter:

    send: cd /project && npm install && npm test
    wait, read — check final result

Prefer separate cycles. They give you the chance to detect
problems early and adjust.

### Piped Commands
Pipes work naturally. Send the full pipeline:

    send: grep -r "TODO" src/ | wc -l

### Background Processes
If you start a background process (`&`), it won't block the shell
prompt. But its output may interleave with future commands.
Consider redirecting output:

    send: ./long-task.sh > /tmp/task.log 2>&1 &

Then check on it later:

    send: cat /tmp/task.log

### Pagers
Commands like `git log`, `man`, or `less` enter a pager that
waits for keyboard navigation. If you just need the content,
bypass the pager:

    send: git log --no-pager
    send: PAGER=cat man ls

If you're already stuck in a pager, press `q` to exit:

    send: q

### Heredocs and Multi-Line Input
To write multi-line content, use heredocs:

    send: cat > /tmp/config.yaml << 'EOF'\n
    send: key: value\n
    send: other: thing\n
    send: EOF\n

## Pitfalls

### Don't skip the wait
It's tempting to send input immediately after the previous
command. Don't. If the shell hasn't finished processing, your
input may land in the wrong place — or be swallowed entirely.
Always wait for quiescence before sending the next input.

### Don't assume the screen is everything
The screen shows only the last N lines (typically 24 rows). A
command that produced 500 lines of output will have 476 lines
in scrollback. If you need full output, read scrollback.

### Watch for prompts you didn't expect
Installers, package managers, and system tools love to ask
surprise questions. If you read the screen and see no shell
prompt but also no obvious output-in-progress, look for a
prompt waiting for your response.

### Destructive commands
You are operating a real terminal on a real machine. `rm`,
`DROP TABLE`, `git push --force` — these do real damage.
Before running destructive commands:
- Confirm with the human via overlay, panel, or input capture
- Double-check paths and arguments
- Prefer dry-run flags when available (--dry-run, --whatif, -n)

### Knowing when to give up
If a command is stuck and not responding to Ctrl+C, don't
hammer it with more input. Strategies in order:
1. Send Ctrl+C (`$'\x03'`)
2. Wait a moment, try Ctrl+C again
3. Send Ctrl+Z (`$'\x1a'`) to suspend, then `kill %1`
4. Tell the human what's happening and ask for help

### Shell state persists
You're in a real shell session. Environment variables you set,
directories you `cd` into, background jobs you spawn — they
all persist. Be mindful of the state you leave behind.
