//! Persistent settings. The JSON layout of `records`, `lockTop` and `lang` matches the
//! electron-store file of the old Electron version, so it can be imported as is.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Record {
    pub id: String,
    pub name: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub process: Option<String>,
    pub hotkey: Option<String>,
    /// Whether the hotkey of this record is active.
    pub status: bool,
    /// 0 (default): each hotkey press switches between `min` and `max`; or [`MODE_ADJUST`].
    pub mode: i32,
    /// Adjust mode: raise / lower the volume by `step` (work only as a pair).
    pub hotkey_up: Option<String>,
    pub hotkey_down: Option<String>,
    /// Adjust mode: mute / unmute.
    pub hotkey_mute: Option<String>,
    /// Adjust mode: volume change per press, in percent.
    pub step: Option<f64>,
}

/// Hotkeys raise / lower the volume by a step and mute / unmute it.
pub const MODE_ADJUST: i32 = 1;

pub const DEFAULT_STEP: i32 = 5;
pub const MAX_STEP: i32 = 50;

/// Highest `min` / `max` a record can store. Above 100% the volume is capped by what the
/// audio backend allows at the moment (see [`crate::audio::VolumeBackend::max_volume`]).
const MAX_PERCENT: f64 = crate::audio::MAX_BOOST as f64 * 100.0;

impl Record {
    pub fn min(&self) -> i32 {
        self.min.unwrap_or(0.0).round().clamp(0.0, MAX_PERCENT) as i32
    }

    pub fn max(&self) -> i32 {
        self.max.unwrap_or(100.0).round().clamp(0.0, MAX_PERCENT) as i32
    }

    pub fn step(&self) -> i32 {
        self.step.map_or(DEFAULT_STEP, |s| s.round().clamp(1.0, MAX_STEP as f64) as i32)
    }

    pub fn adjust_mode(&self) -> bool {
        self.mode == MODE_ADJUST
    }

    /// Hotkeys that are in effect for the record's mode. Up/down count only as a pair.
    pub fn active_hotkeys(&self) -> Vec<&str> {
        if !self.adjust_mode() {
            return self.hotkey.as_deref().into_iter().collect();
        }
        let mut keys = Vec::new();
        if let (Some(up), Some(down)) = (self.hotkey_up.as_deref(), self.hotkey_down.as_deref()) {
            keys.extend([up, down]);
        }
        keys.extend(self.hotkey_mute.as_deref());
        keys
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub records: Vec<Record>,
    pub lock_top: bool,
    pub lang: String,
    /// 0 = follow system, 1 = dark, 2 = light
    pub theme: i32,
    pub close_to_tray: bool,
    pub autostart: bool,
    pub start_minimized: bool,
    pub auto_update: bool,
    /// Also update to pre-releases (beta channel).
    pub beta_updates: bool,
    /// Linux AppImage: offer to add the app to the application menu on start.
    pub ask_desktop_entry: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            records: Vec::new(),
            lock_top: false,
            lang: crate::i18n::system_language().to_string(),
            theme: 1,
            close_to_tray: true,
            autostart: false,
            start_minimized: false,
            auto_update: true,
            beta_updates: false,
            ask_desktop_entry: true,
        }
    }
}

fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("apps-volume-control")
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

/// Config files written by the Electron version (electron-store with `fileExtension: 'control'`).
fn legacy_paths() -> Vec<PathBuf> {
    let Some(base) = dirs::config_dir() else {
        return Vec::new();
    };
    ["Apps Volume Control", "App Volume Control", "apps-volume-control"]
        .iter()
        .map(|dir| base.join(dir).join("config.control"))
        .collect()
}

impl Config {
    pub fn load() -> Self {
        let mut candidates = vec![config_path()];
        candidates.extend(legacy_paths());
        for path in candidates {
            if let Ok(text) = std::fs::read_to_string(&path) {
                match serde_json::from_str::<Config>(&text) {
                    Ok(cfg) => return cfg,
                    Err(err) => eprintln!("Failed to parse {}: {err}", path.display()),
                }
            }
        }
        Config::default()
    }

    pub fn save(&self) {
        let dir = config_dir();
        if let Err(err) = std::fs::create_dir_all(&dir) {
            eprintln!("Failed to create {}: {err}", dir.display());
            return;
        }
        let Ok(text) = serde_json::to_string_pretty(self) else {
            return;
        };
        // Write to a temp file first so a crash never leaves a truncated config behind.
        let tmp = dir.join("config.json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, config_path());
        }
    }

    pub fn record_mut(&mut self, id: &str) -> Option<&mut Record> {
        self.records.iter_mut().find(|r| r.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_electron_store_file() {
        // Shape written by the Electron version (electron-store, `config.control`).
        let legacy = r#"{
            "records": [
                {"id": "1700000000000", "status": true, "name": "Game", "process": "game.exe",
                 "hotkey": "Ctrl+F1", "min": 15, "max": 90},
                {"id": "1700000000001", "status": false}
            ],
            "lockTop": true,
            "lang": "uk"
        }"#;
        let cfg: Config = serde_json::from_str(legacy).unwrap();
        assert_eq!(cfg.records.len(), 2);
        assert!(cfg.lock_top);
        assert_eq!(cfg.lang, "uk");
        let r = &cfg.records[0];
        assert_eq!((r.min(), r.max()), (15, 90));
        assert_eq!(r.hotkey.as_deref(), Some("Ctrl+F1"));
        assert!(r.status);
        let empty = &cfg.records[1];
        assert_eq!((empty.min(), empty.max()), (0, 100));
        assert!(empty.process.is_none());
        // Settings that did not exist before fall back to defaults.
        assert!(cfg.close_to_tray);
        assert!(cfg.auto_update);
    }

    #[test]
    fn clamps_volume_bounds() {
        let r = Record { min: Some(-5.0), max: Some(180.4), ..Default::default() };
        assert_eq!((r.min(), r.max()), (0, 150));
        let r = Record { min: Some(33.6), ..Default::default() };
        assert_eq!(r.min(), 34);
        assert_eq!(r.step(), DEFAULT_STEP);
        let r = Record { step: Some(0.2), ..Default::default() };
        assert_eq!(r.step(), 1);
        let r = Record { step: Some(80.0), ..Default::default() };
        assert_eq!(r.step(), MAX_STEP);
    }

    #[test]
    fn active_hotkeys_follow_mode() {
        let mut r = Record {
            hotkey: Some("Ctrl+F1".into()),
            hotkey_up: Some("Ctrl+Up".into()),
            hotkey_mute: Some("Ctrl+M".into()),
            ..Default::default()
        };
        assert_eq!(r.active_hotkeys(), vec!["Ctrl+F1"]);
        r.mode = MODE_ADJUST;
        // Up without down does nothing.
        assert_eq!(r.active_hotkeys(), vec!["Ctrl+M"]);
        r.hotkey_down = Some("Ctrl+Down".into());
        assert_eq!(r.active_hotkeys(), vec!["Ctrl+Up", "Ctrl+Down", "Ctrl+M"]);
        r.hotkey_mute = None;
        assert_eq!(r.active_hotkeys(), vec!["Ctrl+Up", "Ctrl+Down"]);
    }

    #[test]
    fn round_trips() {
        let mut cfg = Config::default();
        cfg.records.push(Record { id: "a".into(), name: Some("A".into()), status: true, ..Default::default() });
        cfg.theme = 2;
        let text = serde_json::to_string(&cfg).unwrap();
        assert!(text.contains("\"lockTop\""));
        assert!(text.contains("\"closeToTray\""));
        assert!(text.contains("\"hotkeyUp\""));
        let back: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(back.theme, 2);
        assert_eq!(back.records[0].mode, 0);
        assert_eq!(back.records[0].name.as_deref(), Some("A"));
    }

    #[test]
    fn finds_records() {
        let mut cfg = Config::default();
        cfg.records.push(Record { id: "x".into(), ..Default::default() });
        cfg.record_mut("x").unwrap().status = true;
        assert!(cfg.records[0].status);
        assert!(cfg.record_mut("missing").is_none());
    }
}
