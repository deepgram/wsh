# Future Directions: Hub Orchestration Mode

## What the Hub Is

`wsh` today is a single-server system. One `wsh server` process manages sessions on one machine. An AI agent connects to that server's API and drives terminal sessions locally.

The **hub** is a planned feature layer on `wsh server` that lets it act as a proxy to and manager of remote `wsh` servers. A hub maintains an explicit registry of backend servers -- each running their own `wsh server` -- and exposes their sessions through a unified API. An AI agent connects to one hub and can create, list, drive, tag, and query sessions across the entire cluster as if they were local.

The hub is not a separate binary. It is `wsh server` with hub features enabled (via config file or CLI flags). The hub itself remains a fully functional `wsh` server: it can host local sessions alongside its proxy duties.

### How the Registry Works

Backend servers are registered explicitly. There are two mechanisms:

1. **Config file**: A list of backend server addresses loaded at startup.
2. **API**: Runtime registration and deregistration of backends via HTTP/WebSocket.

A hub connects outward to each registered backend as a client, proxying API requests and aggregating session lists. The backends do not need to know they are behind a hub. From the backend's perspective, the hub is just another API client.

### Why Future Directions Matter

Two design questions have surfaced during hub planning that do not need to be solved for v1 but will shape later versions. Documenting them now ensures that v1 decisions do not accidentally foreclose on the right answers, and gives future implementors (human or AI) full context on the reasoning.

---

## 1. Observer Safety and the "Bobby Problem"

### The Scenario

A cluster of 11 machines is running: 1 hub (with an AI agent orchestrating workloads) plus 10 backend `wsh` servers. The AI agent is instructed: "if new machines show up, use them."

Bobby, a human, wants to observe the cluster. He runs `wsh` on his laptop in hub mode, pointing it at the same 10 backend servers. Bobby's hub connects outward to the backends and presents him with a unified view of all sessions across the cluster. He is reading, not writing.

The AI agent's hub sees Bobby's machine appear on the network. Treating it as a new backend server, the agent starts deploying workloads to Bobby's laptop. Bobby did not consent to this. Bobby gets sad.

### Why This Is Not a Problem in v1

The hub's server registry is explicit. Servers become backends because someone registers them via config file or API call. Bobby's hub connects *outward* to existing backends as a client -- it does not register itself anywhere. Bobby's hub and the AI agent's hub are independent views of the same backends. Neither hub knows the other exists.

For the AI agent to deploy workloads to Bobby's laptop, someone would have to explicitly add Bobby's machine to the agent's hub registry. That is a deliberate administrative action, not an accident.

The key property: **connecting to a backend as a client does not make you visible as a backend to other hubs.** Hubs are readers of the cluster, not members of it, unless explicitly enrolled.

### Where the Risk Actually Lives

The Bobby problem becomes real when **auto-discovery** is implemented -- a planned future feature where `wsh` servers could announce themselves on the network (via mDNS, gossip protocol, or similar) and hubs could auto-register what they find.

If Bobby's machine is running a `wsh` server (even just the hub, which IS a server), auto-discovery could sweep it into the AI agent's server list. The agent sees a new backend, starts creating sessions on it, and Bobby's laptop becomes an unwilling compute node.

This is not hypothetical. Any auto-discovery mechanism that treats "present on the network" as "available for workloads" will produce this failure mode.

### Design Principles for Prevention

Three mechanisms, layered together, prevent the Bobby problem from emerging as the system evolves.

#### Backend Roles and Capabilities

Backend servers in the registry should carry a **role** or **capability** designation. The initial implementation needs only two states:

- **Full member**: Accepts session creation and workload assignment.
- **(No role / default)**: Present in the registry but not available for workloads.

This is intentionally minimal. The registry schema should be designed so that additional roles can be added later -- "read-only," "observer," "drain" -- without restructuring the data model. A role field on the registry entry (rather than a boolean flag) provides this extensibility.

The important constraint: the default for a newly added backend should NOT be "full member." New entries should require explicit promotion before they accept workloads.

#### Discovery vs. Enrollment

When auto-discovery lands, it should populate a **discovered** list that is separate from the **active backends** list. The flow:

```
Network announcement  -->  Discovered list  -->  [explicit action]  -->  Active backends
```

Discovery means "here is what exists on the network." Enrollment means "this server accepts workloads from this hub." The gap between the two must always require an explicit action -- human approval, agent confirmation, or a policy rule.

This applies equally to pull-based registration, where a server contacts a hub on startup and says "I exist." The hub should add it to the discovered list, not the active backend list.

Concrete example of the flow:

1. Bobby's laptop announces itself via mDNS.
2. The AI agent's hub adds it to the discovered list.
3. The agent (or a human operator) reviews the discovered list.
4. Bobby's machine is either promoted to active (deliberate) or ignored (default).

Without this two-stage design, auto-discovery is a footgun.

#### Hub Identity Separation

Bobby's hub and the AI agent's hub should be distinguishable. If both hubs are on the same network and auto-discovery is active, a hub should not discover other hubs as potential backends by default. This can be achieved through:

- A "hub" role in the capability system that auto-discovery filters exclude.
- Network-level scoping (discovery groups, tags, or namespaces).
- Configuration that controls whether a server advertises itself for discovery at all.

The simplest v1-compatible approach: servers do not advertise themselves by default. Advertising is opt-in.

### Summary

| Threat | Mitigation | When It Matters |
|--------|-----------|-----------------|
| Bobby added to registry manually | Admin error, outside wsh's scope | Always |
| Bobby swept in by auto-discovery | Discovered vs. active separation | When auto-discovery ships |
| Bobby's hub treated as a backend | Role/capability system, hub identity filtering | When auto-discovery ships |
| Default role allows workloads | Default role = no workloads, explicit promotion required | When roles ship |

None of this is needed for v1. But v1's registry schema should include a role field (even if only "full member" is implemented) so the discovery/enrollment separation can be added without migrating data.

---

## 2. Transitive Hub Chaining

### The Question

Since a hub IS a `wsh` server (same binary, same API, just with hub features enabled), a hub can register another hub as one of its backends. The question: if Hub A is connected to Hub B, and Hub B is connected to Servers C, D, and E, should Hub A see the sessions on C, D, and E?

```
Hub A  -->  Hub B  -->  Server C
                   -->  Server D
                   -->  Server E
```

In v1, Hub A sees only Hub B's local sessions. It does not see C, D, or E. This section explores what transitive visibility would look like and why it is deferred.

### The Value

Transitive hub chaining enables hierarchical cluster topologies that map to real organizational structures.

**Regional clusters.** A company has data centers in US-East, US-West, and EU. Each region runs a regional hub managing its local servers. A top-level hub connects to the three regional hubs and provides a single pane of glass across the entire fleet.

```
Global Hub  -->  US-East Hub  -->  [10 servers]
            -->  US-West Hub  -->  [8 servers]
            -->  EU Hub       -->  [12 servers]
```

An agent connected to the global hub can tag sessions by region, query across all 30 servers, and wait for cross-region quiescence -- without knowing the topology.

**Team federation.** Team A has their own hub and servers. Team B has theirs. A company-wide hub connects to both team hubs, enabling cross-team session visibility for platform engineers or orchestrator agents while each team retains local autonomy.

**Flattened views.** The whole point of transitive chaining is that the agent at the top does not need to know or care about the hierarchy. It sees a flat list of sessions with metadata (tags, server origin) and operates on them uniformly. The hub handles routing.

### The Complexity

Transitive chaining introduces several hard problems that do not exist in a single-level hub topology.

#### Request Depth

Hub A sends a request to Hub B, which forwards it to Server C. That is two proxy hops. Each hop adds latency, and each hop is a point where the request can fail. Error messages must propagate back through the chain with enough context to debug ("Server C returned 404" vs. "Hub B returned 502").

For interactive terminal sessions -- where an agent is sending keystrokes and reading screen state -- even small per-hop latency compounds. A two-hop WebSocket subscription means output from Server C traverses two WebSocket relays before reaching the agent.

#### Registry Synchronization

Hub A needs to know what backends Hub B has. If Hub B adds or removes a backend, Hub A's view of the cluster is stale until it learns about the change. This requires either:

- Polling: Hub A periodically re-fetches Hub B's backend list. Simple but introduces staleness windows.
- Subscription: A hub-to-hub event protocol where Hub B pushes registry changes to Hub A in real time. More complex but necessary for responsive orchestration.

The subscription approach is the right long-term answer, but it requires designing a new protocol layer between hubs that does not exist today.

#### Cycle Detection

If Hub A registers Hub B as a backend, and Hub B registers Hub A as a backend, requests loop forever. Cycles must be detected and rejected.

The likely mechanism: each hub carries a unique ID. When a hub connects to a backend, it sends its ID chain (the list of hub IDs in the path from the top-level client to this point). If a hub sees its own ID in the chain, it rejects the connection.

This also handles longer cycles: Hub A --> Hub B --> Hub C --> Hub A.

#### Naming and Addressing

Sessions in a flat single-server model are identified by name alone: `build-agent`. In a single-level hub, sessions are scoped by backend: `server-c/build-agent`. In a transitive hierarchy, sessions could be: `us-east-hub/server-c/build-agent`.

The addressing gets deeper with each level. But the whole point of the hub is to flatten this -- an agent should be able to query by tag and get results regardless of where sessions live. This means the hub must maintain a mapping from flat identifiers (or tag-based queries) to hierarchical addresses, and route requests accordingly.

Tag-based operations (`list sessions with tag=build`, `wait for quiescence on tag=deploy`) work naturally across the hierarchy because tags are properties of sessions, not of topology. Name-based operations require disambiguation when sessions on different backends share names.

#### Cross-Hop Quiescence

"Wait for all sessions with tag X to go idle" is already non-trivial in a single-level hub (the hub must aggregate idle signals from multiple backends). In a transitive hierarchy, the hub must aggregate signals from sub-hubs, each of which is aggregating from their own backends. The idle signal must propagate up the chain, and a single session becoming active at the bottom must cancel the quiescence wait at the top.

This is solvable but requires careful protocol design to avoid both false positives (reporting idle when a sub-hub hasn't checked all its backends) and excessive chattiness (every keystroke propagating up the full chain).

### Current Decision

In v1, if Hub A connects to Hub B as a backend, Hub A sees Hub B's **local sessions only** -- the sessions running directly on Hub B's `wsh` server process. Hub A does not see Hub B's backends' sessions transitively.

This is the right default for three reasons:

1. It avoids all the complexity above until there is a concrete need.
2. It matches user expectations -- registering a server as a backend exposes that server's sessions, not some unknown transitive set.
3. Nothing in the v1 design precludes adding transitive visibility later. The registry, API, and session model are all extensible.

**Analogy:** This is similar to how Git remotes work. You can have remotes, and your remotes can have their own remotes, but `git fetch` does not transitively fetch your remote's remotes. If you want that, you set it up explicitly.

### Future Plans

When transitive chaining is implemented, the following pieces are needed:

- **Opt-in transitive visibility.** A flag on hub-to-hub connections that controls whether the upstream hub exposes its backends' sessions to the downstream hub. Default: off.
- **Hub-to-hub event subscription protocol.** A mechanism for hubs to push registry changes (backend added/removed, sessions created/destroyed) to connected upstream hubs in real time.
- **Cycle detection via hub-ID propagation.** Each hub connection carries the chain of hub IDs traversed. A hub rejects connections that would create a cycle.
- **Hierarchical naming with automatic flattening.** Sessions carry their full hierarchical address internally but are queryable by tag and flat name at any level. The hub handles routing transparently.
- **Cross-hop quiescence aggregation.** The quiescence protocol extended to aggregate idle signals across multiple hub levels, with backpressure to avoid excessive signaling.

Each of these is a substantial design effort. They should be tackled incrementally as real use cases demand them, not speculatively.

---

## Design Constraints for v1

Both future directions impose the same constraint on v1: **do not bake in assumptions that prevent extension.**

Concretely:

- The backend registry entry should include a `role` field from day one, even if the only value is `"member"`.
- The registry should be a data structure that can accommodate a separate "discovered" list alongside the "active" list without schema migration.
- Hub-to-backend connections should carry metadata (hub ID, connection flags) that can be extended for cycle detection and transitive visibility without protocol-breaking changes.
- Session identifiers returned through the hub API should include backend origin metadata, so clients can distinguish sessions from different backends even before hierarchical naming is fully designed.

These are small, low-cost decisions that keep the door open for the features described above.
