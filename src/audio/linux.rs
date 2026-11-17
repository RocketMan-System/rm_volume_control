//! PulseAudio backend (also works with PipeWire through pipewire-pulse).
//! Every playback stream is a "sink input"; streams are grouped by process binary.
//!
//! Apps close their stream when they go quiet (browsers after a few seconds), so the
//! connected *clients* are listed too. A volume requested for a quiet app is remembered
//! and applied to its next stream.

use super::{AudioApp, VolumeBackend};
use crate::icons::{self, freedesktop};
use libpulse_binding as pulse;
use pulse::callbacks::ListResult;
use pulse::context::{Context, FlagSet, State};
use pulse::mainloop::standard::{IterateResult, Mainloop};
use pulse::operation::{Operation, State as OpState};
use pulse::proplist::{properties, Proplist};
use pulse::volume::{ChannelVolumes, Volume};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

struct SinkInput {
    index: u32,
    key: String,
    title: String,
    volume: ChannelVolumes,
    icon: AppIcon,
}

/// Icon hints an app registered with the sound server.
#[derive(Clone, Default)]
struct AppIcon {
    name: Option<String>,
    pid: Option<u32>,
}

impl AppIcon {
    fn from_proplist(props: &Proplist) -> Self {
        Self {
            name: props.get_str(properties::APPLICATION_ICON_NAME).filter(|n| !n.is_empty()),
            pid: props.get_str(properties::APPLICATION_PROCESS_ID).and_then(|p| p.parse().ok()),
        }
    }

    /// Keeps the first known value of every hint.
    fn merge(&mut self, other: &AppIcon) {
        if self.name.is_none() {
            self.name = other.name.clone();
        }
        if self.pid.is_none() {
            self.pid = other.pid;
        }
    }
}

/// An application connected to the sound server, possibly without a playing stream.
struct Client {
    key: String,
    title: String,
    icon: AppIcon,
}

/// Sound-server internals and desktop helpers that are never worth controlling.
pub fn is_system_client(binary: &str) -> bool {
    const SYSTEM: &[&str] = &[
        "pipewire",
        "pipewire-pulse",
        "wireplumber",
        "pulseaudio",
        "pactl",
        "pacmd",
        "pavucontrol",
        "pwvucontrol",
        "kwin_wayland",
        "kwin_x11",
        "gnome-shell",
        "kded5",
        "kded6",
        "plasmashell",
        "apps-volume-control",
    ];
    SYSTEM.contains(&binary) || binary.starts_with("xdg-desktop-portal") || binary.starts_with("pw-")
}

/// Picks a readable title among the names a binary registered with ("Google Chrome input",
/// "libcanberra", "WEBRTC VoiceEngine", …).
fn better_title(current: &str, candidate: &str, key: &str) -> bool {
    let generic = |t: &str| {
        t.is_empty() || t == key || t.contains("libcanberra") || t.contains("WEBRTC") || t.ends_with(" input")
    };
    generic(current) && !generic(candidate)
}

/// Active streams first (with their volume), then quiet clients.
fn merge(streams: &[SinkInput], clients: &[Client], pending: &HashMap<String, f32>) -> Vec<AudioApp> {
    let mut apps: Vec<AudioApp> = Vec::new();
    for s in streams {
        if apps.iter().any(|a| a.key == s.key) {
            continue;
        }
        let volume = (s.volume.avg().0 as f32 / Volume::NORMAL.0 as f32).min(1.0);
        apps.push(AudioApp { key: s.key.clone(), title: s.title.clone(), volume, active: true, icon: None });
    }
    for c in clients {
        if let Some(app) = apps.iter_mut().find(|a| a.key == c.key) {
            if better_title(&app.title, &c.title, &app.key) {
                app.title = c.title.clone();
            }
            continue;
        }
        let volume = pending.get(&c.key).copied().unwrap_or(1.0);
        apps.push(AudioApp { key: c.key.clone(), title: c.title.clone(), volume, active: false, icon: None });
    }
    apps.sort_by(|a, b| b.active.cmp(&a.active).then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase())));
    apps
}

// Field order matters: the context must be dropped before its main loop.
pub struct Backend {
    context: Context,
    mainloop: Mainloop,
    streams: Vec<SinkInput>,
    clients: Vec<Client>,
    /// Last volume requested per app, applied to streams the app opens later.
    pending: HashMap<String, f32>,
    seen_streams: HashSet<u32>,
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.context.disconnect();
    }
}

impl Backend {
    pub fn new() -> Result<Self, String> {
        let mut props = Proplist::new().ok_or("proplist")?;
        let _ = props.set_str(properties::APPLICATION_NAME, "Apps Volume Control");
        let _ = props.set_str(properties::APPLICATION_ID, "apps-volume-control");
        let mut mainloop = Mainloop::new().ok_or("Failed to create PulseAudio main loop")?;
        let mut context = Context::new_with_proplist(&mainloop, "Apps Volume Control", &props)
            .ok_or("Failed to create PulseAudio context")?;
        context.connect(None, FlagSet::NOFLAGS, None).map_err(|e| format!("PulseAudio connect failed: {e}"))?;
        loop {
            match mainloop.iterate(true) {
                IterateResult::Success(_) => {}
                IterateResult::Quit(_) | IterateResult::Err(_) => return Err("PulseAudio main loop failed".into()),
            }
            match context.get_state() {
                State::Ready => break,
                State::Failed | State::Terminated => return Err("Could not connect to PulseAudio/PipeWire".into()),
                _ => {}
            }
        }
        Ok(Self {
            context,
            mainloop,
            streams: Vec::new(),
            clients: Vec::new(),
            pending: HashMap::new(),
            seen_streams: HashSet::new(),
        })
    }

    fn wait<T: ?Sized>(&mut self, op: Operation<T>) -> bool {
        while op.get_state() == OpState::Running {
            if !matches!(self.mainloop.iterate(true), IterateResult::Success(_)) {
                return false;
            }
        }
        op.get_state() == OpState::Done
    }

    fn reconnect_if_needed(&mut self) {
        if self.context.get_state() != State::Ready {
            if let Ok(mut fresh) = Backend::new() {
                fresh.pending = std::mem::take(&mut self.pending);
                *self = fresh;
            }
        }
    }

    fn load_streams(&mut self) {
        let found: Rc<RefCell<Vec<SinkInput>>> = Rc::default();
        let sink = found.clone();
        let op = self.context.introspect().get_sink_input_info_list(move |res| {
            let ListResult::Item(info) = res else { return };
            if !info.has_volume || !info.volume_writable {
                return;
            }
            let binary = info.proplist.get_str(properties::APPLICATION_PROCESS_BINARY);
            let app_name = info.proplist.get_str(properties::APPLICATION_NAME);
            let Some(key) = binary.clone().or_else(|| app_name.clone()) else { return };
            let title =
                app_name.or_else(|| info.proplist.get_str(properties::MEDIA_NAME)).unwrap_or_else(|| key.clone());
            let icon = AppIcon::from_proplist(&info.proplist);
            sink.borrow_mut().push(SinkInput { index: info.index, key, title, volume: info.volume, icon });
        });
        if self.wait(op) {
            self.streams = found.take();
        }
    }

    fn load_clients(&mut self) {
        let found: Rc<RefCell<Vec<Client>>> = Rc::default();
        let sink = found.clone();
        let own_pid = std::process::id().to_string();
        let op = self.context.introspect().get_client_info_list(move |res| {
            let ListResult::Item(info) = res else { return };
            if info.proplist.get_str(properties::APPLICATION_PROCESS_ID).as_deref() == Some(own_pid.as_str()) {
                return;
            }
            let Some(key) = info.proplist.get_str(properties::APPLICATION_PROCESS_BINARY) else { return };
            if is_system_client(&key) {
                return;
            }
            let title = info.proplist.get_str(properties::APPLICATION_NAME).filter(|t| !t.is_empty());
            let title = title.unwrap_or_else(|| key.clone());
            let icon = AppIcon::from_proplist(&info.proplist);
            let mut list = sink.borrow_mut();
            match list.iter_mut().find(|c| c.key == key) {
                Some(c) => {
                    c.icon.merge(&icon);
                    if better_title(&c.title, &title, &key) {
                        c.title = title;
                    }
                }
                None => list.push(Client { key, title, icon }),
            }
        });
        if self.wait(op) {
            self.clients = found.take();
        }
    }

    /// Applies remembered volumes to streams that appeared since the last refresh.
    fn apply_pending(&mut self) {
        let fresh: Vec<(String, f32)> = self
            .streams
            .iter()
            .filter(|s| !self.seen_streams.contains(&s.index))
            .filter_map(|s| self.pending.get(&s.key).map(|v| (s.key.clone(), *v)))
            .collect();
        self.seen_streams = self.streams.iter().map(|s| s.index).collect();
        for (key, volume) in fresh {
            self.apply_volume(&key, volume);
        }
    }

    fn apply_volume(&mut self, key: &str, volume: f32) -> bool {
        let raw = Volume((volume.clamp(0.0, 1.0) * Volume::NORMAL.0 as f32).round() as u32);
        let targets: Vec<(u32, ChannelVolumes)> = self
            .streams
            .iter()
            .filter(|s| s.key == key)
            .map(|s| {
                let mut cv = s.volume;
                let channels = cv.len();
                cv.set(channels, raw);
                (s.index, cv)
            })
            .collect();
        if targets.is_empty() {
            return false;
        }
        let mut ok = true;
        for (index, cv) in &targets {
            let op = self.context.introspect().set_sink_input_volume(*index, cv, None);
            ok &= self.wait(op);
        }
        for s in self.streams.iter_mut().filter(|s| s.key == key) {
            let channels = s.volume.len();
            s.volume.set(channels, raw);
        }
        ok
    }
}

impl VolumeBackend for Backend {
    fn list(&mut self) -> Vec<AudioApp> {
        self.reconnect_if_needed();
        self.load_streams();
        self.load_clients();
        self.apply_pending();
        merge(&self.streams, &self.clients, &self.pending)
    }

    fn set_volume(&mut self, key: &str, volume: f32) -> bool {
        self.reconnect_if_needed();
        self.pending.insert(key.to_string(), volume);
        // A quiet app has no stream yet: the volume is applied when it starts playing.
        self.apply_volume(key, volume) || self.clients.iter().any(|c| c.key == key)
    }

    fn icon(&mut self, key: &str) -> Option<icons::Source> {
        let mut hints = AppIcon::default();
        let mut title = key.to_string();
        for s in self.streams.iter().filter(|s| s.key == key) {
            hints.merge(&s.icon);
            title = s.title.clone();
        }
        for c in self.clients.iter().filter(|c| c.key == key) {
            hints.merge(&c.icon);
            if better_title(&title, &c.title, key) {
                title = c.title.clone();
            }
        }
        let hints = freedesktop::Hints { binary: key, title: &title, icon_name: hints.name.as_deref(), pid: hints.pid };
        freedesktop::find(&hints).map(icons::Source::File)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a running PulseAudio/PipeWire server: `cargo test -- --ignored`.
    /// Re-applies each app's current volume, so it is safe to run on a desktop.
    #[test]
    #[ignore]
    fn lists_and_sets_real_streams() {
        let mut backend = Backend::new().expect("audio server");
        let apps = backend.list();
        eprintln!("{apps:#?}");
        for app in &apps {
            let icon = backend.icon(&app.key).map(|s| match s {
                icons::Source::File(p) => p,
                _ => unreachable!(),
            });
            eprintln!("{} → {icon:?}", app.key);
        }
        for app in &apps {
            assert!((0.0..=1.0).contains(&app.volume));
            assert!(backend.set_volume(&app.key, app.volume));
        }
        assert!(!backend.set_volume("definitely-not-running", 0.5));
    }

    fn stream(index: u32, key: &str, title: &str, volume: f32) -> SinkInput {
        let mut cv = ChannelVolumes::default();
        cv.set(2, Volume((volume * Volume::NORMAL.0 as f32) as u32));
        SinkInput { index, key: key.into(), title: title.into(), volume: cv, icon: AppIcon::default() }
    }

    fn client(key: &str, title: &str) -> Client {
        Client { key: key.into(), title: title.into(), icon: AppIcon::default() }
    }

    #[test]
    fn quiet_clients_are_listed_after_active_streams() {
        let streams = vec![stream(1, "chrome", "Google Chrome", 0.5)];
        let clients = vec![client("Telegram", "Telegram Desktop"), client("chrome", "Google Chrome input")];
        let pending = HashMap::from([("Telegram".to_string(), 0.3)]);
        let apps = merge(&streams, &clients, &pending);
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].key, "chrome");
        assert!(apps[0].active);
        assert!((apps[0].volume - 0.5).abs() < 0.01);
        assert_eq!(apps[0].title, "Google Chrome");
        assert_eq!(apps[1].key, "Telegram");
        assert!(!apps[1].active);
        assert_eq!(apps[1].volume, 0.3, "remembered volume of a quiet app");
    }

    #[test]
    fn picks_readable_titles() {
        let clients = vec![client("Discord", "WEBRTC VoiceEngine")];
        let streams = vec![stream(1, "Discord", "WEBRTC VoiceEngine", 1.0)];
        let mut apps = merge(&streams, &clients, &HashMap::new());
        assert_eq!(apps[0].title, "WEBRTC VoiceEngine");
        apps =
            merge(&streams, &[client("Discord", "WEBRTC VoiceEngine"), client("Discord", "discord")], &HashMap::new());
        assert_eq!(apps[0].title, "discord");
        assert!(!better_title("Steam", "Steam Voice Settings", "steam"));
    }

    #[test]
    fn hides_sound_server_internals() {
        for b in ["pipewire", "wireplumber", "xdg-desktop-portal-kde", "pw-cat", "kwin_wayland", "pactl"] {
            assert!(is_system_client(b), "{b}");
        }
        for b in ["chrome", "Discord", "Telegram", "steam", "firefox"] {
            assert!(!is_system_client(b), "{b}");
        }
    }
}
