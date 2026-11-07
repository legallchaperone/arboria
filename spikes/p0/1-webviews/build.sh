#!/bin/sh
# Build the spike as a hand-rolled .app with Command Line Tools only (no Xcode).
set -e
cd "$(dirname "$0")"
APP=build/ArboriaP0.app
rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/fonts"
swiftc -O -o "$APP/Contents/MacOS/ArboriaP0" main.swift -framework AppKit -framework WebKit
cp -R web "$APP/Contents/Resources/web"
cp ../../../src/arboria/ui/fonts/*.woff2 "$APP/Contents/Resources/fonts/"
cat > "$APP/Contents/Info.plist" <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>world.cwx.arboria.p0</string>
<key>CFBundleName</key><string>ArboriaP0</string>
<key>CFBundleExecutable</key><string>ArboriaP0</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
</dict></plist>
PL
xattr -cr "$APP"; codesign -s - --force "$APP"
echo built "$APP"
