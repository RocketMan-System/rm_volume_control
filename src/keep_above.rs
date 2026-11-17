//! "Always on top" under Wayland. The protocol lets no client raise itself above others, so
//! winit ignores the window level there. KWin can still do it for us: a small KWin script,
//! loaded over D-Bus, sets `keepAbove` on our windows (and on ones created later, as Slint
//! recreates the window after hiding it to the tray). Other compositors have no such API;
//! on X11 winit handles the flag itself.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use zbus::blocking::Connection;

const SCRIPT_NAME: &str = "apps-volume-control-keep-above";

static WANTED: AtomicBool = AtomicBool::new(false);
/// Serializes D-Bus round trips so quick toggles apply in order.
static BUSY: Mutex<()> = Mutex::new(());

fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

/// Applies the flag in a background thread: D-Bus must never stall the UI.
pub fn set(enabled: bool) {
    if !wayland() {
        return;
    }
    WANTED.store(enabled, Ordering::SeqCst);
    std::thread::spawn(|| {
        let _guard = BUSY.lock();
        if let Err(err) = load(WANTED.load(Ordering::SeqCst)) {
            eprintln!("Failed to set keep-above through KWin: {err}");
        }
    });
}

/// Unloads the script on exit (blocking).
pub fn clear() {
    if !wayland() {
        return;
    }
    let _guard = BUSY.lock();
    if let Ok(conn) = Connection::session() {
        let _ = unload(&conn);
    }
    let _ = std::fs::remove_file(script_path());
}

fn script_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("{SCRIPT_NAME}-{}.js", std::process::id()))
}

fn script(pid: u32, above: bool) -> String {
    // KWin 6 names them windowList/windowAdded, KWin 5 clientList/clientAdded.
    format!(
        "const pid = {pid}, above = {above};\n\
         function apply(w) {{ if (w.pid === pid) w.keepAbove = above; }}\n\
         (workspace.windowList ? workspace.windowList() : workspace.clientList()).forEach(apply);\n\
         if (above) (workspace.windowAdded || workspace.clientAdded).connect(apply);\n"
    )
}

fn unload(conn: &Connection) -> zbus::Result<()> {
    conn.call_method(Some("org.kde.KWin"), "/Scripting", Some("org.kde.kwin.Scripting"), "unloadScript", &SCRIPT_NAME)?;
    Ok(())
}

fn load(above: bool) -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::session()?;
    let path = script_path();
    std::fs::write(&path, script(std::process::id(), above))?;
    unload(&conn)?;
    let id: i32 = conn
        .call_method(
            Some("org.kde.KWin"),
            "/Scripting",
            Some("org.kde.kwin.Scripting"),
            "loadScript",
            &(path.to_string_lossy().as_ref(), SCRIPT_NAME),
        )?
        .body()
        .deserialize()?;
    if id < 0 {
        return Err("KWin refused the script".into());
    }
    // The script object path differs between KWin versions.
    let run =
        |path: String| conn.call_method(Some("org.kde.KWin"), path.as_str(), Some("org.kde.kwin.Script"), "run", &());
    run(format!("/Scripting/Script{id}")).or_else(|_| run(format!("/{id}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_targets_own_pid() {
        assert!(script(42, true).starts_with("const pid = 42, above = true;"));
        assert!(script(42, false).starts_with("const pid = 42, above = false;"));
    }
}
