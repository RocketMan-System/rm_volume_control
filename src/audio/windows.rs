//! WASAPI backend: audio sessions of every active render device, grouped by executable name.

use super::{AudioApp, VolumeBackend};
use crate::icons;
use std::collections::HashMap;
use windows::core::{Interface, BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::Media::Audio::{
    eRender, AudioSessionStateExpired, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
    ISimpleAudioVolume, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, EnumWindows, GetIconInfo, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, PrivateExtractIconsW, HICON, ICONINFO,
};

/// Size of the icons extracted from executables.
const ICON_SIZE: i32 = 32;

struct Session {
    key: String,
    path: String,
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
                    let Some(path) = process_path(pid) else { continue };
                    let Some(key) = path.rsplit(['\\', '/']).next().map(str::to_string) else { continue };
                    let Ok(volume) = control.cast::<ISimpleAudioVolume>() else { continue };
                    sessions.push(Session { key, path, pid, volume });
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
            apps.push(AudioApp { key: s.key.clone(), title, volume, active: true, icon: None });
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

    fn icon(&mut self, key: &str) -> Option<icons::Source> {
        let path = &self.sessions.iter().find(|s| s.key == key)?.path;
        exe_icon(path)
    }
}

fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let res = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(handle);
        res.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// The main icon of an executable as RGBA pixels; `None` when it has none.
fn exe_icon(path: &str) -> Option<icons::Source> {
    let mut name = [0u16; 260];
    let wide: Vec<u16> = path.encode_utf16().collect();
    if wide.len() >= name.len() {
        return None;
    }
    name[..wide.len()].copy_from_slice(&wide);
    let mut handles = [HICON::default()];
    let count = unsafe { PrivateExtractIconsW(&name, 0, ICON_SIZE, ICON_SIZE, Some(&mut handles), None, 0) };
    if count == 0 || count == u32::MAX || handles[0].is_invalid() {
        return None;
    }
    let pixels = unsafe {
        let pixels = icon_pixels(handles[0]);
        let _ = DestroyIcon(handles[0]);
        pixels
    };
    Some(icons::Source::Rgba { width: ICON_SIZE as u32, height: ICON_SIZE as u32, pixels: pixels? })
}

/// Reads a bitmap as top-down 32-bit BGRA.
unsafe fn bitmap_bits(bitmap: HBITMAP) -> Option<Vec<u8>> {
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: ICON_SIZE,
            biHeight: -ICON_SIZE,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = vec![0u8; (ICON_SIZE * ICON_SIZE * 4) as usize];
    let dc = GetDC(None);
    let lines = GetDIBits(dc, bitmap, 0, ICON_SIZE as u32, Some(bits.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
    ReleaseDC(None, dc);
    (lines == ICON_SIZE).then_some(bits)
}

unsafe fn icon_pixels(icon: HICON) -> Option<Vec<u8>> {
    let mut info = ICONINFO::default();
    GetIconInfo(icon, &mut info).ok()?;
    let color = if info.hbmColor.is_invalid() { None } else { bitmap_bits(info.hbmColor) };
    let mask = if info.hbmMask.is_invalid() { None } else { bitmap_bits(info.hbmMask) };
    let _ = DeleteObject(info.hbmColor.into());
    let _ = DeleteObject(info.hbmMask.into());
    let mut pixels = color?;
    // Old icons have no alpha channel: transparency comes from the AND mask instead.
    let has_alpha = pixels.as_chunks::<4>().0.iter().any(|p| p[3] != 0);
    for (i, p) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        p.swap(0, 2); // BGRA → RGBA
        if !has_alpha {
            let transparent = mask.as_ref().is_some_and(|m| m[i * 4] != 0);
            p[3] = if transparent { 0 } else { 255 };
        }
    }
    Some(pixels)
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
