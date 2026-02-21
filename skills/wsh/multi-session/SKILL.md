---
name: wsh:multi-session
description: >
  Use when you need to orchestrate multiple parallel terminal sessions
  via wsh server mode. Examples: "run builds in parallel across several
  projects", "tail logs in one session while working in another",
  "fan out tests across multiple sessions and gather results".
---

# wsh:multi-session — Parallel Terminal Sessions

Sometimes one terminal isn't enough. You need to run a build
while tailing logs. Run tests across three environments
simultaneously. Drive multiple processes that each need
independent input and output. Multi-session gives you this.

## When to Use Multiple Sessions

**Use multi-session when:**
- Tasks are independent and can run in parallel
- You need isolated environments (different directories,
  different env vars, different shells)
- A long-running process needs monitoring while you work
  in another session
- You're coordinating multiple tools that each need their
  own terminal

**Don't use multi-session when:**
- A single shell with `&&` or `&` would suffice
- The tasks are strictly sequential
- You only need to run one thing at a time

## Prerequisites

wsh always runs as a server daemon, so multi-session is
always available. The sessions API endpoint is at
`/sessions/`. You create sessions explicitly and interact
with each via `/sessions/:name/` prefix.

If you're in an attached wsh session (started with `wsh`),
there's already a `default` session. You can create
additional sessions via the API alongside it.

## Creating Sessions

Give each session a descriptive name that reflects its purpose:

    create session "build"
    create session "test" with command: npm test --watch
    create session "logs" with command: tail -f /var/log/app.log

You can specify:
- `name` — identifier (auto-generated if omitted)
- `command` — run a specific command instead of a shell
- `rows`, `cols` — terminal dimensions
- `cwd` — working directory
- `env` — environment variables (object of key-value pairs)
- `tags` — string labels for grouping and filtering

A session with a `command` will exit when that command
finishes. A session without one starts an interactive shell
that persists until you kill it.

## Listing and Inspecting

    list sessions
    get session "build"

## Ending Sessions

Prefer a graceful exit when the session is running an
interactive program:

    # Exit a shell
    send input to "build": exit\n

    # Quit a TUI
    send input to "monitor": q

The session will close automatically when its process exits.

If the process is stuck or you don't care about graceful
shutdown, force-kill it:

    kill session "build"

This terminates the process immediately. Clean up after
yourself — don't leave orphaned sessions running.

## Renaming Sessions

If a session's purpose changes:

    rename session "build" to "build-v2"

## Working Across Sessions

The power of multi-session is parallelism. Here are the
common coordination patterns.

### Fan-Out: Run in Parallel, Gather Results

Spawn several sessions, kick off work in each, then poll
them for completion. Tag them for easy group operations:

    # Create sessions and start work (all tagged "ci")
    create session "test-unit" tagged: ci, send: npm run test:unit
    create session "test-e2e" tagged: ci, send: npm run test:e2e
    create session "lint" tagged: ci, send: npm run lint

    # Poll each for completion
    for each session:
        wait for idle
        read screen
        check for shell prompt (done) or still running

    # Gather results
    read scrollback from each session
    report combined results

This is the most common pattern. The key insight: you don't
have to wait for one to finish before checking another.

**Best approach — wait for any session (with tag filter):**

    wait for idle on sessions tagged "ci" (timeout 1000ms)
    # returns the name of whichever session settled first
    # read its screen, check if done
    # repeat with last_session + last_generation to avoid
    # re-returning the same session immediately

This races all tagged sessions and returns the first to settle.
Much more efficient than polling each one individually. The tag
filter ensures unrelated sessions don't interfere.

**Alternative — poll round-robin:**

    await idle test-unit (short timeout, 1000ms, fresh=true)
    await idle test-e2e (short timeout, 1000ms, fresh=true)
    await idle lint (short timeout, 1000ms, fresh=true)
    # repeat until all show shell prompts
    # fresh=true prevents busy-loop storms when a session is idle

### Watcher: Long-Running Process + Working Session

One session runs something persistent (a dev server, log
tail, file watcher). Other sessions do active work.
Periodically check the watcher for relevant output:

    create session "server", send: npm run dev
    create session "work"

    # Do work in the work session
    send to "work": curl localhost:3000/api/health

    # Check server session for errors if something fails
    read screen from "server"

### Pipeline: Sequential Handoff

One session's output informs the next session's input.
This isn't true parallelism — it's staged work:

    create session "build"
    send to "build": cargo build 2>&1 | tee /tmp/build.log
    wait for build to finish

    create session "deploy"
    send to "deploy": ./deploy.sh
    # only if build succeeded

## Pitfalls

### Session Sprawl
It's easy to create sessions and forget about them. Every
session is a running process consuming resources. Adopt a
discipline:
- Create sessions with a clear purpose
- Destroy or exit sessions as soon as their purpose is served
- Before creating new sessions, list existing ones to see
  if you can reuse one
- If you're doing a fan-out, clean up all sessions when
  the fan-out is complete

### Naming and Tagging Discipline
Names are how you identify individual sessions. Tags are how
you group them. Use both for organization:

    Good: "test-unit", "test-e2e", "build-frontend"
    Bad:  "session1", "s2", "tmp"

If you're creating sessions in a loop, use a predictable
naming scheme so you can iterate over them later:

    test-0, test-1, test-2
    build-api, build-web, build-docs

Tags let you group related sessions for bulk operations.
Tag all test sessions with "test", all build sessions with
"build", then list or wait-for-idle on just that group:

    create "test-unit" tagged: test
    create "test-e2e" tagged: test
    create "lint" tagged: test

    list sessions tagged "test"
    wait for idle on sessions tagged "test"

Tags can be added and removed after creation, so you can
re-categorize sessions as their role changes.

### Don't Multiplex What Doesn't Need It
If you just need to run three commands in sequence, one
session with `&&` is simpler than three sessions. Multi-session
adds overhead — session creation, polling, cleanup. Only use
it when you genuinely need parallelism or isolation.

### Session Exit Detection
A session running a specific command (not a shell) will exit
when that command finishes. The session disappears from the
sessions list. If you're polling and a session vanishes, the
process finished — read its output before it's gone, or
redirect output to a file you can read from another session.

### Context Isolation Cuts Both Ways
Each session is independent — different working directory,
different environment, different shell history. If you `cd`
in one session, the others are unaffected. This is useful
for isolation but means you can't share state between
sessions through shell variables. Use files, environment
variables at creation time, or the filesystem as shared state.
