#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VER="$(awk -F '"' '/^version = / { print $2; exit }' "$ROOT/Cargo.toml")"
BUILD="$ROOT/target/macos"
APP="$BUILD/TxPlayer.app"
ICON_SOURCE="$ROOT/assets/AppIcon.png"

cd "$ROOT"
if [[ "${1:-}" != "--skip-build" ]]; then
    cargo build --release
fi

rm -rf "$APP" "$BUILD/dmg" "$BUILD/AppIcon.iconset"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$BUILD/dmg"
cp "$ROOT/target/release/txplayer" "$APP/Contents/MacOS/TxPlayer"
sed "s|<string>[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*</string>|<string>${VER}</string>|" \
    "$ROOT/packaging/Info.plist" > "$APP/Contents/Info.plist"

if [[ -f "$ICON_SOURCE" ]]; then
    ICONSET="$BUILD/AppIcon.iconset"
    mkdir -p "$ICONSET"
    for SIZE in 16 32 128 256 512; do
        DOUBLE=$((SIZE * 2))
        sips -z "$SIZE" "$SIZE" "$ICON_SOURCE" --out "$ICONSET/icon_${SIZE}x${SIZE}.png" >/dev/null
        sips -z "$DOUBLE" "$DOUBLE" "$ICON_SOURCE" --out "$ICONSET/icon_${SIZE}x${SIZE}@2x.png" >/dev/null
    done
    if ! (command -v iconutil >/dev/null && iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns" 2>/dev/null); then
        python3 - "$ICONSET" "$APP/Contents/Resources/AppIcon.icns" <<'PY'
import os, struct, sys
iconset, out_path = sys.argv[1], sys.argv[2]
mapping = [
    (b"icp4", "icon_16x16.png"),
    (b"ic11", "icon_16x16@2x.png"),
    (b"icp5", "icon_32x32.png"),
    (b"ic12", "icon_32x32@2x.png"),
    (b"ic07", "icon_128x128.png"),
    (b"ic13", "icon_128x128@2x.png"),
    (b"ic08", "icon_256x256.png"),
    (b"ic14", "icon_256x256@2x.png"),
    (b"ic09", "icon_512x512.png"),
    (b"ic10", "icon_512x512@2x.png"),
]
chunks = []
for tag, fname in mapping:
    p = os.path.join(iconset, fname)
    if os.path.isfile(p):
        data = open(p, "rb").read()
        chunks.append(tag + struct.pack(">I", 8 + len(data)) + data)
body = b"".join(chunks)
with open(out_path, "wb") as f:
    f.write(b"icns" + struct.pack(">I", 8 + len(body)) + body)
PY
    fi
fi

if command -v codesign >/dev/null; then
    codesign --force --deep --sign - "$APP" >/dev/null
fi

cp -R "$APP" "$BUILD/dmg/TxPlayer.app"
ln -s /Applications "$BUILD/dmg/Applications"
hdiutil create -quiet -ov -format UDZO -volname TxPlayer -srcfolder "$BUILD/dmg" "$BUILD/TxPlayer.dmg"

printf '%s\n' "$APP"
printf '%s\n' "$BUILD/TxPlayer.dmg"
