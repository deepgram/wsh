# Feature Request: Terminal Session Recording

## Summary

Add the ability to record terminal sessions as asciinema-format files, serve those recordings through the wsh API, and embed them in web pages using `asciinema-player`. The primary motivation is headless CI environments where a human is not present during execution — test runs, deployment pipelines, and automated agent tasks — but someone may need to review exactly what happened later, directly in a browser without installing any tooling.

## Problem

`wsh` gives agents and humans real-time visibility into terminal sessions. But real-time is not always when you need to look. In CI pipelines, automated test runs, and headless deployments, sessions run without a human observer. When something goes wrong (or when you want to audit a successful run), there is no way to go back and see what happened.

Today your options are:
- Parse log files that lack visual context (no colors, no cursor movement, no screen layout)
- Grep through raw scrollback output, which loses temporal information
- Re-run the session and hope you can reproduce the issue

A recording captures the full fidelity of the terminal session — every byte, every escape sequence, every timing — so you can scrub through it in a browser, share a link with a teammate, or embed it directly in a CI report, issue comment, or internal documentation page.

## Proposed Solution

### Recording Format: asciinema v2

The [asciinema v2 format](https://docs.asciinema.org/manual/asciicast/v2/) is the right target:

- Widely supported: asciinema CLI, asciinema-player (embeddable in any web page), and dozens of third-party tools
- Simple: newline-delimited JSON with a header and event stream
- Lossless: preserves raw terminal bytes and timing, including all ANSI escape sequences
- Compact: events are delta-encoded by elapsed time
- Embeddable: `asciinema-player` is a self-contained web component; no server-side rendering required

```jsonc
// Header (line 1)
{"version": 2, "width": 220, "height": 50, "timestamp": 1744070000, "title": "ci-build", "env": {"TERM": "xterm-256color"}}

// Events: [elapsed_seconds, "o" (output) | "i" (input), data]
[0.0, "o", "$ cargo test\r\n"]
[0.312, "o", "   Compiling wsh v0.4.0\r\n"]
[1.847, "o", "\u001b[32m   Finished\u001b[0m test profile\r\n"]
```

### Storage Model

Recordings are stored server-side, identified by a recording ID. Each recording belongs to a session but outlives it — when a session is destroyed, its completed recordings remain accessible. Recordings are stored in a configurable directory (default: `~/.local/share/wsh/recordings/` on Linux, `~/Library/Application Support/wsh/recordings/` on macOS).

A recording has three states:
- **recording**: actively capturing output; the `.cast` file is being appended
- **stopped**: finalized and fully playable
- **failed**: session exited uncleanly; partial file exists and is playable up to the last complete event

### API Design

#### Start Recording

```
POST /sessions/:name/recording
Content-Type: application/json

{
  "title": "ci-build",      // optional: embedded in asciinema header and used as display name
  "capture_input": false    // optional: record "i" events too (default: false)
}
```

Response `201 Created`:
```json
{
  "id": "rec_abc123",
  "session": "ci-build",
  "title": "ci-build",
  "started_at": 1744070000,
  "status": "recording",
  "urls": {
    "cast": "/recordings/rec_abc123/cast",
    "embed": "/recordings/rec_abc123/embed",
    "player": "/recordings/rec_abc123/player"
  }
}
```

A session may have at most one active recording at a time (`409 Conflict` if already recording).

#### Stop Recording

```
DELETE /sessions/:name/recording
```

Response `200 OK`:
```json
{
  "id": "rec_abc123",
  "session": "ci-build",
  "title": "ci-build",
  "started_at": 1744070000,
  "stopped_at": 1744070185,
  "duration_secs": 185.4,
  "bytes_written": 48320,
  "status": "stopped",
  "urls": {
    "cast": "/recordings/rec_abc123/cast",
    "embed": "/recordings/rec_abc123/embed",
    "player": "/recordings/rec_abc123/player"
  }
}
```

#### Get/List Recordings

```
GET /sessions/:name/recording          # active recording for session (404 if none)
GET /recordings                        # list all recordings (all sessions, all states)
GET /recordings?session=ci-build       # filter by session name
GET /recordings?status=stopped         # filter by status
GET /recordings/:id                    # get a single recording by ID
DELETE /recordings/:id                 # delete a recording and its file
```

#### Serve the Cast File

```
GET /recordings/:id/cast
```

Serves the raw `.cast` file with `Content-Type: application/x-asciicast`. While a recording is active, this endpoint streams partial content (suitable for live preview); once stopped, it serves the complete file. This is the URL you point `asciinema-player` at.

#### Serve a Standalone Player Page

```
GET /recordings/:id/player
```

Returns a self-contained HTML page with `asciinema-player` bundled inline, loading the cast file from the same wsh server. This page can be opened directly in a browser or served as a CI artifact.

#### Serve an Embed Snippet

```
GET /recordings/:id/embed
```

Returns an HTML snippet (not a full page) that embeds the player via `asciinema-player`'s web component:

```html
<div id="player-rec_abc123"></div>
<link rel="stylesheet" type="text/css" href="https://cdn.jsdelivr.net/npm/asciinema-player@3/dist/bundle/player.css"/>
<script src="https://cdn.jsdelivr.net/npm/asciinema-player@3/dist/bundle/player.js"></script>
<script>
  AsciinemaPlayer.create(
    'http://localhost:8080/recordings/rec_abc123/cast',
    document.getElementById('player-rec_abc123'),
    { cols: 220, rows: 50, title: "ci-build", autoPlay: false }
  );
</script>
```

The response includes a `X-Player-URL` header with the full standalone player URL for convenience.

#### Auto-Record on Session Create

Extend `POST /sessions` to support an optional `recording` field:

```json
{
  "name": "ci-build",
  "command": "/bin/bash",
  "recording": {
    "title": "CI Build Run"
  }
}
```

When provided, recording begins the moment the PTY is spawned — capturing output from the very first byte. The recording ID is included in the create session response.

### CLI Integration

```bash
# Start a session and immediately begin recording
wsh session create ci-build --record --record-title "CI Build Run"

# Start recording on an existing session
wsh record start ci-build --title "CI Build Run"

# Stop recording; prints the cast URL and standalone player URL
wsh record stop ci-build

# List all recordings
wsh record list
wsh record list --session ci-build

# Get the embed snippet for a recording
wsh record embed rec_abc123

# Open the standalone player page in the default browser
wsh record open rec_abc123

# Delete a recording
wsh record delete rec_abc123
```

`wsh record stop` prints output like:

```
Recording stopped: rec_abc123 (185s, 48.3 KB)
  Cast:    http://localhost:8080/recordings/rec_abc123/cast
  Player:  http://localhost:8080/recordings/rec_abc123/player
  Embed:   http://localhost:8080/recordings/rec_abc123/embed
```

### Web UI Integration

The web UI already has per-session views. Recordings surface in two places:

**Session view**: An active recording shows a red recording indicator (pulsing dot) in the session header. Completed recordings are listed below the session with title, duration, and a play button that opens the embedded `asciinema-player` inline.

**Recordings page**: A dedicated `/recordings` page in the web UI lists all recordings across sessions, filterable by session and status. Each row shows the title, session name, duration, timestamp, and links to play or copy the embed snippet. The player opens inline on the page — no navigation away, no new tab required.

The embed snippet button copies the HTML fragment to the clipboard so it can be pasted directly into GitHub issues, Confluence pages, internal dashboards, or any HTML surface.

### CI / Headless Use Case

The canonical workflow this feature enables:

```yaml
# GitHub Actions example
- name: Run integration tests
  run: |
    SESSION=$(wsh session create test-run --record --record-title "Integration Tests ${{ github.run_id }}" --json | jq -r '.name')
    RECORDING_ID=$(wsh session create test-run --record --json | jq -r '.recording.id')
    wsh send test-run "cargo test --test integration 2>&1; exit \$?"
    wsh await-idle test-run --timeout 300
    wsh record stop test-run

- name: Output recording URL
  if: always()
  run: |
    echo "Terminal recording: http://$WSH_HOST/recordings/$RECORDING_ID/player"
    echo "Embed snippet:"
    wsh record embed $RECORDING_ID
```

The player URL can be posted as a CI status comment, included in a Slack notification, or linked from a test failure report — and opens immediately in any browser with full scrubbing and playback controls, no tooling required.

### Implementation Notes

**Tap point**: Recording taps into the PTY read pipeline at the same point raw bytes are forwarded to the local TTY, before ANSI parsing. This ensures the recorded bytes are identical to what the terminal emulator sees, with no double-parsing overhead.

**Writer properties**:
- Non-blocking: recording I/O never stalls PTY throughput
- Buffered: events are batched and flushed periodically (and on session exit) to reduce syscall overhead
- Crash-safe: partial files are valid asciinema v2 (append-only format; playable up to the last complete line)
- Timestamps: floating-point elapsed seconds from recording start, not wall time

**Lifecycle**:
- Session exit: active recording is finalized (flushed, closed, status set to `stopped`)
- Graceful server shutdown: all active recordings are finalized before exit
- Unclean shutdown: partial file remains; status transitions to `failed` on next server start via a startup scan
- Client disconnect: recording continues unaffected; owned by the session, not the client

**Retention**: Recordings are not automatically deleted. A `max_recordings` and `max_recording_age_days` config option can be added in a follow-on to support automatic cleanup.

## Out of Scope

- **Remote storage**: S3/GCS upload is a separate concern; CI pipelines can upload the cast file as an artifact after `wsh record stop`
- **Format conversion**: SVG/GIF/video export is asciinema's domain, not wsh's
- **Live public sharing**: The `/recordings/:id/player` URL is served by wsh itself; public sharing requires the wsh server to be reachable or the cast file to be uploaded elsewhere

## Acceptance Criteria

- `POST /sessions/:name/recording` starts recording; response includes recording ID and all three URLs
- Recording file is valid asciinema v2 playable with `asciinema play`
- `GET /recordings/:id/cast` serves the cast file; streams partial content while recording is active
- `GET /recordings/:id/player` returns a self-contained HTML page that plays the recording in-browser
- `GET /recordings/:id/embed` returns a copy-pasteable HTML snippet with working player
- Auto-record via `POST /sessions` captures output from PTY spawn with no leading bytes missed
- Recording survives client disconnects without interruption
- Active recordings are finalized on graceful server shutdown
- Unclean shutdown leaves a partial but playable cast file
- Starting a second recording on the same session returns `409 Conflict`
- `wsh record` CLI subcommands work end-to-end
- Web UI shows recording indicator on active sessions and inline player for completed recordings
- Unit tests: asciinema writer (header, event serialization, elapsed timing)
- Integration test: create session, auto-record, run command, stop, verify cast with asciinema parse, verify player page loads
