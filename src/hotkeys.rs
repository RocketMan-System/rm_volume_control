//! Global hotkeys. Records store hotkeys as strings like `Ctrl+F1` (the format of the
//! Electron version), which `global-hotkey` parses directly. Mouse buttons (`Ctrl+Mouse4`)
//! go through our own hook in `mouse`.

use crate::audio::AudioHandle;
use crate::config::Record;
use crate::mouse::{self, Mods, MouseCombo};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use slint::platform::Key;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

/// What a single hotkey press should do.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub process: String,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Toggle { min: f32, max: f32 },
    Step(f32),
    Mute,
}

impl Target {
    pub fn run(&self, audio: &AudioHandle) {
        match self.action {
            Action::Toggle { min, max } => audio.toggle(&self.process, min, max),
            Action::Step(delta) => audio.step(&self.process, delta),
            Action::Mute => audio.mute(&self.process),
        }
    }
}

/// A parsed hotkey string.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Trigger {
    Key(HotKey),
    Mouse(MouseCombo),
}

impl FromStr for Trigger {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        HotKey::from_str(s).map(Self::Key).or_else(|_| s.parse().map(Self::Mouse))
    }
}

/// Targets of every registered hotkey: keyboard ones by hotkey id, mouse ones by combo.
#[derive(Debug, Default)]
pub struct TargetMap {
    keys: HashMap<u32, Vec<Target>>,
    mouse: HashMap<MouseCombo, Vec<Target>>,
}

impl TargetMap {
    fn add(&mut self, trigger: Trigger, target: Target) {
        match trigger {
            Trigger::Key(k) => self.keys.entry(k.id()).or_default().push(target),
            Trigger::Mouse(m) => self.mouse.entry(m).or_default().push(target),
        }
    }

    #[cfg(test)]
    pub fn targets(&self, trigger: &Trigger) -> &[Target] {
        let list = match trigger {
            Trigger::Key(k) => self.keys.get(&k.id()),
            Trigger::Mouse(m) => self.mouse.get(m),
        };
        list.map_or(&[], Vec::as_slice)
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.mouse.is_empty()
    }
}

type Bindings = Arc<Mutex<TargetMap>>;

pub struct Hotkeys {
    manager: Option<GlobalHotKeyManager>,
    registered: Vec<HotKey>,
    bindings: Bindings,
}

fn run_all(targets: &[Target], audio: &AudioHandle) {
    for t in targets {
        t.run(audio);
    }
}

impl Hotkeys {
    /// Must be called on the main thread (required on macOS).
    pub fn new(audio: AudioHandle) -> Self {
        let manager = match GlobalHotKeyManager::new() {
            Ok(m) => Some(m),
            Err(err) => {
                eprintln!("Global hotkeys unavailable: {err}");
                None
            }
        };
        let bindings: Bindings = Arc::default();
        let (map, mouse_audio) = (bindings.clone(), audio.clone());
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state() != HotKeyState::Pressed {
                return;
            }
            if let Some(targets) = map.lock().unwrap().keys.get(&event.id()) {
                run_all(targets, &audio);
            }
        }));
        let map = bindings.clone();
        mouse::set_handler(move |combo| {
            if let Some(targets) = map.lock().unwrap().mouse.get(&combo) {
                run_all(targets, &mouse_audio);
            }
        });
        Self { manager, registered: Vec::new(), bindings }
    }

    pub fn available(&self) -> bool {
        self.manager.is_some()
    }

    /// Re-registers hotkeys of all enabled records. Returns the hotkeys that failed.
    pub fn apply(&mut self, records: &[Record]) -> HashSet<String> {
        let mut failed = HashSet::new();
        let Some(manager) = &self.manager else {
            return failed;
        };
        let _ = manager.unregister_all(&self.registered);
        self.registered.clear();

        let (map, keys) = bindings_for(records);
        let mut combos = Vec::new();
        for (text, trigger) in &keys {
            match *trigger {
                Trigger::Key(hotkey) if manager.register(hotkey).is_ok() => self.registered.push(hotkey),
                Trigger::Key(_) => {
                    failed.insert(text.clone());
                }
                Trigger::Mouse(combo) => combos.push(combo),
            }
        }
        *self.bindings.lock().unwrap() = map;
        let failed_mouse = mouse::set(&combos);
        for (text, trigger) in keys {
            if matches!(trigger, Trigger::Mouse(c) if failed_mouse.contains(&c)) {
                failed.insert(text);
            }
        }
        failed
    }
}

/// Groups enabled records by hotkey. Also returns each distinct hotkey once.
pub fn bindings_for(records: &[Record]) -> (TargetMap, Vec<(String, Trigger)>) {
    let mut map = TargetMap::default();
    let mut keys: Vec<(String, Trigger)> = Vec::new();
    for r in records.iter().filter(|r| r.status) {
        let Some(process) = r.process.as_deref() else { continue };
        for (text, action) in actions(r) {
            let Ok(trigger) = Trigger::from_str(text) else { continue };
            if !keys.iter().any(|(_, t)| *t == trigger) {
                keys.push((text.to_string(), trigger));
            }
            map.add(trigger, Target { process: process.to_string(), action });
        }
    }
    (map, keys)
}

/// Hotkeys of a record with what they do, by the record's mode.
fn actions(r: &Record) -> Vec<(&str, Action)> {
    if !r.adjust_mode() {
        let toggle = Action::Toggle { min: r.min() as f32 / 100.0, max: r.max() as f32 / 100.0 };
        return r.hotkey.as_deref().map(|h| (h, toggle)).into_iter().collect();
    }
    let step = r.step() as f32 / 100.0;
    let mut list = Vec::new();
    // Up and down only work together.
    if let (Some(up), Some(down)) = (r.hotkey_up.as_deref(), r.hotkey_down.as_deref()) {
        list.push((up, Action::Step(step)));
        list.push((down, Action::Step(-step)));
    }
    if let Some(mute) = r.hotkey_mute.as_deref() {
        list.push((mute, Action::Mute));
    }
    list
}

/// Turns a key press captured by the UI into a hotkey string such as `Ctrl+Shift+F5`.
/// `physical` is the winit name of the pressed key (`KeyA`, `Digit5`, `Numpad5`, or empty):
/// it keeps letters independent of the keyboard layout and tells numpad keys apart.
/// Returns `None` for presses that can't be used as a global hotkey.
pub fn format_hotkey(text: &str, physical: &str, ctrl: bool, alt: bool, shift: bool, meta: bool) -> Option<String> {
    let mods = Mods { ctrl, alt, shift, meta };
    let (key, standalone) = match physical_key_name(physical, text).or_else(|| key_name(text)) {
        Some(key) => {
            let is_function_key =
                key.len() >= 2 && key.starts_with('F') && key[1..].chars().all(|c| c.is_ascii_digit());
            (key, is_function_key)
        }
        // Media keys print nothing, and do nothing in normal typing.
        None => (media_key_name(physical)?.to_string(), true),
    };
    // Plain letters/digits without a modifier would swallow normal typing.
    if !mods.any() && !standalone {
        return None;
    }
    let combo = with_modifiers(mods, &key);
    HotKey::from_str(&combo).ok().map(|_| combo)
}

/// Turns a mouse button pressed in the hotkey field (3 middle, 4 back, 5 forward, 6 / 7 wheel
/// up / down) into a hotkey string such as `Ctrl+Mouse4`. The middle button and the wheel need
/// a modifier, as they paste and scroll; left and right buttons are never used.
pub fn format_mouse_hotkey(button: i32, ctrl: bool, alt: bool, shift: bool, meta: bool) -> Option<String> {
    let button = mouse::Button::from_number(button)?;
    let mods = Mods { ctrl, alt, shift, meta };
    if (button == mouse::Button::Middle || button.is_wheel()) && !mods.any() {
        return None;
    }
    Some(with_modifiers(mods, button.name()))
}

fn with_modifiers(mods: Mods, key: &str) -> String {
    let names = [(mods.ctrl, "Ctrl"), (mods.alt, "Alt"), (mods.shift, "Shift"), (mods.meta, "Super")];
    let mut parts: Vec<&str> = names.iter().filter(|(on, _)| *on).map(|(_, name)| *name).collect();
    parts.push(key);
    parts.join("+")
}

/// Text of a synthetic key press that stands for a media key (see `is_media_key`): Slint drops
/// key presses without text, and media keys have none. It is in the private use range that
/// Slint's text inputs ignore, and is no key of its own.
pub const MEDIA_KEY_TEXT: &str = "\u{F7FF}";

pub fn is_media_key(physical: &str) -> bool {
    media_key_name(physical).is_some()
}

/// Media and volume keys by their winit name; the names are what `global-hotkey` parses.
fn media_key_name(physical: &str) -> Option<&'static str> {
    #[rustfmt::skip]
    const MEDIA: &[&str] = &[
        "MediaPlayPause", "MediaStop", "MediaTrackNext", "MediaTrackPrevious", "MediaSelect",
        "AudioVolumeUp", "AudioVolumeDown", "AudioVolumeMute",
        "MediaPlay", "MediaPause", "MediaFastForward", "MediaRewind", "MediaRecord",
        "LaunchMail", "LaunchApp1", "LaunchApp2",
        "BrowserBack", "BrowserForward", "BrowserRefresh", "BrowserStop", "BrowserSearch",
        "BrowserFavorites", "BrowserHome",
    ];
    MEDIA.iter().find(|name| **name == physical).copied()
}

/// Letters, digits, punctuation and numpad keys by their position, so `Ctrl+Ф` on a Cyrillic
/// layout is `Ctrl+A` and `Ctrl+-` is `Ctrl+Minus` whatever the key prints.
fn physical_key_name(physical: &str, text: &str) -> Option<String> {
    #[rustfmt::skip]
    const PUNCTUATION: &[&str] = &[
        "Backquote", "Minus", "Equal", "BracketLeft", "BracketRight", "Backslash", "IntlBackslash",
        "Semicolon", "Quote", "Comma", "Period", "Slash", "PrintScreen",
    ];
    if let Some(c) = physical.strip_prefix("Key").or_else(|| physical.strip_prefix("Digit")) {
        return (c.len() == 1).then(|| c.to_string());
    }
    if PUNCTUATION.contains(&physical) {
        return Some(physical.to_string());
    }
    // Numpad 5 with NumLock off has no Home/End-like meaning: the system sees it as Clear.
    if physical == "Numpad5" && text != "5" {
        return Some("NumpadClear".to_string());
    }
    let numpad = physical.strip_prefix("Numpad")?;
    // With NumLock off numpad keys send Home/End/arrows, and global hotkeys see those instead.
    let numlock_on = match numpad {
        "Add" | "Subtract" | "Multiply" | "Divide" => true,
        "Decimal" => text == "." || text == ",",
        d => d.len() == 1 && d == text,
    };
    numlock_on.then(|| physical.to_string())
}

fn key_name(text: &str) -> Option<String> {
    #[rustfmt::skip]
    const SPECIAL: &[(Key, &str)] = &[
        (Key::F1, "F1"), (Key::F2, "F2"), (Key::F3, "F3"), (Key::F4, "F4"),
        (Key::F5, "F5"), (Key::F6, "F6"), (Key::F7, "F7"), (Key::F8, "F8"),
        (Key::F9, "F9"), (Key::F10, "F10"), (Key::F11, "F11"), (Key::F12, "F12"),
        (Key::F13, "F13"), (Key::F14, "F14"), (Key::F15, "F15"), (Key::F16, "F16"),
        (Key::F17, "F17"), (Key::F18, "F18"), (Key::F19, "F19"), (Key::F20, "F20"),
        (Key::F21, "F21"), (Key::F22, "F22"), (Key::F23, "F23"), (Key::F24, "F24"),
        (Key::Insert, "Insert"), (Key::Delete, "Delete"), (Key::Home, "Home"), (Key::End, "End"),
        (Key::PageUp, "PageUp"), (Key::PageDown, "PageDown"),
        (Key::UpArrow, "Up"), (Key::DownArrow, "Down"), (Key::LeftArrow, "Left"), (Key::RightArrow, "Right"),
        (Key::Space, "Space"), (Key::Tab, "Tab"), (Key::Return, "Enter"), (Key::Backspace, "Backspace"),
        (Key::Pause, "Pause"), (Key::ScrollLock, "ScrollLock"),
    ];
    for (key, name) in SPECIAL {
        let k: slint::SharedString = (*key).into();
        if k.as_str() == text {
            return Some((*name).to_string());
        }
    }
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    // Shift+digit arrives as the shifted symbol on a US layout.
    #[rustfmt::skip]
    let c = match c {
        '!' => '1', '@' => '2', '#' => '3', '$' => '4', '%' => '5',
        '^' => '6', '&' => '7', '*' => '8', '(' => '9', ')' => '0',
        c => c,
    };
    if c.is_ascii_alphanumeric() {
        Some(c.to_ascii_uppercase().to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: Key) -> String {
        let s: slint::SharedString = k.into();
        s.to_string()
    }

    #[test]
    fn formats_function_keys() {
        assert_eq!(format_hotkey(&key(Key::F1), "", true, false, false, false).as_deref(), Some("Ctrl+F1"));
        assert_eq!(format_hotkey(&key(Key::F12), "", false, true, true, false).as_deref(), Some("Alt+Shift+F12"));
        // Function keys may be used without modifiers.
        assert_eq!(format_hotkey(&key(Key::F7), "", false, false, false, false).as_deref(), Some("F7"));
    }

    #[test]
    fn letters_need_a_modifier() {
        assert_eq!(format_hotkey("a", "", false, false, false, false), None);
        assert_eq!(format_hotkey("a", "", true, true, false, false).as_deref(), Some("Ctrl+Alt+A"));
        assert_eq!(format_hotkey("5", "", false, false, false, true).as_deref(), Some("Super+5"));
    }

    #[test]
    fn maps_shifted_digits() {
        assert_eq!(format_hotkey("!", "", true, false, true, false).as_deref(), Some("Ctrl+Shift+1"));
    }

    #[test]
    fn letters_ignore_keyboard_layout() {
        assert_eq!(format_hotkey("ф", "KeyA", true, false, false, false).as_deref(), Some("Ctrl+A"));
        assert_eq!(format_hotkey("%", "Digit5", true, false, true, false).as_deref(), Some("Ctrl+Shift+5"));
        assert_eq!(format_hotkey("-", "Minus", true, false, false, false).as_deref(), Some("Ctrl+Minus"));
        assert_eq!(format_hotkey("ё", "Backquote", true, false, false, false).as_deref(), Some("Ctrl+Backquote"));
        assert_eq!(format_hotkey("+", "Equal", true, false, true, false).as_deref(), Some("Ctrl+Shift+Equal"));
        assert_eq!(format_hotkey("", "PrintScreen", true, false, false, false).as_deref(), Some("Ctrl+PrintScreen"));
    }

    #[test]
    fn separates_numpad_keys() {
        assert_eq!(format_hotkey("5", "Numpad5", true, false, false, false).as_deref(), Some("Ctrl+Numpad5"));
        assert_eq!(format_hotkey("5", "Digit5", true, false, false, false).as_deref(), Some("Ctrl+5"));
        assert_eq!(format_hotkey("+", "NumpadAdd", false, true, false, false).as_deref(), Some("Alt+NumpadAdd"));
        // NumLock off: the key acts as Home.
        assert_eq!(format_hotkey(&key(Key::Home), "Numpad7", true, false, false, false).as_deref(), Some("Ctrl+Home"));
        assert_eq!(format_hotkey("", "Numpad5", true, false, false, false).as_deref(), Some("Ctrl+NumpadClear"));
        // Bare numpad digits would swallow typing too.
        assert_eq!(format_hotkey("5", "Numpad5", false, false, false, false), None);
    }

    #[test]
    fn rejects_unusable_keys() {
        // Modifier alone, non-latin text without a known key and multi-char text.
        assert_eq!(format_hotkey(&key(Key::Control), "", true, false, false, false), None);
        assert_eq!(format_hotkey("ф", "", true, false, false, false), None);
        assert_eq!(format_hotkey("ab", "", true, false, false, false), None);
    }

    #[test]
    fn media_keys_need_no_modifier() {
        assert_eq!(format_hotkey("", "MediaPlayPause", false, false, false, false).as_deref(), Some("MediaPlayPause"));
        assert_eq!(
            format_hotkey("", "AudioVolumeUp", true, false, false, false).as_deref(),
            Some("Ctrl+AudioVolumeUp")
        );
        assert_eq!(
            format_hotkey("", "MediaTrackPrevious", false, false, false, false).as_deref(),
            Some("MediaTrackPrevious")
        );
        assert_eq!(format_hotkey("", "Fn", false, false, false, false), None);
        assert_eq!(
            format_hotkey(MEDIA_KEY_TEXT, "AudioVolumeUp", true, false, false, false).as_deref(),
            Some("Ctrl+AudioVolumeUp")
        );
    }

    #[test]
    fn formats_mouse_buttons() {
        assert_eq!(format_mouse_hotkey(4, false, false, false, false).as_deref(), Some("Mouse4"));
        assert_eq!(format_mouse_hotkey(5, true, false, true, false).as_deref(), Some("Ctrl+Shift+Mouse5"));
        assert_eq!(format_mouse_hotkey(3, false, true, false, false).as_deref(), Some("Alt+Mouse3"));
        // Bare middle click, and left / right buttons.
        assert_eq!(format_mouse_hotkey(3, false, false, false, false), None);
        assert_eq!(format_mouse_hotkey(1, true, false, false, false), None);
        assert_eq!(format_mouse_hotkey(2, true, false, false, false), None);
        assert_eq!(format_mouse_hotkey(6, true, false, false, false).as_deref(), Some("Ctrl+WheelUp"));
        assert_eq!(format_mouse_hotkey(7, false, false, false, false), None);
    }

    #[test]
    fn mouse_bindings() {
        let records = vec![record("1", "Ctrl+Mouse4", "game.exe", true), record("2", "Ctrl+Mouse4", "x.exe", true)];
        let (map, keys) = bindings_for(&records);
        assert_eq!(keys.len(), 1);
        assert!(matches!(keys[0].1, Trigger::Mouse(_)));
        assert_eq!(map.targets(&keys[0].1).len(), 2);
    }

    #[test]
    fn formatted_hotkeys_parse() {
        for text in [
            "Ctrl+F1",
            "Shift+F12",
            "Alt+F5",
            "Ctrl+Alt+A",
            "Super+Space",
            "Ctrl+Shift+Up",
            "Ctrl+Numpad5",
            "Alt+NumpadDecimal",
            "MediaPlayPause",
            "Ctrl+AudioVolumeMute",
            "MediaTrackNext",
            "BrowserBack",
            "LaunchMail",
        ] {
            assert!(HotKey::from_str(text).is_ok(), "{text}");
        }
    }

    fn record(id: &str, hotkey: &str, process: &str, status: bool) -> Record {
        Record {
            id: id.into(),
            hotkey: Some(hotkey.into()),
            process: Some(process.into()),
            min: Some(10.0),
            max: Some(80.0),
            status,
            ..Default::default()
        }
    }

    #[test]
    fn bindings_group_enabled_records() {
        let records = vec![
            record("1", "Ctrl+F1", "game.exe", true),
            record("2", "Ctrl+F1", "discord.exe", true),
            record("3", "Ctrl+F2", "spotify.exe", false),
            record("4", "not a hotkey", "x.exe", true),
            Record { id: "5".into(), hotkey: Some("Alt+F3".into()), status: true, ..Default::default() },
        ];
        let (map, keys) = bindings_for(&records);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].0, "Ctrl+F1");
        let targets = map.targets(&keys[0].1);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0], Target { process: "game.exe".into(), action: Action::Toggle { min: 0.1, max: 0.8 } });
        assert_eq!(targets[1].process, "discord.exe");
    }

    #[test]
    fn adjust_mode_bindings() {
        let mut r = Record {
            id: "1".into(),
            process: Some("game.exe".into()),
            status: true,
            mode: crate::config::MODE_ADJUST,
            hotkey: Some("Ctrl+F1".into()),
            hotkey_up: Some("Ctrl+Up".into()),
            step: Some(10.0),
            ..Default::default()
        };
        // The toggle hotkey is ignored and up alone is incomplete.
        let (map, keys) = bindings_for(std::slice::from_ref(&r));
        assert!(map.is_empty() && keys.is_empty());

        r.hotkey_down = Some("Ctrl+Down".into());
        r.hotkey_mute = Some("Ctrl+M".into());
        let (map, keys) = bindings_for(std::slice::from_ref(&r));
        let names: Vec<&str> = keys.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(names, ["Ctrl+Up", "Ctrl+Down", "Ctrl+M"]);
        let action = |i: usize| map.targets(&keys[i].1)[0].action.clone();
        assert_eq!(action(0), Action::Step(0.1));
        assert_eq!(action(1), Action::Step(-0.1));
        assert_eq!(action(2), Action::Mute);
    }
}
