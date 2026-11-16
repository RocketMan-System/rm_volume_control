## Install

### Windows
Download **`apps-volume-control-windows-x86_64.exe`** and run it — it is a single portable executable,
no installer and no administrator rights needed. Keep it in a folder you own (e.g. `%LOCALAPPDATA%\Programs`)
so the built-in updater can replace it.

### macOS (14.2+, Apple Silicon & Intel)
The app is not signed with an Apple Developer ID, so it has to be taken out of quarantine.
Install or update with one command (asks for your password):

```sh
curl -fsSL https://github.com/RocketMan-System/rm_volume_control/releases/latest/download/install-macos.sh | sh
```

Manual install: unpack `apps-volume-control-macos-universal.zip`, move the app to `/Applications`, then run

```sh
sudo xattr -dr com.apple.quarantine "/Applications/Apps Volume Control.app"
```

On first use macOS asks for permission to capture system audio — it is required to change other apps' volume.

### Linux (x86_64, PulseAudio or PipeWire)

```sh
curl -fsSL https://github.com/RocketMan-System/rm_volume_control/releases/latest/download/install-linux.sh | sh
```

or download `apps-volume-control-linux-x86_64.AppImage` (or the bare `apps-volume-control-linux-x86_64`),
`chmod +x` it and run. On first start the AppImage offers to add itself to the application menu (also in Settings).
Global hotkeys need an X11 session (or XWayland apps focused).

Checksums of all files are in `SHA256SUMS.txt`.
