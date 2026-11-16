//! "Start with system": registry Run key on Windows, XDG autostart entry on Linux,
//! LaunchAgent on macOS. The app is always started with `--minimized` from there.

use std::path::PathBuf;

pub const MINIMIZED_ARG: &str = "--minimized";

fn exe() -> Option<PathBuf> {
    // Inside an AppImage current_exe() is the temporary mount, gone after exit.
    #[cfg(target_os = "linux")]
    if let Some(image) = crate::desktop::appimage() {
        return Some(image);
    }
    std::env::current_exe().ok()
}

#[cfg(windows)]
pub fn set(enabled: bool) -> Result<(), String> {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    let key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    let name = w!("AppsVolumeControl");
    unsafe {
        if enabled {
            let exe = exe().ok_or("unknown executable path")?;
            let value = format!("\"{}\" {}", exe.display(), MINIMIZED_ARG);
            let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
            RegSetKeyValueW(HKEY_CURRENT_USER, key, name, REG_SZ.0, Some(wide.as_ptr().cast()), (wide.len() * 2) as u32)
                .ok()
                .map_err(|e| e.to_string())
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, key, name);
            Ok(())
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn set(enabled: bool) -> Result<(), String> {
    let dir = dirs::config_dir().ok_or("no config dir")?.join("autostart");
    let file = dir.join("apps-volume-control.desktop");
    if !enabled {
        let _ = std::fs::remove_file(file);
        return Ok(());
    }
    let exe = exe().ok_or("unknown executable path")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Apps Volume Control\nExec=\"{}\" {}\nX-GNOME-Autostart-enabled=true\nTerminal=false\n",
        exe.display(),
        MINIMIZED_ARG
    );
    std::fs::write(file, entry).map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
pub fn set(enabled: bool) -> Result<(), String> {
    let dir = dirs::home_dir().ok_or("no home dir")?.join("Library/LaunchAgents");
    let file = dir.join("com.xanderwp.appsvolumecontrol.plist");
    if !enabled {
        let _ = std::fs::remove_file(file);
        return Ok(());
    }
    let exe = exe().ok_or("unknown executable path")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>com.xanderwp.appsvolumecontrol</string>
    <key>ProgramArguments</key>
    <array><string>{}</string><string>{}</string></array>
    <key>RunAtLoad</key><true/>
</dict>
</plist>
"#,
        exe.display(),
        MINIMIZED_ARG
    );
    std::fs::write(file, plist).map_err(|e| e.to_string())
}
