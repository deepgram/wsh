# wsh: The Web Shell

> The terminal as a service. Your shell, exposed to the modern world.

---

## The Problem

**The Terminal Paradox**

The terminal is experiencing a renaissance. Tools like Claude Code, modern CLI applications, and sophisticated TUIs have made the command line more powerful than ever. Yet this power comes with a constraint: you must be *present*. Physically seated at a keyboard, watching output scroll by, ready to respond.

This constraint chafes against modern reality. We carry powerful computers in our pockets, connected everywhere. But when you need to approve a command, answer a question, or review a diff from your AI coding assistant, your phone becomes useless. The options are grim:

1. **Mobile SSH clients**: Cramped terminal emulators fighting with touch keyboards. Escape sequences become finger gymnastics. The experience is hostile.

2. **Web-based terminals**: Marginally better, but fundamentally the same problem. They faithfully recreate the 1980s fixed-grid terminal in your browser - scroll hijacking, tiny fonts or horizontal panning, IME conflicts with autocorrect.

3. **Just wait**: Accept that terminal work requires a "real" computer. Lose momentum. Context-switch. Forget what you were doing.

The irony is thick: we have web browsers capable of rendering rich, responsive, touch-friendly interfaces, yet we use them to simulate hardware terminals from four decades ago.

**The Deeper Problem**

But there's a more fundamental tension emerging. In an increasingly AI-enabled world, the ideal modality for on-the-move interaction is *voice*. Voice is inherently high-level - you speak intent, not keystrokes. Yet our most powerful tools remain locked behind terminals and keyboards, demanding low-level character-by-character input.

We need more than a better mobile terminal. We need a bridge that can evolve: from terminal to web, from web to voice, from direct manipulation to agent-mediated interaction. Imagine AI agents that can summarize terminal output in a sentence, convert your spoken response into the appropriate keystrokes, and liaise between you and your development environment while you're walking the dog.

The problem isn't the terminal *paradigm*. The problem is that we've conflated the terminal *interface* (a fixed character grid) with the terminal *protocol* (streams of text with control sequences). The protocol is fine. The interface needs reimagining - first for the web, then for voice, then for agents.

---

## The Insight

**Separate Protocol from Presentation**

A terminal emulator does two things: it interprets a *protocol* (the stream of bytes, ANSI escape sequences, and control codes coming from programs) and it provides a *presentation* (rendering characters in a fixed grid, handling keyboard input). Every terminal emulator - xterm, alacritty, iTerm2 - fuses these together.

`wsh` cleaves them apart.

The insight is this: the terminal protocol is rich and well-specified. Programs output styled text, move cursors, switch screen buffers, report mouse events. None of this *requires* a fixed character grid. A sufficiently smart interpreter can translate this protocol into whatever presentation makes sense for the context:

- On a desktop terminal: traditional fixed-grid rendering (let alacritty do what it does best)
- On a mobile browser: reflowing HTML with native scrolling and touch-friendly text selection
- Running a full-screen TUI: switch to grid mode temporarily, then back to flowing text
- Via an API: consumed by agents, security auditors, automation tools, or anything else

The terminal becomes a *universal protocol* for program interaction, with presentations adapted to context.

**The Multiplexer Model**

`wsh` sits in the middle. It spawns your shell inside a PTY it controls, captures all I/O, maintains the full terminal state, and exposes it to multiple consumers simultaneously:

- Your local terminal emulator sees the same output as always
- A web browser sees a web-native rendering of that output
- An AI agent receives structured terminal state via API
- A voice interface gets summaries and sends transcribed commands
- Security tools monitor for anomalies in real-time
- Unix pipes and scripts integrate via standard I/O

The web UI is the obvious first frontend - visual, immediate, useful today. But `wsh` is architected as a terminal *service*. Expose terminal I/O via WebSocket, via MCP-style RPC, via REST, via Unix socket - and suddenly the terminal is accessible to the entire modern tooling ecosystem. Ancient Unix philosophy meets contemporary infrastructure.

You start `wsh` in alacritty on your workstation. You pull up the web UI on your phone. An agent watches the session, ready to help. Same session. Same state. Multiple presentations, each native to its context.

---

## Architecture Overview

**Core Principle: The Terminal as a Service**

At its heart, `wsh` is a PTY multiplexer with an API. Everything else - the web UI, future voice interfaces, agent integrations - are clients of that API.

```
┌─────────────────────────────────────────────────────────────────┐
│                         wsh daemon                              │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────────────┐  │
│  │ PTY Manager │───▶│  Terminal   │───▶│    API Server       │  │
│  │             │    │  State      │    │  (WebSocket/HTTP)   │  │
│  │ spawns      │    │  Machine    │    │                     │  │
│  │ $SHELL      │    │             │    │  • Stream output    │  │
│  │             │◀───│  • Parser   │    │  • Accept input     │  │
│  └─────────────┘    │  • Buffers  │    │  • Query state      │  │
│        │            │  • Cursor   │    │  • Subscribe events │  │
│        │            └─────────────┘    └─────────────────────┘  │
│        ▼                                         │              │
│  ┌─────────────┐                                 │              │
│  │ Local TTY   │ (passthrough to alacritty)      │              │
│  └─────────────┘                                 │              │
└──────────────────────────────────────────────────│──────────────┘
                                                   │
                    ┌──────────────────────────────┼───────────────┐
                    │                              │               │
                    ▼                              ▼               ▼
             ┌────────────┐                 ┌────────────┐  ┌────────────┐
             │  Web UI    │                 │   Agent    │  │  Other     │
             │  (bundled) │                 │   (MCP)    │  │  Tools     │
             └────────────┘                 └────────────┘  └────────────┘
```

**The Nested PTY Model**

You run `wsh` inside your existing terminal (alacritty, kitty, whatever you prefer). `wsh` allocates a new PTY pair, spawns your shell on the slave side, and sits on the master side. To alacritty, `wsh` is just a program producing output. To your shell, it's running in a normal terminal. `wsh` is invisible - a transparent proxy that happens to expose everything over an API.

**What the API Exposes**

- **Output stream**: Real-time terminal output, either raw bytes or parsed/structured
- **Input injection**: Send keystrokes, paste text, inject control sequences
- **State queries**: Current screen contents, cursor position, scrollback buffer, alternate screen status
- **Events**: Subscribe to state changes, screen updates, mode transitions

**The Bundled Web UI**

`wsh` ships with a web-based terminal client as a first-class feature. It connects to the API over WebSocket and renders terminal state using web-native technologies. This isn't a demo - it's a production-quality interface for mobile/remote access. But architecturally, it's just another API client.

---

## Terminal Emulation & Rendering

**The State Machine**

`wsh` maintains a complete terminal state machine. Every byte from the PTY is parsed and interpreted: ANSI escape sequences, control characters, UTF-8 text, OSC codes, mouse events. The result is a structured representation of the terminal's state at any moment:

- **Scrollback buffer**: All output history in normal mode, stored as styled text spans
- **Alternate screen buffer**: The fixed-grid screen used by full-screen TUIs (vim, htop, etc.)
- **Cursor state**: Position, visibility, style
- **Text attributes**: Current foreground/background colors, bold, italic, underline, etc.
- **Mode flags**: Alternate screen active, bracketed paste mode, mouse reporting mode, etc.

This state machine lives in the Rust backend. It's the single source of truth. Frontends don't interpret raw terminal output - they receive structured state and render it appropriately.

**Two Rendering Modes**

The web UI operates in two distinct modes, switching automatically based on terminal state:

**Normal Mode** (default):
- Renders scrollback as styled HTML elements (`<div>`, `<span>` with CSS)
- Text reflows naturally to fit viewport width
- Native browser scrolling - no scroll hijacking
- Touch-friendly text selection using native browser capabilities
- Carriage returns, backspaces, and line-local cursor movement update the current line in place

**Alternate Screen Mode** (for TUIs):
- Activated when the terminal enters alternate screen (`\e[?1049h`)
- Renders as a fixed character grid sized to the session dimensions
- Full cursor positioning and screen manipulation
- When the program exits alternate screen, this view is discarded entirely
- Scrollback resumes exactly where it left off - no TUI garbage in history

The transition is seamless. Run `vim`, and the view switches to grid mode. Exit `vim`, and you're back to reflowing HTML with your full scrollback intact.

**Supported Terminal Features**

- ANSI SGR (colors: 16, 256, true color; bold, italic, underline, inverse, strikethrough)
- Cursor positioning and movement
- Line editing (insert, delete, clear)
- Screen manipulation (clear, scroll regions)
- Alternate screen buffer
- Mouse reporting (for tmux, vim, etc.)
- Bracketed paste mode
- Window title (OSC 0/2)
- Clipboard integration (OSC 52) - passthrough to local terminal

---

## Bidirectional Synchronization

**Real-Time, Multi-Source Input**

`wsh` accepts input from multiple sources simultaneously:

- The local terminal (your keyboard in alacritty)
- The web UI (your phone's on-screen keyboard)
- The API (agents, scripts, automation tools)

All input sources are equal. Keystrokes arrive at the PTY in the order they're received, regardless of origin. There's no concept of "primary" or "secondary" - every connected client can type, and every keystroke is immediately visible to all other clients.

**No Conflict Resolution**

What happens if you type on your phone while someone (or something) types at the local terminal? The keystrokes interleave. This is a deliberate non-design: in practice, you're only actively typing from one place at a time. The rare case of simultaneous input doesn't warrant complex locking, cursor ownership, or last-writer-wins semantics. The terminal has always been a single-cursor, single-input-stream interface. `wsh` keeps it that way - it just allows that input stream to originate from anywhere.

**Real-Time Output Distribution**

Output flows the opposite direction with the same philosophy:

1. Program writes to PTY
2. `wsh` reads from PTY master
3. `wsh` updates internal terminal state
4. `wsh` broadcasts to all connected clients:
   - Raw bytes forwarded to local terminal (alacritty)
   - Structured state updates pushed via WebSocket to web clients
   - Events emitted to API subscribers

Latency is minimal. A keystroke on your phone appears in alacritty instantly. Output from a command appears on your phone as fast as your network allows.

**Connection Resilience**

The web UI is a stateless view into `wsh`'s state. If your phone loses connectivity and reconnects, it simply re-fetches the current terminal state and scrollback - no session corruption, no desync. The PTY session is owned by `wsh`, not by any individual client. Clients come and go; the session persists.

---

## Security Model

**The Stakes**

`wsh` exposes terminal access over a network. A compromised `wsh` instance means arbitrary command execution on your machine. Security isn't optional - it's existential.

**Defense in Depth**

The security model has multiple layers:

**Layer 1: Bind Address Defaults**

By default, `wsh` binds to `127.0.0.1` (localhost only). The API and web UI are accessible only from the local machine. To access remotely, you must explicitly bind to another address:

```bash
wsh                          # localhost:8080, no remote access
wsh --bind 0.0.0.0:8080      # all interfaces, requires auth
wsh --bind 192.168.1.50:9000 # specific interface, requires auth
```

If you're binding to localhost, you presumably already have local access to the machine. No additional authentication is required - it would be security theater.

**Layer 2: Token Authentication**

When binding to any non-localhost address, `wsh` requires a bearer token for all API and WebSocket connections. On startup, `wsh` either:

- Generates a random token and prints it to the terminal
- Accepts a user-provided token via `--token` or environment variable

The web UI prompts for this token before connecting. API clients must include it in their requests. No token, no access.

**Layer 3: Your Network, Your Responsibility**

`wsh` provides authentication, not encryption. For remote access over untrusted networks, you should use:

- **SSH tunneling**: `ssh -L 8080:localhost:8080 yourserver`, then connect to localhost
- **Tailscale/WireGuard**: Access your machine over an encrypted mesh VPN
- **Reverse proxy with TLS**: Put `wsh` behind nginx/caddy with HTTPS

This is intentional. `wsh` doesn't bundle a TLS implementation or certificate management - that's infrastructure you likely already have. Keep the tool simple; compose it with your existing security stack.

---

## Web User Interface

**Design Philosophy**

The web UI is not a terminal emulator trapped in a browser. It's a native web application that happens to display terminal content. Every design decision asks: "What would a web-first interface do here?"

**Layout**

The interface is minimal:

- **Main content area**: Terminal output rendered as styled HTML (normal mode) or a character grid (alternate screen mode)
- **Input area**: A text field at the bottom for composing input, with the system keyboard
- **Modifier bar**: A compact row of buttons for special keys (Esc, Tab, Ctrl, Alt, arrow keys)
- **Overflow menu**: Expand button for less common keys and modifiers

No chrome. No sidebars. No tabs (in standalone mode). The terminal content dominates the viewport.

**Mobile-First Interactions**

- **Native scrolling**: Swipe to scroll through history. No scroll hijacking. The browser's scrollbar works normally.
- **Native text selection**: Long-press to select text. Drag handles work. Copy via system menu.
- **System keyboard**: Tap the input area to bring up your phone's keyboard. Autocorrect and predictive text work normally. Text is sent on Enter (or character-by-character for interactive programs - configurable).
- **Responsive sizing**: The UI adapts to viewport size. On a phone, you see fewer columns but readable text. No horizontal scrolling in normal mode.

**The Modifier Bar**

Mobile keyboards lack Ctrl, Alt, Esc, and function keys. A persistent toolbar provides these:

```
[ Esc ] [ Tab ] [ Ctrl ] [ Alt ] [ ↑ ] [ ↓ ] [ ← ] [ → ] [ ⋯ ]
```

Tap `Ctrl`, then type `c` = sends Ctrl+C. The `⋯` button expands to reveal less common keys (function keys, Super, etc.). The bar is compact enough to remain visible without consuming excessive screen space.

**Desktop Experience**

On a desktop browser, the UI works identically but assumes a physical keyboard is available. The modifier bar remains accessible for touchscreen laptops or convenience, but keyboard shortcuts work natively.

---

## Future Roadmap

**The Path Forward**

`wsh` v1 delivers the core: a transparent PTY wrapper with an API, bundled with a production-quality web UI. But the architecture is designed to grow. Here's where it goes next.

**Phase 2: Server Mode**

The initial release runs in standalone mode - one `wsh` invocation, one session, one web UI. Server mode introduces a persistent daemon:

- A background service manages multiple `wsh` sessions
- New `wsh` invocations register with the daemon instead of running independently
- The web UI presents a session list - switch between active sessions with a tap
- Sessions persist even when the originating terminal disconnects

This transforms `wsh` from a tool you run into infrastructure you rely on.

**Phase 3: Voice Integration**

Mobile interaction shouldn't require typing. Voice integration adds:

- **Speech-to-text input**: Speak a command, have it transcribed and sent to the terminal
- **Text-to-speech output**: Terminal output summarized and spoken aloud
- Native integration with platform voice services (Web Speech API, system dictation)

Voice is inherently high-level. Saying "approve" is easier than finding the `y` key on a phone keyboard.

**Phase 4: Agent Hooks**

The API already exposes terminal I/O to external consumers. Agent hooks formalize this:

- **MCP-style interface**: Expose terminal state and input capabilities via Model Context Protocol or similar
- **Structured events**: Semantic notifications like "command completed," "prompt detected," "approval requested"
- **Agent middleware**: Allow AI agents to observe, summarize, filter, and respond to terminal activity

Imagine an agent that watches your Claude Code session, summarizes what it's doing, and asks you via voice notification whether to approve a file edit - while you're away from your desk.

**Phase 5: Gesture Input**

For power users on mobile, gestures provide faster access to common operations:

- Swipe patterns for Esc, Ctrl+C, Ctrl+D
- Long-press modifiers
- Customizable gesture mappings

Gestures complement rather than replace the modifier bar - an optimization for those who want it.

**Phase 6: Plugin Architecture**

Context-aware enhancements without tight coupling:

- Plugins can detect specific programs (tmux, Claude Code) and offer tailored UI
- Quick-action buttons that appear only when relevant
- Custom rendering for recognized output patterns

This is deliberately last. The core must be solid and context-agnostic before we layer intelligence on top.

---

## Why Not Use...

**The Landscape**

Terminal-over-web isn't a new idea. Several tools exist in this space. But they all share a common assumption: the goal is to *view and interact with* a terminal remotely. `wsh` has a different premise: the goal is to *expose terminal I/O as a service* that arbitrary consumers - humans, web UIs, agents, security tools, automation - can hook into.

### The Fundamental Difference

Every existing tool asks: "How do I put a terminal in a browser?"

`wsh` asks: "How do I expose terminal input and output to anything that wants to consume or produce it?"

This isn't a subtle distinction. It's architectural. Existing tools are *viewers*. `wsh` is a *platform*. The web UI is one client. An AI agent watching your session is another. A security monitor scanning for credential leaks is another. An MCP server exposing your terminal to Claude is another. They all connect to the same API, receiving the same state, able to inject the same input.

No existing tool does this.

### Web Terminal Tools

**ttyd / gotty / wetty**

These are the most direct comparisons for the web UI specifically.

- *What they do*: Spawn a shell, attach xterm.js in the browser, bridge input/output over WebSocket
- *Why not*: They're exactly the "terminal emulator in a browser" problem. Fixed grid, scroll hijacking, hostile to mobile. No bidirectional sync with a local terminal - you use the web UI *instead of* your terminal, not *alongside* it. And critically: **no API**. You can't hook an agent into ttyd. You can't query terminal state programmatically. It's a viewer, not a service.

**Domterm**

A terminal emulator that renders using HTML/CSS instead of a character grid.

- *What they do*: Replace your terminal emulator entirely with a web-technology-based one
- *Why not*: You must replace alacritty/kitty/your terminal. No bidirectional sync - it *is* the terminal. And again: **no API exposure**. Domterm solves the rendering problem but doesn't expose terminal I/O as a service for agents or tooling.

**shellinabox**

An older web terminal with HTML rendering.

- *What they do*: Pre-xterm.js web terminal
- *Why not*: Dated, unmaintained, no mobile optimization, no local terminal sync, **no API**. A historical curiosity.

### Terminal Sharing / Collaboration

**tmate / teleconsole / upterm**

These focus on sharing your terminal with other humans.

- *What they do*: Create tunneled connections so others can view/control your terminal
- *Why not*: Different use case. They're about *sharing*, not *exposing as a service*. Traditional terminal rendering. **No programmatic API** - a human must be on the other end, not an agent.

### Persistent Connections

**Mosh / Eternal Terminal**

These solve connection reliability over unreliable networks.

- *What they do*: Better-than-SSH protocols for roaming and intermittent connectivity
- *Why not*: Great technology, wrong problem. Still a traditional terminal experience with a human at a keyboard. **No API**. You can't hook an AI agent into Mosh.

### Tmux Integration

**tmux control mode (-CC)**

Tmux can expose its state in a machine-readable format.

- *What they do*: Structured protocol for querying and controlling tmux
- *Why not*: Tmux-specific - doesn't work for non-tmux sessions. Requires a custom client that speaks the protocol. No web UI. But more importantly: it's designed for IDE integration (iTerm2), not for arbitrary consumers. You couldn't easily hook an MCP server or AI agent into tmux control mode without significant custom work.

### Low-Level Building Blocks

**node-pty / portable-pty / websocketd / socat**

Libraries and tools for PTY management and stream bridging.

- *What they do*: Provide primitives - allocate PTYs, bridge streams to WebSockets
- *Why not*: They're building blocks. You'd use these *to build* `wsh`. websocketd can pipe a PTY to a WebSocket, but you get raw bytes - no state management, no escape sequence parsing, no structured API, no ability to query "what's on screen right now?" or "is the terminal in alternate screen mode?" The intelligence is missing.

### IDEs with Terminals

**code-server / JupyterLab / Theia**

Full development environments with embedded terminals.

- *What they do*: Bring VS Code or Jupyter to the web, with xterm.js terminals included
- *Why not*: The terminal is a feature, not the focus. Same xterm.js problems. And **no external API** - you can't hook an agent into the VS Code terminal from outside VS Code.

### Agent / AI Tooling

**Nothing.**

This is the gap. Where is the tool that lets you:

- Start a terminal session
- Expose a structured API over WebSocket/HTTP
- Query: "What is the current screen content?"
- Query: "What's in the scrollback buffer?"
- Subscribe: "Notify me when output contains 'error'"
- Inject: "Send these keystrokes"
- All while your local terminal continues to work normally?

This tool doesn't exist. If you want to connect Claude to your terminal via MCP, you're writing custom code. If you want a security agent monitoring your session for credential leaks, you're writing custom code. If you want a voice interface that can read terminal output and send spoken commands, you're writing custom code.

`wsh` is the missing infrastructure. The API is the product. The web UI is a proof of capability.

### Summary

| Tool | Web UI | Mobile-Native | Local Sync | Programmatic API | Agent-Ready |
|------|--------|---------------|------------|------------------|-------------|
| ttyd/gotty | Yes | No | No | No | No |
| Domterm | Yes | No | No | No | No |
| tmate/upterm | Yes | No | No | No | No |
| Mosh/et | No | No | No | No | No |
| tmux -CC | No | No | Partial | Partial | No |
| code-server | Yes | No | No | No | No |
| **wsh** | Yes | Yes | Yes | Yes | Yes |

If a tool that does this already exists, we haven't found it. If you have, please tell us before we write any more code.

---

## Summary

**What `wsh` Is**

`wsh` is a transparent PTY wrapper that exposes terminal I/O via an API. It sits invisibly between your terminal emulator and your shell, capturing everything, exposing everything, while changing nothing about your native terminal experience.

The bundled web UI demonstrates the power of this architecture: a mobile-friendly, web-native interface to your terminal that stays perfectly synchronized with your local session. But the web UI is just one client. The API enables agents, voice interfaces, security tools, automation scripts, and applications we haven't imagined yet.

**What `wsh` Is Not**

- Not a new terminal emulator (use alacritty, kitty, whatever you love)
- Not a tmux replacement (use tmux inside `wsh` if you want)
- Not a remote desktop solution (it's terminal-specific by design)
- Not a security product (it provides authentication; you provide encryption)

**The Vision**

Today: run `wsh`, open the web UI on your phone, interact with Claude Code while away from your desk.

Tomorrow: speak to an agent that watches your terminal, summarizes activity, and executes your intent - the terminal as a voice-controlled service.

The terminal protocol has survived for fifty years because it's simple, universal, and composable. `wsh` doesn't replace it. `wsh` opens it up to the modern world.
