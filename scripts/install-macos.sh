#!/bin/sh
# Installs (or updates) Apps Volume Control into /Applications.
# The app is not signed with an Apple Developer ID, so the quarantine flag is removed
# to let Gatekeeper start it. Asks for your password (sudo) to write to /Applications.
#
#   curl -fsSL https://github.com/RocketMan-System/rm_volume_control/releases/latest/download/install-macos.sh | sh
set -eu

APP="Apps Volume Control.app"
URL="https://github.com/RocketMan-System/rm_volume_control/releases/latest/download/apps-volume-control-macos-universal.zip"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Downloading $URL"
curl -fL --progress-bar "$URL" -o "$TMP/app.zip"
ditto -x -k "$TMP/app.zip" "$TMP/unpacked"

pkill -x apps-volume-control 2>/dev/null || true
echo "Installing to /Applications (sudo password may be required)"
sudo rm -rf "/Applications/$APP"
sudo ditto "$TMP/unpacked/$APP" "/Applications/$APP"
sudo xattr -dr com.apple.quarantine "/Applications/$APP" 2>/dev/null || true

echo "Done. Starting Apps Volume Control…"
open "/Applications/$APP"
