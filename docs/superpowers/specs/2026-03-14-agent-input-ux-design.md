# Agent Input UX Improvements

**Date:** 2026-03-14

## Problem

An agent driving a TUI via MCP tools spent 5+ minutes and 3 sessions
debugging input encoding. Three distinct failure modes were observed:

1. **Double-escaping** — agent sent `"\\n"` (literal backslash-n) instead
   of `"\n"` (newline). The `bytes` field in the response revealed this
   (2 bytes instead of 1), but the agent didn't notice.

2. **MCP transport stripping control characters** — agent sent Ctrl+U
   via UTF-8 mode, received `bytes: 0`. Something upstream dropped the
   non-printable byte. Base64 encoding worked as a workaround.

3. **Misleading tool name** — `wsh_run_command` implies command
   execution. The agent expected it to press Enter, handle shell
   semantics, etc. It actually just sends bytes, waits for idle, and
   reads the screen.

None are bugs in wsh. But the tool surface makes all three easy to
hit and hard to diagnose.

## Changes

### 1. Replace `wsh_run_command` with `wsh_send_and_read`

Delete `wsh_run_command`. Add `wsh_send_and_read` with identical
send/wait-for-idle/read-screen behavior but two changes:

**New name.** "Send and read" accurately describes the three-step
operation and works in every context (shell, TUI, agent orchestration).
"Run command" implied command execution, leading agents to expect
auto-Enter behavior and shell semantics.

**New `keys` parameter** (replaces `input`). A typed union array where
each element is either `{"text": "..."}` (literal characters) or
`{"key": "..."}` (named special key):

```json
wsh_send_and_read(session="work", keys=[
  {"text": "ls -la"},
  {"key": "enter"}
], format="plain")
```

```json
wsh_send_and_read(session="work", keys=[
  {"key": "ctrl+c"}
], timeout_ms=1000)
```

Remaining parameters unchanged: `session`, `timeout_ms` (default 2000),
`max_wait_ms` (default 30000), `format` (default styled), `server`.

Response format unchanged: `{"screen": {...}, "generation": N}` on
success, `{"error": "...", "screen": {...}}` on idle timeout.

No deprecation alias. Clean break.

**Federation:** Keys are resolved to bytes locally before proxy
dispatch, same as `wsh_send_keys`.

### 2. Add `wsh_send_keys`

New tool using the same typed union array as `wsh_send_and_read`.
Send-only — no wait, no screen read.

```json
wsh_send_keys(session="work", keys=[
  {"text": "hello"},
  {"key": "enter"}
])
```

```json
wsh_send_keys(session="work", keys=[
  {"key": "escape"},
  {"text": ":wq"},
  {"key": "enter"}
])
```

Parameters: `session`, `keys`, `server`.

Response: `{"status": "sent", "bytes": N}`.

**Supported named keys:**

| Category   | Keys                                                       |
|------------|------------------------------------------------------------|
| Common     | `enter`, `tab`, `escape`, `backspace`, `delete`            |
| Arrows     | `up`, `down`, `left`, `right`                              |
| Navigation | `home`, `end`, `pageup`, `pagedown`                        |
| Modifiers  | `ctrl+a` through `ctrl+z`                                  |
| Function   | `f1` through `f12`                                         |

Key names are case-insensitive (`ctrl+c` and `Ctrl+C` are equivalent).
Invalid key names (e.g., `"ctrl+1"`, `"f13"`) return an
`invalid_params` error listing the unrecognized key.

Implementation: pure mapping from key names to byte sequences, then
`input_tx.send()`. No new protocol.

**Federation:** Keys are resolved to bytes locally before proxy
dispatch. The remote server receives raw bytes via the existing
`POST /sessions/:name/input` endpoint, same as `wsh_send_input`.

### 3. `wsh_send_input` diagnostics

`wsh_send_input` remains as the low-level raw-bytes tool. Three
additions to its response:

**3a. Warn on empty input.** When `bytes: 0`:

```json
{
  "status": "sent",
  "bytes": 0,
  "warning": "Input was empty (0 bytes). If you intended a control character, use base64 encoding or wsh_send_keys."
}
```

**3b. Warn on likely double-escaping.** Scan input bytes for literal
backslash sequences: `\n` (0x5c 0x6e), `\t` (0x5c 0x74),
`\x` (0x5c 0x78), `\u00` (0x5c 0x75 0x30 0x30):

```json
{
  "status": "sent",
  "bytes": 6,
  "warning": "Input contains literal backslash sequences (e.g., '\\u0015'). These were sent as-is. If you intended control characters, use base64 encoding or wsh_send_keys."
}
```

**3c. Decoded preview.** Human-readable representation of what was sent:

```json
{
  "status": "sent",
  "bytes": 7,
  "preview": "ls -la<Enter>"
}
```

Control characters use readable names: `<Enter>`, `<Tab>`, `<Ctrl+C>`,
`<Escape>`, `<Up>`, `<Down>`, etc. Printable characters shown as-is.

**Federation:** Diagnostics are generated locally before proxy dispatch.
The warning and preview fields are computed from the input bytes on the
hub; the remote server receives raw bytes and is unaware of diagnostics.

### 4. Skill doc: `drive-process/SKILL.md`

Replace the Control Characters section (lines 92-108) and inline
references with compact prose. No byte values, no encoding
instructions — the execution context block at the top already directs
agents to their access method.

**Before:**
```
## Control Characters

These are your emergency exits and special actions:

    $'\x03'         # Ctrl+C  — interrupt / cancel
    $'\x04'         # Ctrl+D  — EOF / exit shell
    $'\x1a'         # Ctrl+Z  — suspend process
    $'\x0c'         # Ctrl+L  — clear screen
    $'\x01'         # Ctrl+A  — beginning of line
    $'\x05'         # Ctrl+E  — end of line
    $'\x15'         # Ctrl+U  — clear line
    $'\x1b'         # Escape
```

**After:**
```
## Control Characters

Emergency exits and special actions: Ctrl+C (interrupt), Ctrl+D
(EOF/exit), Ctrl+Z (suspend), Ctrl+L (clear screen), Ctrl+U
(clear line), Escape.

If a command hangs, try Ctrl+C first. If unresponsive, Ctrl+Z
to suspend then `kill %1`.
```

Also update "Knowing when to give up" section (line 250-255) to
remove `$'\x03'` / `$'\x1a'` inline references — replace with
`Ctrl+C` / `Ctrl+Z`.

### 5. Skill doc: `tui/SKILL.md`

Replace the Universal Navigation Keys section (lines 72-82) with
compact prose:

**Before:**
```
### Universal Navigation Keys

    $'\x1b[A'       # Arrow Up
    $'\x1b[B'       # Arrow Down
    $'\x1b[C'       # Arrow Right
    $'\x1b[D'       # Arrow Left
    $'\x1b[5~'      # Page Up
    $'\x1b[6~'      # Page Down
    $'\x1b[H'       # Home
    $'\x1b[F'       # End
    $'\t'           # Tab (often cycles panes or fields)
    $'\n'           # Enter (confirm / open)
    $'\x1b'         # Escape (cancel / back)
```

**After:**
```
### Universal Navigation Keys

Arrow keys, Page Up/Down, Home/End, Tab (cycle panes), Enter
(confirm/open), Escape (cancel/back).
```

### 6. Skill doc: `core-mcp/SKILL.md`

- Replace `wsh_run_command` section with `wsh_send_and_read` using
  `keys` array examples
- Add `wsh_send_keys` section with examples
- Demote the JSON escape control character table to a reference
  footnote — `wsh_send_keys` is now the primary path for special keys
- Add note that base64 bypasses MCP transport issues with control
  characters

## Files Changed

| File | Type |
|------|------|
| `src/mcp/mod.rs` | Delete `wsh_run_command`, add `wsh_send_and_read` and `wsh_send_keys`, add diagnostics to `wsh_send_input`, update `instructions` string in `get_info()` to reference new tool names |
| `src/mcp/tools.rs` | Delete `RunCommandParams`, add `SendAndReadParams` and `SendKeysParams` with `KeyAction` enum, add preview/warning fields to send_input response |
| `src/mcp/prompts.rs` | Update `#[cfg(test)]` assertion that checks core skill content for `wsh_run_command` |
| `skills/drive-process/SKILL.md` | Replace control character section |
| `skills/tui/SKILL.md` | Replace navigation keys section |
| `skills/core-mcp/SKILL.md` | Update for new tools, demote JSON escape table |
| Tests referencing `wsh_run_command` | Update to `wsh_send_and_read` (both `tests/` integration tests and internal `#[cfg(test)]` modules) |
