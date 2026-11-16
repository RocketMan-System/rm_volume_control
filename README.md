<h1 align="center">
  <img src="logo.png" alt="Apps Volume Control" width="160"><br>
  Apps Volume Control
</h1>

<p align="center">
  Control the volume of individual applications with global hotkeys.<br>
  Native app for <b>Windows</b>, <b>macOS</b> and <b>Linux</b>.
</p>

<p align="center">
  <b>English</b> · <a href="README.ru.md">Русский</a> · <a href="README.uk.md">Українська</a>
</p>

<p align="center">
  <a href="https://github.com/RocketMan-System/rm_volume_control/actions/workflows/release.yml"><img src="https://github.com/RocketMan-System/rm_volume_control/actions/workflows/release.yml/badge.svg"></a>
  <a href="https://github.com/RocketMan-System/rm_volume_control/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/RocketMan-System/rm_volume_control"></a>
</p>

## Features

- **Per-app volume** — pick an application that plays audio and set its minimum and maximum volume.
- **Global hotkeys** — one press switches the app between its minimum and maximum. Any combination can be
  recorded (`Ctrl+F1`, `Alt+Shift+M`, …); several records may share one hotkey.
- **Live mixer** — current volume of every record with a slider and a “Toggle” button.
- **Tray** — keeps running in the system tray so hotkeys work with the window closed.
- **Start with system**, start minimized, always on top, dark / light / system theme.
- **English, Українська, Русский.**
- **Self-updating** from GitHub Releases (SHA-256 verified).
- **Lightweight** — a single ~15 MB executable, about 30 MB of RAM, starts instantly.

## Install

See the notes of the [latest release](https://github.com/RocketMan-System/rm_volume_control/releases/latest).

| OS | How |
|---|---|
| Windows 10/11 | Download `apps-volume-control-windows-x86_64.exe` and run it (portable, no admin rights). |
| macOS 14.2+ | `curl -fsSL https://github.com/RocketMan-System/rm_volume_control/releases/latest/download/install-macos.sh \| sh` |
| Linux | `curl -fsSL https://github.com/RocketMan-System/rm_volume_control/releases/latest/download/install-linux.sh \| sh` |

### Platform notes

- **macOS** has no per-application volume API. The app uses Core Audio *process taps* (macOS 14.2+):
  the selected app is muted at the system level and its audio is played back at the chosen volume.
  macOS asks for the *System Audio Recording* permission the first time. The app is not signed with an
  Apple Developer ID: the install script removes the quarantine flag, and updates ask for your password
  to do the same. Because the signature is ad-hoc, macOS may ask for the audio permission again after an update.
- **Linux** works with PulseAudio and PipeWire (`pipewire-pulse`). Global hotkeys use X11; on a pure Wayland
  session they only fire while an XWayland window is focused. The tray needs a StatusNotifierItem host
  (KDE, or GNOME with the AppIndicator extension).
- **Windows** builds are not code-signed, so SmartScreen may warn on first launch.

## Build from source

```sh
# Linux: sudo apt install libpulse-dev libfontconfig1-dev libxcb1-dev libxkbcommon-dev
cargo run --release
cargo test
```

## Releasing

Bump `version` in `Cargo.toml` and push to `main`. The workflow tests every platform and, if
`v<version>` is not released yet, builds Windows / Linux / macOS (universal) binaries and publishes the
release. Running apps pick the update up within 30 minutes.

## Website

[rocketman-streams.com](https://rocketman-streams.com/)

## Credits

- [Slint](https://slint.dev) — UI toolkit (used under the Slint Royalty-free License)
- [global-hotkey](https://github.com/tauri-apps/global-hotkey), [libpulse-binding](https://github.com/jnqnfe/pulse-binding-rust),
  [windows-rs](https://github.com/microsoft/windows-rs), [objc2](https://github.com/madsmtm/objc2)

## License

[PolyForm Noncommercial License 1.0.0](LICENSE). The app is free to use, modify and share for any
non-commercial purpose, and contributions are welcome (see [CONTRIBUTING.md](CONTRIBUTING.md)).
Copies and forks must keep the `Required Notice` lines from the license: links to the project website and repository.
Commercial use is not permitted.
