#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

slint::include_modules!();

mod audio;
mod autostart;
mod config;
#[cfg(target_os = "linux")]
mod desktop;
mod hotkeys;
mod i18n;
mod instance;
mod tray;
mod updater;

use audio::{AudioApp, AudioHandle};
use config::{Config, Record};
use hotkeys::Hotkeys;
use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

const REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const UPDATE_INTERVAL: Duration = Duration::from_secs(30 * 60);

// Values of the `update-state` property in ui/app.slint.
const UPDATE_NONE: i32 = 0;
const UPDATE_AVAILABLE: i32 = 1;
const UPDATE_DOWNLOADING: i32 = 2;
const UPDATE_READY: i32 = 3;
const UPDATE_ERROR: i32 = 4;

/// Everything owned by the UI thread. Slint callbacks reach it through [`with_app`].
struct App {
    ui: AppWindow,
    tray: tray::Tray,
    records: Rc<VecModel<RecordData>>,
    cfg: Config,
    apps: Vec<AudioApp>,
    audio: AudioHandle,
    audio_error: Option<String>,
    hotkeys: Hotkeys,
    failed_hotkeys: HashSet<String>,
    /// Process keys behind the entries of the process combo box ("" = none).
    edit_keys: Vec<String>,
    refresh_edit_list: bool,
    release: Option<updater::Release>,
    downloaded: Option<PathBuf>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| cell.borrow_mut().as_mut().map(f))
}

fn new_record_id() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default()
}

fn open_url(url: &str) {
    #[cfg(windows)]
    let cmd = std::process::Command::new("explorer").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(err) = cmd {
        eprintln!("Failed to open {url}: {err}");
    }
}

fn show_window(ui: &AppWindow) {
    let _ = ui.show();
    ui.window().set_minimized(false);
}

/// Builds the row shown for `record`, merged with the live audio state.
fn record_row(record: &Record, apps: &[AudioApp], failed: &HashSet<String>) -> RecordData {
    let process = record.process.clone().unwrap_or_default();
    let app = apps.iter().find(|a| !process.is_empty() && a.key == process);
    let hotkey = record.hotkey.clone().unwrap_or_default();
    RecordData {
        id: record.id.as_str().into(),
        name: record.name.clone().unwrap_or_default().into(),
        process: process.as_str().into(),
        process_title: match app {
            Some(a) if a.title != a.key => format!("{} · {}", a.title, a.key).into(),
            _ => process.as_str().into(),
        },
        running: app.is_some(),
        hotkey_error: record.status && !hotkey.is_empty() && failed.contains(&hotkey),
        hotkey: hotkey.into(),
        min: record.min(),
        max: record.max(),
        enabled: record.status,
        volume: app.filter(|a| a.active).map(|a| (a.volume * 100.0).round() as i32).unwrap_or(-1),
    }
}

impl App {
    fn render(&self) {
        let rows: Vec<RecordData> =
            self.cfg.records.iter().map(|r| record_row(r, &self.apps, &self.failed_hotkeys)).collect();
        // Update rows in place so open editors keep their focus and state.
        for (i, row) in rows.iter().enumerate() {
            if i < self.records.row_count() {
                if self.records.row_data(i).as_ref() != Some(row) {
                    self.records.set_row_data(i, row.clone());
                }
            } else {
                self.records.push(row.clone());
            }
        }
        while self.records.row_count() > rows.len() {
            self.records.remove(self.records.row_count() - 1);
        }

        let mut status = Vec::new();
        if let Some(err) = &self.audio_error {
            status.push(format!("{}: {err}", self.ui.global::<Tr>().get_audio_unavailable()));
        }
        if !self.hotkeys.available() {
            status.push(self.ui.global::<Tr>().get_hotkeys_unavailable().to_string());
        }
        self.ui.set_status_message(status.join("\n").into());
    }

    fn save(&self) {
        self.cfg.save();
    }

    fn apply_hotkeys(&mut self) {
        self.failed_hotkeys = self.hotkeys.apply(&self.cfg.records);
    }

    fn update_record(&mut self, id: &str, f: impl FnOnce(&mut Record)) {
        if let Some(r) = self.cfg.record_mut(id) {
            f(r);
        }
        self.save();
        self.apply_hotkeys();
        self.render();
    }

    /// Fills the process combo box for the record being edited.
    fn build_edit_list(&mut self, id: &str) {
        let tr = self.ui.global::<Tr>();
        let current = self.cfg.records.iter().find(|r| r.id == id).and_then(|r| r.process.clone());
        let mut labels: Vec<SharedString> = Vec::new();
        let mut keys: Vec<String> = Vec::new();
        match &current {
            None => {
                labels.push(tr.get_no_process());
                keys.push(String::new());
            }
            Some(p) if !self.apps.iter().any(|a| &a.key == p) => {
                labels.push(format!("{p} [{}]", tr.get_process_closed()).into());
                keys.push(p.clone());
            }
            _ => {}
        }
        for a in &self.apps {
            let mut label = if a.title != a.key { format!("{} — {}", a.title, a.key) } else { a.key.clone() };
            if !a.active {
                label = format!("{label} · {}", tr.get_process_idle());
            }
            labels.push(label.into());
            keys.push(a.key.clone());
        }
        let index = current.as_ref().and_then(|p| keys.iter().position(|k| k == p)).unwrap_or(0);
        self.ui.set_process_labels(ModelRc::new(VecModel::from(labels)));
        self.ui.set_process_index(index as i32);
        self.edit_keys = keys;
    }

    fn apply_language(&self) {
        let idx = i18n::index_of(&self.cfg.lang);
        i18n::apply(&self.ui.global::<Tr>(), idx);
        self.tray.set_language(idx);
    }

    fn read_settings_from_ui(&mut self) {
        let ui = &self.ui;
        let autostart = ui.get_autostart();
        if autostart != self.cfg.autostart {
            if let Err(err) = autostart::set(autostart) {
                eprintln!("Failed to change autostart: {err}");
            }
        }
        self.cfg.autostart = autostart;
        self.cfg.lock_top = ui.get_on_top();
        self.cfg.close_to_tray = ui.get_close_to_tray();
        self.cfg.start_minimized = ui.get_start_minimized();
        self.cfg.auto_update = ui.get_auto_update();
        self.save();
    }

    fn set_update_state(&self, state: i32) {
        self.ui.set_update_state(state);
    }
}

fn on_audio_event(event: audio::Event) {
    let _ = slint::invoke_from_event_loop(move || {
        with_app(|app| {
            match event {
                audio::Event::Apps(apps) => {
                    app.audio_error = None;
                    app.apps = apps;
                }
                audio::Event::Unavailable(err) => app.audio_error = Some(err),
            }
            if app.refresh_edit_list {
                app.refresh_edit_list = false;
                let id = app.ui.get_editing_id();
                if !id.is_empty() {
                    app.build_edit_list(&id);
                }
            }
            app.render();
        });
    });
}

/// Runs an update check in the background. `manual` also reports "up to date" and errors.
fn check_updates(manual: bool) {
    if manual {
        with_app(|app| {
            let text = app.ui.global::<Tr>().get_update_checking();
            app.ui.set_update_check_status(text);
        });
    }
    std::thread::spawn(move || {
        let result = updater::check();
        let _ = slint::invoke_from_event_loop(move || {
            with_app(|app| match result {
                Ok(Some(release)) => {
                    app.ui.set_update_version(release.version.as_str().into());
                    app.ui.set_update_check_status(
                        format!("{}: v{}", app.ui.global::<Tr>().get_update_available(), release.version).into(),
                    );
                    if app.ui.get_update_state() == UPDATE_NONE {
                        app.set_update_state(UPDATE_AVAILABLE);
                    }
                    app.release = Some(release);
                }
                Ok(None) => {
                    if manual {
                        let text = app.ui.global::<Tr>().get_update_latest();
                        app.ui.set_update_check_status(text);
                    }
                }
                Err(err) => {
                    eprintln!("Update check failed: {err}");
                    if manual {
                        app.ui.set_update_check_status(
                            format!("{}: {err}", app.ui.global::<Tr>().get_update_error()).into(),
                        );
                    }
                }
            });
        });
    });
}

fn download_update() {
    let Some(release) = with_app(|app| app.release.clone()).flatten() else { return };
    with_app(|app| {
        app.ui.set_update_progress(0.0);
        app.set_update_state(UPDATE_DOWNLOADING);
    });
    std::thread::spawn(move || {
        let last = std::sync::atomic::AtomicU32::new(0);
        let result = updater::download(&release, |p| {
            let percent = (p * 100.0) as u32;
            if last.swap(percent, std::sync::atomic::Ordering::Relaxed) != percent {
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|app| app.ui.set_update_progress(p));
                });
            }
        });
        let _ = slint::invoke_from_event_loop(move || {
            with_app(|app| match result {
                Ok(path) => {
                    app.downloaded = Some(path);
                    app.set_update_state(UPDATE_READY);
                }
                Err(err) => {
                    eprintln!("Update download failed: {err}");
                    app.set_update_state(UPDATE_ERROR);
                }
            });
        });
    });
}

fn install_update() {
    let Some(path) = with_app(|app| app.downloaded.clone()).flatten() else { return };
    match updater::install(&path) {
        Ok(()) => {
            let _ = slint::quit_event_loop();
        }
        Err(err) => {
            eprintln!("Update install failed: {err}");
            with_app(|app| {
                app.set_update_state(UPDATE_ERROR);
                app.ui.set_update_check_status(format!("{}: {err}", app.ui.global::<Tr>().get_update_error()).into());
            });
        }
    }
}

fn wire_callbacks(ui: &AppWindow) {
    ui.on_add_record(|| {
        with_app(|app| {
            let id = new_record_id();
            app.cfg.records.push(Record { id: id.clone(), ..Default::default() });
            app.save();
            app.render();
            // Open the editor of the new record straight away.
            app.ui.set_editing_id(id.as_str().into());
            app.build_edit_list(&id);
            app.audio.refresh();
            app.refresh_edit_list = true;
        });
    });
    ui.on_begin_edit(|id| {
        with_app(|app| app.build_edit_list(&id));
    });
    ui.on_remove_record(|id| {
        with_app(|app| {
            app.cfg.records.retain(|r| r.id != id.as_str());
            app.save();
            app.apply_hotkeys();
            app.render();
        });
    });
    ui.on_set_enabled(|id, enabled| {
        with_app(|app| app.update_record(&id, |r| r.status = enabled));
    });
    ui.on_set_name(|id, name| {
        with_app(|app| {
            let name: String = name.chars().take(40).collect();
            if let Some(r) = app.cfg.record_mut(&id) {
                r.name = (!name.is_empty()).then_some(name);
            }
            app.save();
            app.render();
        });
    });
    ui.on_set_process(|id, index| {
        with_app(|app| {
            let Some(key) = app.edit_keys.get(index as usize).cloned() else { return };
            let title = app.apps.iter().find(|a| a.key == key).map(|a| a.title.clone());
            app.update_record(&id, |r| {
                r.process = (!key.is_empty()).then_some(key);
                if r.name.is_none() {
                    r.name = title;
                }
            });
        });
    });
    ui.on_refresh_processes(|| {
        with_app(|app| {
            app.refresh_edit_list = true;
            app.audio.refresh();
        });
    });
    ui.on_hotkey_captured(|id, text, ctrl, alt, shift, meta| {
        let Some(hotkey) = hotkeys::format_hotkey(&text, ctrl, alt, shift, meta) else { return false };
        with_app(|app| app.update_record(&id, |r| r.hotkey = Some(hotkey)));
        true
    });
    ui.on_hotkey_cleared(|id| {
        with_app(|app| app.update_record(&id, |r| r.hotkey = None));
    });
    ui.on_set_min(|id, v| {
        with_app(|app| app.update_record(&id, |r| r.min = Some(v as f64)));
    });
    ui.on_set_max(|id, v| {
        with_app(|app| app.update_record(&id, |r| r.max = Some(v as f64)));
    });
    ui.on_set_volume(|id, v| {
        with_app(|app| {
            if let Some(p) = app.cfg.records.iter().find(|r| r.id == id.as_str()).and_then(|r| r.process.clone()) {
                app.audio.set_volume(&p, v as f32 / 100.0);
            }
        });
    });
    ui.on_toggle_now(|id| {
        with_app(|app| {
            if let Some(r) = app.cfg.records.iter().find(|r| r.id == id.as_str()) {
                if let Some(p) = &r.process {
                    app.audio.toggle(p, r.min() as f32 / 100.0, r.max() as f32 / 100.0);
                }
            }
        });
    });

    ui.on_language_changed(|index| {
        with_app(|app| {
            app.cfg.lang = i18n::LANGS[(index.max(0) as usize).min(i18n::LANGS.len() - 1)].to_string();
            app.save();
            app.apply_language();
            app.render();
        });
    });
    ui.on_theme_changed(|index| {
        with_app(|app| {
            app.cfg.theme = index;
            app.save();
        });
    });
    ui.on_settings_changed(|| {
        with_app(|app| app.read_settings_from_ui());
    });
    ui.on_open_url(|url| open_url(&url));

    ui.on_check_updates(|| check_updates(true));
    ui.on_download_update(download_update);
    ui.on_install_update(install_update);
    ui.on_open_release_page(|| {
        let page = with_app(|app| app.release.as_ref().map(|r| r.page.clone())).flatten();
        open_url(page.as_deref().unwrap_or(updater::RELEASES_PAGE));
    });

    ui.window().on_close_requested(|| {
        let to_tray = with_app(|app| app.cfg.close_to_tray).unwrap_or(false);
        if !to_tray {
            let _ = slint::quit_event_loop();
        }
        CloseRequestResponse::HideWindow
    });
}

/// Tray actions may fire on a tray thread (Linux), so they hop to the UI thread.
fn create_tray() -> Result<tray::Tray, slint::PlatformError> {
    tray::Tray::new(
        std::sync::Arc::new(|| {
            let _ = slint::invoke_from_event_loop(|| {
                with_app(|app| show_window(&app.ui));
            });
        }),
        std::sync::Arc::new(|| {
            let _ = slint::invoke_from_event_loop(|| {
                let _ = slint::quit_event_loop();
            });
        }),
    )
}

fn main() -> Result<(), slint::PlatformError> {
    let args: Vec<String> = std::env::args().collect();
    let after_update = args.iter().any(|a| a == updater::UPDATED_ARG);

    let Some(listener) = instance::acquire(after_update) else {
        return Ok(()); // another instance was asked to show itself
    };
    updater::cleanup_previous();

    let cfg = Config::load();
    let start_hidden = args.iter().any(|a| a == autostart::MINIMIZED_ARG) || cfg.start_minimized;

    let ui = AppWindow::new()?;
    // Wayland has no per-window icon: the shell takes it from the .desktop file named
    // after the app_id. Must be set after the platform exists but before the window shows.
    #[cfg(target_os = "linux")]
    {
        desktop::integrate();
        slint::set_xdg_app_id(desktop::APP_ID)?;
    }
    let tray = create_tray()?;
    let records = Rc::new(VecModel::<RecordData>::default());
    ui.set_records(ModelRc::from(records.clone()));
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    ui.set_language_names(ModelRc::new(VecModel::from(
        i18n::LANG_NAMES.iter().map(|n| SharedString::from(*n)).collect::<Vec<_>>(),
    )));
    ui.set_language_index(i18n::index_of(&cfg.lang) as i32);
    ui.set_theme_index(cfg.theme);
    ui.set_on_top(cfg.lock_top);
    ui.set_close_to_tray(cfg.close_to_tray);
    ui.set_autostart(cfg.autostart);
    ui.set_start_minimized(cfg.start_minimized);
    ui.set_auto_update(cfg.auto_update);

    let (audio, audio_thread) = audio::spawn(on_audio_event);
    let hotkeys = Hotkeys::new(audio.clone());

    APP.with(|cell| {
        *cell.borrow_mut() = Some(App {
            ui: ui.clone_strong(),
            tray,
            records,
            cfg,
            apps: Vec::new(),
            audio: audio.clone(),
            audio_error: None,
            hotkeys,
            failed_hotkeys: HashSet::new(),
            edit_keys: Vec::new(),
            refresh_edit_list: false,
            release: None,
            downloaded: None,
        })
    });
    with_app(|app| {
        if app.cfg.autostart {
            // Re-write the entry so it follows the executable after moves/updates.
            let _ = autostart::set(true);
        }
        app.apply_language();
        app.apply_hotkeys();
        app.render();
    });
    wire_callbacks(&ui);

    instance::listen(listener, || {
        let _ = slint::invoke_from_event_loop(|| {
            with_app(|app| show_window(&app.ui));
        });
    });

    // Poll the mixer only while the window is visible; hotkeys don't need it.
    let refresh_timer = Timer::default();
    refresh_timer.start(TimerMode::Repeated, REFRESH_INTERVAL, || {
        with_app(|app| {
            if app.ui.window().is_visible() {
                app.audio.refresh();
            }
        });
    });

    let update_timer = Timer::default();
    update_timer.start(TimerMode::Repeated, UPDATE_INTERVAL, || {
        if with_app(|app| app.cfg.auto_update).unwrap_or(false) {
            check_updates(false);
        }
    });
    if with_app(|app| app.cfg.auto_update).unwrap_or(false) {
        Timer::single_shot(Duration::from_secs(3), || check_updates(false));
    }

    if !start_hidden {
        ui.show()?;
    }
    slint::run_event_loop_until_quit()?;

    with_app(|app| app.save());
    audio.shutdown();
    let _ = audio_thread.join();
    APP.with(|cell| cell.borrow_mut().take());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apps() -> Vec<AudioApp> {
        vec![
            AudioApp { key: "game.exe".into(), title: "My Game".into(), volume: 0.42, active: true },
            AudioApp { key: "bare.exe".into(), title: "bare.exe".into(), volume: 1.0, active: true },
            AudioApp { key: "quiet".into(), title: "Quiet App".into(), volume: 0.3, active: false },
        ]
    }

    #[test]
    fn row_for_running_process() {
        let r = Record {
            id: "1".into(),
            name: Some("Game".into()),
            process: Some("game.exe".into()),
            hotkey: Some("Ctrl+F1".into()),
            min: Some(20.0),
            max: Some(90.0),
            status: true,
        };
        let row = record_row(&r, &apps(), &HashSet::new());
        assert!(row.running);
        assert_eq!(row.volume, 42);
        assert_eq!(row.process_title.as_str(), "My Game · game.exe");
        assert_eq!((row.min, row.max), (20, 90));
        assert!(!row.hotkey_error);
    }

    #[test]
    fn row_for_closed_process_and_busy_hotkey() {
        let r = Record {
            id: "2".into(),
            process: Some("closed.exe".into()),
            hotkey: Some("Ctrl+F2".into()),
            status: true,
            ..Default::default()
        };
        let failed: HashSet<String> = ["Ctrl+F2".to_string()].into();
        let row = record_row(&r, &apps(), &failed);
        assert!(!row.running);
        assert_eq!(row.volume, -1);
        assert!(row.hotkey_error);
        assert_eq!(row.name.as_str(), "");
    }

    #[test]
    fn disabled_record_never_reports_hotkey_error() {
        let r = Record { id: "3".into(), hotkey: Some("Ctrl+F2".into()), status: false, ..Default::default() };
        let failed: HashSet<String> = ["Ctrl+F2".to_string()].into();
        assert!(!record_row(&r, &apps(), &failed).hotkey_error);
    }

    #[test]
    fn quiet_app_is_running_without_volume() {
        let r = Record { id: "5".into(), process: Some("quiet".into()), ..Default::default() };
        let row = record_row(&r, &apps(), &HashSet::new());
        assert!(row.running);
        assert_eq!(row.volume, -1);
    }

    #[test]
    fn title_equal_to_key_is_not_repeated() {
        let r = Record { id: "4".into(), process: Some("bare.exe".into()), ..Default::default() };
        assert_eq!(record_row(&r, &apps(), &HashSet::new()).process_title.as_str(), "bare.exe");
    }
}
