# wsh Demo Recording Runbook

## What It Shows

An AI agent orchestrating 4 terminal sessions through the wsh API using
nothing but `curl`. The demo creates `build`, `test`, `agent`, and `monitor`
sessions, drives a simulated CI pipeline (compile, test, AI analysis), and
renders live overlays -- all visible simultaneously in the wsh web UI grid
view.

## Prerequisites

1. **wsh** built and on PATH (from the repo root: `nix develop -c sh -c "cargo build"`)
2. **Recording tools** -- enter the demo dev shell:
   ```
   cd demo && nix develop
   ```
   This provides `wf-recorder`, `ffmpeg`, `gifski`, `curl`, `jq`, and `bc`.
3. **A browser** (for the wsh web UI)
4. **Wayland/Sway** (for `wf-recorder`; see Troubleshooting for X11 alternatives)

## Window Layout

On a 4K 16:9 monitor with Sway, arrange three visible windows:

```
+-------------------------+
|                         |
|    BROWSER (web UI)     |
|    Grid view, 2x2       |
|                         |
+------------+------------+
| TERMINAL 1 | TERMINAL 2 |
| wsh attach | demo.sh    |
+------------+------------+
```

Tips:
- Set the browser to grid view by pressing `g`
- Use a dark theme (Tokyo Night or Dracula work well)
- 90-100% browser zoom for 4K

## Recording Steps

1. **Start the wsh server** (in an off-screen terminal):
   ```
   wsh server --ephemeral
   ```

2. **Open the browser** to http://localhost:8080 and press `g` for grid view.

3. **In Terminal 1** (bottom-left, visible), run:
   ```
   wsh attach build
   ```
   This will block until the demo creates the `build` session -- that is
   expected.

4. **Start screen capture** (in an off-screen terminal):
   ```
   wf-recorder -f demo-raw.mp4
   ```
   Or, to capture a specific region:
   ```
   wf-recorder -g "$(slurp)" -f demo-raw.mp4
   ```

5. **In Terminal 2** (bottom-right, visible), run:
   ```
   ./demo/demo.sh
   ```

6. **When "Demonstration complete." appears**, wait 2-3 seconds, then stop
   recording with Ctrl+C in the `wf-recorder` terminal.

## Converting to GIF

```bash
# Trim to desired length (adjust -t for duration)
ffmpeg -i demo-raw.mp4 -ss 0 -t 20 -vf "fps=15,scale=1280:-1" demo-trimmed.mp4

# Extract frames
mkdir -p /tmp/demo-frames
ffmpeg -i demo-trimmed.mp4 -vf "fps=15" /tmp/demo-frames/frame%04d.png

# Encode GIF (gifski produces much smaller, higher-quality GIFs than ffmpeg)
gifski --fps 15 --width 1280 -o demo.gif /tmp/demo-frames/frame*.png

# Cleanup
rm -rf /tmp/demo-frames demo-trimmed.mp4
```

## Tuning

| Variable  | Default                    | Effect                                         |
|-----------|----------------------------|-------------------------------------------------|
| `SPEED`   | `1.0`                      | Playback speed multiplier. `2` = 2x faster, `0.5` = 2x slower. |
| `WSH_URL` | `http://localhost:8080`    | Base URL of the wsh server (if using a different port). |

Example -- run the demo at double speed:
```
SPEED=2 ./demo/demo.sh
```

## Troubleshooting

| Problem | Solution |
|---------|----------|
| "wsh server is not running at ..." | Start the server first: `wsh server --ephemeral` |
| Sessions not appearing in web UI | Refresh the browser; verify URL matches `WSH_URL` |
| `wsh attach build` hangs | The session has not been created yet -- start `demo.sh` first, then attach |
| Overlays not visible | Make sure the browser is in grid view (press `g`) |
| Timing feels off | Adjust `SPEED` (e.g., `SPEED=1.5` to speed things up) |
| `wf-recorder` errors | Ensure you are running Wayland/Sway. For X11, use `ffmpeg -f x11grab -i :0 demo-raw.mp4` instead |
| GIF too large | Lower `--width` or `--fps` in the gifski command |

## Files

| File              | Purpose                                              |
|-------------------|------------------------------------------------------|
| `demo.sh`         | Main orchestrator -- creates sessions, drives the demo via the wsh HTTP API |
| `sim-build.sh`    | Simulated `cargo build` output                       |
| `sim-test.sh`     | Simulated `cargo test` output                        |
| `sim-agent.sh`    | Simulated AI assistant TUI (alternate screen, interactive prompt) |
| `sim-monitor.sh`  | Simulated system monitor (htop-style, loops forever)  |
| `flake.nix`       | Nix flake providing recording tools (`wf-recorder`, `ffmpeg`, `gifski`, etc.) |
| `README.md`       | This file -- the complete recording runbook           |
