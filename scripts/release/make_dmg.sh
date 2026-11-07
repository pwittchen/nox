#!/usr/bin/env bash
# Build nox-macos-aarch64.dmg: the app plus an /Applications symlink (SPEC §15.2 step 6).
set -euo pipefail
APP=${1:-dist/nox.app}
OUT=${2:-dist/nox-macos-aarch64.dmg}
STAGE=$(mktemp -d)/nox
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$OUT"
hdiutil create -volname nox -srcfolder "$STAGE" -ov -format UDZO -imagekey zlib-level=9 "$OUT"
if [[ -n "${SIGN_IDENTITY:-}" ]]; then
  codesign --force --timestamp --sign "$SIGN_IDENTITY" "$OUT"
fi
echo "==> $OUT ($(du -h "$OUT" | cut -f1))"
