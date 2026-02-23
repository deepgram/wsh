#!/usr/bin/env bash
set -euo pipefail

# Simulated cargo build output for wsh demo.
# Env: SPEED (float, default 1.0) — multiplier for delays.

SPEED="${SPEED:-1.0}"

delay() {
  local base="$1"
  local scaled
  scaled=$(echo "$base / $SPEED" | bc -l)
  sleep "$scaled"
}

green_bold=$'\033[1;32m'
reset=$'\033[0m'

crates=(
  "libc v0.2.155"
  "unicode-ident v1.0.12"
  "proc-macro2 v1.0.86"
  "quote v1.0.36"
  "syn v2.0.72"
  "serde v1.0.204"
  "tokio v1.39.2"
  "bytes v1.7.1"
  "hyper v1.4.1"
  "axum v0.7.5"
  "wsh-core v0.1.0"
  "wsh-server v0.1.0"
)

for crate in "${crates[@]}"; do
  echo "   ${green_bold}Compiling${reset} ${crate}"
  delay 0.2
done

delay 0.3
echo "    ${green_bold}Finished${reset} \`dev\` profile [unoptimized + debuginfo] target(s) in 2.87s"
