//! Global hotkeys. Records store hotkeys as strings like `Ctrl+F1` (the format of the
//! Electron version), which `global-hotkey` parses directly.

use crate::audio::AudioHandle;
use crate::config::Record;
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
    pub min: f32,
    pub max: f32,
}

/// Targets of every registered hotkey, keyed by hotkey id.
type TargetMap = HashMap<u32, Vec<Target>>;
type Bindings = Arc<Mutex<TargetMap>>;

pub struct Hotkeys {
    manager: Option<GlobalHotKeyManager>,
    registered: Vec<HotKey>,
    bindings: Bindings,
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
        let map = bindings.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state() != HotKeyState::Pressed {
                return;
            }
            if let Some(targets) = map.lock().unwrap().get(&event.id()) {
                for t in targets {
                    audio.toggle(&t.process, t.min, t.max);
                }
            }
        }));
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
        for (text, hotkey) in keys {
            if manager.register(hotkey).is_ok() {
                self.registered.push(hotkey);
            } else {
                failed.insert(text);
            }
        }
        *self.bindings.lock().unwrap() = map;
        failed
    }
}

/// Groups enabled records by hotkey id. Also returns each distinct hotkey once.
pub fn bindings_for(records: &[Record]) -> (TargetMap, Vec<(String, HotKey)>) {
    let mut map: TargetMap = HashMap::new();
    let mut keys: Vec<(String, HotKey)> = Vec::new();
    for r in records.iter().filter(|r| r.status) {
        let (Some(text), Some(process)) = (r.hotkey.as_deref(), r.process.as_deref()) else { continue };
        let Ok(hotkey) = HotKey::from_str(text) else { continue };
        if !keys.iter().any(|(_, k)| k.id() == hotkey.id()) {
            keys.push((text.to_string(), hotkey));
        }
        map.entry(hotkey.id()).or_default().push(Target {
            process: process.to_string(),
            min: r.min() as f32 / 100.0,
            max: r.max() as f32 / 100.0,
        });
    }
    (map, keys)
}

/// Turns a key press captured by the UI into a hotkey string such as `Ctrl+Shift+F5`.
/// Returns `None` for presses that can't be used as a global hotkey.
pub fn format_hotkey(text: &str, ctrl: bool, alt: bool, shift: bool, meta: bool) -> Option<String> {
    let key = key_name(text)?;
    let is_function_key = key.len() >= 2 && key.starts_with('F') && key[1..].chars().all(|c| c.is_ascii_digit());
    // Plain letters/digits without a modifier would swallow normal typing.
    if !(ctrl || alt || meta || shift) && !is_function_key {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    if ctrl {
        parts.push("Ctrl");
    }
    if alt {
        parts.push("Alt");
    }
    if shift {
        parts.push("Shift");
    }
    if meta {
        parts.push("Super");
    }
    let combo = if parts.is_empty() { key } else { format!("{}+{}", parts.join("+"), key) };
    HotKey::from_str(&combo).ok().map(|_| combo)
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
        assert_eq!(format_hotkey(&key(Key::F1), true, false, false, false).as_deref(), Some("Ctrl+F1"));
        assert_eq!(format_hotkey(&key(Key::F12), false, true, true, false).as_deref(), Some("Alt+Shift+F12"));
        // Function keys may be used without modifiers.
        assert_eq!(format_hotkey(&key(Key::F7), false, false, false, false).as_deref(), Some("F7"));
    }

    #[test]
    fn letters_need_a_modifier() {
        assert_eq!(format_hotkey("a", false, false, false, false), None);
        assert_eq!(format_hotkey("a", true, true, false, false).as_deref(), Some("Ctrl+Alt+A"));
        assert_eq!(format_hotkey("5", false, false, false, true).as_deref(), Some("Super+5"));
    }

    #[test]
    fn maps_shifted_digits() {
        assert_eq!(format_hotkey("!", true, false, true, false).as_deref(), Some("Ctrl+Shift+1"));
    }

    #[test]
    fn rejects_unusable_keys() {
        // Modifier alone, non-latin layout characters and multi-char text.
        assert_eq!(format_hotkey(&key(Key::Control), true, false, false, false), None);
        assert_eq!(format_hotkey("ф", true, false, false, false), None);
        assert_eq!(format_hotkey("ab", true, false, false, false), None);
    }

    #[test]
    fn formatted_hotkeys_parse() {
        for text in ["Ctrl+F1", "Shift+F12", "Alt+F5", "Ctrl+Alt+A", "Super+Space", "Ctrl+Shift+Up"] {
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
        let targets = &map[&keys[0].1.id()];
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0], Target { process: "game.exe".into(), min: 0.1, max: 0.8 });
        assert_eq!(targets[1].process, "discord.exe");
    }
}
