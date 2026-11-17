#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

slint::include_modules!();

mod audio;
mod autostart;
mod config;
#[cfg(target_os = "linux")]
mod desktop;
mod hotkeys;
mod i18n;
mod icons;
mod instance;
mod tray;
mod updater;

use audio::{AudioApp, AudioHandle};
use config::{Config, Record};
use hotkeys::Hotkeys;
use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
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

// Hotkey slots of a record, as passed by `hotkey-captured` / `hotkey-cleared` in ui/app.slint
// (0 = the toggle hotkey).
const SLOT_UP: i32 = 1;
const SLOT_DOWN: i32 = 2;
const SLOT_MUTE: i32 = 3;

fn hotkey_slot(record: &mut Record, slot: i32) -> &mut Option<String> {
    match slot {
        SLOT_UP => &mut record.hotkey_up,
        SLOT_DOWN => &mut record.hotkey_down,
        SLOT_MUTE => &mut record.hotkey_mute,
        _ => &mut record.hotkey,
    }
}

/// Hotkeys of a record for the header chip, e.g. `Ctrl+Up / Ctrl+Down · Ctrl+M` ("" = none).
fn hotkey_label(record: &Record) -> String {
    if !record.adjust_mode() {
        return record.hotkey.clone().unwrap_or_default();
    }
    let mut parts = Vec::new();
    if let (Some(up), Some(down)) = (&record.hotkey_up, &record.hotkey_down) {
        parts.push(format!("{up} / {down}"));
    }
    parts.extend(record.hotkey_mute.clone());
    parts.join(" · ")
}

/// Everything owned by the UI thread. Slint callbacks reach it through [`with_app`].
struct App {
    ui: AppWindow,
    tray: tray::Tray,
    records: Rc<VecModel<RecordData>>,
    cfg: Config,
    apps: Vec<AudioApp>,
    audio: AudioHandle,
    audio_error: Option<String>,
    /// Highest volume the audio backend allows right now, in percent.
    max_volume: i32,
    hotkeys: Hotkeys,
    failed_hotkeys: HashSet<String>,
    /// Process keys behind the entries of the process combo box ("" = none).
    edit_keys: Vec<String>,
    /// Loaded icon per process key (`None`: no icon). Filled lazily from the icon cache.
    icons: RefCell<HashMap<String, Option<slint::Image>>>,
    /// `icons` resampled for the window's scale factor (the factor they were made for).
    scaled_icons: RefCell<(f32, HashMap<String, slint::Image>)>,
    /// Keys whose icon was refreshed by the audio worker in this session.
    live_icons: HashSet<String>,
    refresh_edit_list: bool,
    release: Option<updater::Release>,
    downloaded: Option<PathBuf>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// Physical key (winit name, e.g. `KeyA`) of the last press in the window: Slint's key
    /// events only carry layout-dependent text and don't tell numpad keys apart.
    static PHYSICAL_KEY: RefCell<String> = const { RefCell::new(String::new()) };
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
    ui.set_animate(true);
}

/// Letter shown in place of a missing icon.
fn initial(label: &str) -> SharedString {
    label.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default().into()
}

fn load_icon(path: &std::path::Path) -> Option<slint::Image> {
    slint::Image::load_from_path(path).ok().filter(|i| i.size().width > 0)
}

/// Largest logical size an app icon is drawn at (the record card's `AppIcon`).
const ICON_SIZE: f32 = 26.0;

/// `image` downscaled to `size` physical pixels; SVGs and small images stay as they are.
fn fit_icon(image: &slint::Image, size: u32) -> slint::Image {
    let Some(buffer) = image.to_rgba8() else { return image.clone() };
    let Some((w, h, pixels)) = icons::downscale(buffer.width(), buffer.height(), buffer.as_bytes(), size) else {
        return image.clone();
    };
    slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(&pixels, w, h))
}

/// Builds the row shown for `record`, merged with the live audio state. Volumes are shown up to
/// `limit` percent (the highest volume allowed right now).
fn record_row(record: &Record, apps: &[AudioApp], failed: &HashSet<String>, limit: i32) -> RecordData {
    let process = record.process.clone().unwrap_or_default();
    let app = apps.iter().find(|a| !process.is_empty() && a.key == process);
    let hotkey = record.hotkey.clone().unwrap_or_default();
    let active = record.active_hotkeys();
    // Adjust mode needs both up and down, or mute: a lone up/down key does nothing.
    let incomplete = record.adjust_mode() && (record.hotkey_up.is_some() != record.hotkey_down.is_some());
    RecordData {
        id: record.id.as_str().into(),
        name: record.name.clone().unwrap_or_default().into(),
        process: process.as_str().into(),
        process_title: match app {
            Some(a) if a.title != a.key => format!("{} · {}", a.title, a.key).into(),
            _ => process.as_str().into(),
        },
        running: app.is_some(),
        hotkey_error: record.status && active.iter().any(|h| failed.contains(*h)),
        hotkey: hotkey.into(),
        hotkey_up: record.hotkey_up.clone().unwrap_or_default().into(),
        hotkey_down: record.hotkey_down.clone().unwrap_or_default().into(),
        hotkey_mute: record.hotkey_mute.clone().unwrap_or_default().into(),
        hotkey_label: hotkey_label(record).into(),
        hotkeys_incomplete: incomplete,
        mode: record.mode,
        step: record.step(),
        min: record.min().min(limit),
        max: record.max().min(limit),
        enabled: record.status,
        volume: app.filter(|a| a.active).map(|a| ((a.volume * 100.0).round() as i32).min(limit)).unwrap_or(-1),
        icon: slint::Image::default(),
        initial: initial(app.map(|a| a.title.as_str()).unwrap_or(&process)),
    }
}

impl App {
    /// Icon of a process key: the one found in this session, else the cached one.
    fn icon(&self, key: &str) -> slint::Image {
        if key.is_empty() {
            return slint::Image::default();
        }
        let mut icons = self.icons.borrow_mut();
        let icon = icons.entry(key.to_string()).or_insert_with(|| icons::cached(key).and_then(|p| load_icon(&p)));
        let Some(icon) = icon else { return slint::Image::default() };
        let scale = self.ui.window().scale_factor();
        let mut scaled = self.scaled_icons.borrow_mut();
        if scaled.0 != scale {
            *scaled = (scale, HashMap::new());
        }
        let size = (ICON_SIZE * scale).ceil() as u32;
        scaled.1.entry(key.to_string()).or_insert_with(|| fit_icon(icon, size)).clone()
    }

    /// Loads icons the audio worker resolved, replacing older cached ones once per session.
    fn load_live_icons(&mut self) {
        for app in &self.apps {
            let Some(path) = &app.icon else { continue };
            if self.live_icons.insert(app.key.clone()) {
                self.icons.borrow_mut().insert(app.key.clone(), load_icon(path));
                self.scaled_icons.borrow_mut().1.remove(&app.key);
            }
        }
    }

    fn render(&self) {
        let rows: Vec<RecordData> = self
            .cfg
            .records
            .iter()
            .map(|r| {
                let mut row = record_row(r, &self.apps, &self.failed_hotkeys, self.max_volume);
                row.icon = self.icon(&row.process);
                row
            })
            .collect();
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
        self.ui.set_enabled_count(rows.iter().filter(|r| r.enabled).count() as i32);
        self.ui.set_running_count(rows.iter().filter(|r| r.running).count() as i32);

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
        let mut items: Vec<ProcessItem> = Vec::new();
        let mut keys: Vec<String> = Vec::new();
        match &current {
            None => {
                items.push(ProcessItem { label: tr.get_no_process(), ..Default::default() });
                keys.push(String::new());
            }
            Some(p) if !self.apps.iter().any(|a| &a.key == p) => {
                let label = format!("{p} [{}]", tr.get_process_closed()).into();
                items.push(ProcessItem { label, icon: self.icon(p), initial: initial(p) });
                keys.push(p.clone());
            }
            _ => {}
        }
        for a in &self.apps {
            let mut label = if a.title != a.key { format!("{} — {}", a.title, a.key) } else { a.key.clone() };
            if !a.active {
                label = format!("{label} · {}", tr.get_process_idle());
            }
            items.push(ProcessItem { label: label.into(), icon: self.icon(&a.key), initial: initial(&a.title) });
            keys.push(a.key.clone());
        }
        let index = current.as_ref().and_then(|p| keys.iter().position(|k| k == p)).unwrap_or(0);
        self.ui.set_process_items(ModelRc::new(VecModel::from(items)));
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
        let beta = ui.get_beta_updates();
        if beta && !self.cfg.beta_updates {
            // Look for a pre-release right away; deferred as this runs inside `with_app`.
            Timer::single_shot(Duration::ZERO, || check_updates(false));
        }
        self.cfg.beta_updates = beta;
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
                    app.load_live_icons();
                }
                audio::Event::MaxVolume(max) => {
                    app.max_volume = (max * 100.0).round() as i32;
                    app.ui.global::<Limits>().set_max_volume(app.max_volume);
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
    let beta = with_app(|app| app.cfg.beta_updates).unwrap_or(false);
    std::thread::spawn(move || {
        let result = updater::check(beta);
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
    use slint::winit_030::{winit, EventResult, WinitWindowAccessor};
    // Runs before Slint dispatches the key, so `hotkey_captured` sees the key of its own press.
    ui.window().on_winit_window_event(|_, event| {
        if let winit::event::WindowEvent::KeyboardInput { event, .. } = event {
            if event.state.is_pressed() {
                let name = match event.physical_key {
                    winit::keyboard::PhysicalKey::Code(code) => format!("{code:?}"),
                    _ => String::new(),
                };
                PHYSICAL_KEY.set(name);
            }
        }
        EventResult::Propagate
    });
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
            app.ui.set_process_index(index);
        });
    });
    ui.on_refresh_processes(|| {
        with_app(|app| {
            app.refresh_edit_list = true;
            app.audio.refresh();
        });
    });
    ui.on_hotkey_captured(|id, slot, text, ctrl, alt, shift, meta| {
        let physical = PHYSICAL_KEY.with_borrow(Clone::clone);
        let Some(hotkey) = hotkeys::format_hotkey(&text, &physical, ctrl, alt, shift, meta) else { return false };
        with_app(|app| app.update_record(&id, |r| *hotkey_slot(r, slot) = Some(hotkey)));
        true
    });
    ui.on_hotkey_cleared(|id, slot| {
        with_app(|app| app.update_record(&id, |r| *hotkey_slot(r, slot) = None));
    });
    ui.on_set_mode(|id, mode| {
        with_app(|app| app.update_record(&id, |r| r.mode = mode));
    });
    ui.on_set_step(|id, v| {
        with_app(|app| app.update_record(&id, |r| r.step = Some(v as f64)));
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
                    if r.adjust_mode() {
                        app.audio.mute(p);
                    } else {
                        app.audio.toggle(p, r.min() as f32 / 100.0, r.max() as f32 / 100.0);
                    }
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
    #[cfg(target_os = "linux")]
    {
        ui.on_desktop_register(|| {
            with_app(|app| app.ui.set_desktop_state(desktop::register()));
        });
        ui.on_desktop_unregister(|| {
            with_app(|app| {
                app.ui.set_desktop_state(desktop::unregister());
                // Removed on purpose: don't offer it again on the next start.
                app.cfg.ask_desktop_entry = false;
                app.save();
            });
        });
        ui.on_desktop_prompt_answer(|add, dont_ask| {
            with_app(|app| {
                app.ui.set_desktop_prompt(false);
                if add {
                    app.ui.set_desktop_state(desktop::register());
                }
                if add || dont_ask {
                    app.cfg.ask_desktop_entry = false;
                    app.save();
                }
            });
        });
    }

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
        with_app(|app| app.ui.set_animate(false));
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
        slint::set_xdg_app_id(desktop::APP_ID)?;
        let state = desktop::state();
        ui.set_desktop_state(state);
        ui.set_desktop_prompt(state == desktop::MISSING && cfg.ask_desktop_entry && !desktop::has_appimagelauncher());
    }
    let tray = create_tray()?;
    let records = Rc::new(VecModel::<RecordData>::default());
    ui.set_records(ModelRc::from(records.clone()));
    ui.set_version(updater::current_version().into());
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
    ui.set_beta_updates(cfg.beta_updates);

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
            max_volume: 100,
            hotkeys,
            failed_hotkeys: HashSet::new(),
            edit_keys: Vec::new(),
            icons: RefCell::default(),
            scaled_icons: RefCell::default(),
            live_icons: HashSet::new(),
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
            let window = app.ui.window();
            if window.is_visible() {
                app.audio.refresh();
            }
            // Catches minimizing, which has no callback; showing and closing set it right away.
            app.ui.set_animate(window.is_visible() && !window.is_minimized());
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
            AudioApp { key: "game.exe".into(), title: "My Game".into(), volume: 0.42, active: true, icon: None },
            AudioApp { key: "bare.exe".into(), title: "bare.exe".into(), volume: 1.0, active: true, icon: None },
            AudioApp { key: "quiet".into(), title: "Quiet App".into(), volume: 0.3, active: false, icon: None },
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
            ..Default::default()
        };
        let row = record_row(&r, &apps(), &HashSet::new(), 100);
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
        let row = record_row(&r, &apps(), &failed, 100);
        assert!(!row.running);
        assert_eq!(row.volume, -1);
        assert!(row.hotkey_error);
        assert_eq!(row.name.as_str(), "");
    }

    #[test]
    fn disabled_record_never_reports_hotkey_error() {
        let r = Record { id: "3".into(), hotkey: Some("Ctrl+F2".into()), status: false, ..Default::default() };
        let failed: HashSet<String> = ["Ctrl+F2".to_string()].into();
        assert!(!record_row(&r, &apps(), &failed, 100).hotkey_error);
    }

    #[test]
    fn quiet_app_is_running_without_volume() {
        let r = Record { id: "5".into(), process: Some("quiet".into()), ..Default::default() };
        let row = record_row(&r, &apps(), &HashSet::new(), 100);
        assert!(row.running);
        assert_eq!(row.volume, -1);
    }

    #[test]
    fn initials_for_missing_icons() {
        assert_eq!(initial("discord").as_str(), "D");
        assert_eq!(initial("  [beta] app").as_str(), "B");
        assert_eq!(initial("ß").as_str(), "SS");
        assert_eq!(initial("").as_str(), "");
        let r = Record { id: "6".into(), process: Some("game.exe".into()), ..Default::default() };
        assert_eq!(record_row(&r, &apps(), &HashSet::new(), 100).initial.as_str(), "M", "from the live title");
        let r = Record { id: "7".into(), process: Some("closed.exe".into()), ..Default::default() };
        assert_eq!(record_row(&r, &apps(), &HashSet::new(), 100).initial.as_str(), "C");
    }

    #[test]
    fn row_for_adjust_mode() {
        let mut r = Record {
            id: "8".into(),
            mode: config::MODE_ADJUST,
            hotkey: Some("Ctrl+F1".into()),
            hotkey_up: Some("Ctrl+Up".into()),
            hotkey_mute: Some("Ctrl+M".into()),
            status: true,
            ..Default::default()
        };
        // The toggle hotkey of the other mode doesn't count, a lone "up" is flagged.
        let failed: HashSet<String> = ["Ctrl+F1".to_string(), "Ctrl+Up".to_string()].into();
        let row = record_row(&r, &apps(), &failed, 100);
        assert!(row.hotkeys_incomplete);
        assert!(!row.hotkey_error);
        assert_eq!(row.hotkey_label.as_str(), "Ctrl+M");
        assert_eq!(row.step, config::DEFAULT_STEP);

        r.hotkey_down = Some("Ctrl+Down".into());
        let row = record_row(&r, &apps(), &failed, 100);
        assert!(!row.hotkeys_incomplete);
        assert!(row.hotkey_error);
        assert_eq!(row.hotkey_label.as_str(), "Ctrl+Up / Ctrl+Down · Ctrl+M");
    }

    #[test]
    fn title_equal_to_key_is_not_repeated() {
        let r = Record { id: "4".into(), process: Some("bare.exe".into()), ..Default::default() };
        assert_eq!(record_row(&r, &apps(), &HashSet::new(), 100).process_title.as_str(), "bare.exe");
    }
}
