#!/bin/sh
# Build Arboria.app with Command Line Tools only (no Xcode): swift build, then a hand-made
# bundle with the web UI, the fonts and xterm.js (pinned, fetched from npm into .build/).
set -e
cd "$(dirname "$0")"
NPM=.build/npm
[ -f $NPM/node_modules/@xterm/addon-fit/lib/addon-fit.js ] || \
  npm install --silent --no-save --prefix $NPM @xterm/xterm@5.5.0 @xterm/addon-fit@0.10.0
# the agent drawer's ACP adapter, pinned in acp/package-lock.json; --omit=optional leaves out
# the agent SDK's bundled claude binary (the drawer runs the user's own claude)
[ -f acp/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js ] || \
  (cd acp && npm ci --silent --omit=optional --ignore-scripts --no-audit --no-fund)
swift build -c release
APP=build/Arboria.app
rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/fonts"
cp .build/release/Arboria "$APP/Contents/MacOS/Arboria"
cp -R web "$APP/Contents/Resources/web"
python3 design/derive.py "$APP/Contents/Resources/web"   # tokens.css, ab-tokens.css
cp design/bundle.css "$APP/Contents/Resources/web/bundle.css"
cp $NPM/node_modules/@xterm/xterm/lib/xterm.js $NPM/node_modules/@xterm/xterm/css/xterm.css \
   $NPM/node_modules/@xterm/addon-fit/lib/addon-fit.js "$APP/Contents/Resources/web/"
mkdir -p "$APP/Contents/Resources/acp" && cp -R acp/node_modules "$APP/Contents/Resources/acp/node_modules"
cp ../src/arboria/ui/fonts/MapleMono-CN-*.woff2 "$APP/Contents/Resources/fonts/"   # the design system uses only Maple Mono CN
cat > "$APP/Contents/Info.plist" <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>world.cwx.arboria</string>
<key>CFBundleName</key><string>Arboria</string>
<key>CFBundleExecutable</key><string>Arboria</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PL
chflags -R nohidden "$APP"; xattr -cr "$APP"
codesign -s - --force "$APP"
echo "built $APP"
