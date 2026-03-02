# wsh: Playwright for Terminals

Programmatic observation and control of terminal sessions. Give AI agents — or any program — the ability to see what's on screen, type what's needed, and interact with the terminal the way a human does.

```
Send input  →  Wait for idle  →  Read screen  →  Decide  →  repeat
```

wsh sits between your terminal emulator and your shell. It maintains a full terminal state machine and exposes everything — screen contents, scrollback, cursor state, input injection, idle detection — via HTTP, WebSocket, and MCP.

![wsh demo](demo/demo.gif)

## Install

```bash
curl -fsSL https://wsh.dev/install.sh | sh
```

Or with Cargo:

```bash
cargo install wsh
```

## For AI Agents

### MCP (zero configuration)

Install as a Claude Code plugin and the skills teach the agent what to do:

```bash
claude /plugin install https://github.com/deepgram/wsh
```

Or add to any MCP host:

```bash
wsh mcp
```

wsh ships with 11 skills — structured knowledge documents that teach agents the send/wait/read loop, how to drive TUIs, manage parallel sessions, orchestrate other agents, and more. The agent doesn't just get an API — it gets the expertise to use it.

### HTTP API

The same loop, explicit:

```bash
# Send a command
curl -X POST http://localhost:8080/sessions/default/input -d 'ls\n'

# Wait for idle
curl -s 'http://localhost:8080/sessions/default/idle?timeout_ms=500&max_wait_ms=10000'

# Read the screen
curl -s http://localhost:8080/sessions/default/screen | jq .
```

Full API reference: [docs/api/](docs/api/README.md)

## For Humans

```bash
# Start a session (auto-spawns server daemon if needed)
wsh

# List sessions
wsh list

# Start a named session with tags
wsh --name dev --tag build --tag frontend

# Attach to a session from another terminal
wsh attach dev

# Detach: Ctrl+\ Ctrl+\
```

Open `http://localhost:8080` in a browser and your terminal is there — live, interactive, fully synced. Type in either place. Pull it up on your phone.

Full CLI reference: [docs/cli.md](docs/cli.md)

## What This Enables

- **Drive interactive tools**: Agents operate installers, debuggers, REPLs, TUIs, AI coding assistants — anything that expects a human at the keyboard
- **Orchestrate AI in parallel**: Run multiple Claude Code instances across separate sessions, coordinating a fleet of AI workers
- **Manage infrastructure**: Federate wsh across machines and let agents handle deployment, configuration, and operations interactively — an agentic alternative to Ansible, Puppet, and Chef
- **Provide live assistance**: Watch a terminal session and render contextual help as overlays directly in the workflow
- **Audit and monitor**: Observe terminal activity for security, compliance, or operational awareness
- **Automate end-to-end**: Set up entire environments, handling every interactive prompt and error along the way

## Skills

Skills are what make wsh more than an API. They're structured knowledge documents that teach AI agents *how* to operate terminals effectively — not just the API calls, but the patterns, error handling, and strategies.

| Skill | What It Teaches |
|-------|-----------------|
| `core` | API mechanics and the send/wait/read/decide loop |
| `drive-process` | Running commands, handling prompts, detecting errors |
| `tui` | Operating full-screen apps (vim, htop, lazygit) |
| `multi-session` | Parallel session management |
| `agent-orchestration` | Driving other AI agents through their terminals |
| `monitor` | Watching and reacting to terminal activity |
| `visual-feedback` | Overlays and panels for in-terminal communication |
| `input-capture` | Intercepting keyboard input for dialogs and approvals |
| `generative-ui` | Dynamic, interactive terminal experiences |
| `cluster-orchestration` | Sessions across federated servers |
| `infrastructure-ops` | Fleet deployment, configuration, and operations |

## Documentation

| Doc | Description |
|-----|-------------|
| [API Reference](docs/api/README.md) | HTTP and WebSocket endpoint reference |
| [CLI Reference](docs/cli.md) | All commands, flags, and environment variables |
| [Architecture](docs/architecture.md) | System design, project structure, and internals |
| [Federation](docs/federation.md) | Multi-server cluster setup and management |
| [Vision](docs/VISION.md) | Project vision and long-term direction |

## Building

Requires a Rust toolchain:

```bash
cargo build --release
```

## License

TBD
