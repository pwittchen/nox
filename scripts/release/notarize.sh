#!/usr/bin/env bash
# Notarize and staple (SPEC §15.2 steps 6–9) with an Apple ID + app-specific password.
#   APPLE_ID=… APPLE_PASSWORD=… APPLE_TEAM_ID=… notarize.sh dist/nox.app
#   APPLE_ID=… APPLE_PASSWORD=… APPLE_TEAM_ID=… notarize.sh dist/nox-macos-aarch64.dmg
# notarytool takes zip / dmg / pkg, so an app is submitted as a zip. The ticket is stapled to the
# app (Gatekeeper accepts it offline once copied out of the dmg) and to the dmg (it opens
# without a warning when downloaded).
set -euo pipefail
TARGET=${1:?usage: notarize.sh path/to/nox.app | path/to/nox.dmg}
: "${APPLE_ID:?}" "${APPLE_PASSWORD:?}" "${APPLE_TEAM_ID:?}"
AUTH=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")

SUBMIT=$TARGET
if [[ "$TARGET" == *.app ]]; then
  SUBMIT="$(mktemp -d)/nox-notarize.zip"
  ditto -c -k --keepParent "$TARGET" "$SUBMIT"
fi
OUT=$(xcrun notarytool submit "$SUBMIT" "${AUTH[@]}" --wait --timeout 30m --output-format json)
echo "$OUT"
[[ "$SUBMIT" != "$TARGET" ]] && rm -f "$SUBMIT"
STATUS=$(echo "$OUT" | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))')
if [[ "$STATUS" != "Accepted" ]]; then
  ID=$(echo "$OUT" | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin).get("id",""))')
  [[ -n "$ID" ]] && xcrun notarytool log "$ID" "${AUTH[@]}"
  exit 1
fi
# stapler talks to Apple's CloudKit, which drops connections now and then (NSURLErrorDomain
# -1005, exit 68); retry so a network blip doesn't fail an already accepted release.
retry() {
  local attempt
  for attempt in 1 2 3 4 5; do
    "$@" && return 0
    echo "attempt $attempt failed: $*" >&2
    [[ $attempt -lt 5 ]] && sleep $((attempt * 10))
  done
  return 1
}
retry xcrun stapler staple "$TARGET"
retry xcrun stapler validate "$TARGET"
if [[ "$TARGET" == *.app ]]; then
  codesign --verify --strict --verbose=2 "$TARGET"
  spctl --assess --type execute -vv "$TARGET"
else
  spctl --assess --type open --context context:primary-signature -vv "$TARGET"
fi
