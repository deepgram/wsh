# Agent Input UX Improvements Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Improve agent UX for terminal input by replacing `wsh_run_command` with `wsh_send_and_read`, adding `wsh_send_keys` with typed union key arrays, adding diagnostics to `wsh_send_input`, and updating skill docs to use protocol-neutral notation.

**Architecture:** New `KeyAction` enum (serde tagged union) maps named keys to byte sequences. Both `wsh_send_keys` and `wsh_send_and_read` accept `Vec<KeyAction>` as their `keys` parameter. Key resolution happens locally; federation proxies raw bytes. `wsh_send_input` gains warning/preview fields in its response.

**Tech Stack:** Rust, serde, schemars, rmcp `#[tool]` macros

**Spec:** `docs/superpowers/specs/2026-03-14-agent-input-ux-design.md`

---

## Chunk 1: KeyAction type and key resolution

### Task 1: Add `KeyAction` enum and `resolve_keys` function to `src/mcp/tools.rs`

**Files:**
- Modify: `src/mcp/tools.rs`

- [ ] **Step 1: Write failing tests for `KeyAction` deserialization**

Add to the `#[cfg(test)]` module at the bottom of `src/mcp/tools.rs`:

```rust
// ── KeyAction ──────────────────────────────────────────────────

#[test]
fn key_action_text() {
    let json = serde_json::json!({"text": "hello"});
    let action: KeyAction = serde_json::from_value(json).unwrap();
    assert!(matches!(action, KeyAction::Text { text } if text == "hello"));
}

#[test]
fn key_action_key() {
    let json = serde_json::json!({"key": "enter"});
    let action: KeyAction = serde_json::from_value(json).unwrap();
    assert!(matches!(action, KeyAction::Key { key } if key == "enter"));
}

#[test]
fn key_action_invalid() {
    let json = serde_json::json!({"other": "x"});
    let result = serde_json::from_value::<KeyAction>(json);
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `nix develop -c sh -c "cargo test key_action_ --lib -p wsh 2>&1"`
Expected: FAIL — `KeyAction` not defined

- [ ] **Step 3: Implement `KeyAction` enum**

Add after the `Encoding` enum (around line 114) in `src/mcp/tools.rs`:

```rust
/// A single element in a key sequence: either literal text or a named key.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    /// Literal text to type (sent as UTF-8 bytes, no transformation).
    #[schemars(description = "Literal text to type. Sent as raw UTF-8 bytes.")]
    Text {
        text: String,
    },
    /// A named special key (e.g., \"enter\", \"ctrl+c\", \"up\").
    #[schemars(description = "A named special key. Examples: enter, tab, escape, backspace, delete, up, down, left, right, home, end, pageup, pagedown, ctrl+a through ctrl+z, f1 through f12.")]
    Key {
        key: String,
    },
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `nix develop -c sh -c "cargo test key_action_ --lib -p wsh 2>&1"`
Expected: PASS

- [ ] **Step 5: Write failing tests for `resolve_keys`**

```rust
#[test]
fn resolve_keys_text() {
    let keys = vec![KeyAction::Text { text: "hello".into() }];
    let bytes = resolve_keys(&keys).unwrap();
    assert_eq!(bytes, b"hello");
}

#[test]
fn resolve_keys_enter() {
    let keys = vec![KeyAction::Key { key: "enter".into() }];
    let bytes = resolve_keys(&keys).unwrap();
    assert_eq!(bytes, b"\n");
}

#[test]
fn resolve_keys_ctrl_c() {
    let keys = vec![KeyAction::Key { key: "ctrl+c".into() }];
    let bytes = resolve_keys(&keys).unwrap();
    assert_eq!(bytes, &[0x03]);
}

#[test]
fn resolve_keys_mixed_sequence() {
    let keys = vec![
        KeyAction::Text { text: "ls -la".into() },
        KeyAction::Key { key: "enter".into() },
    ];
    let bytes = resolve_keys(&keys).unwrap();
    assert_eq!(bytes, b"ls -la\n");
}

#[test]
fn resolve_keys_escape_sequence() {
    let keys = vec![KeyAction::Key { key: "up".into() }];
    let bytes = resolve_keys(&keys).unwrap();
    assert_eq!(bytes, b"\x1b[A");
}

#[test]
fn resolve_keys_case_insensitive() {
    let keys = vec![KeyAction::Key { key: "Ctrl+C".into() }];
    let bytes = resolve_keys(&keys).unwrap();
    assert_eq!(bytes, &[0x03]);
}

#[test]
fn resolve_keys_invalid_key_name() {
    let keys = vec![KeyAction::Key { key: "ctrl+1".into() }];
    let result = resolve_keys(&keys);
    assert!(result.is_err());
}

#[test]
fn resolve_keys_all_common_keys() {
    // Verify all documented named keys resolve without error
    let names = [
        "enter", "tab", "escape", "backspace", "delete",
        "up", "down", "left", "right",
        "home", "end", "pageup", "pagedown",
        "f1", "f2", "f3", "f4", "f5", "f6",
        "f7", "f8", "f9", "f10", "f11", "f12",
    ];
    for name in names {
        let keys = vec![KeyAction::Key { key: name.into() }];
        assert!(
            resolve_keys(&keys).is_ok(),
            "Key '{}' should resolve",
            name
        );
    }
}

#[test]
fn resolve_keys_all_ctrl_keys() {
    for c in b'a'..=b'z' {
        let name = format!("ctrl+{}", c as char);
        let keys = vec![KeyAction::Key { key: name.clone() }];
        let bytes = resolve_keys(&keys).unwrap();
        assert_eq!(
            bytes,
            &[c - b'a' + 1],
            "ctrl+{} should map to byte {}",
            c as char,
            c - b'a' + 1
        );
    }
}
```

- [ ] **Step 6: Run tests to verify they fail**

Run: `nix develop -c sh -c "cargo test resolve_keys_ --lib -p wsh 2>&1"`
Expected: FAIL — `resolve_keys` not defined

- [ ] **Step 7: Implement `resolve_keys`**

Add a public function in `src/mcp/tools.rs` (after the `KeyAction` enum):

```rust
/// Resolve a sequence of key actions into raw bytes.
///
/// Text elements are converted to UTF-8 bytes. Key elements are mapped
/// to their terminal byte sequences. Key names are case-insensitive.
/// Returns an error string for unrecognized key names.
pub fn resolve_keys(keys: &[KeyAction]) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    for action in keys {
        match action {
            KeyAction::Text { text } => buf.extend_from_slice(text.as_bytes()),
            KeyAction::Key { key } => {
                let bytes = resolve_named_key(key)?;
                buf.extend_from_slice(bytes);
            }
        }
    }
    Ok(buf)
}

/// Static lookup table for Ctrl+A (0x01) through Ctrl+Z (0x1a).
static CTRL_BYTES: [u8; 26] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13,
    14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
];

fn resolve_named_key(name: &str) -> Result<&'static [u8], String> {
    match name.to_ascii_lowercase().as_str() {
        // Common
        "enter" => Ok(b"\n"),
        "tab" => Ok(b"\t"),
        "escape" => Ok(b"\x1b"),
        "backspace" => Ok(b"\x7f"),
        "delete" => Ok(b"\x1b[3~"),
        // Arrows
        "up" => Ok(b"\x1b[A"),
        "down" => Ok(b"\x1b[B"),
        "right" => Ok(b"\x1b[C"),
        "left" => Ok(b"\x1b[D"),
        // Navigation
        "home" => Ok(b"\x1b[H"),
        "end" => Ok(b"\x1b[F"),
        "pageup" => Ok(b"\x1b[5~"),
        "pagedown" => Ok(b"\x1b[6~"),
        // Function keys
        "f1" => Ok(b"\x1bOP"),
        "f2" => Ok(b"\x1bOQ"),
        "f3" => Ok(b"\x1bOR"),
        "f4" => Ok(b"\x1bOS"),
        "f5" => Ok(b"\x1b[15~"),
        "f6" => Ok(b"\x1b[17~"),
        "f7" => Ok(b"\x1b[18~"),
        "f8" => Ok(b"\x1b[19~"),
        "f9" => Ok(b"\x1b[20~"),
        "f10" => Ok(b"\x1b[21~"),
        "f11" => Ok(b"\x1b[23~"),
        "f12" => Ok(b"\x1b[24~"),
        // Ctrl+letter
        other => {
            if let Some(letter) = other.strip_prefix("ctrl+") {
                if letter.len() == 1 {
                    let ch = letter.as_bytes()[0].to_ascii_lowercase();
                    if ch.is_ascii_lowercase() {
                        let idx = (ch - b'a') as usize;
                        return Ok(&CTRL_BYTES[idx..idx + 1]);
                    }
                }
            }
            Err(format!("unrecognized key name: '{}'", name))
        }
    }
}
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `nix develop -c sh -c "cargo test key_action_ --lib -p wsh 2>&1 && cargo test resolve_keys_ --lib -p wsh 2>&1"`
Expected: All PASS

- [ ] **Step 9: Commit**

```bash
git add src/mcp/tools.rs
git commit -m "feat(mcp): add KeyAction enum and resolve_keys for named key input"
```

### Task 2: Add `SendKeysParams` and `SendAndReadParams` to `src/mcp/tools.rs`

**Files:**
- Modify: `src/mcp/tools.rs`

- [ ] **Step 1: Write failing tests for the new param types**

Add to the `#[cfg(test)]` module:

```rust
// ── SendKeysParams ─────────────────────────────────────────────

#[test]
fn send_keys_params_basic() {
    let json = serde_json::json!({
        "session": "work",
        "keys": [
            {"text": "hello"},
            {"key": "enter"}
        ]
    });
    let params: SendKeysParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.session, "work");
    assert_eq!(params.keys.len(), 2);
}

#[test]
fn send_keys_params_missing_keys() {
    let json = serde_json::json!({"session": "s"});
    let result = serde_json::from_value::<SendKeysParams>(json);
    assert!(result.is_err());
}

// ── SendAndReadParams ──────────────────────────────────────────

#[test]
fn send_and_read_params_defaults() {
    let json = serde_json::json!({
        "session": "work",
        "keys": [{"text": "ls"}, {"key": "enter"}]
    });
    let params: SendAndReadParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.session, "work");
    assert_eq!(params.keys.len(), 2);
    assert_eq!(params.timeout_ms, 2000);
    assert_eq!(params.max_wait_ms, 30000);
    assert!(matches!(params.format, ScreenFormat::Styled));
}

#[test]
fn send_and_read_params_all_fields() {
    let json = serde_json::json!({
        "session": "s",
        "keys": [{"key": "ctrl+c"}],
        "timeout_ms": 1000,
        "max_wait_ms": 5000,
        "format": "plain"
    });
    let params: SendAndReadParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.timeout_ms, 1000);
    assert_eq!(params.max_wait_ms, 5000);
    assert!(matches!(params.format, ScreenFormat::Plain));
}

#[test]
fn send_and_read_params_missing_keys() {
    let json = serde_json::json!({"session": "s"});
    let result = serde_json::from_value::<SendAndReadParams>(json);
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `nix develop -c sh -c "cargo test send_keys_params --lib -p wsh 2>&1 && cargo test send_and_read_params --lib -p wsh 2>&1"`
Expected: FAIL — types not defined

- [ ] **Step 3: Implement the param types**

Replace `RunCommandParams` (lines 246-276) with both new types:

```rust
/// Parameters for the `wsh_send_keys` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SendKeysParams {
    /// The name of the target session.
    #[schemars(description = "The name of the target session.")]
    pub session: String,

    /// Sequence of key actions to send.
    #[schemars(description = "Array of key actions. Each element is either {\"text\": \"...\"} for literal characters or {\"key\": \"...\"} for named keys (enter, tab, escape, ctrl+c, up, down, etc.).")]
    pub keys: Vec<KeyAction>,

    /// Target a specific federated server by hostname. Omit for local.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Target a specific federated server by hostname. Omit to target the local server.")]
    pub server: Option<String>,
}

/// Parameters for the `wsh_send_and_read` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SendAndReadParams {
    /// The name of the target session.
    #[schemars(description = "The name of the target session.")]
    pub session: String,

    /// Sequence of key actions to send.
    #[schemars(description = "Array of key actions. Each element is either {\"text\": \"...\"} for literal characters or {\"key\": \"...\"} for named keys (enter, tab, escape, ctrl+c, up, down, etc.).")]
    pub keys: Vec<KeyAction>,

    /// Idle timeout in milliseconds. Defaults to 2000.
    #[serde(default = "default_timeout_ms")]
    #[schemars(description = "Idle timeout in milliseconds. Defaults to 2000.")]
    pub timeout_ms: u64,

    /// Maximum wall-clock time to wait in milliseconds. Defaults to 30000.
    #[serde(default = "default_max_wait_ms")]
    #[schemars(description = "Maximum wall-clock time to wait in milliseconds. Defaults to 30000.")]
    pub max_wait_ms: u64,

    /// Output format for the screen snapshot. Defaults to `styled`.
    #[serde(default)]
    #[schemars(description = "Output format: 'styled' (default) includes color/attribute spans, 'plain' returns raw text.")]
    pub format: ScreenFormat,

    /// Target a specific federated server by hostname. Omit for local.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Target a specific federated server by hostname. Omit to target the local server.")]
    pub server: Option<String>,
}
```

Also delete the old `RunCommandParams` tests (`run_command_params_defaults`, `run_command_params_all_fields`, `run_command_params_missing_input`).

Also update the `server_field_on_all_param_types` test (around line 1306) which constructs a `RunCommandParams`. Replace with `SendAndReadParams`:
```rust
    // SendAndReadParams
    let json = serde_json::json!({
        "session": "s",
        "keys": [{"text": "x"}],
        "server": "remote-host"
    });
    let params: SendAndReadParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.server.as_deref(), Some("remote-host"));
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `nix develop -c sh -c "cargo test --lib -p wsh 2>&1"`
Expected: All PASS (new tests pass, old `RunCommandParams` tests deleted)

- [ ] **Step 5: Commit**

```bash
git add src/mcp/tools.rs
git commit -m "feat(mcp): add SendKeysParams and SendAndReadParams, remove RunCommandParams"
```

## Chunk 2: Tool implementations in `src/mcp/mod.rs`

### Task 3: Replace `wsh_run_command` with `wsh_send_and_read` and add `wsh_send_keys`

**Files:**
- Modify: `src/mcp/mod.rs`

- [ ] **Step 1: Delete `wsh_run_command` method (lines 977-1094)**

Remove the entire `wsh_run_command` method including its `#[tool(...)]` attribute.

- [ ] **Step 2: Add `wsh_send_keys` method**

Add after `wsh_send_input` (after line 857):

```rust
    /// Send a sequence of named keys and literal text to a terminal session.
    #[tool(description = "Send keystrokes to a terminal session using named keys. Each element in the keys array is either {\"text\": \"...\"} for literal characters or {\"key\": \"...\"} for named special keys. Supported keys: enter, tab, escape, backspace, delete, up, down, left, right, home, end, pageup, pagedown, ctrl+a through ctrl+z, f1-f12. Key names are case-insensitive. Use 'server' to target a remote federated server.")]
    async fn wsh_send_keys(
        &self,
        Parameters(params): Parameters<tools::SendKeysParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = Bytes::from(
            tools::resolve_keys(&params.keys)
                .map_err(|e| ErrorData::invalid_params(e, None))?,
        );
        let len = data.len();

        // Federation: proxy raw bytes to remote.
        if let McpSessionTarget::Remote(backend) = self.resolve_server(params.server.as_deref())? {
            return proxy_post_bytes(
                &backend,
                &format!("/sessions/{}/input", params.session),
                data,
            ).await;
        }

        let session = self.get_session(&params.session)?;
        tokio::time::timeout(
            Duration::from_secs(5),
            session.input_tx.send(data),
        )
        .await
        .map_err(|_| ErrorData::internal_error("input send timed out", None))?
        .map_err(|e| {
            ErrorData::internal_error(format!("failed to send input: {e}"), None)
        })?;
        session.activity.touch();

        let result = serde_json::json!({
            "status": "sent",
            "bytes": len,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).unwrap_or_default(),
        )]))
    }
```

- [ ] **Step 3: Add `wsh_send_and_read` method**

Add after `wsh_send_keys`:

```rust
    /// Send keys, wait for idle, then return the screen.
    #[tool(description = "Send keystrokes to a terminal session, wait for output to settle, then return the screen contents. This is the primary send/wait/read primitive. Each element in the keys array is either {\"text\": \"...\"} for literal characters or {\"key\": \"...\"} for named special keys (enter, tab, escape, ctrl+c, up, down, etc.). If idle is not reached within max_wait_ms, the screen is still returned but marked as an error. Use 'server' to target a remote federated server.")]
    async fn wsh_send_and_read(
        &self,
        Parameters(params): Parameters<tools::SendAndReadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = Bytes::from(
            tools::resolve_keys(&params.keys)
                .map_err(|e| ErrorData::invalid_params(e, None))?,
        );

        // Federation: proxy to remote as three separate HTTP calls.
        if let McpSessionTarget::Remote(backend) = self.resolve_server(params.server.as_deref())? {
            // 1. Send input
            proxy_post_bytes(
                &backend,
                &format!("/sessions/{}/input", params.session),
                data,
            ).await?;

            // 2. Await idle
            let idle_path = format!(
                "/sessions/{}/idle?timeout_ms={}&max_wait_ms={}",
                params.session, params.timeout_ms, params.max_wait_ms,
            );
            let idle_result = proxy_get_long(&backend, &idle_path).await;

            // 3. Get screen
            let mut screen_path = format!("/sessions/{}/screen", params.session);
            if matches!(params.format, tools::ScreenFormat::Plain) {
                screen_path.push_str("?format=plain");
            }
            let screen_result = proxy_get(&backend, &screen_path).await?;

            match idle_result {
                Ok(_) => Ok(screen_result),
                Err(_) => {
                    let screen_text = screen_result
                        .content
                        .first()
                        .and_then(|c| c.as_text())
                        .map(|t| t.text.clone())
                        .unwrap_or_default();
                    let result = serde_json::json!({
                        "error": "idle timeout exceeded max_wait_ms",
                        "screen": serde_json::from_str::<serde_json::Value>(&screen_text).unwrap_or(serde_json::Value::String(screen_text)),
                    });
                    Ok(CallToolResult::error(vec![Content::text(
                        serde_json::to_string(&result).unwrap_or_default(),
                    )]))
                }
            }
        } else {
            // Local execution
            let session = self.get_session(&params.session)?;

            // 1. Send input
            tokio::time::timeout(
                Duration::from_secs(5),
                session.input_tx.send(data),
            )
            .await
            .map_err(|_| ErrorData::internal_error("input send timed out", None))?
            .map_err(|e| {
                ErrorData::internal_error(format!("failed to send input: {e}"), None)
            })?;
            // Note: no manual activity.touch() — PTY reader handles it.

            // 2. Await idle
            let timeout = Duration::from_millis(params.timeout_ms.min(MAX_WAIT_CEILING_MS));
            let max_wait = Duration::from_millis(params.max_wait_ms.min(MAX_WAIT_CEILING_MS));

            let idle_result = tokio::time::timeout(
                max_wait,
                session.activity.wait_for_idle(timeout, None),
            )
            .await;

            // 3. Get screen
            let format = params.format.into_parser_format();
            let screen = session
                .parser
                .query(Query::Screen { format })
                .await
                .map_err(|e| {
                    ErrorData::internal_error(format!("parser error: {e}"), None)
                })?;

            match idle_result {
                Ok(generation) => {
                    let result = serde_json::json!({
                        "screen": screen,
                        "generation": generation,
                    });
                    Ok(CallToolResult::success(vec![Content::text(
                        serde_json::to_string(&result).unwrap_or_default(),
                    )]))
                }
                Err(_) => {
                    let result = serde_json::json!({
                        "error": "idle timeout exceeded max_wait_ms",
                        "screen": screen,
                    });
                    Ok(CallToolResult::error(vec![Content::text(
                        serde_json::to_string(&result).unwrap_or_default(),
                    )]))
                }
            }
        }
    }
```

- [ ] **Step 4: Update `get_info()` instructions (line 325)**

Change:
```
"wsh exposes terminal sessions as an API. Use wsh_run_command for the common \
```
To:
```
"wsh exposes terminal sessions as an API. Use wsh_send_and_read for the common \
```

- [ ] **Step 5: Update tool count**

The tool count changes from 14 to 15 (removed `wsh_run_command`, added `wsh_send_keys` + `wsh_send_and_read`). Check if there are assertions on tool count in the code — there's one at `tests/mcp_http.rs:237` (`tools.len() >= 14`). This will still pass with 15. Update it to `>= 15` when we get to tests.

- [ ] **Step 6: Verify it compiles**

Run: `nix develop -c sh -c "cargo check 2>&1"`
Expected: PASS (may have warnings about unused imports if `RunCommandParams` was imported elsewhere — fix any)

- [ ] **Step 7: Commit**

```bash
git add src/mcp/mod.rs
git commit -m "feat(mcp): replace wsh_run_command with wsh_send_and_read and wsh_send_keys"
```

### Task 4: Add diagnostics to `wsh_send_input`

**Files:**
- Modify: `src/mcp/mod.rs`

- [ ] **Step 1: Add the `format_preview` helper function**

Add a private helper near the top of the `#[tool_router] impl` block or as a free function in `mod.rs`:

```rust
/// Format input bytes as a human-readable preview string.
/// Control characters are shown as named keys (e.g., <Enter>, <Ctrl+C>).
/// Printable ASCII and valid UTF-8 shown as-is. Max 80 chars, truncated with "...".
fn format_preview(data: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < data.len() {
        if out.len() > 80 {
            out.push_str("...");
            break;
        }
        let b = data[i];
        match b {
            // ESC followed by [ — CSI sequence
            0x1b if data.get(i + 1) == Some(&b'[') => {
                let start = i;
                i += 2; // skip ESC [
                while i < data.len() && !(0x40..=0x7e).contains(&data[i]) {
                    i += 1;
                }
                if i < data.len() {
                    let final_byte = data[i];
                    let param = &data[start + 2..i];
                    match final_byte {
                        b'A' if param.is_empty() => out.push_str("<Up>"),
                        b'B' if param.is_empty() => out.push_str("<Down>"),
                        b'C' if param.is_empty() => out.push_str("<Right>"),
                        b'D' if param.is_empty() => out.push_str("<Left>"),
                        b'H' if param.is_empty() => out.push_str("<Home>"),
                        b'F' if param.is_empty() => out.push_str("<End>"),
                        b'~' => match param {
                            b"3" => out.push_str("<Delete>"),
                            b"5" => out.push_str("<PageUp>"),
                            b"6" => out.push_str("<PageDown>"),
                            _ => out.push_str("<CSI>"),
                        },
                        _ => out.push_str("<CSI>"),
                    }
                    i += 1;
                    continue;
                }
                // Incomplete CSI — treat as bare Escape
                out.push_str("<Escape>");
                i = start + 1;
                continue;
            }
            // ESC followed by O — SS3 sequence (function keys F1-F4)
            0x1b if data.get(i + 1) == Some(&b'O') => {
                if let Some(&final_byte) = data.get(i + 2) {
                    match final_byte {
                        b'P' => out.push_str("<F1>"),
                        b'Q' => out.push_str("<F2>"),
                        b'R' => out.push_str("<F3>"),
                        b'S' => out.push_str("<F4>"),
                        _ => out.push_str("<Escape>"),
                    }
                    i += 3;
                    continue;
                }
                out.push_str("<Escape>");
            }
            // Bare ESC
            0x1b => out.push_str("<Escape>"),
            // Control characters (0x01-0x1a, excluding those handled above)
            0x09 => out.push_str("<Tab>"),
            0x0a => out.push_str("<Enter>"),
            0x0d => out.push_str("<CR>"),
            0x01..=0x1a => {
                let ch = (b'A' + b - 1) as char;
                out.push_str(&format!("<Ctrl+{}>", ch));
            }
            0x7f => out.push_str("<Backspace>"),
            0x20..=0x7e => out.push(b as char),
            _ => {
                // Try to decode as UTF-8
                if let Ok(s) = std::str::from_utf8(&data[i..]) {
                    if let Some(ch) = s.chars().next() {
                        out.push(ch);
                        i += ch.len_utf8();
                        continue;
                    }
                }
                out.push_str(&format!("<0x{:02x}>", b));
            }
        }
        i += 1;
    }
    out
}
```

- [ ] **Step 2: Add the `detect_double_escape` helper function**

```rust
/// Check if input bytes contain patterns that suggest double-escaping.
/// Returns a warning message if suspicious patterns are found, None otherwise.
fn detect_double_escape(data: &[u8]) -> Option<String> {
    // Look for literal backslash followed by common escape letters
    let patterns: &[(&[u8], &str)] = &[
        (b"\\n", "\\n"),
        (b"\\t", "\\t"),
        (b"\\r", "\\r"),
        (b"\\x", "\\x.."),
        (b"\\u00", "\\u00.."),
    ];

    for (pattern, display) in patterns {
        if data.windows(pattern.len()).any(|w| w == *pattern) {
            return Some(format!(
                "Input contains literal backslash sequences (e.g., '{}'). \
                 These were sent as-is. If you intended control characters, \
                 use base64 encoding or wsh_send_keys.",
                display
            ));
        }
    }
    None
}
```

- [ ] **Step 3: Update `wsh_send_input` response to include diagnostics**

In the `wsh_send_input` method, replace the response construction block (around lines 850-856) with:

```rust
        let preview = format_preview(&data_ref);
        let mut result = serde_json::json!({
            "status": "sent",
            "bytes": len,
            "preview": preview,
        });

        // Warn on empty input
        if len == 0 {
            result["warning"] = serde_json::json!(
                "Input was empty (0 bytes). If you intended a control character, \
                 use base64 encoding or wsh_send_keys."
            );
        } else if let Some(warning) = detect_double_escape(&data_ref) {
            result["warning"] = serde_json::json!(warning);
        }
```

Note: you'll need to capture the data bytes before sending. Save a reference before the `send()`:

```rust
        let data_ref = data.clone();
        let len = data.len();
        // ... send data ...
        // ... build response using data_ref ...
```

For the federation proxy path (lines 796-815), compute diagnostics *before* proxying and return them instead of the proxy response. Replace the federation branch:

```rust
        if let McpSessionTarget::Remote(backend) = self.resolve_server(params.server.as_deref())? {
            let data = match params.encoding {
                Encoding::Utf8 => Bytes::from(params.input.into_bytes()),
                Encoding::Base64 => {
                    use base64::Engine;
                    let decoded = base64::engine::general_purpose::STANDARD
                        .decode(&params.input)
                        .map_err(|e| {
                            ErrorData::invalid_params(format!("invalid base64 input: {e}"), None)
                        })?;
                    Bytes::from(decoded)
                }
            };
            let data_ref = data.clone();
            let len = data.len();
            proxy_post_bytes(
                &backend,
                &format!("/sessions/{}/input", params.session),
                data,
            ).await?;

            // Return local diagnostics instead of proxy's generic response
            let preview = format_preview(&data_ref);
            let mut result = serde_json::json!({
                "status": "sent",
                "bytes": len,
                "preview": preview,
            });
            if len == 0 {
                result["warning"] = serde_json::json!(
                    "Input was empty (0 bytes). If you intended a control character, \
                     use base64 encoding or wsh_send_keys."
                );
            } else if let Some(warning) = detect_double_escape(&data_ref) {
                result["warning"] = serde_json::json!(warning);
            }
            return Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&result).unwrap_or_default(),
            )]));
        }
```

- [ ] **Step 4: Verify it compiles**

Run: `nix develop -c sh -c "cargo check 2>&1"`
Expected: PASS

- [ ] **Step 5: Write unit tests for `format_preview` and `detect_double_escape`**

Add to a `#[cfg(test)]` module in `src/mcp/mod.rs` (or a new test submodule):

```rust
#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    #[test]
    fn preview_plain_text() {
        assert_eq!(format_preview(b"hello"), "hello");
    }

    #[test]
    fn preview_with_enter() {
        assert_eq!(format_preview(b"ls -la\n"), "ls -la<Enter>");
    }

    #[test]
    fn preview_ctrl_c() {
        assert_eq!(format_preview(&[0x03]), "<Ctrl+C>");
    }

    #[test]
    fn preview_arrow_up() {
        assert_eq!(format_preview(b"\x1b[A"), "<Up>");
    }

    #[test]
    fn preview_escape_alone() {
        assert_eq!(format_preview(b"\x1b"), "<Escape>");
    }

    #[test]
    fn preview_mixed() {
        assert_eq!(
            format_preview(b"echo hi\n"),
            "echo hi<Enter>"
        );
    }

    #[test]
    fn detect_literal_backslash_n() {
        let data = b"hello\\n";
        assert!(detect_double_escape(data).is_some());
    }

    #[test]
    fn detect_literal_backslash_u00() {
        let data = b"\\u0003";
        assert!(detect_double_escape(data).is_some());
    }

    #[test]
    fn no_false_positive_real_newline() {
        let data = b"hello\n";
        assert!(detect_double_escape(data).is_none());
    }

    #[test]
    fn no_false_positive_plain_text() {
        let data = b"just text";
        assert!(detect_double_escape(data).is_none());
    }
}
```

- [ ] **Step 6: Run all unit tests**

Run: `nix develop -c sh -c "cargo test --lib -p wsh 2>&1"`
Expected: All PASS

- [ ] **Step 7: Commit**

```bash
git add src/mcp/mod.rs
git commit -m "feat(mcp): add diagnostics (preview, warnings) to wsh_send_input"
```

## Chunk 3: Update tests

### Task 5: Update integration tests

**Files:**
- Modify: `tests/mcp_http.rs`
- Modify: `tests/mcp_stdio.rs`
- Modify: `src/mcp/prompts.rs` (test module)

- [ ] **Step 1: Update `tests/mcp_http.rs`**

Four changes:

1. **Line 190-191:** Change `wsh_run_command` → `wsh_send_and_read` in instruction assertion
2. **Line 237:** Change `tools.len() >= 14` → `tools.len() >= 15`
3. **Lines 252-254:** Change `wsh_run_command` → `wsh_send_and_read` in tool list assertion. Also add assertion for `wsh_send_keys`:
   ```rust
   assert!(
       tool_names.contains(&"wsh_send_and_read"),
       "Missing wsh_send_and_read tool"
   );
   assert!(
       tool_names.contains(&"wsh_send_keys"),
       "Missing wsh_send_keys tool"
   );
   ```
4. **Lines 722-724:** Change `wsh_run_command` → `wsh_send_and_read` in prompt content assertion
5. **Lines 1017-1071:** Rewrite the `test_mcp_tool_run_command` test as `test_mcp_tool_send_and_read`:
   ```rust
   // ── Test 14: wsh_send_and_read (core agent loop) ──────────────────

   #[tokio::test]
   async fn test_mcp_tool_send_and_read() {
       let app = create_test_app();
       let addr = start_test_server(app).await;
       let client = reqwest::Client::new();
       let mcp_session = setup_mcp_session(&client, addr).await;

       let sess_name = "mcp-sendread-test";

       // Create session
       let json = call_tool(
           &client,
           addr,
           &mcp_session,
           "wsh_create_session",
           serde_json::json!({"name": sess_name}),
       )
       .await;
       assert_not_error(&json);

       // Give the shell a moment to start
       tokio::time::sleep(Duration::from_millis(500)).await;

       // Send and read using named keys
       let json = call_tool(
           &client,
           addr,
           &mcp_session,
           "wsh_send_and_read",
           serde_json::json!({
               "session": sess_name,
               "keys": [
                   {"text": "echo hello_wsh_test"},
                   {"key": "enter"}
               ],
               "timeout_ms": 2000,
               "max_wait_ms": 15000,
               "format": "plain",
           }),
       )
       .await;

       let text = extract_tool_text(&json);
       let result: serde_json::Value = serde_json::from_str(text).unwrap();

       assert!(
           result.get("screen").is_some(),
           "send_and_read response should contain 'screen' field, got: {}",
           result
       );

       // Cleanup
       cleanup_session(&client, addr, &mcp_session, sess_name).await;
   }
   ```

- [ ] **Step 2: Update `tests/mcp_stdio.rs`**

1. **Lines 234-236:** Change `wsh_run_command` → `wsh_send_and_read`

- [ ] **Step 3: Update `src/mcp/prompts.rs` test**

1. **Lines 159-161:** Change both the assertion and error message:
   ```rust
   assert!(
       text.contains("wsh_send_and_read"),
       "MCP-adapted core skill should reference wsh_send_and_read"
   );
   ```

- [ ] **Step 4: Run all tests**

Run: `nix develop -c sh -c "cargo test 2>&1"`
Expected: Will fail because skill docs still reference `wsh_run_command`. That's OK — we'll fix them next.

- [ ] **Step 5: Commit**

```bash
git add tests/mcp_http.rs tests/mcp_stdio.rs src/mcp/prompts.rs
git commit -m "test(mcp): update tests for wsh_send_and_read rename and wsh_send_keys"
```

## Chunk 4: Skill doc updates

### Task 6: Update `skills/core-mcp/SKILL.md`

**Files:**
- Modify: `skills/core-mcp/SKILL.md`

- [ ] **Step 1: Update frontmatter (lines 3-8)**

Change `wsh_run_command` → `wsh_send_and_read` and add `wsh_send_keys` to the description. The updated description line should read:
```
  wsh_send_input, wsh_get_screen, wsh_send_and_read, wsh_send_keys, and all wsh_* tools.
```

- [ ] **Step 2: Update Getting Started section (lines 52-58)**

Replace the `wsh_run_command` example:
```
    wsh_send_and_read(session="work", keys=[{"text": "ls -la"}, {"key": "enter"}], format="plain")
```

- [ ] **Step 3: Replace "Run a Command" section (lines 85-102)**

Replace with two sections:

```markdown
### Send and Read (Send + Wait + Read)
The primary tool for the send/wait/read loop. Sends keystrokes, waits
for idle, then returns the screen contents.

Use `wsh_send_and_read` with:
- `session` — target session name (e.g., `"default"`)
- `keys` — array of key actions (see wsh_send_keys below)
- `timeout_ms` — idle timeout (default 2000)
- `max_wait_ms` — maximum wall-clock wait (default 30000)
- `format` — `"plain"` or `"styled"` (default `"styled"`)

Example: run `ls -la` and read the result:

    wsh_send_and_read(session="default", keys=[{"text": "ls -la"}, {"key": "enter"}], format="plain")

Returns the screen contents plus a `generation` counter. If the
terminal doesn't settle within `max_wait_ms`, the screen is still
returned but flagged as an error.

### Send Keys
Inject keystrokes into the terminal using named keys. No encoding
to get wrong — use key names instead of escape sequences.

Use `wsh_send_keys` with:
- `session` — target session name
- `keys` — array of key actions

Each element in `keys` is either:
- `{"text": "..."}` — literal characters to type
- `{"key": "..."}` — a named special key

**Named keys:** `enter`, `tab`, `escape`, `backspace`, `delete`,
`up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`,
`ctrl+a` through `ctrl+z`, `f1`-`f12`. Case-insensitive.

Examples:

    wsh_send_keys(session="default", keys=[{"text": "ls -la"}, {"key": "enter"}])
    wsh_send_keys(session="default", keys=[{"key": "ctrl+c"}])
    wsh_send_keys(session="default", keys=[{"key": "escape"}, {"text": ":wq"}, {"key": "enter"}])

Returns `{"status": "sent", "bytes": N}` on success.
```

- [ ] **Step 4: Update Send Input section (lines 104-139)**

Demote the JSON escape table to a reference note. Replace the section opening with:

```markdown
### Send Input (Low-Level)
Raw byte injection for advanced use. Prefer `wsh_send_keys` for
most input — it handles encoding automatically.

Use `wsh_send_input` with:
- `session` — target session name
- `input` — the text or data to send (JSON string encoding)
- `encoding` — `"utf8"` (default) or `"base64"`

Returns `{"status": "sent", "bytes": N, "preview": "..."}`.
Includes a `warning` field if the input looks empty or
double-escaped.

**Base64 encoding** bypasses any MCP transport issues with control
characters:
- `wsh_send_input(session="default", input="Aw==", encoding="base64")` — Ctrl+C
- `wsh_send_input(session="default", input="Cg==", encoding="base64")` — Enter

<details>
<summary>JSON escape reference (for utf8 encoding)</summary>

| Key         | JSON escape  | Example                                    |
|-------------|--------------|--------------------------------------------|
| Enter       | `\n`         | `input="ls -la\n"`                         |
| Tab         | `\t`         | `input="\t"`                               |
| Ctrl+C      | `\u0003`     | `input="\u0003"`                           |
| Ctrl+D      | `\u0004`     | `input="\u0004"`                           |
| Escape      | `\u001b`     | `input="\u001b"`                           |

Any Ctrl+key = `\u00XX` where XX is the ASCII code (A=01, B=02, ..., Z=1a).
</details>
```

- [ ] **Step 5: Verify the skill file compiles into prompts**

Run: `nix develop -c sh -c "cargo test get_prompt_core --lib -p wsh 2>&1"`
Expected: Will fail because assertion now checks for `wsh_send_and_read` — verify the updated skill content contains the string. If it does, PASS.

- [ ] **Step 6: Commit**

```bash
git add skills/core-mcp/SKILL.md
git commit -m "docs(skills): update core-mcp for wsh_send_and_read and wsh_send_keys"
```

### Task 7: Update `skills/drive-process/SKILL.md`

**Files:**
- Modify: `skills/drive-process/SKILL.md`

- [ ] **Step 1: Replace Control Characters section (lines 92-108)**

Replace with:

```markdown
## Control Characters

Emergency exits and special actions: Ctrl+C (interrupt), Ctrl+D
(EOF/exit), Ctrl+Z (suspend), Ctrl+L (clear screen), Ctrl+U
(clear line), Escape.

If a command hangs, try Ctrl+C first. If unresponsive, Ctrl+Z
to suspend then `kill %1`.
```

- [ ] **Step 2: Update "Knowing when to give up" section (lines 249-255)**

Replace:
```
1. Send Ctrl+C (`$'\x03'`)
2. Wait a moment, try Ctrl+C again
3. Send Ctrl+Z (`$'\x1a'`) to suspend, then `kill %1`
```
With:
```
1. Send Ctrl+C
2. Wait a moment, try Ctrl+C again
3. Send Ctrl+Z to suspend, then `kill %1`
```

- [ ] **Step 3: Commit**

```bash
git add skills/drive-process/SKILL.md
git commit -m "docs(skills): replace bash escape notation with protocol-neutral key names"
```

### Task 8: Update `skills/tui/SKILL.md`

**Files:**
- Modify: `skills/tui/SKILL.md`

- [ ] **Step 1: Replace Universal Navigation Keys section (lines 70-82)**

Replace:
```markdown
### Universal Navigation Keys

    $'\x1b[A'       # Arrow Up
    ...
    $'\x1b'         # Escape (cancel / back)
```
With:
```markdown
### Universal Navigation Keys

Arrow keys, Page Up/Down, Home/End, Tab (cycle panes), Enter
(confirm/open), Escape (cancel/back).
```

- [ ] **Step 2: Update F1 reference (line 183)**

Replace `$'\x1bOP'` with just `F1`:
```
3. Try F1 — some use function keys for help
```

- [ ] **Step 3: Commit**

```bash
git add skills/tui/SKILL.md
git commit -m "docs(skills): replace escape sequences with protocol-neutral key names in TUI skill"
```

## Chunk 5: Full test pass and cleanup

### Task 9: Run full test suite and fix any remaining issues

**Files:**
- Possibly any of the above

- [ ] **Step 1: Run full test suite**

Run: `nix develop -c sh -c "cargo test 2>&1"`
Expected: All PASS. If any fail, fix them.

- [ ] **Step 2: Run clippy**

Run: `nix develop -c sh -c "cargo clippy --all-targets 2>&1"`
Expected: No errors. Fix any warnings related to our changes.

- [ ] **Step 3: Verify tool count**

The MCP server should now expose 15 tools (was 14: removed `wsh_run_command`, added `wsh_send_keys` + `wsh_send_and_read`).

- [ ] **Step 4: Final commit if any fixes were needed**

Stage only files modified in this plan (do NOT use `git add -A` — the repo has untracked files that should not be committed):
```bash
git add src/mcp/mod.rs src/mcp/tools.rs src/mcp/prompts.rs tests/mcp_http.rs tests/mcp_stdio.rs skills/
git commit -m "fix: address remaining issues from agent input UX changes"
```
