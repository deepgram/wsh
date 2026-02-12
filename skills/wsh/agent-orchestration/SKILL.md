---
name: wsh:agent-orchestration
description: >
  Use when you need to launch and drive other AI agents (Claude Code, Aider,
  Codex, etc.) through their terminal interfaces via wsh. Examples: "run
  multiple Claude Code sessions in parallel on different tasks", "feed a task
  to an AI agent and handle its approval prompts", "coordinate several AI
  agents working on subtasks of a larger project".
---

# wsh:agent-orchestration — Driving AI Agents

You can use wsh to launch and drive other AI agents — Claude
Code, Aider, Codex, or any AI tool with a terminal interface.
This is not science fiction. You spawn the agent in a wsh
session, feed it tasks, handle its approval prompts, and
review its output. You become a manager of AI workers.

## Why?

- **Parallelism.** You can run 5 Claude Code sessions
  simultaneously, each working on a different task.
- **Delegation.** Break a large project into subtasks and
  assign each to an agent session.
- **Specialization.** Different agents have different
  strengths. Orchestrate the right tool for each job.
- **Automation.** Unattended workflows — an agent that
  spawns agents, reviews their work, and merges the results.

## Launching an Agent

Create a session with the agent as its command:

    create session "agent-auth" with command: claude --print "Implement user auth in src/auth.rs"

Or start a shell and launch the agent interactively:

    create session "agent-auth"
    send: claude
    wait for quiescence
    read screen — verify Claude Code has started
    send: Implement user authentication in src/auth.rs\n

The interactive approach gives you more control — you can
set up the environment first, then launch the agent.

## Agent Interaction Patterns

AI agents aren't like regular CLI programs. They produce
long bursts of output, pause to think, ask for approval,
and sometimes wait indefinitely for human input. You need
to recognize these states.

### The Agent Lifecycle
Most AI terminal agents follow this pattern:

    1. Startup — banner, loading, initialization
    2. Thinking — reading files, planning (may be quiet
       for seconds or minutes)
    3. Working — producing output, writing code, running
       commands
    4. Approval — asking permission before a tool use or
       destructive action
    5. Waiting — idle, expecting the next task
    6. Repeat from 2

### Detecting Agent State

**Startup:** Look for the agent's banner or welcome message.
Each agent has a recognizable one. Wait for it before
sending input.

**Thinking:** The terminal may be silent for extended
periods. This is normal — don't assume it's stuck. Use
longer quiescence timeouts (10-30 seconds) and poll
patiently. Look for spinner characters or progress
indicators.

**Approval prompts:** These are critical. Look for patterns
like:
- "Allow?" "Approve?" "Proceed?" "Y/n"
- "Do you want to run this command?"
- A tool call description followed by a prompt
- Highlighted or bold text asking for confirmation

**Waiting for input:** After completing a task, the agent
shows a prompt for the next instruction. Look for an
input indicator — a cursor, a `>`, or an explicit
"What would you like to do?" message.

### Quiescence Is Tricky with Agents
Agents think. Thinking produces no output. A quiescence
timeout of 2 seconds will trigger constantly during
thinking phases. Use longer timeouts (10-30 seconds) and
always read the screen to distinguish "thinking" from
"waiting for input."

When re-polling, use `last_generation` or `fresh=true` to
avoid busy-loop storms where the server responds immediately
because nothing has changed since the last check.

## Feeding Tasks

Keep instructions clear and self-contained. The agent can't
ask you clarifying questions — or rather, it can, but you
need to detect and answer them programmatically.

### Good Task Instructions
Be specific. Include context the agent needs:

    send: Add input validation to the POST /users endpoint. \
    Reject requests where email is missing or malformed. \
    Return 400 with a JSON error body.\n

Not:

    send: fix the users endpoint\n

### Scoping Tasks
Prefer small, well-defined tasks over large ambiguous ones.
An agent working on "implement the entire auth system" will
make many decisions you might disagree with. An agent working
on "add bcrypt password hashing to the User model" has less
room to go sideways.

## Handling Approvals

Many agents ask for permission before taking actions. You
have three strategies:

### Auto-Approve Everything
If you trust the agent and the task is low-risk, configure
it to skip approvals:

    send: claude --dangerously-skip-permissions\n

Only do this for isolated, low-stakes work. Never for
production systems.

### Selective Approval
Read the approval prompt, decide whether to approve:

    read screen
    # See: "Run command: npm install express?"
    # Looks safe.
    send: y\n

    read screen
    # See: "Run command: rm -rf /tmp/build"
    # Inspect further before approving.

### Reject and Redirect
If the agent proposes something wrong, reject and explain:

    send: n\n
    wait, read
    send: Don't delete that directory. Use a fresh \
    build directory at /tmp/build-new instead.\n

## Reviewing Output

After the agent finishes a task, review what it did:

    read screen — see the final status
    read scrollback — see the full conversation

Look for: files changed, commands run, errors encountered,
tests passed or failed. The scrollback is your audit trail.

## Multi-Agent Coordination

The real power is running multiple agents in parallel. Each
gets its own session, its own task, its own workspace.

### The Delegation Pattern

    1. Plan — break the project into independent subtasks
    2. Spawn — create a session for each subtask
    3. Launch — start an agent in each session with its task
    4. Monitor — poll sessions, handle approvals
    5. Review — read each agent's output when it finishes
    6. Integrate — merge the results

### Monitoring Multiple Agents

Poll round-robin with short quiescence timeouts. You're
looking for approval prompts that need your attention:

    for each agent session:
        quiesce (timeout_ms=3000)
        read screen
        if approval prompt detected:
            evaluate and respond
        if task complete:
            review output, clean up session
        if still working:
            move to next session

Agents that are thinking or working need no intervention.
Focus your attention on agents that are blocked waiting
for approval or input.

### Workspace Isolation

Give each agent its own workspace to avoid conflicts.
Agents editing the same files simultaneously will corrupt
each other's work:

    # Use git worktrees for code tasks
    send to "setup": git worktree add /tmp/agent-auth feature/auth
    send to "setup": git worktree add /tmp/agent-api feature/api

    create session "agent-auth", cwd: /tmp/agent-auth
    create session "agent-api", cwd: /tmp/agent-api

Or use separate branches, separate directories, or
separate repositories. The key principle: agents that
might touch the same files must not run in parallel
without isolation.

### Passing Results Between Agents

Agents can't talk to each other directly. Use the
filesystem as the communication channel:

    # Agent 1 produces an artifact
    agent-1 writes to /tmp/api-spec.json

    # You read it and feed it to Agent 2
    read /tmp/api-spec.json
    send to agent-2: Implement the client based on the
    API spec at /tmp/api-spec.json\n

## Pitfalls

### Don't Over-Parallelize
More agents isn't always better. Each agent consumes
resources — API rate limits, CPU, memory. And each agent
you run is one more you need to monitor. Start with 2-3
agents and scale up once you're comfortable with the
monitoring rhythm.

### Watch for Agents Talking to Each Other Accidentally
If two agents are running in the same git repo without
workspace isolation, one agent's changes (file writes,
branch switches, dependency installs) will affect the
other. This causes bizarre failures that are hard to
diagnose. Always isolate.

### Infinite Loops
An agent can get stuck — retrying a failing command,
asking itself a question, or endlessly editing a file.
If you notice an agent producing repetitive output
without progress, intervene:

    send: Stop. The current approach isn't working. \
    Try a different strategy.\n

If it persists, Ctrl+C and give it a fresh start with
clearer instructions.

### Don't Forget Cleanup
Agent sessions with worktrees, temp files, and running
processes leave debris. When the orchestration is done:
- Exit or kill all agent sessions
- Remove worktrees: `git worktree remove /tmp/agent-auth`
- Clean up temp files
- Verify the main repo is in a clean state

### Know When Not to Orchestrate
If the task requires deep understanding of a complex
system, one focused agent with good context will
outperform three agents with shallow context.
Orchestration works best when tasks are genuinely
independent and well-specified. If tasks are tightly
coupled, work them sequentially in a single session.
