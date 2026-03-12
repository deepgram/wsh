# Agent Onboarding Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ensure AI agents can always bridge from skill knowledge ("what to do") to concrete API calls ("how to do it"), regardless of discovery channel.

**Architecture:** Four independent layers — directive blocks in specialized skills (Layer 1), bootstrap sections in core skills (Layer 2), MCP plugin wiring (Layer 3), and verification of core auto-loading (Layer 4). Each layer peels back to reveal the next; no layer depends on another's content.

**Tech Stack:** Markdown (skills), JSON (plugin config), Rust (test verification only)

**Spec:** `docs/superpowers/specs/2026-03-12-agent-onboarding-design.md`

---

## Chunk 1: Plugin MCP Wiring (Layer 3)

This is the highest-impact change — it gives Claude Code agents MCP tools automatically.

### Task 1: Create `.mcp.json`

**Files:**
- Create: `.mcp.json` (repository root, NOT inside `.claude-plugin/`)

- [ ] **Step 1: Create the MCP server configuration**

Create `.mcp.json` at the repository root with this exact content:

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

Per the Claude Code plugin spec, component files (skills, hooks, MCP configs) live at the plugin root, not inside the `.claude-plugin/` metadata directory.

- [ ] **Step 2: Verify file placement**

Run: `ls -la .mcp.json .claude-plugin/plugin.json`

Expected: Both files exist. `.mcp.json` is at the repo root alongside `.claude-plugin/`.

- [ ] **Step 3: Commit**

```bash
git add .mcp.json
git commit -m "feat(plugin): wire MCP server for Claude Code agents

Configure wsh mcp as a stdio MCP server via .mcp.json. When the plugin
is enabled, Claude Code auto-starts the bridge, giving agents access to
all 18 MCP tools with self-documenting descriptions."
```

---

## Chunk 2: Directive Blocks in Specialized Skills (Layer 1)

Add the execution context directive to all 10 specialized skills. The directive goes immediately after the YAML frontmatter closing `---`, before the skill's `#` heading. **Do not use hardcoded line numbers** — frontmatter length varies per file. Search for the closing `---` and the first `#` heading; insert between them.

The directive block is identical in every file:

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

### Task 2: Add directive to drive-process

**Files:**
- Modify: `skills/drive-process/SKILL.md` (insert after frontmatter closing `---`)

- [ ] **Step 1: Insert directive block**

Insert the directive block (shown above) between the closing `---` of the frontmatter and the `# wsh:drive-process` heading. The result should be:

```
---
<blank line>
> **IMPORTANT: EXECUTION CONTEXT**
> ...directive block...
<blank line>
# wsh:drive-process — Driving CLI Programs
```

- [ ] **Step 2: Verify file structure**

Read the first 25 lines. Confirm: frontmatter → blank line → directive blockquote → blank line → `#` heading.

### Task 3: Add directive to tui

**Files:**
- Modify: `skills/tui/SKILL.md` (insert after frontmatter closing `---`)

- [ ] **Step 1: Insert directive block**

Same directive block as Task 2. Insert between closing `---` and `# wsh:tui` heading.

- [ ] **Step 2: Verify file structure**

Read the first 25 lines. Confirm structure matches Task 2.

### Task 4: Add directive to multi-session

**Files:**
- Modify: `skills/multi-session/SKILL.md` (insert after frontmatter)

- [ ] **Step 1: Insert directive block**

Same directive block. Insert between closing `---` and `# wsh:multi-session` heading.

- [ ] **Step 2: Verify file structure**

### Task 5: Add directive to agent-orchestration

**Files:**
- Modify: `skills/agent-orchestration/SKILL.md` (insert after frontmatter)

- [ ] **Step 1: Insert directive block**

Same directive block. Insert between closing `---` and `# wsh:agent-orchestration` heading.

- [ ] **Step 2: Verify file structure**

### Task 6: Add directive to monitor

**Files:**
- Modify: `skills/monitor/SKILL.md` (insert after frontmatter)

- [ ] **Step 1: Insert directive block**

Same directive block. Insert between closing `---` and `# wsh:monitor` heading.

- [ ] **Step 2: Verify file structure**

### Task 7: Add directive to visual-feedback

**Files:**
- Modify: `skills/visual-feedback/SKILL.md` (insert after frontmatter)

- [ ] **Step 1: Insert directive block**

Same directive block. Insert between closing `---` and `# wsh:visual-feedback` heading.

- [ ] **Step 2: Verify file structure**

### Task 8: Add directive to input-capture

**Files:**
- Modify: `skills/input-capture/SKILL.md` (insert after frontmatter)

- [ ] **Step 1: Insert directive block**

Same directive block. Insert between closing `---` and `# wsh:input-capture` heading.

- [ ] **Step 2: Verify file structure**

### Task 9: Add directive to generative-ui

**Files:**
- Modify: `skills/generative-ui/SKILL.md` (insert after frontmatter)

- [ ] **Step 1: Insert directive block**

Same directive block. Insert between closing `---` and `# wsh:generative-ui` heading.

- [ ] **Step 2: Verify file structure**

### Task 10: Add directive to cluster-orchestration

**Files:**
- Modify: `skills/cluster-orchestration/SKILL.md` (insert after frontmatter)

- [ ] **Step 1: Insert directive block**

Same directive block. Insert between closing `---` and `# wsh:cluster-orchestration` heading.

- [ ] **Step 2: Verify file structure**

### Task 11: Add directive to infrastructure-ops

**Files:**
- Modify: `skills/infrastructure-ops/SKILL.md` (insert after frontmatter)

- [ ] **Step 1: Insert directive block**

Same directive block. Insert between closing `---` and `# wsh:infrastructure-ops` heading.

- [ ] **Step 2: Verify file structure**

### Task 12: Commit all directive blocks

- [ ] **Step 1: Commit**

```bash
git add skills/drive-process/SKILL.md skills/tui/SKILL.md \
  skills/multi-session/SKILL.md skills/agent-orchestration/SKILL.md \
  skills/monitor/SKILL.md skills/visual-feedback/SKILL.md \
  skills/input-capture/SKILL.md skills/generative-ui/SKILL.md \
  skills/cluster-orchestration/SKILL.md skills/infrastructure-ops/SKILL.md
git commit -m "feat(skills): add execution context directive to all specialized skills

Each skill now explicitly tells agents: (1) check for wsh_* MCP tools
and use them directly, (2) if no MCP tools, DO NOT GUESS — load the
core skill for API reference, (3) quick bootstrap fallback. Prevents
hallucination by vacuum — the root cause of the original incident."
```

---

## Chunk 3: Bootstrap Sections in Core Skills (Layer 2)

### Task 13: Add bootstrap section to HTTP core skill

**Files:**
- Modify: `skills/core/SKILL.md` (insert new section before `## Authentication`)

- [ ] **Step 1: Insert the Getting Started section**

Insert the following new section immediately before `## Authentication`. The `## How It Works` section (which includes the MCP callout blockquote) should remain unchanged above it. The result should be: `## How It Works` → MCP callout → `## Getting Started` → `## Authentication`.

```markdown
## Getting Started

Before using the API, make sure a wsh server is reachable.

**Step 1: Check for an existing server.** A wsh server may already be
running — try the health endpoint first:

    curl -sf http://localhost:8080/health

If this returns `200 OK`, you're ready. Skip to step 3.

**Step 2: Start a server (only if needed).** If no server is running,
start one in the background with a unique name to avoid collisions
with other sessions:

    wsh server -L agent-$RANDOM &

Wait a moment, then retry the health check to confirm it's up. The
server defaults to `127.0.0.1:8080`.

**Step 3: Create a session.** Sessions are where commands run. Create
one via the API:

    curl -s -X POST http://localhost:8080/sessions \
      -H "Content-Type: application/json" \
      -d '{"name": "work"}'

Returns `{"name": "work", ...}` on success.

**Step 4: Use the send/wait/read loop.** Now interact with your session
using the API primitives described below. The fundamental loop:

    # Send a command
    curl -s -X POST http://localhost:8080/sessions/work/input -d $'ls -la\n'
    # Wait for idle
    curl -s http://localhost:8080/sessions/work/idle?timeout_ms=2000
    # Read the screen
    curl -s http://localhost:8080/sessions/work/screen?format=plain
```

- [ ] **Step 2: Verify section placement**

Read lines around the insertion. Confirm the order is: `## How It Works` → content → `## Getting Started` → content → `## Authentication`.

### Task 14: Add bootstrap section to MCP core skill

**Files:**
- Modify: `skills/core-mcp/SKILL.md` (insert new section before `## Authentication`)

- [ ] **Step 1: Insert the Getting Started section**

Insert the following new section immediately before `## Authentication`. The `## How It Works` section should remain unchanged above it. The result should be: `## How It Works` → `## Getting Started` → `## Authentication`.

```markdown
## Getting Started

Before using the tools, make sure a wsh server is reachable.

**Step 1: Check for an existing server.** A wsh server may already be
running — try listing sessions:

    wsh_list_sessions()

If this returns a result (even an empty list), you're connected. Skip
to step 3.

**Step 2: Start a server (only if needed).** If the tool call fails
because no server is reachable, the `wsh mcp` bridge couldn't connect.
You may need to start a server manually via Bash:

    wsh server -L agent-$RANDOM &

Wait a moment, then retry `wsh_list_sessions()`.

**Step 3: Create a session.** Sessions are where commands run:

    wsh_create_session(name="work")

Returns the session name and terminal dimensions on success.

**Step 4: Use the send/wait/read loop.** Now interact with your session
using the tools described below. The primary tool for the loop:

    wsh_run_command(session="work", input="ls -la\n", format="plain")

This sends input, waits for idle, and returns the screen in one call.
For more control, use `wsh_send_input`, `wsh_await_idle`, and
`wsh_get_screen` separately.
```

- [ ] **Step 2: Verify section placement**

Read lines around the insertion. Confirm the order is: `## How It Works` → content → `## Getting Started` → content → `## Authentication`.

### Task 15: Commit bootstrap sections

- [ ] **Step 1: Commit**

```bash
git add skills/core/SKILL.md skills/core-mcp/SKILL.md
git commit -m "feat(skills): add bootstrap sequence to core skills

Both core skills now teach: (1) check for an existing server first,
(2) only start one if needed with a unique -L name, (3) create a
session via the API, (4) use the send/wait/read loop. Eliminates the
'zero to interactive session' knowledge gap."
```

---

## Chunk 4: Verification and Layer 4

### Task 16: Run prompt tests

**Files:**
- Test: `src/mcp/prompts.rs` (existing tests, no modifications expected)

- [ ] **Step 1: Run the prompt unit tests**

Run: `nix develop -c sh -c "cargo test --lib prompts"`

Expected: All 6 tests pass. The existing tests check for prompt count (11), names, descriptions, and specific content — all additive-safe.

**Note on test coverage:** Only `skills/core-mcp/SKILL.md` and the 10 specialized skills are compiled into `prompts.rs` via `include_str!`. The `skills/core/SKILL.md` file (HTTP core skill) is NOT served via MCP prompts — it exists only on disk for Claude Code plugin agents and HTTP-only agents. Changes to `core/SKILL.md` (Task 13) are verified only by manual inspection in Task 18.

- [ ] **Step 2: If any test fails, investigate**

The most likely failure would be the `get_prompt_core_returns_content` test which checks for `"core-mcp"` and `"wsh_run_command"` in the MCP core skill content. These strings still exist in `skills/core-mcp/SKILL.md` (we only added content, didn't remove any), so it should pass.

### Task 17: Verify Layer 4 (core auto-loading)

**Files:**
- Possibly modify: `.claude-plugin/plugin.json` or `settings.json` (only if auto-load doesn't work)

- [ ] **Step 1: Check if `user-invocable: false` causes auto-loading**

Read the Claude Code plugin documentation on skills. The core skills have `user-invocable: false` in their frontmatter. Determine whether Claude Code auto-loads these as background context.

One way to verify: install the plugin locally (`claude --plugin-dir .`) and check if the core skill content appears in the agent's context without explicitly invoking it.

- [ ] **Step 2: If auto-loading works, no changes needed**

Layer 4 is complete. Document the finding.

- [ ] **Step 3: If auto-loading does NOT work, add explicit configuration**

If `user-invocable: false` is not respected as auto-load, update `.claude-plugin/plugin.json` to explicitly reference the core skills as background context. The exact format depends on what Claude Code supports — check the plugin spec's `skills` field.

### Task 18: Final commit and summary

- [ ] **Step 1: Commit any Layer 4 changes (if any)**

Only if Task 17 step 3 was needed:

```bash
git add .claude-plugin/plugin.json
git commit -m "fix(plugin): configure core skills as background context

Claude Code does not auto-load user-invocable: false skills. Explicitly
configure core skills as background context in plugin.json."
```

- [ ] **Step 2: Run full test suite**

Run: `nix develop -c sh -c "cargo test"`

Expected: All tests pass (unit + integration).

- [ ] **Step 3: Verify the complete layer stack**

Manually check:
1. `.mcp.json` exists at repo root with `wsh` MCP server config
2. All 10 specialized skills have the directive block after frontmatter
3. Both core skills have the "Getting Started" section before "Authentication"
4. Core skills are loadable as background context (Layer 4 verified)
