# Lifecycle Stress Tests Design

**Date**: 2026-02-13
**Problem**: Intermittent client hangs on exit and zombie `wsh` processes after session termination.

## Architecture

### Test Harness (`WshTestHarness`)

Manages a `wsh server --ephemeral` process with a unique socket + HTTP port per test.

- `new()` — spawns server, waits for health endpoint
- `spawn_client(name)` — spawns `wsh --socket <path> --name <name>` inside a real PTY via `portable-pty`
- `spawn_attach(name)` — spawns `wsh attach <name> --socket <path>` inside a PTY
- `create_overlay(session, ...)` / `delete_overlay(session, id)` — via HTTP
- `detach_remote(session)` — spawns `wsh detach <name> --socket <path>`
- `kill_session(session)` — via HTTP DELETE
- `list_sessions()` — via HTTP GET
- `shutdown()` — kill server, assert clean exit
- Drop impl kills server + all clients, removes socket

### PTY-Wrapped Client (`WshClient`)

Each client runs inside a real PTY (via `portable-pty`), exercising the full terminal code path: raw mode, poll()-based stdin, SIGWINCH.

- `send(bytes)` — write raw bytes to PTY master
- `send_line(text)` — write text + `\r`
- `send_ctrl_d()` / `send_ctrl_c()` — send control characters
- `send_ctrl_backslash()` — send `0x1c`
- `detach()` — double Ctrl+\ with 100ms gap
- `read_until(pattern, timeout)` — read PTY output until pattern appears or timeout
- `read_available(timeout)` — drain available output
- `wait_exit(timeout)` — wait for process to exit
- `is_alive()` — check if process is still running

### Alt Screen via Raw ANSI

Enter: `printf '\x1b[?1049h\n'` sent as a shell command.
Leave: `printf '\x1b[?1049l\n'` sent as a shell command.

This is fully portable (no ncurses/tput dependency).

## Test Scenarios

All scenarios: #[ignore], run with `cargo test -- --ignored`.
All scenarios assert: client exits within 5s, server exits within 10s, no orphan processes.

### Scenario 1: Extended session with detach cycles
Create → echo a → date → ls /tmp → detach (Ctrl+\ ×2) → reattach → echo b → echo c → detach → reattach → echo d → Ctrl+D → assert.

### Scenario 2: Alt screen interleaved with commands
Create → echo start → alt screen on → echo inside-alt → alt screen off → echo back → detach → reattach → alt screen on → alt screen off → echo final → Ctrl+D → assert.

### Scenario 3: Overlay + detach + commands
Create → echo x → create overlay (HTTP) → echo y → detach → reattach → echo z → delete overlay → echo w → Ctrl+D → assert.

### Scenario 4: Remote detach + multiple reattach
Create → echo one → remote detach (wsh detach) → reattach → echo two → remote detach → reattach → echo three → Ctrl+D → assert.

### Scenario 5: Kitchen sink
Create → echo a → alt screen on → echo b → alt screen off → create overlay → detach → reattach → echo c → delete overlay → remote detach → reattach → alt screen on → alt screen off → echo d → echo e → Ctrl+D → assert.

### Scenario 6: Random walk stress test
Actions pool: {run command, detach+reattach, enter/leave alt screen, create/delete overlay, resize}.
20-50 iterations, random delays 10-200ms.
End with Ctrl+D. Log RNG seed + full action trace on failure.

## Process Assertions

After each scenario:
1. `client.wait_exit(5s)` — panics if client hangs (catches bug #1)
2. `harness.shutdown()` — waits for server exit
3. Verify no `wsh` processes with our socket path remain (catches bug #2)

## File Structure

Single test file: `tests/lifecycle_stress.rs`
- Harness structs at top
- Scripted scenarios as individual `#[tokio::test] #[ignore]` functions
- Random walk as a separate `#[tokio::test] #[ignore]` function
