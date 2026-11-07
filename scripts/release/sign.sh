#!/usr/bin/env bash
# Sign nox.app inside-out with the Developer ID identity, hardened runtime, never --deep
# (SPEC §15.2 step 4). Usage: SIGN_IDENTITY="Developer ID Application: …" sign.sh dist/nox.app
set -euo pipefail
APP=${1:?usage: sign.sh path/to/nox.app}
: "${SIGN_IDENTITY:?SIGN_IDENTITY is required}"
ENT="$(dirname "$0")/../../assets/app/nox.entitlements"
sign() { codesign --force --options runtime --timestamp --sign "$SIGN_IDENTITY" "$@"; }

SPARKLE="$APP/Contents/Frameworks/Sparkle.framework"
if [[ -d "$SPARKLE" ]]; then
  for xpc in "$SPARKLE"/Versions/B/XPCServices/*.xpc; do sign "$xpc"; done
  sign "$SPARKLE/Versions/B/Autoupdate"
  sign "$SPARKLE/Versions/B/Updater.app"
  sign "$SPARKLE"
fi
sign "$APP/Contents/Resources/bin/nox"
sign --entitlements "$ENT" "$APP"
codesign --verify --strict --verbose=2 "$APP"
