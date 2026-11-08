#!/bin/sh
# Builds and runs nox from a minimal dev bundle (target/debug/nox.app), so the menu bar, the Dock
# tooltip and the app switcher say "nox" instead of the binary's file name "nox-app" (the Dock
# names a bare executable after its file). The binaries are hard-linked, not copied.
set -eu
cd "$(dirname "$0")/.."
cargo build -p nox --bin nox-app -p nox_cli
APP=target/debug/nox.app
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/bin"
cat > "$APP/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>nox</string>
	<key>CFBundleDisplayName</key>
	<string>nox</string>
	<key>CFBundleExecutable</key>
	<string>nox-app</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
EOF
ln -f target/debug/nox-app "$APP/Contents/MacOS/nox-app"
ln -f target/debug/nox "$APP/Contents/Resources/bin/nox"
exec "$APP/Contents/MacOS/nox-app" "$@"
