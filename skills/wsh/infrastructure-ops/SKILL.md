---
name: wsh:infrastructure-ops
description: >
  Use when you need to manage infrastructure across multiple servers
  interactively via wsh — deploying applications, configuring services,
  managing packages, performing rolling updates, and handling the prompts
  and judgment calls that declarative tools cannot. Examples: "deploy this
  application across 10 servers with health checks between each",
  "upgrade packages across the fleet and handle diverse prompts",
  "inspect and modify configuration across servers", "roll back a
  failed deployment".
---

# wsh:infrastructure-ops -- Fleet Management

Ansible, Puppet, and Chef exist because you can't interactively
operate 50 machines at once. They solve this with declarative
configs and idempotent operations -- you describe the desired state,
the tool converges toward it. This works brilliantly for the 80%
of infrastructure work that's predictable and repeatable.

But 20% of real infrastructure work isn't predictable. Interactive
installers that ask questions nobody anticipated. Package upgrades
that present merge conflicts in config files. Services that fail
in ways that require investigation, not just a restart. Approval
prompts that need human judgment. Diagnostics that require poking
around, reading logs, trying things.

wsh changes the equation. An AI agent can sit at every terminal
simultaneously -- reading screens, handling prompts, making
decisions. This enables an imperative, interactive model for the
work that declarative tools can't handle. Not a replacement for
Ansible. A complement for the cases where Ansible isn't enough.

## When to Use This

**Use infrastructure-ops when:**
- Operations require interactive input -- installers, prompts,
  approval dialogs, merge conflict resolution
- Each host may behave differently and needs per-host judgment
- You need to verify results interactively between steps
  (read logs, check endpoints, inspect state)
- The operation is exploratory -- diagnosing an issue across
  a fleet, where what you do on host N depends on what you
  found on hosts 1 through N-1
- Rolling deployments need health verification with real
  traffic before proceeding to the next batch

**Don't use infrastructure-ops when:**
- The operation is fully predictable and idempotent -- use
  Ansible, Terraform, or your existing config management
- You're provisioning infrastructure from scratch -- use
  Terraform or CloudFormation
- The operation is a single command with no interaction --
  use `parallel-ssh` or `ansible -m shell`
- You don't need per-host decision-making -- fan out with
  a simple script instead

## Prerequisites

You need a federated wsh cluster: a hub server with backends
registered for each target machine. Each backend is a wsh server
running on the target host.

    list servers
    # Expect: hub (local), plus one backend per target host

If backends aren't registered yet, add them:

    add server at address http://10.0.1.10:8080
    add server at address http://10.0.1.11:8080
    add server at address http://10.0.1.12:8080

    # Wait for all to become healthy
    loop:
        list servers
        if all target servers are healthy: break
        wait briefly
        retry

See the **wsh:cluster-orchestration** skill for details on server
registration, health monitoring, and authentication.

You also need the patterns from **wsh:drive-process** (the
send/wait/read loop) and **wsh:multi-session** (parallel session
management, tagging, fan-out). This skill composes on top of both.

## Core Pattern: Same Operation Across N Hosts

The fundamental infrastructure pattern: run the same operation on
every host, but handle each host's response individually. This is
different from `parallel-ssh` because you react to what each host
does -- you don't just fire and forget.

### Sequential Fan-Out

The safe default. Operate on one host at a time, verify success
before moving to the next:

    hosts = ["web-1", "web-2", "web-3", "web-4", "web-5"]
    results = {}

    for host in hosts:
        create session "op-{host}" on server "{host}"
        send to "op-{host}": sudo systemctl restart myapp\n
        wait for idle on "op-{host}"
        read screen from "op-{host}"

        if password prompt detected:
            send to "op-{host}": {sudo_password}\n
            wait for idle on "op-{host}"
            read screen from "op-{host}"

        if success:
            results[host] = "ok"
        else:
            results[host] = "failed"
            # Decide: continue to next host, or stop?

        kill session "op-{host}"

    report results

Sequential is slower but safer. If the operation breaks the first
host, you haven't touched the other four.

### Parallel Fan-Out

When the operation is safe to run everywhere simultaneously --
read-only commands, status checks, log collection:

    hosts = ["web-1", "web-2", "web-3", "web-4", "web-5"]

    # Launch all at once
    for host in hosts:
        create session "check-{host}" on server "{host}", tagged: fleet-check
        send to "check-{host}": systemctl status myapp\n

    # Poll for completion
    completed = {}
    while not all hosts completed:
        wait for idle on sessions tagged "fleet-check" (timeout 2000ms)
        # Returns whichever session settled first
        read screen from that session
        record result
        mark as completed
        # Use last_session + last_generation to advance

    # Clean up
    for host in hosts:
        kill session "check-{host}"

    report results

### Handling Per-Host Prompt Diversity

The real reason this pattern exists. When you run `apt upgrade`
across 10 servers, each one may present different prompts:

- Server 1: no prompts, clean upgrade
- Server 2: "Configuration file changed, keep or replace? [Y/n]"
- Server 3: "Service nginx needs to be restarted. Restart now? [y/N]"
- Server 4: "Pending kernel upgrade. Reboot required."
- Server 5: dependency conflict requiring manual resolution

A declarative tool must handle all these cases upfront with
configuration flags. An agent reads each screen and responds
with judgment:

    read screen from "upgrade-{host}"

    if "keep or replace" in screen:
        # Inspect the diff, decide based on whether we've
        # customized this config
        send: d\n    # show diff first
        wait, read
        if config has local customizations:
            send: N\n  # keep ours
        else:
            send: Y\n  # take the new version

    if "restart now" in screen:
        # Check if this is a load-balanced service that can restart
        send: y\n

    if "reboot required" in screen:
        # Note for later -- don't reboot mid-upgrade-sweep
        results[host].needs_reboot = true

This per-host judgment is the core value proposition. The agent
adapts to what each host actually presents.

## Rolling Deployments

Deploy to a fleet incrementally, verifying health between each
batch. If something goes wrong, stop before it affects the whole
fleet.

### The Rolling Deploy Pattern

    hosts = ["web-1", "web-2", "web-3", "web-4", "web-5", "web-6"]
    batch_size = 2
    failed = []

    for batch in chunk(hosts, batch_size):
        # Deploy this batch in parallel
        for host in batch:
            create session "deploy-{host}" on server "{host}", tagged: deploy
            send to "deploy-{host}": cd /opt/myapp && ./deploy.sh v2.1.0\n

        # Wait for all in this batch to finish
        for host in batch:
            wait for idle on "deploy-{host}" (timeout 30000ms)
            read screen from "deploy-{host}"

            if deployment prompt detected:
                handle prompt (see per-host prompt handling)

            wait for idle on "deploy-{host}"
            read screen from "deploy-{host}"

            if deployment failed:
                failed.append(host)

        # Health check this batch before proceeding
        for host in batch:
            create session "health-{host}" on server "{host}", tagged: health
            send to "health-{host}": curl -sf http://localhost:8080/health\n
            wait for idle on "health-{host}"
            read screen from "health-{host}"

            if health check failed:
                failed.append(host)

            kill session "health-{host}"

        # Clean up deploy sessions for this batch
        for host in batch:
            kill session "deploy-{host}"

        # Gate: stop if this batch had failures
        if failed is not empty:
            report: "Stopping rolling deploy. Failed hosts: {failed}"
            report: "Remaining hosts not deployed: {remaining}"
            break

        # Optional: wait for the new version to soak
        wait 30 seconds
        # Re-check health after soak period
        for host in batch:
            create session "soak-{host}" on server "{host}"
            send to "soak-{host}": curl -sf http://localhost:8080/health\n
            wait for idle, read screen
            if unhealthy:
                failed.append(host)
            kill session "soak-{host}"

        if failed is not empty:
            report: "Soak check failed. Stopping."
            break

    if failed is empty:
        report: "Rolling deploy complete. All hosts healthy."

### Choosing Batch Size

- **Batch of 1**: Safest. Every host is individually verified.
  Slowest. Use for the first deploy of a risky change.
- **Batch of N/3**: Good balance. You always have 2/3 of the
  fleet on the old version if something goes wrong.
- **Batch of N-1**: Deploy to all but one (the canary). If the
  canary is still fine on the old version after you've deployed
  everywhere else, you know the issue is with the new version.

### Canary Pattern

A special case of rolling deployment: deploy to one host first,
soak for longer, then proceed if healthy:

    canary = hosts[0]
    rest = hosts[1:]

    # Deploy to canary
    create session "deploy-canary" on server "{canary}"
    send to "deploy-canary": ./deploy.sh v2.1.0\n
    wait for idle, read screen, handle prompts
    kill session "deploy-canary"

    # Extended health check on canary
    for i in range(5):
        wait 60 seconds
        create session "canary-check" on server "{canary}"
        send to "canary-check": curl -sf http://localhost:8080/health && echo "HEALTHY" || echo "UNHEALTHY"\n
        wait for idle, read screen
        if "UNHEALTHY" in screen:
            report: "Canary failed health check. Aborting."
            initiate rollback on canary
            stop
        kill session "canary-check"

    # Canary is healthy after 5 minutes. Deploy the rest.
    proceed with rolling deploy on rest

## Configuration Management

Inspect current configuration, apply changes, and verify the
result -- interactively, with the ability to inspect and judge
at each step.

### Inspect-Modify-Verify

The fundamental configuration pattern:

    for host in hosts:
        create session "config-{host}" on server "{host}"

        # 1. Inspect current state
        send to "config-{host}": cat /etc/myapp/config.yaml\n
        wait for idle, read screen
        record current_config[host]

        # 2. Decide whether to modify
        if config needs updating:
            # Back up first
            send: cp /etc/myapp/config.yaml /etc/myapp/config.yaml.bak\n
            wait for idle

            # Apply the change (using sed, patch, or writing a new file)
            send: sed -i 's/max_connections: 100/max_connections: 200/' /etc/myapp/config.yaml\n
            wait for idle

            # 3. Verify the change
            send: cat /etc/myapp/config.yaml\n
            wait for idle, read screen
            if change is correct:
                results[host] = "updated"
            else:
                # Restore backup
                send: cp /etc/myapp/config.yaml.bak /etc/myapp/config.yaml\n
                wait for idle
                results[host] = "reverted -- change didn't apply correctly"
        else:
            results[host] = "no change needed"

        kill session "config-{host}"

### Configuration Drift Detection

Compare configurations across the fleet to find hosts that
have drifted from the expected state:

    expected_config = "..."  # known-good configuration
    drifted = []

    for host in hosts:
        create session "audit-{host}" on server "{host}", tagged: audit
        send to "audit-{host}": cat /etc/myapp/config.yaml\n

    for host in hosts:
        wait for idle on "audit-{host}"
        read screen from "audit-{host}"
        if screen does not match expected_config:
            drifted.append(host)
            # Optionally: read scrollback for the full config if it's long
        kill session "audit-{host}"

    if drifted:
        report: "Configuration drift detected on: {drifted}"
        # Inspect each drifted host for details

### Interactive Configuration Tools

Some services have their own interactive configuration tools --
`mysql_secure_installation`, `dpkg-reconfigure`, `certbot`,
database migration wizards. These can't be driven by declarative
tools, but an agent can handle them:

    create session "certbot-{host}" on server "{host}"
    send to "certbot-{host}": sudo certbot --nginx\n
    wait for idle, read screen

    # Certbot asks a series of questions
    if "Enter email address" in screen:
        send: admin@example.com\n
        wait for idle, read screen

    if "Terms of Service" in screen:
        send: A\n   # Agree
        wait for idle, read screen

    if "Which names would you like to activate HTTPS for" in screen:
        send: 1\n   # Select the first domain
        wait for idle, read screen

    # Verify the certificate was issued
    send: sudo certbot certificates\n
    wait for idle, read screen
    verify certificate is present and valid

    kill session "certbot-{host}"

## Package Management at Scale

Package upgrades are one of the most common infrastructure tasks,
and one of the most prone to interactive surprises.

### The Upgrade Pattern

    hosts = ["web-1", "web-2", "web-3", "db-1", "db-2"]

    for host in hosts:
        create session "upgrade-{host}" on server "{host}"

        # Update package lists
        send to "upgrade-{host}": sudo apt update\n
        wait for idle on "upgrade-{host}" (timeout 30000ms)
        read screen

        if password prompt:
            send: {sudo_password}\n
            wait for idle, read screen

        # Check what would be upgraded (dry run)
        send: apt list --upgradable\n
        wait for idle, read screen
        record pending_upgrades[host]

    # Review: show the operator what each host needs
    report pending upgrades per host
    # At this point, decide whether to proceed

    # Execute upgrades
    for host in hosts:
        send to "upgrade-{host}": sudo apt upgrade -y\n
        wait for idle (timeout 60000ms)
        read screen

        # Handle the prompts that -y doesn't suppress
        loop until shell prompt returns:
            if "Configuration file" in screen:
                # Package wants to overwrite a config file
                # Decide based on whether we maintain custom configs
                send: N\n  # keep current version
                wait for idle, read screen

            if "Daemons using outdated libraries" in screen:
                send: \n   # accept defaults
                wait for idle, read screen

            if "restart services" in screen:
                send: y\n
                wait for idle, read screen

            if shell prompt detected:
                break

            wait for idle (timeout 5000ms)
            read screen

        # Verify
        send: apt list --upgradable\n
        wait for idle, read screen
        if packages still pending:
            results[host] = "partial upgrade"
        else:
            results[host] = "fully upgraded"

        kill session "upgrade-{host}"

### Handling Package Manager Differences

Different hosts may use different package managers. Detect and
adapt:

    send to "detect-{host}": which apt yum dnf apk 2>/dev/null\n
    wait for idle, read screen

    if "apt" in screen:
        update_cmd = "sudo apt update && sudo apt upgrade -y"
    elif "dnf" in screen:
        update_cmd = "sudo dnf upgrade -y"
    elif "yum" in screen:
        update_cmd = "sudo yum update -y"
    elif "apk" in screen:
        update_cmd = "sudo apk update && sudo apk upgrade"

    send to "upgrade-{host}": {update_cmd}\n

### Specific Package Installation

When you need to install a specific package with version pinning:

    for host in hosts:
        create session "install-{host}" on server "{host}", tagged: install
        send to "install-{host}": sudo apt install nginx=1.24.0-1~jammy\n
        wait for idle, read screen

        if "Do you want to continue?" in screen:
            send: Y\n
            wait for idle, read screen

        if "unable to locate package" in screen or "has no installation candidate" in screen:
            results[host] = "package not available"
        elif "is already the newest version" in screen:
            results[host] = "already installed"
        else:
            # Verify installation
            send: nginx -v\n
            wait for idle, read screen
            if "1.24.0" in screen:
                results[host] = "installed"
            else:
                results[host] = "unexpected version"

        kill session "install-{host}"

## Health Verification

Post-operation checks are what separate a careful deployment from
a reckless one. Always verify after making changes.

### Service Health Check

    create session "health-{host}" on server "{host}"

    # Check the service is running
    send to "health-{host}": systemctl is-active myapp\n
    wait for idle, read screen
    service_running = "active" in screen

    # Check the endpoint responds
    send: curl -sf -o /dev/null -w '%{http_code}' http://localhost:8080/health\n
    wait for idle, read screen
    endpoint_healthy = "200" in screen

    # Check recent logs for errors
    send: journalctl -u myapp --since '5 minutes ago' --no-pager | grep -i error | tail -5\n
    wait for idle, read screen
    recent_errors = screen contains error lines

    # Check resource usage
    send: systemctl show myapp --property=MemoryCurrent,CPUUsageNSec\n
    wait for idle, read screen
    record resource metrics

    kill session "health-{host}"

    health[host] = {
        running: service_running,
        endpoint: endpoint_healthy,
        errors: recent_errors,
        resources: metrics
    }

### Deep Health Check

For critical deployments, go beyond surface-level checks:

    # Check the application version matches what we deployed
    send: curl -sf http://localhost:8080/version\n
    wait for idle, read screen
    verify version matches expected

    # Check database connectivity
    send: curl -sf http://localhost:8080/health/db\n
    wait for idle, read screen

    # Check dependent services
    send: curl -sf http://localhost:8080/health/redis\n
    wait for idle, read screen

    # Run a smoke test
    send: curl -sf -X POST http://localhost:8080/api/test -d '{"ping":"pong"}'\n
    wait for idle, read screen
    verify expected response

### Fleet-Wide Health Dashboard

Collect health from all hosts and present a summary:

    health_report = {}

    for host in hosts:
        create session "health-{host}" on server "{host}", tagged: health
        send to "health-{host}": curl -sf http://localhost:8080/health && echo "OK" || echo "FAIL"\n

    for host in hosts:
        wait for idle on "health-{host}"
        read screen from "health-{host}"
        health_report[host] = "OK" if "OK" in screen else "FAIL"
        kill session "health-{host}"

    healthy_count = count of "OK" in health_report
    report: "{healthy_count}/{total} hosts healthy"
    if any "FAIL":
        report: "Unhealthy hosts: {list of failed hosts}"

## Rollback

When something fails mid-fleet, you need a plan. The rollback
strategy depends on what went wrong and how far you got.

### Pre-Deployment Rollback Preparation

Before deploying, ensure you can roll back:

    for host in deploy_hosts:
        create session "prep-{host}" on server "{host}"

        # Record current version
        send to "prep-{host}": readlink /opt/myapp/current\n
        wait for idle, read screen
        previous_version[host] = extract version from screen

        # Ensure previous release artifacts exist
        send: ls /opt/myapp/releases/\n
        wait for idle, read screen
        verify previous version directory exists

        kill session "prep-{host}"

### Rollback After Failed Deployment

When a rolling deploy fails partway through:

    # failed_hosts: hosts where deployment failed
    # deployed_hosts: hosts where deployment succeeded
    # remaining_hosts: hosts not yet touched

    # Step 1: Fix the failed hosts
    for host in failed_hosts:
        create session "rollback-{host}" on server "{host}"
        send to "rollback-{host}": cd /opt/myapp && ./rollback.sh {previous_version[host]}\n
        wait for idle, read screen, handle prompts
        verify service is healthy
        kill session "rollback-{host}"

    # Step 2: Decide about successfully deployed hosts
    # Option A: Roll them back too (full rollback)
    for host in deployed_hosts:
        create session "rollback-{host}" on server "{host}"
        send to "rollback-{host}": cd /opt/myapp && ./rollback.sh {previous_version[host]}\n
        wait for idle, read screen, handle prompts
        verify service is healthy
        kill session "rollback-{host}"

    # Option B: Leave them on the new version (partial deploy)
    # Only do this if the new version is working correctly on
    # those hosts and the old/new versions are compatible

    # Step 3: Remaining hosts were never touched -- no action needed

    # Step 4: Verify fleet health
    run fleet-wide health check (see Health Verification)

### Rollback Decision Framework

Not every failure requires a full rollback. Consider:

- **One host failed, rest succeeded**: Investigate the failed
  host. It may be a host-specific issue (disk full, different
  OS version). Fix the host and retry, rather than rolling back
  the entire fleet.
- **Health checks fail after deploy**: Roll back the current
  batch. Don't proceed to the next batch. Investigate before
  deciding whether to roll back hosts that are already on the
  new version.
- **Multiple hosts fail the same way**: Likely a problem with
  the release itself. Full rollback, fix the release, try again.
- **Intermittent failures**: Some hosts fail health checks
  sometimes. This might be a pre-existing issue, not caused by
  the deploy. Compare with pre-deployment health baselines.

## Pitfalls

### Don't Parallelize Destructive Operations Blindly

Running `apt upgrade` on 50 hosts simultaneously is tempting
but dangerous. If the upgrade has an unexpected prompt or failure
mode, you'll have 50 hosts in an unknown state. Start sequential,
move to small batches once you've seen the operation succeed on
the first few hosts.

### Handle Prompt Diversity

The same command produces different prompts on different hosts
depending on installed packages, OS version, configuration, and
state. Don't assume that because host 1 had no prompts, host 2
won't either. Always read the screen and handle what's actually
there.

### Clean Up Sessions Aggressively

Infrastructure operations can create dozens of sessions --
deploy, health check, rollback, audit sessions for each host.
Kill sessions as soon as they've served their purpose. A session
is a running process on the backend machine. Leaving 50 orphaned
sessions across your fleet wastes resources and clutters the
session list.

### Don't Forget sudo

Many infrastructure operations require elevated privileges.
Handle `sudo` password prompts consistently. If you're working
across many hosts, consider whether `NOPASSWD` rules for
specific commands are appropriate to avoid interactive password
entry on every host.

### Record What You Do

Infrastructure changes are auditable events. For each operation,
record:
- What was done on which hosts
- What the state was before and after
- Which hosts succeeded and which failed
- What prompts were encountered and how they were answered

Read scrollback from each session before killing it. This is
your audit trail.

### Know When Ansible Is Better

wsh + AI is powerful for interactive, judgment-heavy operations.
But Ansible is better for:
- Fully predictable, idempotent operations at scale
- Operations that need to be reproduced exactly the same way
  every time (compliance, auditing)
- Simple fan-out where no per-host judgment is needed
- Infrastructure-as-code workflows where the desired state
  should be version-controlled

Use wsh for the 20% that requires interaction and judgment.
Use Ansible for the 80% that doesn't. They complement each
other.

### Test Your Rollback Before You Need It

The worst time to discover your rollback procedure doesn't
work is during a failed deployment at 2am. Test rollback on
a staging fleet first. Verify that the previous version's
artifacts exist, that the rollback script works, and that
the health checks pass after rolling back.

### Mind the Blast Radius

If you're deploying across 100 hosts, don't set a batch size
of 50. A failed batch of 50 means half your fleet is down.
Start small. Increase batch size only after the first few
batches succeed. A common progression: 1, 2, 5, 10, 25, 50.
