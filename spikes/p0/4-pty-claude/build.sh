#!/bin/sh
# CLT only. xterm.js is fetched from npm into build/ (pinned), never committed.
set -e
cd "$(dirname "$0")"
APP=build/ClaudeTerm.app
[ -f build/npm/node_modules/@xterm/xterm/lib/xterm.js ] || npm install --silent --prefix build/npm @xterm/xterm@5.5.0
rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/web" "$APP/Contents/Resources/fonts"
swiftc -O -o "$APP/Contents/MacOS/ClaudeTerm" main.swift -framework AppKit -framework WebKit
cp web/term.html build/npm/node_modules/@xterm/xterm/lib/xterm.js build/npm/node_modules/@xterm/xterm/css/xterm.css "$APP/Contents/Resources/web/"
cp ../../../src/arboria/ui/fonts/MapleMono-CN-Regular.woff2 "$APP/Contents/Resources/fonts/"
printf '%s' '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>world.cwx.arboria.p0.term</string><key>CFBundleExecutable</key><string>ClaudeTerm</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>' > "$APP/Contents/Info.plist"
xattr -cr "$APP"; codesign -s - --force "$APP"
