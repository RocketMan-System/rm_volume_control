//! Global mouse-button hotkeys (`global-hotkey` handles only the keyboard). A hotkey is the
//! middle or a side button, or a wheel turn, with optional modifiers, written like
//! `Ctrl+Mouse4` or `Ctrl+WheelUp`.
//!
//! Each OS hooks the mouse on its own thread, started on first use, and reports buttons to
//! `on_button`. A press that matches a hotkey is swallowed together with its release; each
//! wheel step is a press of its own.

use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Middle,
    Back,
    Forward,
    WheelUp,
    WheelDown,
}

const BUTTONS: [Button; 5] = [Button::Middle, Button::Back, Button::Forward, Button::WheelUp, Button::WheelDown];

impl Button {
    /// Number as passed by the UI: 3 middle, 4 back, 5 forward, 6 wheel up, 7 wheel down.
    pub fn from_number(n: i32) -> Option<Self> {
        BUTTONS.get(usize::try_from(n).ok()?.checked_sub(3)?).copied()
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Middle => "Mouse3",
            Self::Back => "Mouse4",
            Self::Forward => "Mouse5",
            Self::WheelUp => "WheelUp",
            Self::WheelDown => "WheelDown",
        }
    }

    pub fn is_wheel(self) -> bool {
        matches!(self, Self::WheelUp | Self::WheelDown)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl Mods {
    pub fn any(self) -> bool {
        self.ctrl || self.alt || self.shift || self.meta
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MouseCombo {
    pub mods: Mods,
    pub button: Button,
}

impl FromStr for MouseCombo {
    type Err = ();

    /// Parses `Ctrl+Shift+Mouse4`; modifier names as in `global-hotkey`.
    fn from_str(s: &str) -> Result<Self, ()> {
        let mut parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let key = parts.pop().ok_or(())?;
        let button = BUTTONS.into_iter().find(|b| b.name().eq_ignore_ascii_case(key)).ok_or(())?;
        let mut mods = Mods::default();
        for part in parts {
            let flag = match part.to_ascii_uppercase().as_str() {
                "CTRL" | "CONTROL" => &mut mods.ctrl,
                "ALT" | "OPTION" => &mut mods.alt,
                "SHIFT" => &mut mods.shift,
                "SUPER" | "META" | "CMD" | "COMMAND" => &mut mods.meta,
                _ => return Err(()),
            };
            *flag = true;
        }
        Ok(Self { mods, button })
    }
}

type Handler = Box<dyn Fn(MouseCombo) + Send + Sync>;

static HANDLER: OnceLock<Handler> = OnceLock::new();

struct State {
    active: Vec<MouseCombo>,
    /// Buttons whose press was swallowed, so their release is swallowed too.
    held: [bool; BUTTONS.len()],
}

static STATE: Mutex<State> = Mutex::new(State { active: Vec::new(), held: [false; BUTTONS.len()] });

/// Sets what a matching press does. Only the first call has an effect.
pub fn set_handler(handler: impl Fn(MouseCombo) + Send + Sync + 'static) {
    let _ = HANDLER.set(Box::new(handler));
}

/// Whether this session can deliver `button` to a global hotkey. Under Wayland only XWayland's
/// grabs are left, and KWin (with "allow access to mouse buttons") passes them extra buttons
/// only: never left, right or middle, and no wheel.
pub fn supported(button: Button) -> bool {
    !cfg!(target_os = "linux") || !(button == Button::Middle || button.is_wheel()) || !wayland_session()
}

fn wayland_session() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v == "wayland")
}

/// Replaces the active mouse hotkeys. Returns those that could not be registered.
pub fn set(wanted: &[MouseCombo]) -> Vec<MouseCombo> {
    let combos: Vec<MouseCombo> = wanted.iter().filter(|c| supported(c.button)).copied().collect();
    let unsupported = wanted.iter().filter(|c| !supported(c.button)).copied();
    if combos.is_empty() && STATE.lock().unwrap().active.is_empty() {
        // Never start a hook (or ask for a permission) nobody needs.
        return unsupported.collect();
    }
    let ok = platform::set(&combos);
    STATE.lock().unwrap().active = ok.clone();
    combos.iter().filter(|c| !ok.contains(c)).copied().chain(unsupported).collect()
}

/// Called by the platform hook for every press and release. Returns whether to swallow it.
fn on_button(button: Button, mods: Mods, pressed: bool) -> bool {
    let mut state = STATE.lock().unwrap();
    let held = &mut state.held[button as usize];
    if !pressed {
        return std::mem::take(held);
    }
    let combo = MouseCombo { mods, button };
    if !state.active.contains(&combo) {
        return false;
    }
    // A wheel step has no release.
    state.held[button as usize] = !button.is_wheel();
    drop(state);
    if let Some(handler) = HANDLER.get() {
        handler(combo);
    }
    true
}

#[cfg(windows)]
mod platform {
    //! A low-level mouse hook on a thread with its own message loop.
    use super::{on_button, Button, Mods, MouseCombo};
    use std::sync::{mpsc, OnceLock};
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, SetWindowsHookExW, HC_ACTION, MSG, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_MBUTTONDOWN,
        WM_MBUTTONUP, WM_MOUSEWHEEL, WM_XBUTTONDOWN, WM_XBUTTONUP,
    };

    static HOOKED: OnceLock<bool> = OnceLock::new();

    pub fn set(combos: &[MouseCombo]) -> Vec<MouseCombo> {
        if *HOOKED.get_or_init(start) {
            combos.to_vec()
        } else {
            Vec::new()
        }
    }

    fn start() -> bool {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("mouse-hook".into()).spawn(move || unsafe {
            let module = GetModuleHandleW(None).ok().map(Into::into);
            let hooked = SetWindowsHookExW(WH_MOUSE_LL, Some(hook), module, 0).is_ok();
            let _ = tx.send(hooked);
            if hooked {
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
            }
        });
        spawned.is_ok() && rx.recv().unwrap_or(false)
    }

    fn down(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
        unsafe { GetAsyncKeyState(vk.0 as i32) < 0 }
    }

    unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            let high = (info.mouseData >> 16) as u16;
            let side = match high {
                1 => Some(Button::Back),
                2 => Some(Button::Forward),
                _ => None,
            };
            let event = match wparam.0 as u32 {
                WM_MBUTTONDOWN => Some((Button::Middle, true)),
                WM_MBUTTONUP => Some((Button::Middle, false)),
                WM_XBUTTONDOWN => side.map(|b| (b, true)),
                WM_XBUTTONUP => side.map(|b| (b, false)),
                WM_MOUSEWHEEL if high as i16 > 0 => Some((Button::WheelUp, true)),
                WM_MOUSEWHEEL if (high as i16) < 0 => Some((Button::WheelDown, true)),
                _ => None,
            };
            if let Some((button, pressed)) = event {
                let mods = Mods {
                    ctrl: down(VK_CONTROL),
                    alt: down(VK_MENU),
                    shift: down(VK_SHIFT),
                    meta: down(VK_LWIN) || down(VK_RWIN),
                };
                if on_button(button, mods, pressed) {
                    return LRESULT(1);
                }
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    //! Button grabs on the X11 root window, like `global-hotkey` does for keys.
    use super::{on_button, Button, Mods, MouseCombo};
    use std::sync::{Arc, OnceLock};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ButtonIndex, ConnectionExt, EventMask, GrabMode, KeyButMask, ModMask, Window};
    use x11rb::protocol::Event;
    use x11rb::rust_connection::RustConnection;

    static CONN: OnceLock<Option<(Arc<RustConnection>, Window)>> = OnceLock::new();

    // Grabs need the exact state, which includes NumLock (M2) and CapsLock when they are on.
    fn locks() -> [ModMask; 4] {
        [ModMask::default(), ModMask::M2, ModMask::LOCK, ModMask::M2 | ModMask::LOCK]
    }

    pub fn set(combos: &[MouseCombo]) -> Vec<MouseCombo> {
        let Some((conn, root)) = CONN.get_or_init(connect) else { return Vec::new() };
        let _ = conn.ungrab_button(ButtonIndex::ANY, *root, ModMask::ANY);
        let mut ok = Vec::new();
        for combo in combos {
            let grabbed = locks().into_iter().all(|lock| grab(conn, *root, combo, lock));
            if grabbed {
                ok.push(*combo);
            } else {
                for lock in locks() {
                    let _ = conn.ungrab_button(button_index(combo.button), *root, mods_mask(combo.mods) | lock);
                }
            }
        }
        let _ = conn.flush();
        ok
    }

    fn grab(conn: &RustConnection, root: Window, combo: &MouseCombo, lock: ModMask) -> bool {
        let mask = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE;
        conn.grab_button(
            false,
            root,
            mask,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
            x11rb::NONE,
            x11rb::NONE,
            button_index(combo.button),
            mods_mask(combo.mods) | lock,
        )
        .ok()
        .and_then(|cookie| cookie.check().ok())
        .is_some()
    }

    fn connect() -> Option<(Arc<RustConnection>, Window)> {
        let (conn, screen) = x11rb::connect(None).map_err(|err| eprintln!("Mouse hotkeys unavailable: {err}")).ok()?;
        let root = conn.setup().roots[screen].root;
        let conn = Arc::new(conn);
        let events = conn.clone();
        std::thread::Builder::new()
            .name("mouse-hook".into())
            .spawn(move || {
                while let Ok(event) = events.wait_for_event() {
                    let (e, pressed) = match event {
                        Event::ButtonPress(e) => (e, true),
                        Event::ButtonRelease(e) => (e, false),
                        _ => continue,
                    };
                    if let Some(button) = button_from(e.detail) {
                        on_button(button, mods_from(e.state), pressed);
                    }
                }
            })
            .ok()?;
        Some((conn, root))
    }

    fn button_index(button: Button) -> ButtonIndex {
        match button {
            Button::Middle => ButtonIndex::M2,
            Button::Back => ButtonIndex::from(8u8),
            Button::Forward => ButtonIndex::from(9u8),
            Button::WheelUp => ButtonIndex::M4,
            Button::WheelDown => ButtonIndex::M5,
        }
    }

    fn button_from(detail: u8) -> Option<Button> {
        match detail {
            2 => Some(Button::Middle),
            4 => Some(Button::WheelUp),
            5 => Some(Button::WheelDown),
            8 => Some(Button::Back),
            9 => Some(Button::Forward),
            _ => None,
        }
    }

    fn mods_mask(mods: Mods) -> ModMask {
        let mut mask = ModMask::default();
        for (on, bit) in [
            (mods.ctrl, ModMask::CONTROL),
            (mods.alt, ModMask::M1),
            (mods.shift, ModMask::SHIFT),
            (mods.meta, ModMask::M4),
        ] {
            if on {
                mask |= bit;
            }
        }
        mask
    }

    fn mods_from(state: KeyButMask) -> Mods {
        Mods {
            ctrl: state.contains(KeyButMask::CONTROL),
            alt: state.contains(KeyButMask::MOD1),
            shift: state.contains(KeyButMask::SHIFT),
            meta: state.contains(KeyButMask::MOD4),
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    //! An event tap for "other" mouse buttons and the wheel (needs the Accessibility permission).
    use super::{on_button, Button, Mods, MouseCombo};
    use objc2_core_foundation::{kCFRunLoopCommonModes, CFMachPort, CFRunLoop};
    use objc2_core_graphics::{
        CGEvent, CGEventField, CGEventFlags, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
        CGEventTapProxy, CGEventType,
    };
    use std::ffi::c_void;
    use std::ptr::{null_mut, NonNull};
    use std::sync::atomic::{AtomicPtr, Ordering};
    use std::sync::{mpsc, OnceLock};

    static HOOKED: OnceLock<bool> = OnceLock::new();
    /// The tap, to re-enable it when the system disables it after a slow callback.
    static TAP: AtomicPtr<CFMachPort> = AtomicPtr::new(null_mut());

    pub fn set(combos: &[MouseCombo]) -> Vec<MouseCombo> {
        if *HOOKED.get_or_init(start) {
            combos.to_vec()
        } else {
            Vec::new()
        }
    }

    fn start() -> bool {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("mouse-hook".into()).spawn(move || unsafe {
            let mask = [CGEventType::OtherMouseDown, CGEventType::OtherMouseUp, CGEventType::ScrollWheel]
                .iter()
                .fold(0u64, |mask, ty| mask | 1 << ty.0);
            let tap = CGEvent::tap_create(
                CGEventTapLocation::SessionEventTap,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                mask,
                Some(callback),
                null_mut(),
            );
            let Some(tap) = tap else {
                eprintln!("Mouse hotkeys unavailable: no Accessibility permission");
                let _ = tx.send(false);
                return;
            };
            let Some(source) = CFMachPort::new_run_loop_source(None, Some(&tap), 0) else {
                let _ = tx.send(false);
                return;
            };
            let Some(run_loop) = CFRunLoop::current() else {
                let _ = tx.send(false);
                return;
            };
            run_loop.add_source(Some(&source), kCFRunLoopCommonModes);
            CGEvent::tap_enable(&tap, true);
            TAP.store(objc2_core_foundation::CFRetained::as_ptr(&tap).as_ptr(), Ordering::Release);
            let _ = tx.send(true);
            CFRunLoop::run();
        });
        spawned.is_ok() && rx.recv().unwrap_or(false)
    }

    unsafe extern "C-unwind" fn callback(
        _proxy: CGEventTapProxy,
        ty: CGEventType,
        event: NonNull<CGEvent>,
        _info: *mut c_void,
    ) -> *mut CGEvent {
        if ty == CGEventType::TapDisabledByTimeout || ty == CGEventType::TapDisabledByUserInput {
            if let Some(tap) = unsafe { TAP.load(Ordering::Acquire).as_ref() } {
                CGEvent::tap_enable(tap, true);
            }
            return event.as_ptr();
        }
        let e = unsafe { event.as_ref() };
        let button = if ty == CGEventType::ScrollWheel {
            match CGEvent::integer_value_field(Some(e), CGEventField::ScrollWheelEventDeltaAxis1).signum() {
                1 => Button::WheelUp,
                -1 => Button::WheelDown,
                _ => return event.as_ptr(),
            }
        } else {
            match CGEvent::integer_value_field(Some(e), CGEventField::MouseEventButtonNumber) {
                2 => Button::Middle,
                3 => Button::Back,
                4 => Button::Forward,
                _ => return event.as_ptr(),
            }
        };
        let flags = CGEvent::flags(Some(e));
        let mods = Mods {
            ctrl: flags.contains(CGEventFlags::MaskControl),
            alt: flags.contains(CGEventFlags::MaskAlternate),
            shift: flags.contains(CGEventFlags::MaskShift),
            meta: flags.contains(CGEventFlags::MaskCommand),
        };
        if on_button(button, mods, ty != CGEventType::OtherMouseUp) {
            null_mut()
        } else {
            event.as_ptr()
        }
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod platform {
    use super::MouseCombo;

    pub fn set(_: &[MouseCombo]) -> Vec<MouseCombo> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_combos() {
        let c: MouseCombo = "Ctrl+Shift+Mouse4".parse().unwrap();
        assert_eq!(
            c,
            MouseCombo { mods: Mods { ctrl: true, shift: true, ..Default::default() }, button: Button::Back }
        );
        assert_eq!("Mouse5".parse(), Ok(MouseCombo { mods: Mods::default(), button: Button::Forward }));
        assert_eq!("super+mouse3".parse::<MouseCombo>().unwrap().button, Button::Middle);
        assert!("Ctrl+F1".parse::<MouseCombo>().is_err());
        assert!("Hyper+Mouse4".parse::<MouseCombo>().is_err());
        assert!("Mouse1".parse::<MouseCombo>().is_err());
        assert_eq!("Alt+WheelDown".parse::<MouseCombo>().unwrap().button, Button::WheelDown);
    }

    #[test]
    fn button_numbers() {
        assert_eq!(Button::from_number(3), Some(Button::Middle));
        assert_eq!(Button::from_number(6), Some(Button::WheelUp));
        assert_eq!(Button::from_number(7), Some(Button::WheelDown));
        assert_eq!(Button::from_number(2), None);
        assert_eq!(Button::from_number(8), None);
    }
}
