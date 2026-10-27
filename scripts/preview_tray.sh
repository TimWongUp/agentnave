#!/bin/sh
# Build the current checkout's tray as "AgentNave 预览" and open it beside the installed app.
# The preview reads the same data but never writes, updates or installs anything (PREVIEW).
set -eu

[ "$(uname)" = Darwin ] || { echo "预览版目前只支持 macOS" >&2; exit 1; }

ROOT=$(cd "$(dirname "$0")/.." && pwd)
TRAY="$ROOT/apps/tray"
ID=io.github.timwongup.agentnave.preview
NAME="AgentNave 预览"
APP="$HOME/Applications/$NAME.app"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$TRAY/Cargo.toml" | head -n 1)

# Its own target dir, so the preview flags never mix with a normal build.
AGENTNAVE_PREVIEW=1 \
TAURI_CONFIG="{\"identifier\":\"$ID\",\"productName\":\"$NAME\"}" \
CARGO_TARGET_DIR="$TRAY/target/preview" \
  cargo build --manifest-path "$TRAY/Cargo.toml"

osascript -e "quit app id \"$ID\"" 2>/dev/null || true
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$TRAY/target/preview/debug/agentnave-tray" "$APP/Contents/MacOS/agentnave-tray"
cp "$TRAY/icons/icon.icns" "$APP/Contents/Resources/icon.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDisplayName</key><string>$NAME</string>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleExecutable</key><string>agentnave-tray</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>CFBundleIdentifier</key><string>$ID</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
# Ad-hoc signed: enough to run locally and for screenshots, not for distribution.
codesign --force --deep --sign - "$APP" >/dev/null
open "$APP"
echo "已打开 $APP"
