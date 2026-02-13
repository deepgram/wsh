# 2026-02-13: External Orchestrator Design for Multi-Agent Context and Project Tracking

## 1. Purpose

Build a standalone orchestrator process outside the Rust `wsh` codebase to coordinate many agent sessions while preserving existing terminal behavior.

Core constraints:
- do **not** modify existing terminal engine APIs or parser internals in Rust,
- keep `wsh` as the execution substrate (sessions, I/O, screen state),
- add cross-session coordination, human-readable context, and project continuity outside `wsh`.

## 2. Design choice

Use an **external orchestrator service** that interacts with `wsh` only through existing HTTP/WebSocket APIs.

The orchestrator manages:
- session lifecycle (`/sessions/*`) for each worker,
- periodic human-readable context updates,
- task assignment and handoff notes,
- project-wide state snapshots and summaries,
- optional human escalation events (`approval_needed`, `blocked`, `error`).

No protocol changes are required in `wsh` for the first version.

## 3. Core data model (append-first)

- `ProjectContext`: `project_id`, `name`, `goal`, `status`, `default_cwd`, `active_branch`, `owner`, `updated_at`.
- `SessionContext`: `session_name`, `project_id`, `agent_id`, `role`, `state`, `next_action`, `last_signal`, `updated_at`.
- `ContextEntry`: append-only event with `id`, `project_id`, `session_name`, `actor`, `kind`, `ts`, `text`, optional `refs`, optional `human_attention_needed`.
- `ProjectSnapshot`: compact materialized state with `summary`, `open_blockers`, `next_steps`, `recent_highlights`.

Natural-language notes (`ContextEntry.text`) are first-class; structured refs are optional metadata for reproducibility.

## 4. Runtime flow

### “do project X”
1. Resolve/create project context.
2. Create or reuse dedicated sessions under naming scheme: `proj-{name}::{role}::{idx}`.
3. Execute command loops using `wsh` APIs:
   - send input,
   - wait/quiesce,
   - read screen/scrollback,
   - append status/handoff notes.
4. Consolidate notes into session updates and project snapshots.

### Pull any session output
- Orchestrator can inspect any session on demand via existing endpoints and/or subscriptions.
- Context store keeps enough metadata to trace what each session has done.

## 5. Human channel strategy

Context events emit to a generic event bus first (`ContextEntry`), then adapters consume:
- web UI,
- mobile app,
- phone/voice bridge.

All channels consume the same event format so escalation behavior is uniform.

## 6. Error and safety behavior

- Retry with bounded backoff for transient `wsh` calls.
- Idempotent command/session creation identifiers.
- Human-gated safety for destructive actions by policy.
- Optional redaction of secrets before persisting context entries.

## 7. Delivery scope (v1)

- External process only.
- Filesystem context store (JSONL events + JSON snapshots).
- Session orchestration + heartbeat updates.
- Project/agent status command-line commands.
- API wrappers for status polling and pull-by-session inspection.

## 8. Build target

The initial implementation should be runnable as a standalone Python entrypoint and include:
- context schema definitions,
- `wsh` client wrapper,
- session task loop,
- status/snapshot writes,
- minimal CLI.

Future versions can evolve storage to SQLite and add richer adapters without changing core behavior.
