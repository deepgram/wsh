#!/usr/bin/env bash
set -euo pipefail

# Simulated cargo clippy output for wsh demo.
# Env: SPEED (float, default 1.0) — multiplier for delays.

SPEED="${SPEED:-1.0}"

delay() {
  local base="$1"
  local scaled
  scaled=$(echo "$base / $SPEED" | bc -l)
  sleep "$scaled"
}

green_bold=$'\033[1;32m'
yellow_bold=$'\033[1;33m'
yellow=$'\033[0;33m'
cyan=$'\033[0;36m'
bold=$'\033[1m'
reset=$'\033[0m'

printf "    ${green_bold}Checking${reset} wsh v0.1.0\n"
delay 0.8

warnings=(
  "warning: unused variable: \`buf\`
  ${cyan}-->${reset} src/parser/decode.rs:42:9
   ${cyan}|${reset}
${cyan}42${reset} ${cyan}|${reset}     let buf = Vec::new();
   ${cyan}|${reset}         ${yellow_bold}^^^ help: if this is intentional, prefix it with an underscore: \`_buf\`${reset}
   ${cyan}|${reset}
   = note: \`#[warn(unused_variables)]\` on by default"

  "warning: this expression creates a reference which is immediately dereferenced by the compiler
  ${cyan}-->${reset} src/session.rs:118:22
   ${cyan}|${reset}
${cyan}118${reset}${cyan}|${reset}     broker.publish(&bytes);
   ${cyan}|${reset}                      ${yellow_bold}^^^^^^ help: change this to: \`bytes\`${reset}
   ${cyan}|${reset}
   = note: \`#[warn(clippy::needless_borrow)]\` on by default"

  "warning: this \`if let\` can be collapsed into the outer \`if let\`
  ${cyan}-->${reset} src/api/handlers.rs:205:9
   ${cyan}|${reset}
${cyan}205${reset}${cyan}|${reset} /         if let Some(session) = sessions.get(name) {
${cyan}206${reset}${cyan}|${reset} ${cyan}|${reset}             if let Some(screen) = session.screen() {
   ${cyan}|${reset} ${yellow_bold}|____________________________________________________^${reset}
   ${cyan}|${reset}
   = note: \`#[warn(clippy::collapsible_if)]\` on by default"
)

for w in "${warnings[@]}"; do
  printf '%s\n\n' "$w"
  delay 0.6
done

printf "    ${yellow_bold}Finished${reset} \`dev\` profile [unoptimized + debuginfo] target(s) in 1.54s\n"
printf "warning: ${bold}wsh${reset} (bin \"wsh\") generated 3 warnings\n"
