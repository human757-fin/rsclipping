#!/usr/bin/env bash
# Builds the Linux portable AppImage:
#   release/rsclipping-<VER>-x86_64.AppImage
#
# Requires: cargo, curl. Downloads appimagetool on first run.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"
echo "==> Building rsclipping $VERSION (release)"
cargo build --release

APP="release/AppDir"
rm -rf "$APP"
mkdir -p "$APP/usr/bin" \
         "$APP/usr/share/applications" \
         "$APP/usr/share/icons/hicolor/256x256/apps" \
         "$APP/usr/share/icons/hicolor/512x512/apps"

cp target/release/rsclipping "$APP/usr/bin/rsclipping"
cp packaging/linux/rsclipping.desktop "$APP/usr/share/applications/"
cp packaging/linux/rsclipping.desktop "$APP/rsclipping.desktop"
cp packaging/assets/rsclipping.png  "$APP/usr/share/icons/hicolor/256x256/apps/rsclipping.png"
cp packaging/assets/rsclipping.png  "$APP/usr/share/icons/hicolor/512x512/apps/rsclipping.png"
cp packaging/assets/rsclipping.png  "$APP/.DirIcon"
cp packaging/linux/AppRun "$APP/AppRun"
chmod +x "$APP/AppRun"

TOOL="release/appimagetool.AppImage"
if [ ! -x "$TOOL" ]; then
  echo "==> Downloading appimagetool"
  curl -fsSL -o "$TOOL" \
    https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
  chmod +x "$TOOL"
fi

OUT="release/rsclipping-${VERSION}-x86_64.AppImage"
echo "==> Packaging AppImage"
ARCH=x86_64 "$TOOL" --appimage-extract-and-run \
  --desktop-file "$APP/rsclipping.desktop" \
  --icon-file "$APP/.DirIcon" \
  "$APP" "$OUT"
rm -rf "$APP"

ls -lh "$OUT"
echo "Done: $OUT"