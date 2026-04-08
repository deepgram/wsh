# Session Recording

`wsh` can record terminal sessions to [asciinema v2](https://docs.asciinema.org/manual/asciicast/v2/) format (`.cast` files). Recordings are served directly from the wsh API, so anyone with a browser can play them back — no tooling required. The primary use case is headless CI environments: record a test run or deployment pipeline, then review exactly what the terminal looked like at every moment when something goes wrong.

## How It Works

When you start a recording, wsh subscribes to the session's output stream and writes each chunk of PTY output as a timestamped event to a `.cast` file on disk. Recordings outlive their sessions — the file persists after the session is destroyed and remains accessible via the API.

The asciinema v2 format is append-only, so partial recordings from unclean shutdowns are still valid and playable up to the last complete event line.

## Quickstart

```bash
# Start wsh server in persistent mode
wsh server --persistent &

# Create a session and start recording immediately
curl -X POST http://localhost:8080/sessions \
  -H 'Content-Type: application/json' \
  -d '{"name": "build", "recording": {"title": "CI Build"}}'

# Run your workload
curl -X POST http://localhost:8080/sessions/build/input \
  -d 'cargo test 2>&1; echo "exit: $?"
'

# Wait for it to go idle
curl "http://localhost:8080/sessions/build/idle?max_wait_ms=60000"

# Stop recording and get the URLs
curl -X DELETE http://localhost:8080/sessions/build/recording

# Open the player in your browser
open http://localhost:8080/recordings/<id>/player
```

## API Reference

### Start a Recording

```
POST /sessions/:name/recording
Content-Type: application/json

{
  "title": "My Recording"   // optional display name
}
```

Response `201 Created`:

```json
{
  "id": "a3f2b1c4-...",
  "session": "build",
  "title": "CI Build",
  "started_at": 1744070000,
  "bytes_written": 0,
  "status": "recording",
  "width": 220,
  "height": 50,
  "urls": {
    "cast":   "/recordings/a3f2b1c4-.../cast",
    "player": "/recordings/a3f2b1c4-.../player",
    "embed":  "/recordings/a3f2b1c4-.../embed"
  }
}
```

Only one recording per session at a time. Starting a second returns `409 Conflict`.

### Get Active Recording Status

```
GET /sessions/:name/recording
```

Returns the same shape as above, `404` if no recording is active.

### Stop a Recording

```
DELETE /sessions/:name/recording
```

Flushes and finalizes the file, sets `status` to `"stopped"`. Returns the recording info. `404` if no recording is active.

### List All Recordings

```
GET /recordings
GET /recordings?session=build          # filter by session name
GET /recordings?status=stopped         # filter by status
GET /recordings?session=build&status=stopped
```

Response:

```json
{
  "recordings": [
    { "id": "...", "session": "build", "status": "stopped", ... }
  ]
}
```

### Get a Single Recording

```
GET /recordings/:id
```

### Delete a Recording

```
DELETE /recordings/:id
```

Returns `204 No Content`. Also deletes the `.cast` file from disk.

### Serve the Cast File

```
GET /recordings/:id/cast
```

Returns the raw asciinema v2 `.cast` file (`application/x-asciicast`). While a recording is active, this returns the bytes written so far — a valid partial cast playable up to the last complete event line.

This is the URL you pass to any asciinema-compatible tool:

```bash
# Play locally
curl http://localhost:8080/recordings/<id>/cast > session.cast
asciinema play session.cast

# Or download and play in one step
asciinema play <(curl -s http://localhost:8080/recordings/<id>/cast)
```

### Standalone Player Page

```
GET /recordings/:id/player
```

Returns a self-contained HTML page with [`asciinema-player`](https://github.com/asciinema/asciinema-player) embedded. Open it in any browser — no installation, no account, no external service required.

The player loads the cast from the same wsh server using a relative URL, so it works as long as the browser can reach wsh.

### HTML Embed Snippet

```
GET /recordings/:id/embed
```

Returns a copy-pasteable HTML fragment that embeds the player on any web page. The cast URL in the snippet is absolute (built from the request's `Host` header) so it works when pasted into external pages like GitHub issues, Confluence, or internal dashboards.

The response also includes an `X-Player-URL` header with the full standalone player URL for convenience.

Example snippet:

```html
<div id="wsh-player-abc123"></div>
<link rel="stylesheet" type="text/css"
      href="https://cdn.jsdelivr.net/npm/asciinema-player@3/dist/bundle/player.css">
<script src="https://cdn.jsdelivr.net/npm/asciinema-player@3/dist/bundle/player.js"></script>
<script>
  AsciinemaPlayer.create(
    'http://your-wsh-host:8080/recordings/<id>/cast',
    document.getElementById('wsh-player-abc123'),
    { cols: 220, rows: 50, title: 'CI Build', autoPlay: false, fit: 'width' }
  );
</script>
```

## Auto-Record on Session Create

Pass a `recording` field to `POST /sessions` to start recording from the very first PTY byte — before any other API client connects:

```bash
curl -X POST http://localhost:8080/sessions \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "deploy",
    "command": "/bin/bash",
    "recording": {
      "title": "Production Deploy 2026-04-08"
    }
  }'
```

The response includes a `recording_id` field when a recording is active. `GET /sessions/:name` also includes `recording_id` whenever the session has an active recording.

## CI / GitHub Actions Example

```yaml
jobs:
  integration-test:
    runs-on: ubuntu-latest
    services:
      wsh:
        image: ghcr.io/deepgram/wsh:latest
        ports: ["8080:8080"]
        options: --name wsh

    steps:
      - name: Create session with recording
        id: record
        run: |
          RESP=$(curl -sf -X POST http://localhost:8080/sessions \
            -H 'Content-Type: application/json' \
            -d "{\"name\": \"ci\", \"recording\": {\"title\": \"$GITHUB_WORKFLOW #$GITHUB_RUN_NUMBER\"}}")
          echo "recording_id=$(echo $RESP | jq -r .recording_id)" >> $GITHUB_OUTPUT

      - name: Run tests
        run: |
          curl -sf -X POST http://localhost:8080/sessions/ci/input \
            -d 'cargo test --test integration 2>&1; echo "EXIT:$?"
'
          curl -sf "http://localhost:8080/sessions/ci/idle?max_wait_ms=300000"

      - name: Stop recording
        if: always()
        run: |
          curl -sf -X DELETE http://localhost:8080/sessions/ci/recording
          # Download cast file as a CI artifact
          curl -sf http://localhost:8080/recordings/${{ steps.record.outputs.recording_id }}/cast \
            -o recording.cast

      - name: Upload recording
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: terminal-recording
          path: recording.cast

      - name: Print player URL
        if: always()
        run: |
          echo "Play recording: http://localhost:8080/recordings/${{ steps.record.outputs.recording_id }}/player"
```

The downloaded `.cast` artifact can be played locally with `asciinema play recording.cast`, or the player URL can be shared with anyone who can reach the wsh server.

## Recording Storage

Cast files are stored on the wsh server's filesystem at:

| Platform | Default path |
|----------|-------------|
| Linux | `~/.local/share/wsh/recordings/` |
| macOS | `~/Library/Application Support/wsh/recordings/` |

The directory is created automatically on first use. Files are named `<recording-id>.cast` and are never automatically deleted — use `DELETE /recordings/:id` to clean them up, or point a cron job at the API.

## Recording Status Values

| Status | Meaning |
|--------|---------|
| `recording` | Actively capturing output |
| `stopped` | Cleanly finalized and fully playable |
| `failed` | Session exited uncleanly; partial file exists and is playable up to the last complete event |

## Serving Recordings Publicly

By default, wsh binds to `127.0.0.1` only. To share player URLs externally you have two options:

**1. Expose wsh with a reverse proxy** (nginx, Caddy, etc.) and use the embed snippet — the absolute cast URL in the snippet will use whatever `Host` header the proxy sets.

**2. Download and re-host** the `.cast` file anywhere that serves static files, then point `asciinema-player` at it directly:

```html
<script>
  AsciinemaPlayer.create(
    'https://static.example.com/recordings/my-session.cast',
    document.getElementById('player'),
    { cols: 220, rows: 50 }
  );
</script>
```

This approach works for GitHub Pages, S3, or any static host — the cast file is just a text file.
