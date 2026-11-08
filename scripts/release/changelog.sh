#!/usr/bin/env bash
# Release notes + CHANGELOG.md for a tag (SPEC §15.2 step 11).
#
#   scripts/release/changelog.sh v1.2.0 [notes.md]
#
# Notes come from the `## [Unreleased]` section of CHANGELOG.md if it has content, else from
# the section of this version (`## [1.2.0] …`), else from the commit subjects since the previous
# `v*` tag (release commits excluded). Both sections are then replaced in CHANGELOG.md by an
# empty `## [Unreleased]` and `## [1.2.0] — <date>` with the notes, above all older sections.
# The notes are also written to the given file (default: notes.md).
set -euo pipefail
cd "$(dirname "$0")/../.."

TAG=${1:?tag}
NOTES=${2:-notes.md}
VERSION=${TAG#v}
DATE=$(date -u +%Y-%m-%d)
FILE=CHANGELOG.md
[[ -f $FILE ]] || printf '# Changelog\n' > "$FILE"

# A section heading is "## " followed by the version (optionally in brackets) or "Unreleased",
# then the end of the line or a non-version character (so 1.2.0 doesn't match 1.2.0-beta.1).
# kind(): 1 = Unreleased, 2 = this version, 0 = anything else.
SECTION_AWK='
  function kind(line,   h, rest) {
    if (line !~ /^## /) return 0
    h = substr(line, 4); sub(/^\[/, "", h)
    if (tolower(substr(h, 1, 10)) == "unreleased") return 1
    if (substr(h, 1, length(v)) != v) return 0
    rest = substr(h, length(v) + 1)
    return (rest == "" || rest ~ /^[] \t]/) ? 2 : 0
  }
'

# Trimmed body of the first section of the given kind.
section_body() {
  awk -v v="$VERSION" -v want="$1" "$SECTION_AWK"'
    /^## / { if (in_s) exit; in_s = (kind($0) == want); next }
    in_s { print }
  ' "$FILE" | sed -e '/./,$!d' | sed -e ':a' -e '/^\n*$/{$d;N;ba' -e '}'
}

BODY=$(section_body 1)
[[ -n "${BODY//[[:space:]]/}" ]] || BODY=$(section_body 2)

if [[ -z "${BODY//[[:space:]]/}" ]]; then
  PREV=$(git describe --tags --abbrev=0 --match 'v*' "$TAG^" 2>/dev/null || true)
  BODY=$(git log --no-merges --pretty=format:'- %s (%h)' ${PREV:+"$PREV.."}"$TAG" \
    | grep -v -E '^- Release [0-9]' || true)
  [[ -n "$BODY" ]] || BODY="- No changes."
fi
printf '%s\n' "$BODY" > "$NOTES"

# Drop the Unreleased and this version's sections; insert a fresh empty Unreleased section and
# the new one before the first remaining "## " heading.
awk -v v="$VERSION" -v head="## [$VERSION] — $DATE" -v body_file="$NOTES" "$SECTION_AWK"'
  function insert() {
    if (done) return
    print "## [Unreleased]"; print ""
    print head; print ""
    while ((getline l < body_file) > 0) print l
    print ""; done = 1
  }
  /^## / { if (kind($0)) { skip = 1; next } skip = 0; insert() }
  skip { next }
  { print }
  END { if (!done) { print ""; insert() } }
' "$FILE" > "$FILE.tmp"
# Collapse runs of blank lines and drop trailing ones.
cat -s "$FILE.tmp" | sed -e ':a' -e '/^\n*$/{$d;N;ba' -e '}' > "$FILE"
rm -f "$FILE.tmp"

echo "==> notes for $TAG"
cat "$NOTES"
