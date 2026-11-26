#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VER="$(awk -F '"' '/^version = / { print $2; exit }' "$ROOT/Cargo.toml")"
OUT="$ROOT/target/linux"
BIN="$ROOT/target/release/txplayer"

cd "$ROOT"
if [[ "${1:-}" != "--skip-build" ]]; then
    cargo build --release
fi

rm -rf "$OUT"
mkdir -p "$OUT/deb/DEBIAN" \
         "$OUT/deb/usr/bin" \
         "$OUT/deb/usr/share/applications" \
         "$OUT/deb/usr/share/icons/hicolor/256x256/apps" \
         "$OUT/deb/usr/share/doc/txplayer" \
         "$OUT/portable"

# 1. Debian / Ubuntu .deb package
cp "$BIN" "$OUT/deb/usr/bin/txplayer"
chmod 0755 "$OUT/deb/usr/bin/txplayer"
cp "$ROOT/packaging/linux/txplayer.desktop" "$OUT/deb/usr/share/applications/txplayer.desktop"
cp "$ROOT/assets/AppIcon.png" "$OUT/deb/usr/share/icons/hicolor/256x256/apps/txplayer.png"
cp "$ROOT/LICENSE" "$OUT/deb/usr/share/doc/txplayer/copyright"

INSTALLED_KB="$(du -sk "$OUT/deb/usr" | awk '{print $1}')"
cat > "$OUT/deb/DEBIAN/control" <<EOF
Package: txplayer
Version: ${VER}
Section: sound
Priority: optional
Architecture: amd64
Installed-Size: ${INSTALLED_KB}
Depends: libasound2t64 | libasound2, libgtk-3-0t64 | libgtk-3-0, libxkbcommon0
Maintainer: Tuxie <tuxie@tuxie.eu>
Homepage: https://tuxie.eu/txplayer/
Description: Native Rust audiophile music player, 88-key visualizer & master library
 TxPlayer is a 100% free, native Rust music player featuring Bit-Perfect DAC
 output, native DoP v1.1 DSD64-512 & SACD ISO playback, 64-bit DSP with
 interactive EQ & FFT convolution, 88-key Piano & Grand Staff visualizers,
 Blind ABX Lab, and an offline master library database.
EOF

dpkg-deb --root-owner-group --build "$OUT/deb" "$OUT/txplayer_${VER}_amd64.deb"

# 2. Universal Linux portable .tar.gz with user-local desktop installer
cp "$BIN" "$OUT/portable/txplayer"
chmod 0755 "$OUT/portable/txplayer"
cp "$ROOT/packaging/linux/txplayer.desktop" "$OUT/portable/txplayer.desktop"
cp "$ROOT/assets/AppIcon.png" "$OUT/portable/txplayer.png"
cp "$ROOT/README.md" "$OUT/portable/README.md"
cp "$ROOT/LICENSE" "$OUT/portable/LICENSE"
cat > "$OUT/portable/install.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications" "$HOME/.local/share/icons/hicolor/256x256/apps"
install -m 0755 "$DIR/txplayer" "$HOME/.local/bin/txplayer"
install -m 0644 "$DIR/txplayer.png" "$HOME/.local/share/icons/hicolor/256x256/apps/txplayer.png"
sed "s|^Exec=txplayer|Exec=$HOME/.local/bin/txplayer|" "$DIR/txplayer.desktop" > "$HOME/.local/share/applications/txplayer.desktop"
echo "Installed TxPlayer to $HOME/.local/bin/txplayer"
EOF
chmod 0755 "$OUT/portable/install.sh"

tar -czf "$OUT/txplayer-v${VER}-linux-x86_64.tar.gz" -C "$OUT/portable" .

printf '%s\n' "$OUT/txplayer_${VER}_amd64.deb"
printf '%s\n' "$OUT/txplayer-v${VER}-linux-x86_64.tar.gz"
