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
}

impl Record {
    pub fn min(&self) -> i32 {
        self.min.unwrap_or(0.0).round().clamp(0.0, 100.0) as i32
    }

    pub fn max(&self) -> i32 {
        self.max.unwrap_or(100.0).round().clamp(0.0, 100.0) as i32
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
        let r = Record { min: Some(-5.0), max: Some(150.4), ..Default::default() };
        assert_eq!((r.min(), r.max()), (0, 100));
        let r = Record { min: Some(33.6), ..Default::default() };
        assert_eq!(r.min(), 34);
    }

    #[test]
    fn round_trips() {
        let mut cfg = Config::default();
        cfg.records.push(Record { id: "a".into(), name: Some("A".into()), status: true, ..Default::default() });
        cfg.theme = 2;
        let text = serde_json::to_string(&cfg).unwrap();
        assert!(text.contains("\"lockTop\""));
        assert!(text.contains("\"closeToTray\""));
        let back: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(back.theme, 2);
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
