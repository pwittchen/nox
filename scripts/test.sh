#!/usr/bin/env bash
# Runs the local checks CI runs — cargo audit, fmt check, lint (clippy::pedantic), tests — and
# prints a summary table. Every step runs even if an earlier one fails; the exit code is non-zero
# if any step failed. Full output of each step goes to target/test-logs/<step>.log; the tail of a
# failing step's log is printed after the table.
#
# Usage: scripts/test.sh [-v] [audit] [fmt] [lint] [test]
#   no step names: all of them, in that order
#   -v: stream each step's output instead of only logging it
set -u
cd "$(dirname "$0")/.."

VERBOSE=0
STEPS=()
for arg in "$@"; do
  case "$arg" in
    -v | --verbose) VERBOSE=1 ;;
    -h | --help)
      sed -n '2,/^set /p' "$0" | grep '^#' | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    audit | fmt | lint | test) STEPS+=("$arg") ;;
    *)
      echo "unknown argument: $arg (expected audit, fmt, lint, test or -v)" >&2
      exit 2
      ;;
  esac
done
[ ${#STEPS[@]} -eq 0 ] && STEPS=(audit fmt lint test)

LOGS=target/test-logs
mkdir -p "$LOGS"

if [ -t 1 ]; then
  GREEN=$'\033[32m' RED=$'\033[31m' YELLOW=$'\033[33m' BOLD=$'\033[1m' DIM=$'\033[2m' RESET=$'\033[0m'
else
  GREEN='' RED='' YELLOW='' BOLD='' DIM='' RESET=''
fi

# Results, one entry per step (bash 3.2 has no associative arrays).
NAMES=() RESULTS=() TIMES=() DETAILS=()

duration() {
  local s=$1
  if [ "$s" -ge 60 ]; then printf '%dm %02ds' $((s / 60)) $((s % 60)); else printf '%ds' "$s"; fi
}

command_for() {
  case "$1" in
    audit) echo "cargo audit --deny warnings" ;;
    fmt) echo "cargo fmt --all -- --check --color never" ;;
    lint) echo "cargo lint" ;;
    test) echo "cargo test --workspace" ;;
  esac
}

# One-line detail for the summary, from the step's log and exit status.
detail_for() {
  local step=$1 log=$2 status=$3 n
  case "$step" in
    audit)
      n=$(grep -Eo 'Scanning Cargo.lock for vulnerabilities \([0-9]+' "$log" | grep -Eo '[0-9]+$' | tail -1)
      if [ "$status" -eq 0 ]; then
        echo "no advisories${n:+ ($n crates)}"
      else
        grep -Eo '[0-9]+ (vulnerabilit(y|ies)|(denied|allowed) warnings?) found' "$log" | paste -sd ',' - | sed 's/,/, /g'
      fi
      ;;
    fmt)
      n=$(grep -c '^Diff in ' "$log")
      if [ "$status" -eq 0 ]; then echo "formatted"; else echo "$n diff(s), run cargo fmt --all"; fi
      ;;
    lint)
      n=$(grep -Ec '^(error|warning)(\[|:)' "$log")
      n=$((n - $(grep -Ec '^error: could not compile|^warning: build failed|generated [0-9]+ warning' "$log")))
      if [ "$status" -eq 0 ]; then echo "no warnings"; else echo "$n problem(s)"; fi
      ;;
    test)
      awk '/^test result:/ {
             for (i = 1; i <= NF; i++) {
               if ($(i+1) ~ /^passed/) p += $i
               if ($(i+1) ~ /^failed/) f += $i
               if ($(i+1) ~ /^ignored/) g += $i
             }
           }
           END { printf "%d passed, %d failed, %d ignored", p, f, g }' "$log"
      if [ "$status" -ne 0 ] && ! grep -q '^test result:' "$log"; then echo " (build failed)"; fi
      ;;
  esac
}

for step in "${STEPS[@]}"; do
  log="$LOGS/$step.log"
  cmd=$(command_for "$step")
  printf '%s▸ %-6s%s %s%s%s\n' "$BOLD" "$step" "$RESET" "$DIM" "$cmd" "$RESET"
  if [ "$step" = audit ] && ! command -v cargo-audit >/dev/null 2>&1; then
    NAMES+=("$step") RESULTS+=(SKIP) TIMES+=(-) DETAILS+=("cargo-audit not installed (cargo install cargo-audit)")
    continue
  fi
  start=$SECONDS
  if [ "$VERBOSE" -eq 1 ]; then
    $cmd 2>&1 | tee "$log"
    status=${PIPESTATUS[0]}
  else
    $cmd >"$log" 2>&1
    status=$?
  fi
  elapsed=$((SECONDS - start))
  if [ "$status" -eq 0 ]; then result=PASS; else result=FAIL; fi
  NAMES+=("$step") RESULTS+=("$result") TIMES+=("$(duration "$elapsed")")
  DETAILS+=("$(detail_for "$step" "$log" "$status" | tr -d '\n')")
done

# ---- summary table ----------------------------------------------------------------------------

w1=4 w2=6 w3=4 w4=7 # header widths: Step, Result, Time, Details
for i in "${!NAMES[@]}"; do
  [ ${#NAMES[$i]} -gt $w1 ] && w1=${#NAMES[$i]}
  [ ${#TIMES[$i]} -gt $w3 ] && w3=${#TIMES[$i]}
  [ ${#DETAILS[$i]} -gt $w4 ] && w4=${#DETAILS[$i]}
done

rule() {
  printf '+-%s-+-%s-+-%s-+-%s-+\n' "$(printf '%*s' $w1 '' | tr ' ' -)" "$(printf '%*s' $w2 '' | tr ' ' -)" \
    "$(printf '%*s' $w3 '' | tr ' ' -)" "$(printf '%*s' $w4 '' | tr ' ' -)"
}

echo
rule
printf "| %-${w1}s | %-${w2}s | %${w3}s | %-${w4}s |\n" Step Result Time Details
rule
failed=0
for i in "${!NAMES[@]}"; do
  case "${RESULTS[$i]}" in
    PASS) color=$GREEN ;;
    FAIL) color=$RED failed=1 ;;
    *) color=$YELLOW ;;
  esac
  # Pad outside the color codes so the columns stay aligned.
  printf "| %-${w1}s | %s%-${w2}s%s | %${w3}s | %-${w4}s |\n" \
    "${NAMES[$i]}" "$color" "${RESULTS[$i]}" "$RESET" "${TIMES[$i]}" "${DETAILS[$i]}"
done
rule

if [ "$failed" -eq 1 ]; then
  for i in "${!NAMES[@]}"; do
    [ "${RESULTS[$i]}" = FAIL ] || continue
    echo
    echo "${RED}${BOLD}${NAMES[$i]} failed${RESET} — last lines of $LOGS/${NAMES[$i]}.log:"
    tail -n 25 "$LOGS/${NAMES[$i]}.log" | sed 's/^/  /'
  done
  exit 1
fi
echo "${GREEN}${BOLD}All checks passed.${RESET} Logs: $LOGS/"
