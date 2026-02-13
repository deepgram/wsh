# wsh Orchestrator (external service)

This folder contains a standalone orchestrator process that coordinates many
`wsh` sessions from a single control plane while keeping core Rust untouched.

## What it does

- Maintains project/session context outside `wsh`.
- Stores natural-language notes in an append-only event log.
- Spawns/uses `wsh` sessions and pushes commands through the public `wsh` API.
- Produces snapshots that a UI/human can read at any time.

## Quick start

```bash
python -m orchestrator init project-alpha "Project Alpha" "Migrate legacy scripts"
python -m orchestrator assign project-alpha builder "npm install" "npm run test"
python -m orchestrator status project-alpha
python -m orchestrator list
```

## Environment variables

- `WSH_ORCH_BASE_URL` (default `http://127.0.0.1:8080`)
- `WSH_ORCH_TOKEN` (optional token for WSH auth)
- `WSH_ORCH_STATE_DIR` (default `~/.local/share/wsh-orchestrator`)
- `WSH_ORCH_POLL_INTERVAL_SECONDS` (default `60`)

## Commands

- `init`: create/update a project context.
- `assign`: create or reuse a session role and dispatch one or more commands.
- `send`: send one command to an existing tracked session.
- `status`: print project snapshot and recent activity.
- `pull`: read latest screen + scrollback from a specific session.
- `list`: list active sessions from `wsh`.

## Data layout

Context is stored as JSON files:
- `projects/<project_id>/project.json`
- `projects/<project_id>/snapshot.json`
- `projects/<project_id>/events.jsonl`
- `projects/<project_id>/sessions.json`

## Build

```bash
python -m compileall orchestrator
```

or (with tooling installed):

```bash
python -m pip install -e .
python -m wsh-orchestrator --help
```
