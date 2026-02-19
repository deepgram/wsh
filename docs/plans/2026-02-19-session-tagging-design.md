# Session Tagging and Grouping

## Problem

Agents and users managing multiple sessions have no way to logically group
them or perform operations across a subset. Every operation targets a single
session by name. There's no filtering, no grouping, and no way to say "wait
for quiescence on any of my build sessions."

## Design

### Data Model

Tags are simple strings assigned to sessions. A session can have zero or more
tags. Tags are mutable: assignable at creation time and modifiable afterward.

**Session struct** gains:

```rust
pub tags: Arc<RwLock<HashSet<String>>>,
```

**SessionRegistry** gains a reverse index for O(1) tag lookups:

```rust
struct RegistryInner {
    sessions: HashMap<String, Session>,
    tags_index: HashMap<String, HashSet<String>>,  // tag -> session names
    next_id: u64,
    max_sessions: Option<usize>,
}
```

The registry maintains the index across all mutation paths:

- `insert()` — index initial tags
- `remove()` — remove session from all tag entries
- `rename()` — update session name in all tag entries
- `add_tags()` / `remove_tags()` — update Session and index under write lock

**Tag validation rules:**

- Non-empty, max 64 characters
- Alphanumeric, hyphens, underscores, dots (no whitespace or special chars)
- Case-sensitive

**SessionInfo** gains `tags: Vec<String>` (sorted for stable output). This
propagates to all API surfaces: HTTP, WebSocket, MCP, and socket protocol.

### Events

New variant on `SessionEvent`:

```rust
SessionEvent::TagsChanged { name: String, added: Vec<String>, removed: Vec<String> }
```

Push-based tag event subscriptions are deferred. The event variant is for
internal use and future extension.

### API Surface

Tags are a scoping/filtering mechanism. Existing operations gain optional tag
parameters; tag management uses existing update endpoints.

#### HTTP

| Operation | Endpoint | Change |
|-----------|----------|--------|
| Create with tags | `POST /sessions` | Optional `tags: Vec<String>` in body |
| List with filter | `GET /sessions?tag=X&tag=Y` | Union: sessions with ANY listed tag |
| Get session | `GET /sessions/:name` | `SessionInfo` now includes `tags` |
| Modify tags | `PATCH /sessions/:name` | `add_tags` / `remove_tags` fields |
| Group quiesce | `POST /quiesce?tag=X` | Wait for any tagged session to become quiescent |

Multiple `tag` query params use **union** (OR) semantics: return sessions
matching any of the listed tags.

#### WebSocket (JSON-RPC)

| Method | Change |
|--------|--------|
| `create_session` | Optional `tags` param |
| `list_sessions` | Optional `tag` filter param |
| `add_tags` / `remove_tags` | New methods, or variants of `manage_session` |

#### MCP

| Tool | Change |
|------|--------|
| `wsh_create_session` | `tags: Option<Vec<String>>` in params |
| `wsh_list_sessions` | `tag: Option<Vec<String>>` filter in params |
| `wsh_manage_session` | `AddTags` / `RemoveTags` action variants |

#### Socket Protocol

- `SessionInfoMsg` and `CreateSessionResponseMsg` gain `tags: Vec<String>`
- `CreateSessionMsg` gains optional `tags` field
- Tag management via existing or new frame types

#### CLI

- `wsh --tag build --tag test` — tag the auto-created session
- `wsh list` — TAGS column in table output
- `wsh tag <session> add <tag> [<tag>...]` — add tags
- `wsh tag <session> remove <tag> [<tag>...]` — remove tags

### Group-Scoped Quiescence

`POST /quiesce?tag=X` waits for **any** session with the given tag to become
quiescent. The response includes which session triggered it. Callers can loop
to collect more results.

"Wait for ALL sessions to become quiescent" is a client-side concern: call
quiesce per-session for each member.

The same tag-as-scope pattern extends naturally to future operations (group
input broadcast, group kill) without architectural changes.

### Computed Property Filters (Deferred)

Automatic/computed tags (alternate screen mode, quiescent state, foreground
process) are a separate concern from manual tags. They would be implemented as
query filters on computed session properties rather than stored tags:

```
GET /sessions?screen_mode=alternate
GET /sessions?tag=build&screen_mode=alternate
```

This keeps manual tags (stable, user-assigned) distinct from dynamic state
(transient, system-derived). Deferred to a future design.

## Scope

**In scope:**

1. Data model: tags on Session, reverse index in SessionRegistry, validation
2. Tag management: add/remove via PATCH, set at creation
3. Filtering: list sessions by tag across HTTP, WS, MCP, socket
4. SessionInfo enrichment: tags in all API responses
5. Group-scoped quiescence: `POST /quiesce?tag=X`
6. CLI: `--tag` flag, tags column in `wsh list`, `wsh tag` subcommand
7. `SessionEvent::TagsChanged` event variant
8. Skill/docs updates: API docs, core skill, README

**Deferred:**

- Computed property filters (alternate screen, foreground process)
- Push-based tag event subscriptions
- Group-scoped input broadcast, group kill
- Web UI tag filtering/display

## Testing

- **Unit:** SessionRegistry tag CRUD, index consistency across
  create/remove/rename, tag validation, union filtering
- **Integration:** HTTP create-with-tags, PATCH add/remove, list with filter,
  quiesce by tag. MCP same flows. Socket protocol tags in SessionInfoMsg.
- **Edge cases:** empty tag list, duplicate tags, removing nonexistent tags,
  rename preserves tags, destroy cleans index, filter with no matches
