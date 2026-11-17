//! UI translations. Each entry fills one property of the `Tr` global in `ui/app.slint`.

use crate::Tr;

pub const LANGS: [&str; 3] = ["en", "uk", "ru"];
pub const LANG_NAMES: [&str; 3] = ["English", "Українська", "Русский"];

pub fn index_of(lang: &str) -> usize {
    LANGS.iter().position(|l| *l == lang).unwrap_or(0)
}

/// Best guess of the user's language, used only for the very first launch.
pub fn system_language() -> &'static str {
    let locale = system_locale().to_lowercase();
    if locale.starts_with("uk") {
        "uk"
    } else if locale.starts_with("ru") {
        "ru"
    } else {
        "en"
    }
}

#[cfg(windows)]
fn system_locale() -> String {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buf = [0u16; 85];
    let len = unsafe { GetUserDefaultLocaleName(&mut buf) };
    if len > 1 {
        String::from_utf16_lossy(&buf[..len as usize - 1])
    } else {
        String::new()
    }
}

#[cfg(target_os = "macos")]
fn system_locale() -> String {
    std::process::Command::new("defaults")
        .args(["read", "-g", "AppleLocale"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn system_locale() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default()
}

/// Tray menu labels, needed outside of Slint by the Linux tray.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn tray_text(lang: usize, quit: bool) -> &'static str {
    const SHOW: [&str; 3] = ["Show window", "Показати вікно", "Показать окно"];
    const QUIT: [&str; 3] = ["Quit", "Вийти", "Выйти"];
    let lang = lang.min(2);
    if quit {
        QUIT[lang]
    } else {
        SHOW[lang]
    }
}

macro_rules! translations {
    ($($setter:ident => [$en:expr, $uk:expr, $ru:expr]),* $(,)?) => {
        pub fn apply(tr: &Tr, lang: usize) {
            let lang = lang.min(2);
            $( tr.$setter([$en, $uk, $ru][lang].into()); )*
        }
    };
}

translations! {
    set_new_record => ["New record", "Новий запис", "Новая запись"],
    set_new_record_desc => ["Create new controller", "Створити новий контролер", "Создать новый контроллер"],
    set_settings => ["Settings", "Налаштування", "Настройки"],
    set_about => ["About", "Про програму", "О программе"],
    set_back => ["Back", "Назад", "Назад"],
    set_empty_data => [
        "No records yet. Create one to start controlling app volume.",
        "Записів поки немає. Створіть запис, щоб керувати гучністю програм.",
        "Записей пока нет. Создайте запись, чтобы управлять громкостью приложений."
    ],
    set_empty_name => ["Unnamed", "Без назви", "Без названия"],
    set_no_process => ["No process selected", "Процес не вибрано", "Процесс не выбран"],
    set_process_closed => ["not running", "не запущено", "не запущен"],
    set_process_idle => ["silent", "без звуку", "без звука"],
    set_no_hotkey => ["No hotkey", "Без гарячої клавіші", "Без горячей клавиши"],
    set_hotkey_busy => [
        "This hotkey could not be registered (used by another app?)",
        "Не вдалося зареєструвати гарячу клавішу (зайнята іншою програмою?)",
        "Не удалось зарегистрировать горячую клавишу (занята другим приложением?)"
    ],
    set_edit => ["Edit", "Редагувати", "Редактировать"],
    set_close => ["Close", "Закрити", "Закрыть"],
    set_remove => ["Remove", "Видалити", "Удалить"],
    set_remove_confirm => ["Remove this record?", "Видалити цей запис?", "Удалить эту запись?"],
    set_remove_yes => ["Yes, remove", "Так, видалити", "Да, удалить"],
    set_remove_no => ["Cancel", "Скасувати", "Отмена"],
    set_toggle => ["Toggle", "Перемкнути", "Переключить"],
    set_toggle_desc => [
        "Switch between minimum and maximum volume now",
        "Перемкнути між мінімальною та максимальною гучністю",
        "Переключить между минимальной и максимальной громкостью"
    ],
    set_field_name => ["Name", "Назва", "Название"],
    set_field_process => ["Process", "Процес", "Процесс"],
    set_field_refresh => ["Refresh", "Оновити", "Обновить"],
    set_field_hotkey => ["Hotkey", "Гаряча клавіша", "Горячая клавиша"],
    set_hotkey_record => ["Record", "Записати", "Записать"],
    set_hotkey_press => ["Press a key combination…", "Натисніть комбінацію клавіш…", "Нажмите комбинацию клавиш…"],
    set_hotkey_clear => ["Clear", "Очистити", "Очистить"],
    set_field_min => ["Minimum volume", "Мінімальна гучність", "Минимальная громкость"],
    set_field_max => ["Maximum volume", "Максимальна гучність", "Максимальная громкость"],
    set_field_mode => ["Mode", "Режим", "Режим"],
    set_mode_toggle => ["Switch between volumes", "Перемикання між рівнями", "Переключение между уровнями"],
    set_mode_adjust => ["Change volume", "Зміна гучності", "Изменение громкости"],
    set_field_hotkey_up => ["Volume up", "Гучніше", "Громче"],
    set_field_hotkey_down => ["Volume down", "Тихіше", "Тише"],
    set_field_hotkey_mute => ["Mute / unmute", "Вимк. / увімк. звук", "Выкл. / вкл. звук"],
    set_field_step => ["Step", "Крок", "Шаг"],
    set_adjust_hint => [
        "Set volume up and down hotkeys, a mute hotkey, or both",
        "Задайте клавіші «гучніше» і «тихіше», клавішу вимкнення звуку або все разом",
        "Задайте клавиши «громче» и «тише», клавишу выключения звука или всё вместе"
    ],
    set_hotkeys_incomplete => [
        "Volume up and down hotkeys only work together: set both",
        "Клавіші «гучніше» і «тихіше» працюють лише разом: задайте обидві",
        "Клавиши «громче» и «тише» работают только вместе: задайте обе"
    ],
    set_mute => ["Mute / unmute", "Вимк. / увімк. звук", "Выкл. / вкл. звук"],
    set_update_available => ["Update available", "Доступне оновлення", "Доступно обновление"],
    set_update_download => ["Download", "Завантажити", "Скачать"],
    set_update_downloading => ["Downloading…", "Завантаження…", "Скачивание…"],
    set_update_install => ["Install and restart", "Встановити й перезапустити", "Установить и перезапустить"],
    set_update_error => ["Error when updating application", "Помилка під час оновлення", "Ошибка при обновлении приложения"],
    set_update_open_page => ["Open release page", "Відкрити сторінку релізу", "Открыть страницу релиза"],
    set_update_check => ["Check for updates", "Перевірити оновлення", "Проверить обновления"],
    set_update_checking => ["Checking…", "Перевірка…", "Проверка…"],
    set_update_latest => ["You have the latest version", "У вас остання версія", "У вас последняя версия"],
    set_set_language => ["Language", "Мова", "Язык"],
    set_set_theme => ["Theme", "Тема", "Тема"],
    set_theme_system => ["System", "Системна", "Системная"],
    set_theme_dark => ["Dark", "Темна", "Тёмная"],
    set_theme_light => ["Light", "Світла", "Светлая"],
    set_set_on_top => ["Always on top", "Завжди зверху", "Всегда сверху"],
    set_set_close_to_tray => ["Close to tray", "Закривати в трей", "Закрывать в трей"],
    set_set_close_to_tray_desc => [
        "Keep running in the tray when the window is closed, so hotkeys keep working",
        "Працювати в треї після закриття вікна, щоб гарячі клавіші працювали далі",
        "Работать в трее после закрытия окна, чтобы горячие клавиши продолжали работать"
    ],
    set_set_autostart => ["Start with system", "Запускати разом із системою", "Запускать вместе с системой"],
    set_set_start_minimized => ["Start minimized to tray", "Запускати згорнутим у трей", "Запускать свёрнутым в трей"],
    set_set_auto_update => ["Check for updates automatically", "Автоматично перевіряти оновлення", "Автоматически проверять обновления"],
    set_desktop_title => ["Application menu", "Меню застосунків", "Меню приложений"],
    set_desktop_missing => [
        "Not added to the application menu: no menu entry, and the taskbar may show a generic icon",
        "Не додано до меню застосунків: немає пункту в меню, а панель завдань може показувати стандартну іконку",
        "Не добавлено в меню приложений: нет пункта в меню, а панель задач может показывать стандартную иконку"
    ],
    set_desktop_ours => ["Added to the application menu", "Додано до меню застосунків", "Добавлено в меню приложений"],
    set_desktop_other => [
        "Added to the application menu by another tool (e.g. AppImageLauncher)",
        "Додано до меню застосунків іншою програмою (наприклад, AppImageLauncher)",
        "Добавлено в меню приложений другой программой (например, AppImageLauncher)"
    ],
    set_desktop_add => ["Add to menu", "Додати до меню", "Добавить в меню"],
    set_desktop_remove => ["Remove from menu", "Прибрати з меню", "Убрать из меню"],
    set_desktop_prompt_title => ["Add to the application menu?", "Додати до меню застосунків?", "Добавить в меню приложений?"],
    set_desktop_prompt_text => [
        "Apps Volume Control is running as an AppImage and is not in the application menu yet. Add it so it can be started from the menu and shows its own icon in the taskbar.",
        "Apps Volume Control запущено як AppImage, і його ще немає в меню застосунків. Додайте його, щоб запускати з меню й бачити власну іконку на панелі завдань.",
        "Apps Volume Control запущен как AppImage, и его ещё нет в меню приложений. Добавьте его, чтобы запускать из меню и видеть собственную иконку на панели задач."
    ],
    set_desktop_prompt_later => ["Not now", "Не зараз", "Не сейчас"],
    set_desktop_dont_ask => ["Don't ask again", "Більше не питати", "Больше не спрашивать"],
    set_hotkeys_unavailable => [
        "Global hotkeys are unavailable in this session",
        "Глобальні гарячі клавіші недоступні в цьому сеансі",
        "Глобальные горячие клавиши недоступны в этом сеансе"
    ],
    set_audio_unavailable => [
        "Audio backend is unavailable",
        "Аудіосистема недоступна",
        "Аудиосистема недоступна"
    ],
    set_about_text => [
        "Control the volume of individual applications with global hotkeys.",
        "Керуйте гучністю окремих програм за допомогою глобальних гарячих клавіш.",
        "Управляйте громкостью отдельных приложений с помощью глобальных горячих клавиш."
    ],
    set_about_dev => ["Website", "Сайт", "Сайт"],
    set_about_project => ["Source code", "Вихідний код", "Исходный код"],
    set_tray_show => [tray_text(0, false), tray_text(1, false), tray_text(2, false)],
    set_tray_quit => [tray_text(0, true), tray_text(1, true), tray_text(2, true)],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_index() {
        assert_eq!(index_of("en"), 0);
        assert_eq!(index_of("uk"), 1);
        assert_eq!(index_of("ru"), 2);
        assert_eq!(index_of("de"), 0);
        assert_eq!(LANGS.len(), LANG_NAMES.len());
    }

    #[test]
    fn system_language_is_supported() {
        assert!(LANGS.contains(&system_language()));
    }
}
