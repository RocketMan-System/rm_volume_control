#!/bin/sh
# Installs Apps Volume Control for the current user into ~/.local (no root needed,
# which also lets the built-in updater replace the binary).
#
#   curl -fsSL https://github.com/RocketMan-System/rm_volume_control/releases/latest/download/install-linux.sh | sh
set -eu

BASE="https://github.com/RocketMan-System/rm_volume_control/releases/latest/download"
ARCH="$(uname -m)"
BIN="$HOME/.local/bin/apps-volume-control"

mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications" "$HOME/.local/share/icons/hicolor/256x256/apps"
curl -fL --progress-bar "$BASE/apps-volume-control-linux-$ARCH" -o "$BIN.new"
chmod +x "$BIN.new"
mv "$BIN.new" "$BIN"
ICON="$HOME/.local/share/icons/hicolor/256x256/apps/apps-volume-control.png"
DESKTOP="$HOME/.local/share/applications/apps-volume-control.desktop"
# Write to a temp file and rename: the desktop watches these folders and would pick up
# a half-written entry (no Name/Exec) as a broken "apps-volume-control" launcher.
curl -fsSL "$BASE/apps-volume-control.png" -o "$ICON.tmp" && mv "$ICON.tmp" "$ICON" || rm -f "$ICON.tmp"
curl -fsSL "$BASE/apps-volume-control.desktop" | sed "s|^Exec=.*|Exec=$BIN|" > "$DESKTOP.tmp"
if [ -s "$DESKTOP.tmp" ]; then mv "$DESKTOP.tmp" "$DESKTOP"; else rm -f "$DESKTOP.tmp"; fi
echo "Installed to $BIN"
