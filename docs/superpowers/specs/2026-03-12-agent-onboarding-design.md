# Agent Onboarding: Bridging Skills to API Calls

**Date:** 2026-03-12
**Status:** Draft

## Problem

A Claude Code agent was asked to drive a wsh session. It loaded the
`wsh:drive-process` skill, which teaches CLI interaction patterns using
abstract operations: "send input", "wait for idle", "read screen". The
skill — by design — does not specify how to call the API. The agent had
no MCP tools (the plugin didn't configure them) and had not loaded the
core skill (which has every endpoint documented). It hallucinated
endpoints, got 404s on everything, and failed completely.

The root cause was not missing documentation. It was **hallucination by
vacuum**: the agent didn't know that it didn't know. Nothing told it
"you don't have this information yet — here's how to get it."

## Agent Use Cases

Three types of agents interact with wsh:

**1. Claude Code plugin agent.** Discovers wsh via the `.claude-plugin/`
directory. Gets on-disk skills injected into context (static, no runtime
modification). If the plugin configures an MCP server and it starts
successfully, the agent also gets MCP tools. Falls back to Bash + curl
when MCP is unavailable.

**2. MCP client.** Connects to wsh's MCP server (via `wsh mcp` stdio
bridge or directly to `/mcp` on a running HTTP server). Has 18 MCP tools
and can request skill prompts via `prompts/get`. Tool descriptions are
self-documenting.

**3. HTTP-only agent.** No MCP. Interacts through HTTP endpoints using
curl. Learns about wsh by reading skill files from disk or
documentation. Needs concrete curl commands for every operation.

## Architecture: Four Independent Layers

The solution is a layered design where each layer is independent —
no layer depends on another layer's content. Each layer peels back
to reveal the next. The agent never reaches a dead end.

```
Layer 1: Directive block (10 specialized skills)
         "Here's how to find the 'how'"
         Changes when: new delivery channel added (rare)

Layer 2: Core skills with bootstrap section (1 HTTP, 1 MCP)
         "Here's every endpoint / tool"
         Changes when: API surface changes

Layer 3: Plugin MCP wiring (.mcp.json)
         Ensures MCP tools are available (the 95% path)
         Changes when: server binary interface changes

Layer 4: Plugin structure (core as background context)
         Belt-and-suspenders for Claude Code fallback
         Changes when: Claude Code plugin spec changes
```

### Layer 1: Directive Block

Every specialized skill gets a ~5-line directive block at the top,
immediately after the frontmatter. This block makes the knowledge gap
explicit and gives the agent a concrete resolution path for every
scenario.

```markdown
> **IMPORTANT: EXECUTION CONTEXT**
> This skill describes *what to do* — domain patterns and decision-making.
> It does NOT describe *how* to call the API.
>
> 1. **If you have `wsh_*` tools** (check your toolkit for `wsh_send_input`,
>    `wsh_get_screen`, etc.): use them directly. Operation names in this
>    skill generally map to tool names (e.g., "send input" → `wsh_send_input`).
>    When in doubt, list your available `wsh_*` tools.
> 2. **If you do NOT have `wsh_*` tools**: you are in HTTP/curl fallback mode.
>    **DO NOT GUESS endpoints or CLI subcommands.**
>    Load the full API reference first: search your workspace for
>    `skills/core/` and read `SKILL.md`. It contains every endpoint
>    with working curl examples and a bootstrap sequence.
> 3. **Quick bootstrap**: `curl -sf http://localhost:8080/health`
>    — if that fails and you need a server:
>    `wsh server -L agent-$RANDOM &` then retry the health check.
```

Line-by-line reasoning:

- "It does NOT describe how to call the API" — Makes the knowledge gap
  explicit. The original agent didn't know it had a gap. This prevents
  hallucination.
- "check your toolkit for `wsh_send_input`" — Gives the agent a
  concrete thing to look for, not an abstract "check if you have tools."
- "Operation names ... generally map to tool names" — The rosetta stone.
  "send input" → `wsh_send_input`. The agent can infer the pattern from
  one example. The "when in doubt, list your tools" fallback handles
  cases where the mapping isn't obvious (e.g., "capture input" →
  `wsh_input_mode`, "create overlay" → `wsh_overlay`).
- "DO NOT GUESS endpoints or CLI subcommands" — The single most valuable
  line. Had this existed, the original incident would not have happened.
- "search your workspace for `skills/core/`" — Deliberately says
  "search" rather than giving a hardcoded path. `Glob("**/skills/core/SKILL.md")`
  will find it regardless of plugin installation location.
- Quick bootstrap — Two lines of concrete transport content. The health
  check is the probe; if it fails, the agent backgrounds a server and
  retries. `$RANDOM` suffix avoids server-name collisions. The `&` is
  essential — without it, `wsh server` blocks the shell.

**Files changed:** 10 specialized SKILL.md files.

**Changes when:** A new delivery channel is added (rare).

### Layer 2: Core Skills with Bootstrap Section

Both core skills (`skills/core/SKILL.md` for HTTP, `skills/core-mcp/SKILL.md`
for MCP) get a new "Getting Started" section near the top, before the
API primitives.

The bootstrap section teaches:

1. **Try connecting first.** A wsh server may already be running.
   - HTTP: `curl -sf http://localhost:8080/health`
   - MCP: `wsh_list_sessions()`
2. **Only start a server if needed.** Background it and use a unique
   server name for parallel safety.
   - `wsh server -L <unique-name> &` (defaults to `127.0.0.1:8080`)
3. **Create a session via the API** (not CLI subcommands).
   - HTTP: `POST /sessions` with `{"name": "work"}`
   - MCP: `wsh_create_session(name="work")`
4. **Now use the send/wait/read loop** on that session.

The existing API primitives and endpoint documentation remain unchanged.

**Files changed:** `skills/core/SKILL.md`, `skills/core-mcp/SKILL.md`.

**Changes when:** API surface changes.

### Layer 3: Plugin MCP Wiring

Add `.mcp.json` at the **repository root** (not inside `.claude-plugin/`)
to configure `wsh mcp` as a stdio MCP server. Per the Claude Code plugin
spec, component files (skills, hooks, MCP configs) live at the plugin
root, not inside the `.claude-plugin/` metadata directory.

```json
{
  "mcpServers": {
    "wsh": {
      "command": "wsh",
      "args": ["mcp"]
    }
  }
}
```

This is the primary fix and the 95% path. When it works, the agent gets
18 MCP tools with self-documenting descriptions. The directive block's
step 1 applies: "check your toolkit, use tools directly."

When it fails (no `wsh` binary on PATH, no running server for the bridge
to connect to), the agent falls through to directive step 2 (load core
skill) or step 3 (quick bootstrap).

**Files changed:** New `.mcp.json` at plugin root.

**Changes when:** Server binary interface changes.

### Layer 4: Plugin Structure (Core as Background Context) — Contingent

The core skills are marked `user-invocable: false` in their frontmatter.
This signals to Claude Code that they should be loaded as background
context rather than user-triggered commands. This is belt-and-suspenders:
if MCP is working, the tools are sufficient; if MCP fails, the core
skill is already in context.

**This layer is contingent.** We need to verify that Claude Code's
plugin loader respects `user-invocable: false` as an auto-load signal.
If it does, Layer 4 requires no file changes. If it does not, explore
alternatives:
- Configure the plugin to explicitly list core as background context
- Use the plugin's `settings.json` to influence skill loading

**Files changed:** 0–1 files (`plugin.json` or `settings.json`) depending
on verification.

**Changes when:** Claude Code plugin spec changes.

## What NOT to Do

**No `prompts.rs` prepend.** When an MCP agent calls `prompts/get` for a
specialized skill, serve it exactly as it exists on disk, directive block
included. The agent has tools. The directive tells it to use them. Tool
descriptions tell it how. No extra mapping needed.

This keeps `prompts.rs` simple (no preamble constants, no prepend logic)
and ensures skill content is identical whether loaded from disk or served
via MCP. One source, one version, no divergence.

**No transport-specific tables in specialized skills.** Core is the
single source of truth for API surface. The directive points there. One
update path, not ten.

**No sort-order hacks for skill loading.** If load order matters, the
fix is in the plugin spec or the directive itself — not in naming
conventions like `00_core/`.

## Replaying the Incident

Agent loads `wsh:drive-process`. Reads the directive.

**MCP working (95%):** Sees step 1. Checks toolkit. Finds
`wsh_send_input`. Uses tools. Done.

**MCP broken, core loaded as background:** Sees step 2. Finds core
content already in context. Uses curl commands from core. Done.

**MCP broken, core NOT in context:** Sees step 2. Reads "DO NOT GUESS."
Reads "search your workspace for `skills/core/`." Runs glob. Finds
`SKILL.md`. Reads it. Gets bootstrap sequence + curl commands. Done.

**Absolute worst case (no tools, can't find core files):** Sees step 3.
Runs `curl -sf http://localhost:8080/health`. If it succeeds, the agent
has a live endpoint. If it fails, the agent backgrounds a server
(`wsh server -L agent-$RANDOM &`), retries the health check, and now has
a live endpoint. From there, it can discover the API surface (e.g.,
`GET /openapi.yaml`). Degraded but functional.

Every layer peels back to reveal the next. The agent never reaches a
dead end where it has no actionable instruction.

## Escape Hatch

If we find in practice that MCP agents struggle to map abstract operation
names to tool names despite tool descriptions, we can add a condensed
MCP mapping table as a `prompts.rs` prepend. But start without it. You
can always add complexity; removing it is harder.

## Implementation Summary

| Change | Files | Layer |
|--------|-------|-------|
| Add directive block to specialized skills | 10 SKILL.md files | 1 |
| Add bootstrap section to core skills | 2 SKILL.md files | 2 |
| Add `.mcp.json` for MCP server | 1 new file | 3 |
| Verify/configure core auto-loading | 0–1 files (contingent) | 4 |
| **Total** | **13–14 files** | |

No changes to `prompts.rs`. No changes to Rust code. Pure
documentation and configuration.

**Note:** The skill file changes (Layers 1–2) are picked up by
`prompts.rs` at compile time via `include_str!`. The existing prompt
tests should be re-run to verify the directive blocks don't break
assertions, though the changes are additive (prepending a blockquote)
and should pass without modification.
