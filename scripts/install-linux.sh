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
curl -fsSL "$BASE/apps-volume-control.png" -o "$HOME/.local/share/icons/hicolor/256x256/apps/apps-volume-control.png" || true
curl -fsSL "$BASE/apps-volume-control.desktop" | sed "s|^Exec=.*|Exec=$BIN|" > "$HOME/.local/share/applications/apps-volume-control.desktop"
echo "Installed to $BIN"
