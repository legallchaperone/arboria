#!/bin/sh
set -e
cd "$(dirname "$0")"
APP=build/Enclave.app
rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS"
swiftc -O -o "$APP/Contents/MacOS/Enclave" main.swift
cat > "$APP/Contents/Info.plist" <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>world.cwx.arboria.p0.enclave</string>
<key>CFBundleExecutable</key><string>Enclave</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>
PL
xattr -cr "$APP"; codesign -s - --force "$APP"
