<h1 align="center">
  <img src="logo.png" alt="Apps Volume Control" width="160"><br>
  Apps Volume Control
</h1>

<p align="center">
  Управление громкостью отдельных приложений с помощью глобальных горячих клавиш.<br>
  Нативное приложение для <b>Windows</b>, <b>macOS</b> и <b>Linux</b>.
</p>

<p align="center">
  <a href="README.md">English</a> · <b>Русский</b> · <a href="README.uk.md">Українська</a>
</p>

<p align="center">
  <a href="https://github.com/RocketMan-System/rm_volume_control/actions/workflows/release.yml"><img src="https://github.com/RocketMan-System/rm_volume_control/actions/workflows/release.yml/badge.svg"></a>
  <a href="https://github.com/RocketMan-System/rm_volume_control/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/RocketMan-System/rm_volume_control"></a>
</p>

## Возможности

- **Громкость по приложениям** — выберите приложение, которое воспроизводит звук, и задайте для него минимальную и максимальную громкость.
- **Глобальные горячие клавиши** — одно нажатие переключает приложение между минимумом и максимумом. Можно записать
  любую комбинацию (`Ctrl+F1`, `Alt+Shift+M`, …); несколько записей могут использовать одну клавишу.
- **Живой микшер** — текущая громкость каждой записи с ползунком и кнопкой «Переключить».
- **Трей** — приложение работает в системном трее, поэтому горячие клавиши действуют и при закрытом окне.
- **Автозапуск** вместе с системой, запуск свёрнутым, «поверх всех окон», тёмная / светлая / системная тема.
- **English, Українська, Русский.**
- **Автообновление** из GitHub Releases (с проверкой SHA-256).
- **Лёгкое** — один исполняемый файл около 15 МБ, примерно 30 МБ оперативной памяти, мгновенный запуск.

## Установка

Подробности — в описании [последнего релиза](https://github.com/RocketMan-System/rm_volume_control/releases/latest).

| ОС | Как установить |
|---|---|
| Windows 10/11 | Скачайте `apps-volume-control-windows-x86_64.exe` и запустите (портативная версия, права администратора не нужны). |
| macOS 14.2+ | `curl -fsSL https://github.com/RocketMan-System/rm_volume_control/releases/latest/download/install-macos.sh \| sh` |
| Linux | `curl -fsSL https://github.com/RocketMan-System/rm_volume_control/releases/latest/download/install-linux.sh \| sh` |

### Особенности платформ

- **macOS** не умеет регулировать громкость отдельных приложений. Поэтому используются *process taps* Core Audio
  (macOS 14.2+): выбранное приложение приглушается на уровне системы, а его звук воспроизводится с нужной громкостью.
  При первом использовании macOS запросит разрешение на *запись системного звука*. Приложение не подписано
  сертификатом Apple Developer ID: скрипт установки снимает с него карантин, а при обновлении приложение запросит
  пароль, чтобы сделать то же самое. Подпись ad-hoc, поэтому после обновления macOS может снова запросить
  разрешение на звук.
- **Linux** работает с PulseAudio и PipeWire (`pipewire-pulse`). Глобальные горячие клавиши работают через X11;
  в чистой сессии Wayland они срабатывают, только когда в фокусе окно XWayland. Для значка в трее нужна поддержка
  StatusNotifierItem (KDE или GNOME с расширением AppIndicator).
- **Windows**: сборки не подписаны, поэтому SmartScreen может показать предупреждение при первом запуске.

## Сборка из исходников

```sh
# Linux: sudo apt install libpulse-dev libfontconfig1-dev libxcb1-dev libxkbcommon-dev
cargo run --release
cargo test
```

## Выпуск релиза

Увеличьте `version` в `Cargo.toml` и запушьте в `main`. Workflow прогонит тесты на всех платформах и, если релиза
`v<версия>` ещё нет, соберёт версии для Windows / Linux / macOS (universal) и опубликует релиз. Запущенные копии
приложения увидят обновление в течение 30 минут.

## Сайт

[rocketman-streams.com](https://rocketman-streams.com/)

## Благодарности

- [Slint](https://slint.dev) — UI-библиотека (используется по лицензии Slint Royalty-free)
- [global-hotkey](https://github.com/tauri-apps/global-hotkey), [libpulse-binding](https://github.com/jnqnfe/pulse-binding-rust),
  [windows-rs](https://github.com/microsoft/windows-rs), [objc2](https://github.com/madsmtm/objc2)

## Лицензия

[PolyForm Noncommercial License 1.0.0](LICENSE). Приложение можно бесплатно использовать, изменять и распространять
в любых некоммерческих целях, участие в разработке приветствуется (см. [CONTRIBUTING.md](CONTRIBUTING.md)).
Копии и форки обязаны сохранять строки `Required Notice` из лицензии — ссылки на сайт и репозиторий проекта.
Коммерческое использование запрещено.
