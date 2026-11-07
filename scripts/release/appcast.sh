#!/usr/bin/env bash
# Update archive + appcast (SPEC §15.3). Needs Sparkle's `sign_update` on PATH and
# SPARKLE_ED_PRIVATE_KEY. Appends to the previous appcast so it keeps recent history.
#   appcast.sh dist/nox.app <version> <build> [previous-appcast.xml]
set -euo pipefail
APP=${1:?app}; VERSION=${2:?version}; BUILD=${3:?build}; PREV=${4:-}
ZIP=dist/nox-macos-aarch64.zip
ditto -c -k --keepParent "$APP" "$ZIP"
SIG=$(echo "$SPARKLE_ED_PRIVATE_KEY" | sign_update --ed-key-file - "$ZIP")
LEN=$(stat -f %z "$ZIP")
URL="https://github.com/pwittchen/nox/releases/download/v$VERSION/nox-macos-aarch64.zip"
ITEM="<item><title>nox $VERSION</title><sparkle:version>$BUILD</sparkle:version><sparkle:shortVersionString>$VERSION</sparkle:shortVersionString><sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion><sparkle:releaseNotesLink>https://github.com/pwittchen/nox/releases/tag/v$VERSION</sparkle:releaseNotesLink><pubDate>$(date -R)</pubDate><enclosure url=\"$URL\" $SIG length=\"$LEN\" type=\"application/octet-stream\"/></item>"
OLD_ITEMS=""
if [[ -n "$PREV" && -f "$PREV" ]]; then
  OLD_ITEMS=$(/usr/bin/python3 -c 'import re,sys; s=open(sys.argv[1]).read(); print("".join(re.findall(r"<item>.*?</item>", s, re.S)[:4]))' "$PREV")
fi
cat > dist/appcast.xml <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
<channel><title>nox</title>$ITEM$OLD_ITEMS</channel></rss>
XML
(cd dist && shasum -a 256 nox-macos-aarch64.dmg nox-macos-aarch64.zip > SHA256SUMS)
