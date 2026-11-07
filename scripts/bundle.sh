#!/usr/bin/env bash
# Build nox.app (SPEC §3, §15): release binaries, Info.plist, icon, bundled assets, the `nox`
# CLI in Contents/Resources/bin. Ad-hoc signed unless SIGN_IDENTITY is set (see release/sign.sh).
#
#   scripts/bundle.sh            # → dist/nox.app
#   BUILD_NUMBER=42 scripts/bundle.sh
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET=aarch64-apple-darwin
VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/version = "(.*)"/\1/')
BUILD=${BUILD_NUMBER:-1}
FEATURES=${NOX_FEATURES:-}
DIST=dist
APP=$DIST/nox.app

echo "==> building nox $VERSION ($BUILD) for $TARGET"
cargo build --release --target "$TARGET" -p nox -p nox_cli ${FEATURES:+--features "$FEATURES"}

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/bin" "$APP/Contents/Resources/assets"
cp "target/$TARGET/release/nox-app" "$APP/Contents/MacOS/nox-app"
cp "target/$TARGET/release/nox" "$APP/Contents/Resources/bin/nox"

# Bundled assets: fonts (with licenses), sounds. Icons are compiled into the binary.
cp -R assets/fonts "$APP/Contents/Resources/assets/fonts"
cp -R assets/sounds "$APP/Contents/Resources/assets/sounds"
cp LICENSE "$APP/Contents/Resources/LICENSE"

# Info.plist with versions.
sed -e "s/__VERSION__/$VERSION/" -e "s/__BUILD__/$BUILD/" assets/app/Info.plist > "$APP/Contents/Info.plist"
if [[ ",$FEATURES," == *",updater,"* ]]; then
  : "${SU_PUBLIC_ED_KEY:?SU_PUBLIC_ED_KEY is required with the updater feature}"
  SPARKLE="<key>SUFeedURL</key><string>https://github.com/pwittchen/nox/releases/latest/download/appcast.xml</string><key>SUPublicEDKey</key><string>$SU_PUBLIC_ED_KEY</string><key>SUScheduledCheckInterval</key><integer>86400</integer><key>SUEnableSystemProfiling</key><false/>"
  sed -i '' "s#<!--__SPARKLE__-->#$SPARKLE#" "$APP/Contents/Info.plist"
fi

# Icon: Assets.xcassets images → .iconset → nox.icns (sips normalizes them to plain PNG).
ICONSET=$(mktemp -d)/nox.iconset
mkdir -p "$ICONSET"
SRC=Assets.xcassets/AppIcon.appiconset
png() { sips -s format png "$SRC/$1-mac.png" --out "$ICONSET/$2" >/dev/null; }
png 16 icon_16x16.png
png 32 icon_16x16@2x.png
png 32 icon_32x32.png
png 64 icon_32x32@2x.png
png 128 icon_128x128.png
png 256 icon_128x128@2x.png
png 256 icon_256x256.png
png 512 icon_256x256@2x.png
png 512 icon_512x512.png
png 1024 icon_512x512@2x.png
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/nox.icns"

if [[ -n "${SIGN_IDENTITY:-}" ]]; then
  scripts/release/sign.sh "$APP"
else
  codesign --force --sign - --entitlements assets/app/nox.entitlements "$APP/Contents/Resources/bin/nox"
  codesign --force --sign - --entitlements assets/app/nox.entitlements "$APP"
fi
echo "==> $APP ($(du -sh "$APP" | cut -f1))"
