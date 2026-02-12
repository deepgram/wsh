# Error Responses

All wsh API errors return JSON with a consistent structure:

```json
{
  "error": {
    "code": "machine_readable_code",
    "message": "Human-readable description of what went wrong."
  }
}
```

The `code` field is a stable, machine-readable identifier suitable for
programmatic error handling. The `message` field is human-readable and may
change between versions.

## Error Codes

### Authentication Errors

| Status | Code | Message | When |
|--------|------|---------|------|
| `401` | `auth_required` | Authentication required. Provide a token via Authorization header or ?token= query parameter. | No credentials provided on a protected endpoint |
| `403` | `auth_invalid` | Invalid authentication token. | Credentials provided but don't match |

### Not Found Errors

| Status | Code | Message | When |
|--------|------|---------|------|
| `404` | `not_found` | Not found. | Generic resource not found |
| `404` | `overlay_not_found` | No overlay exists with id '{id}'. | Overlay ID doesn't exist |
| `404` | `panel_not_found` | No panel exists with id '{id}'. | Panel ID doesn't exist |
| `404` | `session_not_found` | Session not found: {name}. | Session name doesn't exist |

### Validation Errors

| Status | Code | Message | When |
|--------|------|---------|------|
| `400` | `invalid_request` | Invalid request: {detail}. | Malformed request body or parameters |
| `400` | `invalid_overlay` | Invalid overlay: {detail}. | Invalid overlay specification |
| `400` | `invalid_input_mode` | Invalid input mode: {detail}. | Invalid input mode value |
| `400` | `invalid_format` | Invalid format: {detail}. | Invalid format query parameter |
| --- | `unknown_method` | Unknown method '{method}'. | WebSocket method name not recognized |

### Conflict Errors

| Status | Code | Message | When |
|--------|------|---------|------|
| `409` | `session_name_conflict` | Session name already exists: {name}. | Session name already in use |

### Not Found Errors (Sessions)

| Status | Code | Message | When |
|--------|------|---------|------|
| `404` | `no_sessions` | No sessions exist. | Server-level `GET /quiesce` called with no sessions in the registry |

### Timeout Errors

| Status | Code | Message | When |
|--------|------|---------|------|
| `408` | `quiesce_timeout` | Terminal did not become quiescent within the deadline. | `max_wait_ms` exceeded on `GET /quiesce` or `await_quiesce` WS method |

### Server Errors

| Status | Code | Message | When |
|--------|------|---------|------|
| `503` | `channel_full` | Server is overloaded. Try again shortly. | Internal channel backpressure |
| `503` | `parser_unavailable` | Terminal parser is unavailable. | Parser actor is down or unreachable |
| `500` | `input_send_failed` | Failed to send input to terminal. | PTY input channel is broken |
| `500` | `session_create_failed` | Failed to create session: {detail}. | PTY spawn or session creation error |
| `500` | `internal_error` | Internal error: {detail}. | Unexpected server error |

## Handling Errors

### By Status Code

For simple error handling, use HTTP status codes:

- **4xx**: Client error. Fix the request and retry.
- **503**: Temporary server issue. Retry with backoff.
- **500**: Server bug. Report if persistent.

### By Error Code

For precise handling, switch on the `code` field:

```python
response = requests.post(f"{base}/input", data=b"hello")
if response.status_code != 204:
    error = response.json()["error"]
    match error["code"]:
        case "auth_required":
            # Need to provide credentials
            pass
        case "auth_invalid":
            # Wrong token
            pass
        case "input_send_failed":
            # Terminal session may have ended
            pass
        case "parser_unavailable":
            # Retry after a short delay
            pass
```

### Parameterized Messages

Some error codes include context in their message. The `code` is always
stable, but the `message` may contain dynamic details:

```json
{
  "error": {
    "code": "overlay_not_found",
    "message": "No overlay exists with id 'abc-123'."
  }
}
```

The `{id}` in `overlay_not_found` or `{detail}` in validation errors gives
the specific value that caused the failure.

## Non-JSON Errors

In rare cases (malformed request before routing, connection-level failures),
you may receive a non-JSON error from the HTTP framework. These are standard
HTTP error responses without the `{"error": {...}}` wrapper. Robust clients
should handle both JSON and non-JSON error bodies.
