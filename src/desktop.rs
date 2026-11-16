//! Linux desktop integration. Wayland shells take the window icon from the .desktop file that
//! matches the window's app_id (file name or StartupWMClass). An AppImage carries its .desktop
//! inside the image, so it can add (and remove) its own entry and icon in ~/.local/share.
//! Entries made by other tools (AppImageLauncher, Gear Lever) are respected to avoid duplicates.

use std::path::{Path, PathBuf};

pub const APP_ID: &str = "apps-volume-control";

const DESKTOP_ENTRY: &str = include_str!("../packaging/linux/apps-volume-control.desktop");
const ICON: &[u8] = include_bytes!("../assets/icon256.png");

// Values of the `desktop-state` property in ui/app.slint.
/// Not an AppImage: installed by the script or a package, nothing to manage.
pub const UNAVAILABLE: i32 = 0;
pub const MISSING: i32 = 1;
/// Our own entry points to this AppImage.
pub const OURS: i32 = 2;
/// Another tool has registered this AppImage.
pub const OTHER: i32 = 3;

/// Path of the running AppImage file (not the temporary mount the binary runs from).
pub fn appimage() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_file())
}

/// AppImageLauncher asks about integration itself when an AppImage starts.
pub fn has_appimagelauncher() -> bool {
    let Some(path) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path).any(|dir| dir.join("AppImageLauncher").is_file() || dir.join("ail-cli").is_file())
}

fn data_home() -> Option<PathBuf> {
    dirs::data_dir()
}

fn entry_path(data: &Path) -> PathBuf {
    data.join("applications").join(format!("{APP_ID}.desktop"))
}

fn icon_path(data: &Path) -> PathBuf {
    data.join("icons/hicolor/256x256/apps").join(format!("{APP_ID}.png"))
}

/// All `applications` folders the desktop reads entries from.
fn application_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = data_home().into_iter().collect();
    let system = std::env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty());
    for dir in system.as_deref().unwrap_or("/usr/local/share:/usr/share").split(':') {
        dirs.push(PathBuf::from(dir));
    }
    dirs.into_iter().map(|d| d.join("applications")).collect()
}

/// The program of the `Exec=` line of a .desktop file, without arguments and quotes.
fn exec_program(entry: &str) -> Option<String> {
    let exec = entry.lines().find_map(|l| l.trim().strip_prefix("Exec="))?.trim();
    let program = match exec.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => exec.split_whitespace().next()?,
    };
    Some(program.to_string())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Finds entries launching `image`: whether our own one is among them, and another tool's one.
fn find_entries(dirs: &[PathBuf], own_entry: &Path, image: &Path) -> (bool, Option<PathBuf>) {
    let (mut ours, mut other) = (false, None);
    for dir in dirs {
        let Ok(files) = std::fs::read_dir(dir) else { continue };
        for path in files.flatten().map(|f| f.path()) {
            if path.extension().is_none_or(|e| e != "desktop") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            if !exec_program(&text).is_some_and(|p| same_file(Path::new(&p), image)) {
                continue;
            }
            if same_file(&path, own_entry) {
                ours = true;
            } else if other.is_none() {
                other = Some(path);
            }
        }
    }
    (ours, other)
}

/// Current integration state. Also refreshes our entry (the icon may change with an update)
/// and drops it when another tool has registered the AppImage too.
pub fn state() -> i32 {
    let (Some(image), Some(data)) = (appimage(), data_home()) else { return UNAVAILABLE };
    match find_entries(&application_dirs(), &entry_path(&data), &image) {
        (ours, Some(_)) => {
            if ours {
                remove_own(&data);
            }
            OTHER
        }
        (true, None) => {
            write_entry(&image, &data);
            OURS
        }
        (false, None) => MISSING,
    }
}

pub fn register() -> i32 {
    if let (Some(image), Some(data)) = (appimage(), data_home()) {
        write_entry(&image, &data);
    }
    state()
}

pub fn unregister() -> i32 {
    // Only our own entry for this AppImage; another tool's entry is that tool's business.
    if let (OURS, Some(data)) = (state(), data_home()) {
        remove_own(&data);
    }
    state()
}

fn write_entry(image: &Path, data: &Path) {
    let icon = icon_path(data);
    let entry = DESKTOP_ENTRY
        .lines()
        .map(|line| match line.split_once('=') {
            Some(("Exec", _)) => format!("Exec=\"{}\"", image.display()),
            Some(("Icon", _)) => format!("Icon={}", icon.display()),
            _ => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    write_if_changed(&icon, ICON);
    write_if_changed(&entry_path(data), entry.as_bytes());
}

fn remove_own(data: &Path) {
    let _ = std::fs::remove_file(entry_path(data));
    let _ = std::fs::remove_file(icon_path(data));
}

fn write_if_changed(path: &Path, content: &[u8]) {
    if std::fs::read(path).is_ok_and(|old| old == content) {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Rename into place: the desktop watches these folders and must not see a half-written file.
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, content).is_err() || std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exec_program() {
        assert_eq!(
            exec_program("[Desktop Entry]\nExec=\"/a b/App.AppImage\" %U\n").as_deref(),
            Some("/a b/App.AppImage")
        );
        assert_eq!(exec_program("Exec=/opt/App.AppImage --flag\nTryExec=x").as_deref(), Some("/opt/App.AppImage"));
        assert_eq!(exec_program("Name=x"), None);
    }

    #[test]
    fn finds_own_and_foreign_entries() {
        let root = std::env::temp_dir().join(format!("avc-desktop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let apps = root.join("applications");
        std::fs::create_dir_all(&apps).unwrap();
        let image = root.join("App.AppImage");
        std::fs::write(&image, "").unwrap();
        let own = apps.join(format!("{APP_ID}.desktop"));
        let dirs = [apps.clone(), root.join("missing")];

        assert_eq!(find_entries(&dirs, &own, &image), (false, None));

        std::fs::write(&own, format!("Exec=\"{}\"\n", image.display())).unwrap();
        std::fs::write(apps.join("unrelated.desktop"), "Exec=/usr/bin/other\n").unwrap();
        assert_eq!(find_entries(&dirs, &own, &image), (true, None));

        let foreign = apps.join("appimagekit_0123-Apps_Volume_Control.desktop");
        std::fs::write(&foreign, format!("Exec={} %U\n", image.display())).unwrap();
        assert_eq!(find_entries(&dirs, &own, &image), (true, Some(foreign)));

        // Our file launching something else (e.g. the script-installed binary) is not this AppImage's entry.
        std::fs::write(&own, "Exec=/home/u/.local/bin/apps-volume-control\n").unwrap();
        assert!(!find_entries(&dirs, &own, &image).0);
        let _ = std::fs::remove_dir_all(&root);
    }
}
