//! Per-application volume control. Every platform backend runs on its own worker
//! thread (COM apartments, the PulseAudio main loop and CoreAudio taps all prefer a
//! single owning thread); the rest of the app talks to it through [`AudioHandle`].

use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux::Backend;
#[cfg(target_os = "macos")]
use macos::Backend;
#[cfg(windows)]
use windows::Backend;

/// An application that currently has an audio stream, grouped by executable.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioApp {
    /// Stable identifier stored in records: executable name (or bundle id on macOS).
    pub key: String,
    /// Human friendly label (window title, application name, …).
    pub title: String,
    /// 0.0 ..= 1.0
    pub volume: f32,
    /// Has an open audio stream right now. Quiet apps (Linux) are listed so they can be
    /// picked, but their volume is only applied once they start playing.
    pub active: bool,
}

/// Operations every platform backend implements.
pub trait VolumeBackend {
    fn list(&mut self) -> Vec<AudioApp>;
    fn set_volume(&mut self, key: &str, volume: f32) -> bool;
}

enum Cmd {
    Refresh,
    SetVolume(String, f32),
    Toggle { key: String, min: f32, max: f32 },
    Shutdown,
}

#[derive(Clone)]
pub struct AudioHandle(Sender<Cmd>);

impl AudioHandle {
    pub fn refresh(&self) {
        let _ = self.0.send(Cmd::Refresh);
    }

    pub fn set_volume(&self, key: &str, volume: f32) {
        let _ = self.0.send(Cmd::SetVolume(key.to_string(), volume.clamp(0.0, 1.0)));
    }

    /// Switches `key` to whichever of `min`/`max` is farther from its current volume.
    pub fn toggle(&self, key: &str, min: f32, max: f32) {
        let _ = self.0.send(Cmd::Toggle { key: key.to_string(), min, max });
    }

    /// Asks the worker to drop the backend (restoring tapped apps on macOS) and exit.
    pub fn shutdown(&self) {
        let _ = self.0.send(Cmd::Shutdown);
    }
}

/// Volume a toggle switches to: whichever bound is farther from the current volume.
pub fn toggle_target(current: f32, min: f32, max: f32) -> f32 {
    if (current - min).abs() > (current - max).abs() {
        min
    } else {
        max
    }
}

pub enum Event {
    Apps(Vec<AudioApp>),
    Unavailable(String),
}

/// Starts the platform backend thread. `on_event` is called from that thread.
pub fn spawn(on_event: impl Fn(Event) + Send + 'static) -> (AudioHandle, JoinHandle<()>) {
    spawn_with(Backend::new, on_event)
}

pub fn spawn_with<B: VolumeBackend + 'static>(
    make: impl Fn() -> Result<B, String> + Send + 'static,
    on_event: impl Fn(Event) + Send + 'static,
) -> (AudioHandle, JoinHandle<()>) {
    let (tx, rx) = channel();
    let join = std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || worker(rx, make, on_event))
        .expect("failed to spawn audio thread");
    (AudioHandle(tx), join)
}

fn worker<B: VolumeBackend>(rx: Receiver<Cmd>, make: impl Fn() -> Result<B, String>, on_event: impl Fn(Event)) {
    let mut backend = loop {
        match make() {
            Ok(b) => break b,
            Err(err) => {
                on_event(Event::Unavailable(err));
                // Audio servers (PulseAudio/PipeWire, coreaudiod) may start after us.
                match rx.recv_timeout(Duration::from_secs(5)) {
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) | Ok(Cmd::Shutdown) => return,
                    _ => continue,
                }
            }
        }
    };

    let mut apps = backend.list();
    on_event(Event::Apps(apps.clone()));

    while let Ok(first) = rx.recv() {
        // Coalesce bursts (e.g. dragging a slider) so only the latest value per app is applied.
        let mut cmds = vec![first];
        cmds.extend(rx.try_iter());

        let mut needs_refresh = false;
        let mut pending_volumes: Vec<(String, f32)> = Vec::new();
        for cmd in cmds {
            match cmd {
                Cmd::Shutdown => return,
                Cmd::Refresh => needs_refresh = true,
                Cmd::SetVolume(key, vol) => {
                    pending_volumes.retain(|(k, _)| *k != key);
                    pending_volumes.push((key, vol));
                }
                Cmd::Toggle { key, min, max } => {
                    // Toggling must act on the real current volume, so flush first.
                    flush(&mut backend, &mut apps, &mut pending_volumes);
                    apps = backend.list();
                    if let Some(app) = apps.iter().find(|a| a.key == key) {
                        let target = toggle_target(app.volume, min, max);
                        pending_volumes.push((key, target));
                    }
                }
            }
        }
        flush(&mut backend, &mut apps, &mut pending_volumes);
        if needs_refresh {
            apps = backend.list();
        }
        on_event(Event::Apps(apps.clone()));
    }
}

fn flush<B: VolumeBackend>(backend: &mut B, apps: &mut [AudioApp], pending: &mut Vec<(String, f32)>) {
    for (key, vol) in pending.drain(..) {
        if backend.set_volume(&key, vol) {
            if let Some(app) = apps.iter_mut().find(|a| a.key == key) {
                app.volume = vol;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;
    use std::sync::{Arc, Mutex};

    #[test]
    fn toggle_goes_to_the_farther_bound() {
        assert_eq!(toggle_target(1.0, 0.2, 1.0), 0.2);
        assert_eq!(toggle_target(0.2, 0.2, 1.0), 1.0);
        assert_eq!(toggle_target(0.7, 0.2, 1.0), 0.2);
        assert_eq!(toggle_target(0.55, 0.2, 1.0), 1.0);
        assert_eq!(toggle_target(0.5, 0.2, 0.8), 0.8); // equal distance → max
        assert_eq!(toggle_target(0.0, 0.3, 0.3), 0.3);
    }

    /// In-memory mixer that records every volume change.
    #[derive(Clone, Default)]
    struct Mock {
        apps: Arc<Mutex<Vec<AudioApp>>>,
        sets: Arc<Mutex<Vec<(String, f32)>>>,
    }

    impl VolumeBackend for Mock {
        fn list(&mut self) -> Vec<AudioApp> {
            self.apps.lock().unwrap().clone()
        }

        fn set_volume(&mut self, key: &str, volume: f32) -> bool {
            self.sets.lock().unwrap().push((key.to_string(), volume));
            let mut apps = self.apps.lock().unwrap();
            match apps.iter_mut().find(|a| a.key == key) {
                Some(a) => {
                    a.volume = volume;
                    true
                }
                None => false,
            }
        }
    }

    fn app(key: &str, volume: f32) -> AudioApp {
        AudioApp { key: key.into(), title: key.to_uppercase(), volume, active: true }
    }

    fn start(mock: &Mock) -> (AudioHandle, JoinHandle<()>, Receiver<Vec<AudioApp>>) {
        let (tx, rx) = channel();
        let backend = mock.clone();
        let (handle, join) = spawn_with(
            move || Ok(backend.clone()),
            move |e| {
                if let Event::Apps(apps) = e {
                    let _ = tx.send(apps);
                }
            },
        );
        (handle, join, rx)
    }

    fn next(rx: &Receiver<Vec<AudioApp>>) -> Vec<AudioApp> {
        rx.recv_timeout(Duration::from_secs(5)).expect("no event from worker")
    }

    #[test]
    fn reports_apps_on_start_and_refresh() {
        let mock = Mock::default();
        mock.apps.lock().unwrap().push(app("game.exe", 1.0));
        let (handle, join, rx) = start(&mock);
        assert_eq!(next(&rx), vec![app("game.exe", 1.0)]);

        mock.apps.lock().unwrap().push(app("chat.exe", 0.5));
        handle.refresh();
        assert_eq!(next(&rx).len(), 2);

        handle.shutdown();
        join.join().unwrap();
    }

    #[test]
    fn toggle_switches_between_bounds() {
        let mock = Mock::default();
        mock.apps.lock().unwrap().push(app("game.exe", 1.0));
        let (handle, join, rx) = start(&mock);
        next(&rx);

        handle.toggle("game.exe", 0.25, 1.0);
        assert_eq!(next(&rx)[0].volume, 0.25);
        handle.toggle("game.exe", 0.25, 1.0);
        assert_eq!(next(&rx)[0].volume, 1.0);

        // Unknown apps are ignored.
        handle.toggle("missing.exe", 0.0, 1.0);
        next(&rx);
        assert!(mock.sets.lock().unwrap().iter().all(|(k, _)| k == "game.exe"));

        handle.shutdown();
        join.join().unwrap();
    }

    #[test]
    fn set_volume_is_clamped() {
        let mock = Mock::default();
        mock.apps.lock().unwrap().push(app("a", 0.5));
        let (handle, join, rx) = start(&mock);
        next(&rx);
        handle.set_volume("a", 1.7);
        assert_eq!(next(&rx)[0].volume, 1.0);
        handle.set_volume("a", -1.0);
        assert_eq!(next(&rx)[0].volume, 0.0);
        handle.shutdown();
        join.join().unwrap();
    }

    #[test]
    fn retries_until_backend_is_available() {
        let (tx, rx) = channel();
        let attempts = Arc::new(Mutex::new(0));
        let counter = attempts.clone();
        let (handle, join) = spawn_with(
            move || {
                let mut n = counter.lock().unwrap();
                *n += 1;
                if *n < 2 {
                    Err("not yet".to_string())
                } else {
                    Ok(Mock::default())
                }
            },
            move |e| {
                let _ = tx.send(matches!(e, Event::Unavailable(_)));
            },
        );
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "first event reports the error");
        handle.refresh(); // wakes the retry loop instead of waiting 5s
        assert!(!rx.recv_timeout(Duration::from_secs(5)).unwrap(), "then the app list arrives");
        handle.shutdown();
        join.join().unwrap();
    }
}
