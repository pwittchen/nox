#!/usr/bin/env bash
# Runs the local checks CI runs — cargo audit, fmt check, lint (clippy::pedantic), tests — and
# prints a summary table. Every step runs even if an earlier one fails; the exit code is non-zero
# if any step failed. Full output of each step goes to target/test-logs/<step>.log; the tail of a
# failing step's log is printed after the table.
#
# Usage: scripts/test.sh [-v] [--coverage] [audit] [fmt] [lint] [test]
#   no step names: all of them, in that order
#   -v: stream each step's output instead of only logging it
#   --coverage: run the tests under cargo-llvm-cov, like CI, and print a per-crate coverage
#               table (slower; needs cargo-llvm-cov and jq)
set -u
cd "$(dirname "$0")/.."

VERBOSE=0
COVERAGE=0
STEPS=()
for arg in "$@"; do
  case "$arg" in
    -v | --verbose) VERBOSE=1 ;;
    --coverage) COVERAGE=1 ;;
    -h | --help)
      sed -n '2,/^set /p' "$0" | grep '^#' | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    audit | fmt | lint | test) STEPS+=("$arg") ;;
    *)
      echo "unknown argument: $arg (expected audit, fmt, lint, test, -v or --coverage)" >&2
      exit 2
      ;;
  esac
done
[ ${#STEPS[@]} -eq 0 ] && STEPS=(audit fmt lint test)

LOGS=target/test-logs
mkdir -p "$LOGS"
COV_JSON=$LOGS/coverage.json
rm -f "$COV_JSON"
COV_NOTE=""
if [ "$COVERAGE" -eq 1 ]; then
  if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
    COVERAGE=0 COV_NOTE="no coverage: cargo-llvm-cov not installed (cargo install cargo-llvm-cov)"
  elif ! command -v jq >/dev/null 2>&1; then
    COVERAGE=0 COV_NOTE="no coverage: jq not installed"
  fi
fi

if [ -t 1 ]; then
  GREEN=$'\033[32m' RED=$'\033[31m' YELLOW=$'\033[33m' BOLD=$'\033[1m' DIM=$'\033[2m' RESET=$'\033[0m'
else
  GREEN='' RED='' YELLOW='' BOLD='' DIM='' RESET=''
fi

# Results, one entry per step (bash 3.2 has no associative arrays).
NAMES=() RESULTS=() TIMES=() DETAILS=()

# Wall-clock seconds with millisecond precision (bash 3.2 has no $EPOCHREALTIME; perl ships with
# macOS). Falls back to whole seconds.
now() {
  perl -MTime::HiRes=time -e 'printf "%.3f", time' 2>/dev/null || date +%s
}

# `0.4s`, `12.3s`, `1m 05.2s` for the seconds between two `now` values.
duration() {
  awk -v a="$1" -v b="$2" 'BEGIN {
    d = b - a; if (d < 0) d = 0
    if (d >= 60) printf "%dm %04.1fs", int(d / 60), d - 60 * int(d / 60)
    else printf "%.1fs", d
  }'
}

command_for() {
  case "$1" in
    audit) echo "cargo audit --deny warnings" ;;
    fmt) echo "cargo fmt --all -- --check --color never" ;;
    lint) echo "cargo lint" ;;
    test)
      if [ "$COVERAGE" -eq 1 ]; then
        echo "cargo llvm-cov --workspace --json --summary-only --output-path $COV_JSON"
      else
        echo "cargo test --workspace"
      fi
      ;;
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
      if [ -s "$COV_JSON" ]; then
        jq -r '.data[0].totals.lines | if .count == 0 then "" else
          ", \((.covered * 1000 / .count | round) / 10 | tostring | if test("[.]") then . else . + ".0" end) % lines covered" end' "$COV_JSON"
      fi
      ;;
  esac
}

# Pad the commands so the live PASS / FAIL column lines up.
CMD_W=0
for step in "${STEPS[@]}"; do
  c=$(command_for "$step")
  c=${c%% --json*}
  [ ${#c} -gt $CMD_W ] && CMD_W=${#c}
done

TOTAL_START=$(now)
for step in "${STEPS[@]}"; do
  log="$LOGS/$step.log"
  cmd=$(command_for "$step")
  # Shown without llvm-cov's report flags, to keep the line short.
  printf "%s▸ %-6s%s %s%-${CMD_W}s%s" "$BOLD" "$step" "$RESET" "$DIM" "${cmd%% --json*}" "$RESET"
  [ "$VERBOSE" -eq 1 ] && echo
  if [ "$step" = audit ] && ! command -v cargo-audit >/dev/null 2>&1; then
    NAMES+=("$step") RESULTS+=(SKIP) TIMES+=(-) DETAILS+=("cargo-audit not installed (cargo install cargo-audit)")
    printf '  %sSKIP%s\n' "$YELLOW" "$RESET"
    continue
  fi
  start=$(now)
  if [ "$VERBOSE" -eq 1 ]; then
    $cmd 2>&1 | tee "$log"
    status=${PIPESTATUS[0]}
  else
    $cmd >"$log" 2>&1
    status=$?
  fi
  took=$(duration "$start" "$(now)")
  if [ "$status" -eq 0 ]; then result=PASS color=$GREEN; else result=FAIL color=$RED; fi
  [ "$VERBOSE" -eq 1 ] && printf '%s▸ %-6s%s' "$BOLD" "$step" "$RESET"
  printf '  %s%s%s %s\n' "$color" "$result" "$RESET" "$took"
  NAMES+=("$step") RESULTS+=("$result") TIMES+=("$took")
  DETAILS+=("$(detail_for "$step" "$log" "$status" | tr -d '\n')")
done

TOTAL=$(duration "$TOTAL_START" "$(now)")

# ---- summary table ----------------------------------------------------------------------------

passed=0 failed=0
for r in "${RESULTS[@]}"; do
  case "$r" in
    PASS) passed=$((passed + 1)) ;;
    FAIL) failed=$((failed + 1)) ;;
  esac
done
TOTAL_DETAIL="$passed of ${#NAMES[@]} passed"

w1=5 w2=6 w3=${#TOTAL} w4=${#TOTAL_DETAIL} # widths: "total", "Result", time, details
[ $w4 -lt 7 ] && w4=7
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
for i in "${!NAMES[@]}"; do
  case "${RESULTS[$i]}" in
    PASS) color=$GREEN ;;
    FAIL) color=$RED ;;
    *) color=$YELLOW ;;
  esac
  # Pad outside the color codes so the columns stay aligned.
  printf "| %-${w1}s | %s%-${w2}s%s | %${w3}s | %-${w4}s |\n" \
    "${NAMES[$i]}" "$color" "${RESULTS[$i]}" "$RESET" "${TIMES[$i]}" "${DETAILS[$i]}"
done
rule
if [ "$failed" -gt 0 ]; then total_result=FAIL color=$RED; else total_result=PASS color=$GREEN; fi
printf "| %s%-${w1}s%s | %s%-${w2}s%s | %s%${w3}s%s | %-${w4}s |\n" "$BOLD" total "$RESET" \
  "$color" "$total_result" "$RESET" "$BOLD" "$TOTAL" "$RESET" "$TOTAL_DETAIL"
rule

# ---- coverage table ---------------------------------------------------------------------------

# Per crate + total, from the `cargo llvm-cov --json --summary-only` report (same grouping as
# scripts/ci/coverage_summary.sh). Rows are tab-separated, then laid out as an ASCII table.
if [ -s "$COV_JSON" ]; then
  echo
  echo "${BOLD}Test coverage${RESET}"
  jq -r '
    def pct(c; n): if n == 0 then "-" else
      "\((c * 1000 / n | round) / 10 | tostring | if test("[.]") then . else . + ".0" end) %" end;
    def row(name; s): [name, pct(s.lines.covered; s.lines.count), "\(s.lines.covered)/\(s.lines.count)",
                       pct(s.functions.covered; s.functions.count), pct(s.regions.covered; s.regions.count)];
    .data[0] as $d
    | (["Crate", "Lines", "Covered", "Functions", "Regions"] | @tsv),
      ([ $d.files[]
         | select(.filename | test("/crates/[^/]+/"))
         | { crate: (.filename | capture("/crates/(?<c>[^/]+)/").c), s: .summary } ]
       | group_by(.crate)
       | .[]
       | row(.[0].crate; { lines:     { covered: (map(.s.lines.covered) | add),     count: (map(.s.lines.count) | add) },
                           functions: { covered: (map(.s.functions.covered) | add), count: (map(.s.functions.count) | add) },
                           regions:   { covered: (map(.s.regions.covered) | add),   count: (map(.s.regions.count) | add) } })
       | @tsv),
      (row("total"; $d.totals) | @tsv)
  ' "$COV_JSON" | awk -F '\t' -v bold="$BOLD" -v reset="$RESET" '
    { n = NR; for (i = 1; i <= NF; i++) { cell[n, i] = $i; if (length($i) > w[i]) w[i] = length($i) } }
    function rule(   i, line) {
      line = "+"; for (i = 1; i <= 5; i++) { line = line "-"; for (j = 0; j < w[i]; j++) line = line "-"; line = line "-+" }
      print line
    }
    function row(r, b,   i, line) {
      line = "|"
      for (i = 1; i <= 5; i++)
        line = line " " (b ? bold : "") sprintf(i == 1 ? "%-" w[i] "s" : "%" w[i] "s", cell[r, i]) (b ? reset : "") " |"
      print line
    }
    END {
      rule(); row(1, 0); rule()
      for (r = 2; r < n; r++) row(r, 0)
      rule(); row(n, 1); rule()
    }'
elif [ -n "$COV_NOTE" ] && printf '%s\n' "${STEPS[@]}" | grep -qx test; then
  echo "${DIM}${COV_NOTE}${RESET}"
fi

if [ "$failed" -gt 0 ]; then
  for i in "${!NAMES[@]}"; do
    [ "${RESULTS[$i]}" = FAIL ] || continue
    echo
    echo "${RED}${BOLD}${NAMES[$i]} failed${RESET} — last lines of $LOGS/${NAMES[$i]}.log:"
    tail -n 25 "$LOGS/${NAMES[$i]}.log" | sed 's/^/  /'
  done
  echo
  echo "${RED}${BOLD}$failed of ${#NAMES[@]} checks failed${RESET} in $TOTAL. Logs: $LOGS/"
  exit 1
fi
echo "${GREEN}${BOLD}All checks passed${RESET} in $TOTAL. Logs: $LOGS/"
