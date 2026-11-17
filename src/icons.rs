//! Application icons. Backends resolve an icon the first time they see an app in a
//! session; it is copied into `<cache_dir>/apps-volume-control/icons` so records of apps
//! that are not running keep their icon.

use std::path::{Path, PathBuf};

/// An icon as a backend found it.
pub enum Source {
    /// A PNG or SVG file on disk (Linux icon themes).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    File(PathBuf),
    /// Encoded PNG (macOS).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Png(Vec<u8>),
    /// Straight (non-premultiplied) RGBA pixels (Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    Rgba { width: u32, height: u32, pixels: Vec<u8> },
}

const EXTENSIONS: [&str; 2] = ["png", "svg"];

fn cache_dir() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("apps-volume-control").join("icons"))
}

/// File name (without extension) of `key`'s icon; keys are executable names or bundle ids.
fn file_stem(key: &str) -> String {
    let stem: String = key
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    // "." and ".." must never become path components.
    format!("app-{stem}")
}

/// The cached icon of `key`, if one was ever stored.
pub fn cached(key: &str) -> Option<PathBuf> {
    cached_in(&cache_dir()?, key)
}

fn cached_in(dir: &Path, key: &str) -> Option<PathBuf> {
    let stem = file_stem(key);
    EXTENSIONS.iter().map(|ext| dir.join(format!("{stem}.{ext}"))).find(|p| p.is_file())
}

/// Stores `source` as the icon of `key` and returns the cached file.
pub fn store(key: &str, source: Source) -> Option<PathBuf> {
    store_in(&cache_dir()?, key, source)
}

fn store_in(dir: &Path, key: &str, source: Source) -> Option<PathBuf> {
    let (ext, bytes) = match source {
        Source::File(path) => {
            let ext = path.extension()?.to_str()?.to_lowercase();
            if !EXTENSIONS.contains(&ext.as_str()) {
                return None;
            }
            (ext, std::fs::read(&path).ok()?)
        }
        Source::Png(bytes) => ("png".to_string(), bytes),
        Source::Rgba { width, height, pixels } => ("png".to_string(), encode_png(width, height, &pixels)?),
    };
    std::fs::create_dir_all(dir).ok()?;
    let stem = file_stem(key);
    let path = dir.join(format!("{stem}.{ext}"));
    // Drop an older icon of the other format so `cached` finds the new one.
    for other in EXTENSIONS.iter().filter(|e| **e != ext) {
        let _ = std::fs::remove_file(dir.join(format!("{stem}.{other}")));
    }
    if std::fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) {
        std::fs::write(&path, &bytes).ok()?;
    }
    Some(path)
}

fn encode_png(width: u32, height: u32, pixels: &[u8]) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || pixels.len() != (width * height * 4) as usize {
        return None;
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(pixels).ok()?;
    writer.finish().ok()?;
    Some(out)
}

/// Freedesktop icon lookup: `.desktop` entries and icon themes.
#[cfg(target_os = "linux")]
pub mod freedesktop {
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    /// What the sound server tells about an app.
    pub struct Hints<'a> {
        pub binary: &'a str,
        pub title: &'a str,
        pub icon_name: Option<&'a str>,
        pub pid: Option<u32>,
    }

    #[derive(Debug, Default, PartialEq)]
    pub struct DesktopEntry {
        /// File name without `.desktop`.
        pub id: String,
        pub name: String,
        pub icon: String,
        pub exec: String,
        pub wm_class: String,
    }

    /// `$XDG_DATA_HOME` and `$XDG_DATA_DIRS`, most important first.
    fn data_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(home) = dirs::data_dir() {
            dirs.push(home);
        }
        let system = std::env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty());
        let system = system.as_deref().unwrap_or("/usr/local/share:/usr/share");
        dirs.extend(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
        for extra in ["/var/lib/flatpak/exports/share", "/usr/share"] {
            let extra = PathBuf::from(extra);
            if !dirs.contains(&extra) {
                dirs.push(extra);
            }
        }
        dirs
    }

    pub fn parse_desktop(id: &str, text: &str) -> Option<DesktopEntry> {
        let mut entry = DesktopEntry { id: id.to_string(), ..Default::default() };
        let mut in_main = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_main = line == "[Desktop Entry]";
                continue;
            }
            if !in_main {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim().to_string();
            match k.trim() {
                "Name" => entry.name = v,
                "Icon" => entry.icon = v,
                "Exec" => entry.exec = v,
                "StartupWMClass" => entry.wm_class = v,
                _ => {}
            }
        }
        (!entry.icon.is_empty()).then_some(entry)
    }

    /// Every application entry, parsed once per run.
    fn entries() -> &'static [DesktopEntry] {
        static ENTRIES: OnceLock<Vec<DesktopEntry>> = OnceLock::new();
        ENTRIES.get_or_init(|| {
            let mut out: Vec<DesktopEntry> = Vec::new();
            for dir in data_dirs() {
                let Ok(files) = std::fs::read_dir(dir.join("applications")) else { continue };
                for file in files.flatten() {
                    let path = file.path();
                    let Some(id) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".desktop"))
                    else {
                        continue;
                    };
                    // Earlier data dirs override later ones.
                    if out.iter().any(|e| e.id == id) {
                        continue;
                    }
                    if let Some(entry) = std::fs::read_to_string(&path).ok().and_then(|t| parse_desktop(id, &t)) {
                        out.push(entry);
                    }
                }
            }
            out
        })
    }

    /// Basename of the program an `Exec=` line starts (skipping `env VAR=…`).
    pub fn exec_binary(exec: &str) -> Option<&str> {
        let mut words = exec.split_whitespace();
        let mut first = words.next()?;
        if first == "env" || first.ends_with("/env") {
            first = words.find(|w| !w.contains('='))?;
        }
        Some(first.trim_matches('"').rsplit('/').next().unwrap_or(first))
    }

    /// Desktop ids from the systemd scope the launcher put the process in, e.g.
    /// `app-flatpak-org.telegram.desktop-1326087048.scope` → `org.telegram.desktop`.
    pub fn cgroup_ids(cgroup: &str) -> Vec<String> {
        let mut ids = Vec::new();
        for line in cgroup.lines() {
            let unit = line.rsplit('/').next().unwrap_or("");
            let Some(rest) = unit.strip_prefix("app-") else { continue };
            let id = if let Some(r) = rest.strip_suffix(".scope") {
                // app-[<launcher>-]<id>-<random>.scope
                r.rsplit_once('-').map(|(id, _)| id).unwrap_or(r)
            } else if let Some(r) = rest.strip_suffix(".service") {
                // app-[<launcher>-]<id>[@<random>].service
                r.split('@').next().unwrap_or(r)
            } else {
                continue;
            };
            let id = id.replace("\\x2d", "-");
            if let Some((_, without_launcher)) = id.split_once('-') {
                ids.push(without_launcher.to_string());
            }
            ids.push(id);
        }
        ids
    }

    fn find_entry<'a>(entries: &'a [DesktopEntry], hints: &Hints) -> Option<&'a DesktopEntry> {
        let binary = hints.binary;
        if let Some(pid) = hints.pid {
            if let Ok(cgroup) = std::fs::read_to_string(format!("/proc/{pid}/cgroup")) {
                for id in cgroup_ids(&cgroup) {
                    if let Some(e) = entries.iter().find(|e| e.id == id) {
                        return Some(e);
                    }
                }
            }
        }
        let same = |a: &str, b: &str| !a.is_empty() && a.eq_ignore_ascii_case(b);
        entries
            .iter()
            .find(|e| exec_binary(&e.exec).is_some_and(|b| same(b, binary)))
            .or_else(|| entries.iter().find(|e| same(&e.wm_class, binary) || same(&e.id, binary)))
            .or_else(|| entries.iter().find(|e| e.id.rsplit('.').next().is_some_and(|last| same(last, binary))))
            .or_else(|| entries.iter().find(|e| same(&e.name, hints.title)))
    }

    /// The icon theme the desktop is configured with (KDE, then GTK).
    fn user_theme() -> Option<String> {
        let config = dirs::config_dir()?;
        let read_key = |file: &str, section: &str, key: &str| -> Option<String> {
            let text = std::fs::read_to_string(config.join(file)).ok()?;
            let mut in_section = false;
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    in_section = line == section;
                } else if in_section {
                    if let Some((k, v)) = line.split_once('=') {
                        if k.trim() == key && !v.trim().is_empty() {
                            return Some(v.trim().to_string());
                        }
                    }
                }
            }
            None
        };
        read_key("kdeglobals", "[Icons]", "Theme")
            .or_else(|| read_key("gtk-3.0/settings.ini", "[Settings]", "gtk-icon-theme-name"))
    }

    /// Looks `name` up in the user's theme, then hicolor and common fallbacks, then pixmaps.
    pub fn lookup(name: &str) -> Option<PathBuf> {
        if name.is_empty() {
            return None;
        }
        let as_path = Path::new(name);
        if as_path.is_absolute() {
            return as_path.is_file().then(|| as_path.to_path_buf());
        }
        const SIZES: [&str; 9] =
            ["scalable", "64x64", "48x48", "128x128", "96x96", "256x256", "32x32", "512x512", "24x24"];
        // Breeze/Papirus lay out "apps/<size>/" instead of "<size>/apps/".
        const BREEZE_SIZES: [&str; 5] = ["scalable", "64", "48", "32", "128"];
        let mut themes: Vec<String> = user_theme().into_iter().collect();
        for t in ["hicolor", "breeze", "Adwaita"] {
            if !themes.iter().any(|x| x == t) {
                themes.push(t.to_string());
            }
        }
        let mut bases: Vec<PathBuf> = Vec::new();
        if let Some(home) = dirs::home_dir() {
            bases.push(home.join(".icons"));
        }
        bases.extend(data_dirs().into_iter().map(|d| d.join("icons")));

        let file =
            |dir: &Path| ["svg", "png"].iter().map(|ext| dir.join(format!("{name}.{ext}"))).find(|p| p.is_file());
        for theme in &themes {
            for base in &bases {
                let root = base.join(theme);
                if !root.is_dir() {
                    continue;
                }
                let found = SIZES
                    .iter()
                    .find_map(|s| file(&root.join(s).join("apps")))
                    .or_else(|| BREEZE_SIZES.iter().find_map(|s| file(&root.join("apps").join(s))));
                if found.is_some() {
                    return found;
                }
            }
        }
        data_dirs().iter().find_map(|d| file(&d.join("pixmaps")))
    }

    /// Best icon file for an app, or `None` when nothing matches.
    pub fn find(hints: &Hints) -> Option<PathBuf> {
        if let Some(found) = hints.icon_name.and_then(lookup) {
            return Some(found);
        }
        if let Some(found) = find_entry(entries(), hints).and_then(|e| lookup(&e.icon)) {
            return Some(found);
        }
        lookup(hints.binary).or_else(|| lookup(&hints.binary.to_lowercase()))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_desktop_entries() {
            let text = "[Desktop Entry]\nName=Google Chrome\nExec=/usr/bin/google-chrome-stable %U\n\
                        Icon=google-chrome\nStartupWMClass=Google-chrome\n\n\
                        [Desktop Action new-window]\nName=New Window\nExec=/usr/bin/other\n";
            let e = parse_desktop("google-chrome", text).unwrap();
            assert_eq!(e.name, "Google Chrome");
            assert_eq!(e.icon, "google-chrome");
            assert_eq!(e.exec, "/usr/bin/google-chrome-stable %U");
            assert_eq!(e.wm_class, "Google-chrome");
            assert!(parse_desktop("x", "[Desktop Entry]\nName=No icon\n").is_none());
        }

        #[test]
        fn exec_binary_names() {
            assert_eq!(exec_binary("/usr/bin/google-chrome-stable %U"), Some("google-chrome-stable"));
            assert_eq!(exec_binary("env GDK_BACKEND=x11 /opt/app/Discord --flag"), Some("Discord"));
            assert_eq!(exec_binary(""), None);
        }

        #[test]
        fn desktop_ids_from_cgroups() {
            let cg = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-flatpak-org.telegram.desktop-1326087048.scope";
            assert_eq!(cgroup_ids(cg), vec!["org.telegram.desktop", "flatpak-org.telegram.desktop"]);
            let cg = "0::/user.slice/user@1000.service/app.slice/app-com.google.Chrome-6306.scope";
            assert!(cgroup_ids(cg).contains(&"com.google.Chrome".to_string()));
            let cg = "0::/user.slice/user@1000.service/app.slice/app-gnome-org.gnome.Rhythmbox3@ab12.service";
            assert!(cgroup_ids(cg).contains(&"org.gnome.Rhythmbox3".to_string()));
            let cg = "0::/user.slice/user@1000.service/app.slice/app-google\\x2dchrome-77.scope";
            assert!(cgroup_ids(cg).contains(&"google-chrome".to_string()));
            assert!(cgroup_ids("0::/user.slice/session-2.scope").is_empty());
        }

        #[test]
        fn matches_entries_by_exec_class_id_and_name() {
            let entry = |id: &str, name: &str, exec: &str, class: &str| DesktopEntry {
                id: id.into(),
                name: name.into(),
                icon: format!("{id}-icon"),
                exec: exec.into(),
                wm_class: class.into(),
            };
            let entries = vec![
                entry("google-chrome", "Google Chrome", "/usr/bin/google-chrome-stable %U", "Google-chrome"),
                entry("discord", "Discord", "/usr/bin/discord", ""),
                entry("com.valvesoftware.Steam", "Steam", "flatpak run com.valvesoftware.Steam", ""),
            ];
            let hints = |binary: &'static str, title: &'static str| Hints { binary, title, icon_name: None, pid: None };
            let id = |h: Hints| find_entry(&entries, &h).map(|e| e.id.clone());
            assert_eq!(id(hints("Discord", "WEBRTC VoiceEngine")).as_deref(), Some("discord"));
            assert_eq!(id(hints("chrome", "Google Chrome")).as_deref(), Some("google-chrome"));
            assert_eq!(id(hints("steam", "x")).as_deref(), Some("com.valvesoftware.Steam"));
            assert_eq!(id(hints("unknown", "Unknown")), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("avc-icons-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn file_stems_are_safe() {
        assert_eq!(file_stem("Discord"), "app-discord");
        assert_eq!(file_stem("com.apple.Music"), "app-com.apple.music");
        assert_eq!(file_stem("../x y/z"), "app-.._x_y_z");
        assert_eq!(file_stem(".."), "app-..");
    }

    #[test]
    fn stores_and_finds_icons() {
        let dir = temp_dir("store");
        assert_eq!(cached_in(&dir, "game.exe"), None);

        let rgba = Source::Rgba { width: 2, height: 1, pixels: vec![255, 0, 0, 255, 0, 0, 255, 128] };
        let png = store_in(&dir, "Game.exe", rgba).unwrap();
        assert_eq!(png.extension().unwrap(), "png");
        assert_eq!(cached_in(&dir, "game.exe"), Some(png.clone()));
        assert!(std::fs::read(&png).unwrap().starts_with(b"\x89PNG"));

        // A newer SVG replaces the PNG.
        let svg_src = dir.join("source.svg");
        std::fs::write(&svg_src, "<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap();
        let svg = store_in(&dir, "game.exe", Source::File(svg_src)).unwrap();
        assert!(!png.exists());
        assert_eq!(cached_in(&dir, "game.exe"), Some(svg));

        let xpm = dir.join("icon.xpm");
        std::fs::write(&xpm, "x").unwrap();
        assert!(store_in(&dir, "other", Source::File(xpm)).is_none(), "unsupported format");
        assert!(store_in(&dir, "bad", Source::Rgba { width: 2, height: 2, pixels: vec![0; 4] }).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
