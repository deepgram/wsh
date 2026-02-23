#!/usr/bin/env bash
set -euo pipefail

# ── Main demo orchestrator ──────────────────────────────────────
# Creates 4 wsh sessions via the HTTP API and drives a choreographed
# demonstration: build, test, AI agent interaction, and system monitor.
#
# Prerequisites:
#   - A running wsh server (e.g. `wsh server --bind 127.0.0.1:8080`)
#   - curl and bc available on PATH
#
# Environment:
#   WSH_URL   Base URL of the wsh server (default: http://localhost:8080)
#   SPEED     Playback speed multiplier (default: 1.0, higher = faster)

BASE="${WSH_URL:-http://localhost:8080}"
SPEED="${SPEED:-1.0}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ── Helpers ─────────────────────────────────────────────────────

delay() {
  local base="$1"
  local scaled
  scaled=$(echo "$base / $SPEED" | bc -l)
  sleep "$scaled"
}

narrate() {
  printf '\033[1;34m▸ %s\033[0m\n' "$1"
}

create_session() {
  local name="$1"
  local tag="$2"
  curl -sf "${BASE}/sessions" \
    -H 'Content-Type: application/json' \
    -d "{\"name\":\"${name}\",\"tags\":[\"${tag}\"]}" \
    -o /dev/null
}

send_input() {
  local name="$1"
  local data="$2"
  curl -sf "${BASE}/sessions/${name}/input" \
    -H 'Content-Type: application/octet-stream' \
    --data-binary "$data" \
    -o /dev/null
}

wait_idle() {
  local name="$1"
  curl -sf "${BASE}/sessions/${name}/idle?timeout_ms=500&max_wait_ms=30000" \
    -o /dev/null
}

read_screen() {
  local name="$1"
  curl -sf "${BASE}/sessions/${name}/screen?format=plain"
}

add_overlay() {
  local name="$1"
  local text="$2"
  local fg_color="$3"
  local width=$(( ${#text} + 4 ))
  curl -sf "${BASE}/sessions/${name}/overlay" \
    -H 'Content-Type: application/json' \
    -d "{
      \"x\": 2,
      \"y\": 1,
      \"width\": ${width},
      \"height\": 1,
      \"background\": {\"bg\": \"black\"},
      \"spans\": [{
        \"text\": \" ${text} \",
        \"fg\": \"${fg_color}\",
        \"bold\": true
      }]
    }" \
    -o /dev/null
}

type_slowly() {
  local name="$1"
  local text="$2"
  local i
  for (( i = 0; i < ${#text}; i++ )); do
    local char="${text:$i:1}"
    send_input "$name" "$char"
    delay 0.06
  done
}

kill_session() {
  local name="$1"
  curl -sf "${BASE}/sessions/${name}" -X DELETE -o /dev/null 2>/dev/null || true
}

# ── Cleanup ─────────────────────────────────────────────────────

cleanup() {
  kill_session build
  kill_session test
  kill_session agent
  kill_session monitor
}
trap cleanup EXIT

# ── Beat 1: Setup ──────────────────────────────────────────────

# Preflight: ensure wsh server is running
if ! curl -sf "${BASE}/health" -o /dev/null 2>/dev/null; then
  echo "Error: wsh server is not running at ${BASE}" >&2
  echo "Start it with:  wsh server --bind 127.0.0.1:8080" >&2
  exit 1
fi

narrate "Creating sessions..."
create_session build ci
printf '   + build (ci)\n'
create_session test ci
printf '   + test (ci)\n'
create_session agent ai
printf '   + agent (ai)\n'
create_session monitor ops
printf '   + monitor (ops)\n'

# ── Beat 2: Launch ─────────────────────────────────────────────

narrate "Launching monitor + agent..."
send_input monitor "SPEED=$SPEED $SCRIPT_DIR/sim-monitor.sh"$'\n'
send_input agent "SPEED=$SPEED $SCRIPT_DIR/sim-agent.sh"$'\n'

delay 1

narrate "Starting build..."
send_input build "SPEED=$SPEED $SCRIPT_DIR/sim-build.sh"$'\n'

# ── Beat 3: Build completes ────────────────────────────────────

narrate "Waiting for build to complete..."
wait_idle build
narrate "Build complete — adding overlay"
add_overlay build "✓ Build passed" green

# ── Beat 4: Agent interaction ──────────────────────────────────

narrate "Agent: analyzing build output..."
type_slowly agent "analyze the build"
send_input agent $'\n'
delay 4

narrate "Agent: requesting test run..."
type_slowly agent "run the tests"
send_input agent $'\n'
delay 1

# ── Beat 5: Tests ──────────────────────────────────────────────

narrate "Running tests..."
send_input test "SPEED=$SPEED $SCRIPT_DIR/sim-test.sh"$'\n'
wait_idle test
narrate "Tests complete — adding overlay"
add_overlay test "✓ 22 passed, 0 failed" green

# ── Beat 6: Final hold ─────────────────────────────────────────

narrate "Done. All sessions visible in grid view."

delay 3

printf '\n \033[1mDemonstration complete.\033[0m\n'
