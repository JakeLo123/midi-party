#!/bin/bash
# Builds the release binary and assembles target/bundle/MIDI Party.app.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"

cargo build -p mp-app --release

version="$(cargo metadata --no-deps --format-version 1 | python3 -I -c '
import json, sys
meta = json.load(sys.stdin)
for pkg in meta["packages"]:
    if any(t["name"] == "midi-party" and "bin" in t["kind"] for t in pkg["targets"]):
        print(pkg["version"])
        break
else:
    sys.exit("no package in the workspace builds a midi-party binary")
')"

bundle="$repo/target/bundle/MIDI Party.app"
contents="$bundle/Contents"
rm -rf "$bundle"
mkdir -p "$contents/MacOS" "$contents/Resources/assets"

cp target/release/midi-party "$contents/MacOS/midi-party"
cp -R assets/soundfonts "$contents/Resources/assets/soundfonts"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
iconset="$tmp/icon.iconset"
mkdir "$iconset"
for px in 16 32 128 256 512; do
    cp "assets/icon/icon-$px.png" "$iconset/icon_${px}x${px}.png"
    cp "assets/icon/icon-$((px * 2)).png" "$iconset/icon_${px}x${px}@2x.png"
done
/usr/bin/iconutil -c icns "$iconset" -o "$contents/Resources/icon.icns"

cat > "$contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>MIDI Party</string>
    <key>CFBundleDisplayName</key>
    <string>MIDI Party</string>
    <key>CFBundleIdentifier</key>
    <string>com.jakeloew.midi-party</string>
    <key>CFBundleExecutable</key>
    <string>midi-party</string>
    <key>CFBundleIconFile</key>
    <string>icon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

echo "$bundle"
