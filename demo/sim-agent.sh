#!/usr/bin/env bash
set -euo pipefail

# Simulated AI assistant TUI for wsh demo.
# Runs in the alternate screen buffer with a bordered interface.
# Reads character-by-character input; Enter triggers canned responses.
# Env: SPEED (float, default 1.0) — multiplier for delays.

SPEED="${SPEED:-1.0}"

delay() {
  local base="$1"
  local scaled
  scaled=$(echo "$base / $SPEED" | bc -l)
  sleep "$scaled"
}

# --- Terminal dimensions ---
COLS=$(tput cols 2>/dev/null || echo 80)
LINES=$(tput lines 2>/dev/null || echo 24)

# --- Colors ---
green_bold=$'\033[1;32m'
cyan_bold=$'\033[1;36m'
cyan=$'\033[0;36m'
white=$'\033[0;37m'
yellow_bold=$'\033[1;33m'
dim=$'\033[2m'
bold=$'\033[1m'
reset=$'\033[0m'

# --- Alternate screen + cursor ---
enter_alt() {
  printf '\033[?1049h'  # enter alternate screen buffer
  printf '\033[2J'      # clear screen
  printf '\033[?25l'    # hide cursor
}

exit_alt() {
  printf '\033[?25h'    # show cursor
  printf '\033[?1049l'  # leave alternate screen buffer
}

trap 'exit_alt' EXIT
enter_alt

# --- Layout constants ---
# Box spans cols 1..COLS, rows 1..LINES
# Row 1: top border with header
# Row 2: blank
# Rows 3..(LINES-4): content area
# Row (LINES-3): divider
# Row (LINES-2): prompt row
# Row (LINES-1): bottom border
# Row LINES: unused (avoids scroll)

CONTENT_TOP=3
CONTENT_BOTTOM=$(( LINES - 4 ))
DIVIDER_ROW=$(( LINES - 3 ))
PROMPT_ROW=$(( LINES - 2 ))
BOTTOM_ROW=$(( LINES - 1 ))
INNER_WIDTH=$(( COLS - 4 ))  # width between "│ " and " │"

# --- Drawing helpers ---
move() {
  # move cursor to row $1, col $2
  printf '\033[%d;%dH' "$1" "$2"
}

draw_hline() {
  local row="$1" left="$2" fill="$3" right="$4"
  move "$row" 1
  printf '%s%s' "$dim" "$left"
  for (( i = 0; i < COLS - 2; i++ )); do
    printf '%s' "$fill"
  done
  printf '%s%s' "$right" "$reset"
}

draw_chrome() {
  # Top border with header
  move 1 1
  printf '%s┌─%s%s AI Assistant %s' "$dim" "$reset" "$green_bold" "$reset"
  # Fill remaining top border
  local header_text=" AI Assistant "
  local header_len=${#header_text}
  local remaining=$(( COLS - 2 - 1 - header_len ))
  printf '%s' "$dim"
  for (( i = 0; i < remaining; i++ )); do
    printf '─'
  done
  printf '┐%s' "$reset"

  # Side borders for content area (rows 2 to DIVIDER-1)
  for (( row = 2; row < DIVIDER_ROW; row++ )); do
    move "$row" 1
    printf '%s│%s' "$dim" "$reset"
    # Clear the row interior
    printf '%*s' "$(( COLS - 2 ))" ''
    move "$row" "$COLS"
    printf '%s│%s' "$dim" "$reset"
  done

  # Divider
  draw_hline "$DIVIDER_ROW" '├' '─' '┤'

  # Prompt row
  move "$PROMPT_ROW" 1
  printf '%s│%s' "$dim" "$reset"
  printf '%*s' "$(( COLS - 2 ))" ''
  move "$PROMPT_ROW" "$COLS"
  printf '%s│%s' "$dim" "$reset"

  # Bottom border
  draw_hline "$BOTTOM_ROW" '└' '─' '┘'
}

clear_content() {
  for (( row = CONTENT_TOP; row <= CONTENT_BOTTOM; row++ )); do
    move "$row" 3
    printf '%*s' "$INNER_WIDTH" ''
  done
}

draw_prompt() {
  move "$PROMPT_ROW" 3
  printf '%*s' "$INNER_WIDTH" ''
  move "$PROMPT_ROW" 3
  printf '%s%s> %s%s' "$bold" "$white" "$reset" "$INPUT_BUF"
  # Show cursor at current input position
  printf '\033[?25h'
}

# --- Spinner ---
SPINNER_FRAMES=( '⠋' '⠙' '⠹' '⠸' '⠼' '⠴' '⠦' '⠧' '⠇' '⠏' )

show_spinner() {
  local message="$1"
  local duration_base="$2"
  local duration
  duration=$(echo "$duration_base / $SPEED" | bc -l)
  # Convert to centiseconds for iteration
  local iterations
  iterations=$(echo "$duration * 10" | bc -l | cut -d. -f1)
  if [[ -z "$iterations" || "$iterations" -lt 1 ]]; then
    iterations=10
  fi

  printf '\033[?25l'  # hide cursor
  local frame_count=${#SPINNER_FRAMES[@]}
  for (( i = 0; i < iterations; i++ )); do
    local frame_idx=$(( i % frame_count ))
    move "$CONTENT_TOP" 3
    printf '%*s' "$INNER_WIDTH" ''
    move "$CONTENT_TOP" 3
    printf '%s%s %s%s' "$cyan_bold" "${SPINNER_FRAMES[$frame_idx]}" "$message" "$reset"
    sleep 0.1
  done
  # Clear spinner line
  move "$CONTENT_TOP" 3
  printf '%*s' "$INNER_WIDTH" ''
}

# --- Response streaming ---
stream_response() {
  local -a lines=("$@")
  local row=$CONTENT_TOP
  for line in "${lines[@]}"; do
    if (( row > CONTENT_BOTTOM )); then
      break
    fi
    move "$row" 3
    printf '%s' "$line"
    (( row++ ))
    delay 0.15
  done
}

# --- Canned responses ---
respond_build() {
  local lines=(
    "${cyan}●${reset} ${white}Analyzing build output...${reset}"
    ""
    "  ${white}Build completed ${green_bold}successfully${reset}${white}.${reset}"
    "  ${white}Found ${green_bold}0 errors${reset}${white}, ${yellow_bold}3 warnings${reset}${white} in ${dim}\`src/parser.rs\`${reset}${white}.${reset}"
    "  ${white}Recommendation: run test suite to verify.${reset}"
  )
  stream_response "${lines[@]}"
}

respond_test() {
  local lines=(
    "${cyan}●${reset} ${white}Launching test suite...${reset}"
    ""
    "  ${white}Running tests across ${bold}4 modules${reset}${white}:${reset}"
    "  ${dim}  api::handlers  api::ws_methods${reset}"
    "  ${dim}  session        terminal::parser${reset}"
    ""
    "  ${white}Monitoring for failures. Will report when done.${reset}"
  )
  stream_response "${lines[@]}"
}

respond_lint() {
  local lines=(
    "${cyan}●${reset} ${white}Reviewing lint results...${reset}"
    ""
    "  ${white}Found ${yellow_bold}3 warnings${reset}${white}:${reset}"
    "  ${dim}  1. unused variable \`buf\` in parser/decode.rs${reset}"
    "  ${dim}  2. needless borrow in session.rs${reset}"
    "  ${dim}  3. collapsible if in api/handlers.rs${reset}"
    ""
    "  ${white}All are low-severity style issues. Safe to proceed.${reset}"
  )
  stream_response "${lines[@]}"
}

respond_results() {
  local lines=(
    "${cyan}●${reset} ${white}Reviewing all results...${reset}"
    ""
    "  ${green_bold}Build${reset}${white}:  passed (0 errors)${reset}"
    "  ${green_bold}Tests${reset}${white}:  22/22 passed${reset}"
    "  ${yellow_bold}Lint${reset}${white}:   3 warnings (non-blocking)${reset}"
    ""
    "  ${white}All checks passed. ${green_bold}Ready to deploy.${reset}"
  )
  stream_response "${lines[@]}"
}

respond_default() {
  local lines=(
    "${cyan}●${reset} ${white}Here's what I can help with:${reset}"
    ""
    "  ${cyan}●${reset} ${white}Analyze build output for errors and warnings${reset}"
    "  ${cyan}●${reset} ${white}Run and monitor test suites${reset}"
    "  ${cyan}●${reset} ${white}Investigate failures and suggest fixes${reset}"
    "  ${cyan}●${reset} ${white}Review code changes for issues${reset}"
    ""
    "  ${dim}Try: \"analyze the build\" or \"run the tests\"${reset}"
  )
  stream_response "${lines[@]}"
}

handle_enter() {
  local input="$INPUT_BUF"
  INPUT_BUF=""

  # Clear content and show spinner
  clear_content
  show_spinner "Thinking..." 1.0

  # Clear and show response
  clear_content

  local lower_input
  lower_input=$(echo "$input" | tr '[:upper:]' '[:lower:]')

  if [[ "$lower_input" == *"lint"* || "$lower_input" == *"clippy"* ]]; then
    respond_lint
  elif [[ "$lower_input" == *"result"* || "$lower_input" == *"summary"* || "$lower_input" == *"status"* ]]; then
    respond_results
  elif [[ "$lower_input" == *"build"* ]]; then
    respond_build
  elif [[ "$lower_input" == *"test"* ]]; then
    respond_test
  else
    respond_default
  fi

  # Redraw prompt
  draw_prompt
}

# --- Main ---
INPUT_BUF=""

draw_chrome
draw_prompt

while true; do
  # Read a single character from stdin.
  # -n1: one char, -s: silent, -r: raw
  if IFS= read -r -n1 -s char; then
    if [[ -z "$char" ]]; then
      # Empty string means Enter was pressed
      handle_enter
    elif [[ "$char" == $'\x7f' || "$char" == $'\x08' ]]; then
      # Backspace (DEL or BS)
      if [[ -n "$INPUT_BUF" ]]; then
        INPUT_BUF="${INPUT_BUF%?}"
        draw_prompt
      fi
    else
      # Regular character
      INPUT_BUF+="$char"
      draw_prompt
    fi
  else
    # read failed (EOF or error), exit
    break
  fi
done
