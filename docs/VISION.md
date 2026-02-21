# wsh: An API for Your Terminal

> Give AI agents the ability to *interact* with your terminal -- not just run commands, but use programs the way a human does. The terminal is the fundamental interface of modern computers. `wsh` makes it programmable.

---

## The Problem

**AI Can Run Commands. It Can't Use Them.**

Today's AI agents can execute shell commands. They spawn a process, capture stdout, and parse the result. This works for `ls` and `grep`. It falls apart for everything else.

The terminal is not a batch processor. It's an *interactive* medium. Programs prompt for input. TUIs render full-screen interfaces. Installers ask questions. Build tools stream progress. Debuggers wait for breakpoints. AI coding assistants (like Claude Code itself) carry on extended conversations through the terminal.

An agent that can only run commands and read output is like a person who can only send letters. They can't have a conversation. They can't react to what they see. They can't navigate a menu, answer a prompt, approve a change, or recover from an error. They're locked out of the most powerful interface on the computer.

This is the gap: **AI has no way to sit at a terminal and use it like a human does.**

**The Scale of What's Missing**

The terminal is not just *an* interface -- it's *the* interface. Every server, every container, every CI pipeline, every development environment bottlenecks through a terminal. The entire modern computing stack is operated through shell sessions. When you give AI the ability to interact with terminals, you give it the ability to:

- **Drive interactive tools**: AI coding assistants, installers, debuggers, REPLs, configuration wizards -- anything that expects a human at the keyboard
- **Operate in parallel**: Not one shell session, but dozens -- running builds, tests, deployments, and development tasks simultaneously
- **Provide live assistance**: Watch what a human is doing and offer contextual help, warnings, or suggestions in real time
- **Audit and monitor**: Observe shell sessions for security, compliance, or operational awareness
- **Set up entire environments**: Configure a new machine from scratch, test everything end-to-end, and document the process
- **Orchestrate other agents**: Use terminal-based AI tools (including other instances of itself) as sub-agents for complex tasks

The terminal is the fundamental UI of the modern computer. Giving AI full access to it -- the ability to see, type, react, and interact -- makes AI a *co-processor* for both the machine and the human operating it.

**Why This Doesn't Exist Yet**

The terminal has been around for fifty years, and the tools for working with it remotely have barely changed. SSH gives you a remote session. Tmux gives you persistence. Screen recording gives you playback. But none of these expose terminal I/O as a *structured, programmable API*.

There's no tool that lets an AI agent:
- See what's on the screen right now (as structured data, not raw bytes)
- Send keystrokes that interleave naturally with local input
- Subscribe to output events in real time
- Detect whether the terminal is idle or busy
- Manage multiple sessions in parallel
- All while the human continues to use their terminal normally

This is the missing infrastructure.

---

## The Insight

**The Terminal as a Service**

A terminal emulator does two things: it interprets a *protocol* (the stream of bytes, ANSI escape sequences, and control codes from programs) and provides a *presentation* (rendering characters in a grid, handling keyboard input). Every terminal emulator -- xterm, alacritty, iTerm2 -- fuses these together.

`wsh` cleaves them apart.

`wsh` sits between your terminal emulator and your shell. It captures all I/O, maintains a complete terminal state machine, and exposes everything through a structured API. Your terminal works exactly as before. But now agents, automation, web clients, and any other consumer can tap into that same session -- seeing what you see, typing alongside you, reacting to output in real time.

```
┌─────────────────────────────────────────────────────────────────┐
│                         wsh                                      │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────────────┐  │
│  │ PTY Manager │───▶│  Terminal   │───▶│    API Server       │  │
│  │             │    │  State      │    │  (HTTP/WebSocket)   │  │
│  │ spawns      │    │  Machine    │    │                     │  │
│  │ $SHELL      │    │             │    │  • Stream output    │  │
│  │             │◀───│  • Parser   │    │  • Accept input     │  │
│  └─────────────┘    │  • Buffers  │    │  • Query state      │  │
│        │            │  • Cursor   │    │  • Subscribe events │  │
│        │            └─────────────┘    └─────────────────────┘  │
│        ▼                                         │              │
│  ┌─────────────┐                                 │              │
│  │ Local TTY   │ (passthrough to your terminal)  │              │
│  └─────────────┘                                 │              │
└──────────────────────────────────────────────────│──────────────┘
                                                   │
                    ┌──────────────────────────────┼───────────────┐
                    │                              │               │
                    ▼                              ▼               ▼
             ┌────────────┐                 ┌────────────┐  ┌────────────┐
             │ AI Agents  │                 │  Web UI    │  │  Other     │
             │ (skills)   │                 │  (mobile)  │  │  Tools     │
             └────────────┘                 └────────────┘  └────────────┘
```

**What the API Exposes**

- **Screen state**: Current screen contents as structured data -- text, colors, cursor position, alternate screen mode
- **Scrollback buffer**: Complete output history, paginated and queryable
- **Input injection**: Send keystrokes, paste text, inject control sequences -- indistinguishable from human input
- **Real-time events**: Subscribe to output changes, cursor movement, mode transitions via WebSocket
- **Idle detection**: Wait for the terminal to go idle -- essential for the send-wait-read pattern agents use
- **Visual feedback**: Overlays and panels that agents can render directly in the user's terminal
- **Input capture**: Temporarily intercept keyboard input for agent-driven dialogs and approvals
- **Session management**: Create, list, attach to, and destroy named sessions -- enabling parallel operation

The API is the product. Everything else is a client.

---

## AI as Co-Processor

**The Agent Loop**

With `wsh`, an AI agent interacts with a terminal the same way a human does: send input, wait for output, read the screen, decide what to do next.

```
Send input  →  Wait for idle  →  Read screen  →  Decide  →  repeat
```

This loop is simple but powerful. It works for any program, any interface, any situation. The agent doesn't need to understand the program's internals or speak its protocol. It reads what's on screen and types what's needed -- exactly like a human.

**What This Enables**

*Driving interactive tools:* An agent can run an installer, answer its prompts, handle errors, and verify the result. It can operate `vim`, navigate `lazygit`, step through a debugger, or interact with a REPL. Any program that a human can use through a terminal, an agent can use through `wsh`.

*Orchestrating AI coding tools:* An agent can launch Claude Code (or any terminal-based AI tool) in a `wsh` session, feed it tasks, monitor its progress, approve or reject its actions, and collect results. It can run multiple instances in parallel across separate sessions, coordinating a fleet of AI workers.

*Providing live assistance:* An agent can watch a human's terminal session in real time, understand what they're doing, and proactively offer help -- rendering suggestions as overlays directly in the terminal, or intercepting input to offer contextual menus.

*Auditing and monitoring:* An agent can observe all terminal activity across sessions, flagging security concerns, logging commands for compliance, or alerting on anomalous behavior.

*End-to-end automation:* An agent can set up an entire development environment: install dependencies, configure services, run tests, troubleshoot failures, and produce a step-by-step guide of everything it did -- all through interactive terminal sessions, handling every prompt and error along the way.

**Multiple Sessions, Parallel Work**

`wsh` always uses a client/server architecture. Running `wsh` with no subcommand auto-spawns an ephemeral server if needed, creates a session, and attaches -- giving you immediate, transparent access. Running `wsh server` starts the daemon explicitly for persistent, multi-session operation.

An agent can spin up a dozen sessions, run different tasks in each, monitor progress across all of them, and report results -- while the human continues working in their own terminal undisturbed.

---

## Skills: Teaching AI What It Can Do

`wsh` provides the infrastructure -- the API, the sessions, the state machine. But AI agents also need to know *how* to use that infrastructure effectively. This is where **skills** come in.

Skills are structured knowledge documents that teach AI agents the patterns, techniques, and strategies for using `wsh` to accomplish specific tasks. They're not code libraries -- they're expertise, packaged for AI consumption.

### Why Skills Matter

An AI agent with raw API access can technically do anything. But without guidance, it will fumble. It won't know the send-wait-read loop. It won't know to wait for idle before reading the screen. It won't know how to detect that a TUI has finished loading, or how to navigate a menu, or how to safely operate a destructive command.

Skills encode this operational knowledge:

| Skill | What It Teaches |
|-------|-----------------|
| **Core** | The API primitives and the fundamental send/wait/read/decide loop |
| **Drive Process** | Running CLI commands, handling prompts, detecting errors and exit codes |
| **TUI** | Operating full-screen applications (vim, htop, lazygit, k9s) |
| **Multi-Session** | Creating and managing parallel sessions for concurrent work |
| **Agent Orchestration** | Driving other AI agents through their terminal interfaces |
| **Monitor** | Watching human terminal activity and reacting to events |
| **Visual Feedback** | Using overlays and panels to communicate with users |
| **Input Capture** | Intercepting keyboard input for dialogs, approvals, and menus |
| **Generative UI** | Building dynamic, interactive terminal experiences |

### Skills as a Platform

Anyone can author skills for `wsh`. The skill format is AI-native: plain text documents that describe *what* to do, not *how* to call an API. This means skills are portable across protocols (HTTP, MCP, etc.) and can be consumed by any AI agent that can read.

Examples of skills that could be built:

- **Claude Code Orchestrator**: Drive multiple Claude Code instances to work on different parts of a codebase in parallel
- **Environment Setup**: Configure a new machine with a full development stack, interactively handling every installer and configuration prompt
- **Security Auditor**: Monitor terminal sessions for credential exposure, dangerous commands, or policy violations
- **Contextual Helper**: Watch what the user is doing and overlay relevant documentation, suggestions, or warnings
- **Local AI Setup**: Install and configure local AI tools (image generation, embeddings, etc.), test everything end-to-end, and produce a user guide

Skills turn `wsh` from an API into a *capability platform*. The API gives agents hands. Skills give them expertise.

---

## Terminal Emulation

**The State Machine**

`wsh` maintains a complete terminal state machine. Every byte from the PTY is parsed and interpreted: ANSI escape sequences, control characters, UTF-8 text, OSC codes, mouse events. The result is a structured representation of the terminal's state at any moment:

- **Scrollback buffer**: All output history, stored as styled text spans
- **Alternate screen buffer**: The fixed-grid screen used by full-screen TUIs (vim, htop, etc.)
- **Cursor state**: Position, visibility, style
- **Text attributes**: Current foreground/background colors, bold, italic, underline, etc.
- **Mode flags**: Alternate screen active, bracketed paste mode, mouse reporting mode, etc.

This state machine is the single source of truth. API clients don't interpret raw terminal output -- they receive structured data and use it however they need.

**Supported Features**

- ANSI SGR (colors: 16, 256, true color; bold, italic, underline, inverse, strikethrough)
- Cursor positioning and movement
- Line editing (insert, delete, clear)
- Screen manipulation (clear, scroll regions)
- Alternate screen buffer
- Mouse reporting (for tmux, vim, etc.)
- Bracketed paste mode
- Window title (OSC 0/2)
- Clipboard integration (OSC 52) -- passthrough to local terminal

---

## Bidirectional Synchronization

**Real-Time, Multi-Source Input**

`wsh` accepts input from multiple sources simultaneously:

- The local terminal (your keyboard)
- The API (AI agents, automation scripts)
- Future clients (web UI, voice interfaces)

All input sources are equal. Keystrokes arrive at the PTY in the order they're received, regardless of origin. There's no concept of "primary" or "secondary" -- every connected client can type, and every keystroke is immediately visible to all other clients.

**Real-Time Output Distribution**

Output flows the opposite direction with the same philosophy:

1. Program writes to PTY
2. `wsh` reads from PTY master
3. `wsh` updates internal terminal state
4. `wsh` broadcasts to all connected clients:
   - Raw bytes forwarded to local terminal
   - Structured state updates pushed via WebSocket to API clients
   - Events emitted to subscribers

Latency is minimal. An agent's keystroke appears in your terminal instantly. Output from a command reaches the agent as fast as the local socket allows.

**Connection Resilience**

API clients are stateless views into `wsh`'s state. If a client disconnects and reconnects, it simply re-fetches the current terminal state and scrollback -- no session corruption, no desync. The PTY session is owned by `wsh`, not by any individual client. Clients come and go; the session persists.

---

## Security Model

**The Stakes**

`wsh` exposes terminal access via an API. A compromised `wsh` instance means arbitrary command execution on your machine. Security is existential.

**Defense in Depth**

**Layer 1: Localhost by Default**

By default, `wsh` binds to `127.0.0.1` only. The API is accessible only from the local machine. To expose it remotely, you must explicitly bind to another address.

```bash
wsh                          # localhost:8080, no remote access
wsh --bind 0.0.0.0:8080      # all interfaces, requires auth
```

**Layer 2: Token Authentication**

When binding to any non-localhost address, `wsh` requires a bearer token for all API and WebSocket connections. On startup, `wsh` either generates a random token or accepts one via `--token` or environment variable.

**Layer 3: Your Network, Your Responsibility**

`wsh` provides authentication, not encryption. For remote access, compose it with your existing security stack: SSH tunneling, Tailscale/WireGuard, or a reverse proxy with TLS.

---

## Web UI

`wsh` ships with a web-based terminal client as one demonstration of the API's power. It connects over WebSocket and renders terminal state using web-native technologies -- reflowing HTML in normal mode, fixed character grid for full-screen TUIs.

The web UI features a sidebar with live mini-previews of all sessions organized by tag, three view modes (carousel with 3D depth, auto-grid tiling, and idle-driven queue), a command palette, keyboard shortcuts, drag-and-drop tag management, and six themes. It adapts to screen size: bottom sheet on phones, overlay sidebar on tablets, persistent sidebar on desktop. Touch gestures, native scrolling, and a modifier bar for special keys make it a production-quality interface for accessing your terminal from any device.

But architecturally, it's just another API client -- no different from an AI agent or an automation script. It demonstrates what `wsh` makes possible; it is not the point.

---

## Why Not Use...

Every existing terminal-sharing tool asks the same question: *"How do I put a terminal in a browser?"*

`wsh` asks a different question: *"How do I expose terminal I/O to anything that wants to consume or produce it?"*

This isn't a subtle distinction. Existing tools are *viewers*. `wsh` is a *platform*. The API is the product.

| Tool | Programmatic API | Agent-Ready | Local Sync | Multi-Session | Web UI |
|------|------------------|-------------|------------|---------------|--------|
| ttyd/gotty | No | No | No | No | Yes |
| Domterm | No | No | No | No | Yes |
| tmate/upterm | No | No | No | No | Yes |
| Mosh/et | No | No | No | No | No |
| tmux -CC | Partial | No | Partial | Yes | No |
| code-server | No | No | No | No | Yes |
| **wsh** | **Yes** | **Yes** | **Yes** | **Yes** | **Yes** |

The critical columns are the first two. No existing tool provides a structured, programmable API for terminal I/O that AI agents can use. `wsh` is the missing infrastructure.

---

## Roadmap

**Now: The API Platform**

The core is built: PTY management, terminal state machine, HTTP/WebSocket API, session management, overlays, panels, input capture, idle detection. MCP integration exposes `wsh` as a first-class MCP server -- 14 tools, 3 resources, 9 prompts -- via Streamable HTTP and stdio transports. AI agents can drive interactive terminal sessions today through HTTP, WebSocket, or MCP.

**Next: Richer Agent Capabilities**

- **Structured events**: Semantic notifications -- "command completed," "prompt detected," "approval requested" -- layered on top of raw terminal output
- **Agent middleware**: Allow agents to observe, filter, transform, and respond to terminal activity through composable hooks

**Future: New Modalities**

- **Voice integration**: Speech-to-text input and text-to-speech output summaries for hands-free terminal interaction
- **Web UI enhancements**: Plugin architecture for context-aware UI extensions
- **Distributed operation**: Manage `wsh` sessions across multiple machines from a single control plane

---

## Summary

**What `wsh` Is**

`wsh` is an API for your terminal. It sits transparently between your terminal emulator and your shell, capturing all I/O, maintaining structured state, and exposing everything through HTTP and WebSocket. Your terminal works exactly as before. But now AI agents can see what you see, type what's needed, and interact with programs the way a human does.

The terminal is the fundamental UI of the modern computer. `wsh` makes it programmable -- turning AI into a co-processor for both the machine and the human operating it.

**What `wsh` Is Not**

- Not a new terminal emulator (use alacritty, kitty, whatever you love)
- Not a tmux replacement (use tmux inside `wsh` if you want)
- Not a remote desktop solution (it's terminal-specific by design)
- Not just a web terminal (the web UI is one client among many)

**The Vision**

Today: AI agents drive interactive terminal sessions -- running builds, operating TUIs, orchestrating other AI tools, providing live help -- through a structured API.

Tomorrow: `wsh` becomes the default interface between AI and computers. Every shell session is a `wsh` session. Agents and humans share the terminal as co-processors, each contributing what they do best.

The terminal protocol has survived for fifty years because it's simple, universal, and composable. `wsh` doesn't replace it. `wsh` opens it up to AI.
