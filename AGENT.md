# AGENT.md

Guide for AI agents and contributors working on this repository.

## What this is

Apps Volume Control — a tray app that changes the volume of individual applications with global
hotkeys (each record toggles one app between a min and a max volume). It is a native Rust rewrite of
the Electron app <https://github.com/XXanderWP/AppsVolumeControl> (v2.x) and targets Windows, macOS
and Linux. Goals: small memory footprint, instant start, no web view.

## Stack

| Concern | Choice | Why |
|---|---|---|
| UI | [Slint](https://slint.dev) 1.19, `renderer-software` + `backend-winit`, style `fluent` | No GPU context or browser engine; ~30 MB RSS |
| Tray | Slint's `SystemTrayIcon` on Windows/macOS; own `ksni` StatusNotifierItem on Linux (`src/tray.rs`) | Slint's Linux tray registers on D-Bus synchronously on the UI thread and could hang startup |
| Hotkeys | `global-hotkey` | Windows / macOS / X11 |
| Audio | WASAPI (`windows`), PulseAudio (`libpulse-binding`), Core Audio process taps (`objc2-core-audio`) | Native per-app volume |
| HTTP | `ureq` — native TLS on Windows/macOS, rustls on Linux | Avoids OpenSSL version coupling on Linux |
| Updater | own code in `src/updater.rs` over GitHub Releases | No installer framework needed |

Do **not** reintroduce a web view (Tauri, wry, Electron) — the user explicitly rejected it for size/RAM.

## Layout

```
ui/app.slint          All UI: AppWindow (records / settings / about pages), TrayIcon, `Tr` global with strings
src/main.rs           Wiring: UI thread state (`App`, reached via `with_app`), callbacks, timers, updater flow
src/config.rs         `Config` / `Record`, JSON at <config_dir>/apps-volume-control/config.json, legacy import
src/i18n.rs           Translations (en / uk / ru) → `Tr` properties; `translations!` table
src/hotkeys.rs        Hotkey registration, record → hotkey bindings, key-capture formatting
src/audio/mod.rs      `VolumeBackend` trait, worker thread, `AudioHandle` commands, toggle logic
src/audio/windows.rs  WASAPI sessions of all active render devices, grouped by exe name
src/audio/linux.rs    PulseAudio/PipeWire sink inputs + connected clients, grouped by `application.process.binary`
src/audio/macos.rs    Process taps + private aggregate device + IO proc that applies gain
src/icons.rs          App icons: cache in <cache_dir>/apps-volume-control/icons, freedesktop theme lookup (Linux)
src/autostart.rs      Registry Run key / XDG autostart / LaunchAgent, always with `--minimized`
src/desktop.rs        Linux app_id; AppImage menu entry + icon (prompt / Settings), respects AppImageLauncher entries
src/tray.rs           Tray wrapper: Slint TrayIcon (Win/macOS) or ksni on a background thread (Linux)
src/instance.rs       Single instance via loopback TCP 127.0.0.1:47613 ("show" message)
src/updater.rs        Check / download / verify / swap binary and relaunch
packaging/            macOS Info.plist template, Linux .desktop
scripts/              install-macos.sh, install-linux.sh (also published as release assets)
.github/workflows/release.yml   CI + release
```

## Architecture rules

- **Threads.** Slint objects live on the main thread only. Mutable UI-side state is the `App` struct in a
  `thread_local!`, accessed with `with_app(|app| …)`. Background threads (audio worker, updater, instance
  listener) send results back with `slint::invoke_from_event_loop`. Never hold `with_app` across a call that
  can re-enter Slint callbacks.
- **Audio worker.** Each OS backend is owned by one thread (`audio::spawn`). Talk to it only through
  `AudioHandle` (`refresh`, `set_volume`, `toggle`, `shutdown`). Commands are coalesced, so slider drags are cheap.
  A backend implements `VolumeBackend { list, set_volume }`; `key` is the identifier stored in records
  (exe name on Windows — compatible with the Electron config —, process binary on Linux, bundle id on macOS).
- **Quiet apps (Linux).** Apps close their PulseAudio stream when silent (browsers after a few seconds), so
  `list()` also returns connected *clients* with `active: false`. The UI shows them as "silent" with no volume;
  a volume set for such an app is remembered in the backend and applied to its next stream. Sound-server
  internals are filtered by `linux::is_system_client`.
- **Toggle semantics** (kept from the Electron app): go to whichever of min/max is *farther* from the current
  volume (`audio::toggle_target`).
- **Record modes** (`Record::mode`): 0 = toggle (default, `hotkey`, `min`/`max`); `MODE_ADJUST` = volume
  up/down by `step` % (`hotkeyUp` + `hotkeyDown`, active only as a pair) and/or mute (`hotkeyMute`). Mute
  remembers the previous volume in the audio worker (in memory; unmuting with nothing remembered → 100%).
- **Model updates.** `App::render` updates the `VecModel` row by row; do not replace the whole model, or the
  open editor (LineEdit focus, ComboBox) resets every 2 s refresh.
- **Nothing slow on the UI thread before `show()`.** The Linux tray registration with the tray host is a
  blocking D-Bus call that was observed to hang; it runs on the `tray` thread. Keep it that way.
- **Polling.** The mixer is polled every 2 s only while the window is visible; hotkeys don't depend on it.
- **Config compatibility.** Keep the field names `records`, `lockTop`, `lang`, and record fields
  (`id, name, min, max, process, hotkey, status`) — they match the Electron store and are imported from
  `%APPDATA%/<old app name>/config.control`.
- **Strings.** Every user-visible string goes through the `Tr` global: add the property in `ui/app.slint`
  and the three translations in `src/i18n.rs` (the macro calls the generated `set_*` setter, so a typo fails
  to compile).

## Updater & releases

- Release assets (names are hard-coded in `updater::ASSET_NAME` and the workflow — change both together):
  `apps-volume-control-windows-x86_64.exe`, `apps-volume-control-linux-x86_64`,
  `apps-volume-control-macos-universal.zip`, plus `SHA256SUMS.txt`, install scripts, `.desktop`, icon.
- Install: rename the running binary to `*.old`, put the new one in place, start it with `--updated`
  (it waits for the single-instance port), quit. `*.old` is removed on the next start.
- **macOS has no Developer ID.** CI signs ad-hoc (`codesign -s -`). The updater swaps the `.app` bundle via
  `osascript … with administrator privileges` (system password prompt) and removes
  `com.apple.quarantine`; the script is `updater::macos_swap_script` and is unit-tested in `sh`.
  Fresh installs use `scripts/install-macos.sh` (sudo + `xattr -dr com.apple.quarantine`).
- CI (`.github/workflows/release.yml`): every push/PR runs `cargo test` on Linux, Windows and macOS. On a push
  to `main`, if release `v<Cargo.toml version>` does not exist, it builds all platforms and publishes it with
  `.github/release-notes.md` + generated notes. To release: bump `version` in `Cargo.toml` (and keep
  `Cargo.lock` committed — the build uses `--locked`).

## Commands

```sh
cargo run                      # debug run
cargo test                     # unit tests (no audio server needed)
cargo test -- --ignored        # + real PulseAudio/PipeWire test on Linux (re-applies current volumes)
cargo clippy --all-targets     # must stay warning-free
cargo fmt                      # rustfmt.toml: max_width 120
```

Linux build deps: `libpulse-dev libfontconfig1-dev libxcb1-dev libxkbcommon-dev`.

Cross-checking other OSes from Linux works for `cargo check`/`clippy` with
`--target x86_64-pc-windows-msvc` and `--target aarch64-apple-darwin` (rustup targets) — the Windows and
macOS backends can't be run there, so keep platform code small and covered by CI.

## Tests

Unit tests live next to the code (`#[cfg(test)] mod tests`):

- `audio` — toggle math; the worker driven by an in-memory `Mock` backend (refresh, toggle, clamping, retry
  when the audio server is not up yet).
- `hotkeys` — key-capture formatting, rejected keys, binding grouping of enabled records.
- `config` — import of the Electron store file, defaults, clamping, round trip.
- `updater` — version comparison, checksum parsing, shell/AppleScript quoting, the macOS swap script
  (executed with `sh`, including the rollback path).
- `main` — `record_row` (record + live audio state → UI row).
- `icons` — icon cache store/lookup; `.desktop` parsing, systemd-scope desktop ids, entry matching (Linux).
- `audio::linux` — `#[ignore]` integration test against the real audio server.

Add tests for any new pure logic; keep platform FFI thin so the logic around it stays testable.

## Known limitations

- Linux global hotkeys need X11 (no Wayland global-shortcut portal yet).
- Hotkey capture maps Latin letters/digits/F-keys; keys typed on a non-Latin layout are ignored.
- macOS process taps require 14.2+; the app's audio is re-played through the default output device, so
  device switches are picked up on the next refresh (≤ 2 s while the window is open, otherwise on next toggle).
