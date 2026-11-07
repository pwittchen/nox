#!/usr/bin/env bash
# Notarize and staple (SPEC §15.2 steps 7–9) with an App Store Connect API key.
#   APPSTORE_API_KEY_PATH=key.p8 APPSTORE_API_KEY_ID=… APPSTORE_API_ISSUER_ID=… notarize.sh dist/nox-macos-aarch64.dmg dist/nox.app
set -euo pipefail
DMG=${1:?dmg}
APP=${2:?app}
: "${APPSTORE_API_KEY_PATH:?}" "${APPSTORE_API_KEY_ID:?}" "${APPSTORE_API_ISSUER_ID:?}"
OUT=$(xcrun notarytool submit "$DMG" --key "$APPSTORE_API_KEY_PATH" --key-id "$APPSTORE_API_KEY_ID" \
  --issuer "$APPSTORE_API_ISSUER_ID" --wait --timeout 30m --output-format json)
echo "$OUT"
STATUS=$(echo "$OUT" | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])')
if [[ "$STATUS" != "Accepted" ]]; then
  ID=$(echo "$OUT" | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
  xcrun notarytool log "$ID" --key "$APPSTORE_API_KEY_PATH" --key-id "$APPSTORE_API_KEY_ID" --issuer "$APPSTORE_API_ISSUER_ID"
  exit 1
fi
xcrun stapler staple "$APP"
xcrun stapler staple "$DMG"
codesign --verify --strict --verbose=2 "$APP"
spctl --assess --type execute -vv "$APP"
spctl --assess --type open --context context:primary-signature -vv "$DMG"
xcrun stapler validate "$DMG"
