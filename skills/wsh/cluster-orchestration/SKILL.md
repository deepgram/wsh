---
name: wsh:cluster-orchestration
description: >
  Use when you need to manage sessions across multiple wsh servers
  in a federated cluster. Examples: "distribute builds across several
  machines", "create sessions on a specific backend", "monitor health
  across a cluster of servers", "coordinate work across server boundaries".
---

# wsh:cluster-orchestration — Distributed Terminal Sessions

Sometimes one machine isn't enough. You need to spread work across
multiple servers — running builds on beefy hardware, tests in
isolated environments, deployments on production hosts. Cluster
orchestration lets you manage sessions across a fleet of wsh
servers from a single hub.

## When to Use Cluster Orchestration

**Use cluster orchestration when:**
- Work needs to run on specific machines (different hardware,
  different networks, different environments)
- You need to scale beyond what one machine can handle
- Sessions need isolation across physical or virtual boundaries
- You're coordinating a distributed workflow (build here,
  test there, deploy somewhere else)

**Don't use cluster orchestration when:**
- All your work fits on one machine — use multi-session instead
- You only need one session — use the basic session primitives
- The tasks don't benefit from distribution

## Concepts

### Hub and Backends

A cluster has one **hub** server and one or more **backend** servers.
The hub is the server you talk to — it receives your requests and
either handles them locally or forwards them to the right backend.
Backends are regular wsh servers that the hub knows about.

You interact exclusively with the hub. The hub handles routing,
health monitoring, and aggregation transparently. From your
perspective, it looks like one server with sessions spread across
multiple machines.

### Server Identity

Every server in the cluster has a **hostname** — a unique identifier.
Backends acquire their hostname automatically when they connect to
the hub (the hub queries each backend's identity). The hub's own
hostname is its system hostname or a configured override.

You use hostnames to target specific servers when creating sessions
or querying state.

### Health Monitoring

The hub continuously monitors each backend's health. A backend
can be in one of three states:

- **healthy** — connected and responding normally
- **connecting** — initial connection in progress or reconnecting
  after a disruption
- **unavailable** — connection lost, not responding

Only healthy backends participate in session operations. The hub
automatically reconnects to backends that become unavailable, so
transient network issues resolve on their own.

## Server Registration and Monitoring

### Discovering the Cluster

Before creating sessions on remote servers, check what's available:

    list servers

This returns every server in the cluster, including the hub
itself. Each entry shows the hostname, health status, and role.
The hub always appears as "local" with health "healthy".

### Adding a Backend

Register a new backend server with the hub. Backend addresses
require a scheme (`http://` or `https://`) and may include a path
prefix for reverse-proxy deployments:

    add server at address http://10.0.1.10:8080
    add server at address https://proxy.example.com/wsh-node-1

The hub immediately begins connecting to the backend. It starts
in "connecting" state and transitions to "healthy" once the
connection is established and the backend's hostname is resolved.

If the backend requires authentication, provide a token:

    add server at address http://10.0.1.10:8080 with token "secret"

### Checking a Specific Server

Get detailed status for a single server by hostname:

    get server "prod-1"

### Removing a Backend

Deregister a backend when it's no longer needed:

    remove server "prod-1"

This disconnects from the backend and removes it from the
cluster. Sessions that were running on that backend become
inaccessible through the hub (they continue running on the
backend itself).

### Waiting for Backends to Become Healthy

After adding a backend, you need to wait for it to become healthy
before creating sessions on it. Poll the server list until the
backend's health transitions from "connecting" to "healthy":

    loop:
        list servers
        if target server is healthy: break
        wait briefly
        retry

This typically takes a few seconds. Don't proceed with session
creation until the backend is healthy — requests to unhealthy
backends will fail.

## Distributed Session Creation

### Creating a Session on a Specific Backend

Target a specific server by hostname when creating a session:

    create session "build" on server "prod-1"

The hub forwards the creation request to the named backend. The
session runs on that backend's hardware, in its environment, with
its resources. All subsequent operations on that session are
automatically routed through the hub to the right backend.

### Creating Local Sessions

Sessions created without a server target run on the hub itself:

    create session "local-work"

This is exactly the same as single-server operation. The hub
handles it locally without involving any backend.

### Choosing Where to Place Work

Consider these factors when deciding where to create sessions:

- **Hardware requirements**: CPU-intensive builds on powerful
  machines, memory-heavy tests on high-RAM servers
- **Network locality**: Operations that access local resources
  (databases, filesystems) should run on the same machine
- **Isolation**: Untrusted or experimental work on dedicated
  backends, away from production sessions
- **Load distribution**: Spread parallel work across backends
  to avoid overloading any single machine

## Tag-Based Cross-Server Workflows

Tags work transparently across server boundaries. Sessions on
different backends can share the same tags, and tag-based queries
aggregate results from all healthy servers.

### Distributed Fan-Out

Spread parallel work across the cluster using tags to track it:

    create session "build-api" on server "build-1", tagged: ci
    create session "build-web" on server "build-2", tagged: ci
    create session "test-e2e" on server "test-1", tagged: ci

    send each session its respective command
    wait for idle across sessions tagged "ci"

The idle detection races across all tagged sessions regardless
of which server they're on. The first to settle is returned.

### Listing Sessions Across Servers

    list sessions

Without a server filter, the session list aggregates across all
healthy backends plus the hub. Each session in the response
includes a `server` field indicating which server it lives on.

    list sessions tagged "ci"

Tag filtering also works across the full cluster. Only sessions
matching the tag are returned, from any server.

    list sessions on server "build-1"

To see sessions on a specific backend only, filter by server.

### Session Operations Are Transparent

Once a session exists, all operations work the same regardless of
where it lives. The hub routes requests automatically:

    send input to "build-api": cargo build\n
    wait for idle on "build-api"
    read screen from "build-api"
    kill session "build-api"

You don't need to remember which server a session is on. The hub
tracks this mapping and routes transparently.

## Cross-Server Quiescence Patterns

### Waiting for Any Session to Settle

The server-level idle detection races across all sessions in the
cluster:

    wait for idle across all sessions (timeout 2000ms)

Returns whichever session becomes idle first, including its name
and the server it's running on. Use `last_session` and
`last_generation` to avoid re-returning the same session.

### Waiting for a Tagged Subset

    wait for idle across sessions tagged "build" (timeout 2000ms)

This is the most common pattern for distributed fan-out. Tag
all related work, then poll idle across the group.

### Polling a Specific Backend's Sessions

If you need to check just one server's sessions:

    list sessions on server "build-1"
    for each session:
        wait for idle
        read screen
        check results

### Coordinating Sequential Stages

When one stage must complete before the next begins:

    # Stage 1: Build on the build server
    create session "build" on server "build-1", tagged: pipeline
    send to "build": make release\n
    wait for idle on "build" (timeout 5000ms)
    read screen from "build"
    # verify success

    # Stage 2: Test on the test server
    create session "test" on server "test-1", tagged: pipeline
    send to "test": ./run-tests.sh\n
    wait for idle on "test" (timeout 5000ms)
    read screen from "test"
    # verify success

    # Stage 3: Deploy on the deploy server
    create session "deploy" on server "deploy-1", tagged: pipeline
    send to "deploy": ./deploy.sh\n
    ...

Each stage runs on a different server but follows the same
send/wait/read/decide loop.

## Failure Handling

### Backend Goes Down

When a backend becomes unavailable:
- **Existing sessions on that backend become inaccessible.** Operations
  targeting those sessions will fail with a server unavailable error.
- **The hub continues operating normally.** Local sessions and sessions
  on other healthy backends are unaffected.
- **The hub automatically attempts to reconnect.** If the backend comes
  back, the connection is re-established and its sessions become
  accessible again.
- **Session listing excludes unavailable backends.** Only sessions from
  healthy servers appear in aggregated listings.

### Recovery Strategies

**Check health before critical operations:**

    list servers
    if target server is unavailable:
        fall back to another server or report the failure

**Design for partial failure:**

When fanning out across multiple backends, some may fail while
others succeed. Collect results from the successful ones and
handle failures individually rather than treating any failure as
a total failure.

    results = {}
    for each session in the fan-out:
        try:
            wait for idle
            read screen
            results[session] = success
        catch server unavailable:
            results[session] = failed
    report partial results

**Use tags for recovery:**

If a backend fails mid-workflow, you can recreate the affected
sessions on a different backend with the same tags:

    # Original session on failed backend
    # create session "build" on server "build-1", tagged: ci

    # Recovery: recreate on another backend
    create session "build-retry" on server "build-2", tagged: ci
    send to "build-retry": (same command)

### Session Lifetime and Server Lifetime

Sessions are owned by the backend they run on. If you remove
a backend from the cluster, its sessions continue running on
that machine — they just become unreachable through the hub.
If the backend process exits, its sessions end.

The hub doesn't migrate sessions. If a backend goes down and
its sessions are lost, you need to recreate them on another
backend.

## Pitfalls

### Don't Over-Distribute

Distribution adds latency and complexity. Every request to a
remote session goes through the hub to the backend and back.
If all your work can run on one machine, use multi-session on
a single server.

### Monitor Backend Health

Don't assume backends are always available. Check health before
starting critical workflows, and design for graceful degradation
when backends fail.

### Clean Up Remote Sessions

Remote sessions consume resources on backend machines. Clean up
after yourself — don't leave orphaned sessions running on
backends. The hub won't automatically kill sessions when you
remove a backend.

### Backend Authentication

Backends may require authentication tokens. Ensure tokens are
configured correctly when adding backends. Without proper
authentication, the hub won't be able to connect.

### IP Access Control

When the hub has an `[ip_access]` section in its federation config,
backend addresses are checked against the blocklist and allowlist
at registration time. Backends whose resolved IPs fall outside the
allowed ranges will be rejected. There is no hardcoded blocklist --
the operator owns the threat model.

### Hostname Uniqueness

Every server in the cluster must have a unique hostname. If two
backends have the same hostname, the second registration will be
rejected. Configure unique hostnames for each backend if the
system hostnames collide.
