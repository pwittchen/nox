#!/usr/bin/env bash
# Prints a Markdown summary (per crate + total) of a
# `cargo llvm-cov --json --summary-only` report (CI step summary).
# Usage: scripts/ci/coverage_summary.sh coverage.json
set -euo pipefail

report="${1:?usage: coverage_summary.sh <cargo llvm-cov JSON report>}"

if [[ ! -s "$report" ]]; then
  echo "## Test coverage"
  echo
  echo "No report was produced (see the test step's log)."
  exit 0
fi

jq -r '
  def pct(c; n): if n == 0 then "–" else "\((c * 1000 / n | round) / 10) %" end;
  def cells(s): "\(pct(s.lines.covered; s.lines.count)) (\(s.lines.covered)/\(s.lines.count)) | \(pct(s.functions.covered; s.functions.count)) | \(pct(s.regions.covered; s.regions.count))";
  .data[0] as $d
  | [ $d.files[]
      | select(.filename | test("/crates/[^/]+/"))
      | { crate: (.filename | capture("/crates/(?<c>[^/]+)/").c), s: .summary } ]
  | group_by(.crate)
  | map({ crate: .[0].crate,
          s: { lines:     { covered: (map(.s.lines.covered) | add),     count: (map(.s.lines.count) | add) },
               functions: { covered: (map(.s.functions.covered) | add), count: (map(.s.functions.count) | add) },
               regions:   { covered: (map(.s.regions.covered) | add),   count: (map(.s.regions.count) | add) } } })
  | "## Test coverage",
    "",
    "| Crate | Lines | Functions | Regions |",
    "|---|---|---|---|",
    (.[] | "| \(.crate) | \(cells(.s)) |"),
    "| **Total** | \(cells($d.totals)) |"
' "$report"
