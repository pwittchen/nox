#!/usr/bin/env bash
# Prints a Markdown summary of a `cargo audit --json` report (CI step summary).
# Usage: scripts/ci/audit_summary.sh audit.json
set -euo pipefail

report="${1:?usage: audit_summary.sh <cargo-audit JSON report>}"

if [[ ! -s "$report" ]]; then
  echo "## cargo audit"
  echo
  echo "No report was produced (see the cargo audit step's log)."
  exit 0
fi

jq -r '
  def row: "| \(.advisory.id) | \(.package.name) \(.package.version) | \(.advisory.title | gsub("\\|"; "\\|")) |";
  (.vulnerabilities.list // []) as $vulns
  | ([.warnings // {} | to_entries[] | .value[] | select(.advisory != null)]) as $warns
  | (.settings.ignore // []) as $ignored
  | "## cargo audit",
    "",
    "| | |",
    "|---|---|",
    "| Dependencies scanned | \(.lockfile."dependency-count") |",
    "| Advisory database | \(.database."advisory-count") advisories, updated \(.database."last-updated" // "unknown") |",
    "| Vulnerabilities | \($vulns | length) |",
    "| Warnings | \($warns | length) |",
    "| Ignored (.cargo/audit.toml) | \($ignored | length) |",
    "",
    if ($vulns | length) > 0 then
      "### Vulnerabilities", "", "| Advisory | Package | Title |", "|---|---|---|",
      ($vulns[] | row), ""
    else empty end,
    if ($warns | length) > 0 then
      "### Warnings", "", "| Advisory | Package | Title |", "|---|---|---|",
      ($warns[] | row), ""
    else empty end,
    if ($ignored | length) > 0 then
      "<details><summary>Ignored advisories</summary>", "",
      ($ignored[] | "- \(.)"), "", "</details>"
    else empty end
' "$report"
