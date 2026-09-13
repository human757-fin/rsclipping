#!/usr/bin/env bash
# RSClipping online installer (Linux) - fetches the latest release and runs it.
set -euo pipefail
REPO="human757-fin/rsclipping"
API="https://api.github.com/repos/$REPO/releases/latest"
echo "RSClipping installer - checking for the latest release..."
RELEASE="$(curl -fsSL -H "User-Agent: rsclipping-online-installer" "$API")"
VERSION="$(echo "$RELEASE" | grep -m1 '"tag_name"' | sed -E 's/.*"([^"]+)".*/\1')"
VERSION="${VERSION#v}"
echo "Latest version: v$VERSION"
ASSET="$(echo "$RELEASE" | grep -m1 '"browser_download_url".*\.AppImage"' | sed -E 's/.*"([^"]+)".*/\1')"
if [ -z "$ASSET" ]; then
  echo "No Linux AppImage asset found in the latest release."
  exit 1
fi
BIN="rsclipping-${VERSION}-x86_64.AppImage"
curl -# -o "$BIN" "$ASSET"
chmod +x "$BIN"
echo "Downloaded $BIN. Move it where you like and run it directly:"
echo "  ./$BIN"