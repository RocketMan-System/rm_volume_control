//! Linux desktop integration. Wayland shells take the window icon from the .desktop file whose
//! name matches the window's app_id, so an AppImage (whose .desktop stays inside the image)
//! registers its own entry and icon in ~/.local/share on start.

use std::path::PathBuf;

pub const APP_ID: &str = "apps-volume-control";

const DESKTOP_ENTRY: &str = include_str!("../packaging/linux/apps-volume-control.desktop");
const ICON: &[u8] = include_bytes!("../assets/icon256.png");

/// Path of the running AppImage file (not the temporary mount the binary runs from).
pub fn appimage() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_file())
}

/// Writes the .desktop entry and icon when running as an AppImage; files are rewritten only if changed.
pub fn integrate() {
    let (Some(image), Some(data)) = (appimage(), dirs::data_dir()) else { return };
    let icon = data.join("icons/hicolor/256x256/apps").join(format!("{APP_ID}.png"));
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
    write_if_changed(&data.join("applications").join(format!("{APP_ID}.desktop")), entry.as_bytes());
}

fn write_if_changed(path: &PathBuf, content: &[u8]) {
    if std::fs::read(path).is_ok_and(|old| old == content) {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, content);
}
