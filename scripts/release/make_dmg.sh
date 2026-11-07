#!/usr/bin/env bash
# Build nox-macos-aarch64.dmg: the (notarized, stapled) app plus an /Applications symlink
# (SPEC §15.2 step 9). Signed when SIGN_IDENTITY is set; notarize.sh then notarizes it.
set -euo pipefail
APP=${1:-dist/nox.app}
OUT=${2:-dist/nox-macos-aarch64.dmg}
STAGE=$(mktemp -d)/nox
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$OUT"
# hdiutil occasionally fails with "Resource busy" on GitHub runners, so retry.
for attempt in 1 2 3; do
  hdiutil detach /Volumes/nox -force 2>/dev/null || true
  if hdiutil create -volname nox -srcfolder "$STAGE" -ov -format UDZO -imagekey zlib-level=9 "$OUT"; then
    break
  fi
  [[ $attempt == 3 ]] && exit 1
  echo "hdiutil create failed (attempt $attempt), retrying in 5s" >&2
  sleep 5
done
if [[ -n "${SIGN_IDENTITY:-}" ]]; then
  codesign --force --timestamp --sign "$SIGN_IDENTITY" "$OUT"
  codesign --verify --verbose=2 "$OUT"
fi
echo "==> $OUT ($(du -h "$OUT" | cut -f1))"
