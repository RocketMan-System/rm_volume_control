<h1 align="center">
  <img src="logo.png" alt="Apps Volume Control" width="160"><br>
  Apps Volume Control
</h1>

<p align="center">
  Керування гучністю окремих програм за допомогою глобальних гарячих клавіш.<br>
  Нативний застосунок для <b>Windows</b>, <b>macOS</b> і <b>Linux</b>.
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.ru.md">Русский</a> · <b>Українська</b>
</p>

<p align="center">
  <a href="https://github.com/RocketMan-System/rm_volume_control/actions/workflows/release.yml"><img src="https://github.com/RocketMan-System/rm_volume_control/actions/workflows/release.yml/badge.svg"></a>
  <a href="https://github.com/RocketMan-System/rm_volume_control/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/RocketMan-System/rm_volume_control"></a>
</p>

## Можливості

- **Гучність окремих програм** — виберіть програму, що відтворює звук, і задайте для неї мінімальну та максимальну гучність.
- **Глобальні гарячі клавіші** — одне натискання перемикає програму між мінімумом і максимумом. Можна записати
  будь-яку комбінацію (`Ctrl+F1`, `Alt+Shift+M`, …), зокрема медіаклавіші й кнопки миші (`Ctrl+Mouse4`); кілька записів можуть використовувати одну клавішу.
- **Живий мікшер** — поточна гучність кожного запису з повзунком і кнопкою «Перемкнути».
- **Трей** — застосунок працює в системному треї, тож гарячі клавіші діють і із закритим вікном.
- **Автозапуск** разом із системою, запуск згорнутим, «завжди зверху», темна / світла / системна тема.
- **English, Українська, Русский.**
- **Автооновлення** з GitHub Releases (з перевіркою SHA-256).
- **Легкий** — один виконуваний файл близько 15 МБ, приблизно 30 МБ оперативної пам'яті, миттєвий запуск.

## Встановлення

Подробиці — в описі [останнього релізу](https://github.com/RocketMan-System/rm_volume_control/releases/latest).

| ОС | Як встановити |
|---|---|
| Windows 10/11 | Завантажте `apps-volume-control-windows-x86_64.exe` і запустіть (портативна версія, права адміністратора не потрібні). |
| macOS 14.2+ | `curl -fsSL https://github.com/RocketMan-System/rm_volume_control/releases/latest/download/install-macos.sh \| sh` |
| Linux | `curl -fsSL https://github.com/RocketMan-System/rm_volume_control/releases/latest/download/install-linux.sh \| sh` або `apps-volume-control-linux-x86_64.AppImage` |

### Особливості платформ

- **macOS** не вміє регулювати гучність окремих програм. Тому використовуються *process taps* Core Audio
  (macOS 14.2+): вибрана програма приглушується на рівні системи, а її звук відтворюється з потрібною гучністю.
  Під час першого використання macOS попросить дозвіл на *запис системного звуку*. Застосунок не підписаний
  сертифікатом Apple Developer ID: скрипт встановлення знімає з нього карантин, а під час оновлення застосунок
  попросить пароль, щоб зробити те саме. Підпис ad-hoc, тому після оновлення macOS може знову попросити
  дозвіл на звук.
- **Linux** працює з PulseAudio і PipeWire (`pipewire-pulse`). Глобальні гарячі клавіші працюють через X11;
  у чистому сеансі Wayland вони спрацьовують, лише коли у фокусі вікно XWayland. Для значка в треї потрібна
  підтримка StatusNotifierItem (KDE або GNOME з розширенням AppIndicator).
- **Windows**: збірки не підписані, тому SmartScreen може показати попередження під час першого запуску.

## Збирання з вихідних кодів

```sh
# Linux: sudo apt install libpulse-dev libfontconfig1-dev libxcb1-dev libxkbcommon-dev
cargo run --release
cargo test
```

## Випуск релізу

Збільште `version` у `Cargo.toml` і запуште в `main`. Workflow прожене тести на всіх платформах і, якщо релізу
`v<версія>` ще немає, збере версії для Windows / Linux / macOS (universal) та опублікує реліз. Запущені копії
застосунку побачать оновлення протягом 30 хвилин.

## Сайт

[rocketman-streams.com](https://rocketman-streams.com/)

## Подяки

- [Slint](https://slint.dev) — UI-бібліотека (використовується за ліцензією Slint Royalty-free)
- [global-hotkey](https://github.com/tauri-apps/global-hotkey), [libpulse-binding](https://github.com/jnqnfe/pulse-binding-rust),
  [windows-rs](https://github.com/microsoft/windows-rs), [objc2](https://github.com/madsmtm/objc2)

## Ліцензія

[PolyForm Noncommercial License 1.0.0](LICENSE). Застосунок можна безкоштовно використовувати, змінювати й поширювати
з будь-якою некомерційною метою, участь у розробці вітається (див. [CONTRIBUTING.md](CONTRIBUTING.md)).
Копії та форки зобов'язані зберігати рядки `Required Notice` з ліцензії — посилання на сайт і репозиторій проєкту.
Комерційне використання заборонено.
