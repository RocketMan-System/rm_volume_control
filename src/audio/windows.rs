//! WASAPI backend: audio sessions of every active render device, grouped by executable name.

use super::{AudioApp, VolumeBackend};
use std::collections::HashMap;
use windows::core::{Interface, BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::Media::Audio::{
    eRender, AudioSessionStateExpired, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
    ISimpleAudioVolume, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
};

struct Session {
    key: String,
    pid: u32,
    volume: ISimpleAudioVolume,
}

pub struct Backend {
    enumerator: IMMDeviceEnumerator,
    sessions: Vec<Session>,
    own_pid: u32,
}

impl Backend {
    pub fn new() -> Result<Self, String> {
        unsafe {
            // The audio thread lives for the whole process, so COM is never uninitialized.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("WASAPI unavailable: {e}"))?;
            Ok(Self { enumerator, sessions: Vec::new(), own_pid: std::process::id() })
        }
    }

    fn load_sessions(&mut self) -> windows::core::Result<()> {
        let mut sessions = Vec::new();
        unsafe {
            let devices = self.enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            for i in 0..devices.GetCount()? {
                let Ok(device) = devices.Item(i) else { continue };
                let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                    continue;
                };
                let Ok(list) = manager.GetSessionEnumerator() else { continue };
                for j in 0..list.GetCount().unwrap_or(0) {
                    let Ok(control) = list.GetSession(j) else { continue };
                    if control.GetState().map(|s| s == AudioSessionStateExpired).unwrap_or(true) {
                        continue;
                    }
                    let Ok(control2) = control.cast::<IAudioSessionControl2>() else { continue };
                    // System sounds session has pid 0.
                    let pid = control2.GetProcessId().unwrap_or(0);
                    if pid == 0 || pid == self.own_pid {
                        continue;
                    }
                    let Some(key) = process_name(pid) else { continue };
                    let Ok(volume) = control.cast::<ISimpleAudioVolume>() else { continue };
                    sessions.push(Session { key, pid, volume });
                }
            }
        }
        self.sessions = sessions;
        Ok(())
    }
}

impl VolumeBackend for Backend {
    fn list(&mut self) -> Vec<AudioApp> {
        if let Err(err) = self.load_sessions() {
            eprintln!("Failed to enumerate audio sessions: {err}");
        }
        let titles = window_titles();
        let mut apps: Vec<AudioApp> = Vec::new();
        for s in &self.sessions {
            if let Some(app) = apps.iter_mut().find(|a| a.key == s.key) {
                if app.title == app.key {
                    if let Some(t) = titles.get(&s.pid) {
                        app.title = t.clone();
                    }
                }
                continue;
            }
            let volume = unsafe { s.volume.GetMasterVolume() }.unwrap_or(1.0);
            let title = titles.get(&s.pid).cloned().unwrap_or_else(|| s.key.clone());
            apps.push(AudioApp { key: s.key.clone(), title, volume, active: true });
        }
        apps.sort_by_key(|a| a.key.to_lowercase());
        apps
    }

    fn set_volume(&mut self, key: &str, volume: f32) -> bool {
        let mut ok = false;
        for s in self.sessions.iter().filter(|s| s.key.eq_ignore_ascii_case(key)) {
            ok |= unsafe { s.volume.SetMasterVolume(volume, std::ptr::null()) }.is_ok();
        }
        ok
    }
}

fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let res = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(handle);
        res.ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit(['\\', '/']).next().map(str::to_string)
    }
}

/// First visible, non-empty window title of every process.
fn window_titles() -> HashMap<u32, String> {
    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let map = &mut *(lparam.0 as *mut HashMap<u32, String>);
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return BOOL(1);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != 0 && !map.contains_key(&pid) {
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(hwnd, &mut buf);
            if n > 0 {
                map.insert(pid, String::from_utf16_lossy(&buf[..n as usize]));
            }
        }
        BOOL(1)
    }

    let mut map: HashMap<u32, String> = HashMap::new();
    unsafe {
        let _ = EnumWindows(Some(callback), LPARAM(&mut map as *mut _ as isize));
    }
    map
}
