//! System tray icon. Windows and macOS use Slint's `SystemTrayIcon` (`TrayIcon` in
//! ui/app.slint). On Linux the StatusNotifierItem is served by `ksni` from a background
//! thread instead: registering with the tray host is a blocking D-Bus round trip that
//! can take seconds (or hang) and must never delay the main window.

use std::sync::Arc;

pub type Action = Arc<dyn Fn() + Send + Sync>;

#[cfg(not(target_os = "linux"))]
pub struct Tray(crate::TrayIcon);

#[cfg(not(target_os = "linux"))]
impl Tray {
    pub fn new(on_show: Action, on_quit: Action) -> Result<Self, slint::PlatformError> {
        let tray = crate::TrayIcon::new()?;
        tray.on_show_window(move || on_show());
        tray.on_quit_app(move || on_quit());
        Ok(Self(tray))
    }

    pub fn set_language(&self, lang: usize) {
        crate::i18n::apply(&self.0.global::<crate::Tr>(), lang);
    }
}

#[cfg(target_os = "linux")]
pub use linux::Tray;

#[cfg(target_os = "linux")]
mod linux {
    use super::Action;
    use ksni::blocking::{Handle, TrayMethods};
    use ksni::menu::StandardItem;
    use std::sync::{Arc, Mutex};

    const ICON_PNG: &[u8] = include_bytes!("../assets/64x64.png");

    struct Sni {
        icon: Option<ksni::Icon>,
        show_label: String,
        quit_label: String,
        on_show: Action,
        on_quit: Action,
    }

    impl ksni::Tray for Sni {
        fn id(&self) -> String {
            "apps-volume-control".into()
        }

        fn title(&self) -> String {
            "Apps Volume Control".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            self.icon.iter().cloned().collect()
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip { title: "Apps Volume Control".into(), ..Default::default() }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            (self.on_show)();
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            vec![
                StandardItem {
                    label: self.show_label.clone(),
                    activate: Box::new(|tray: &mut Self| (tray.on_show)()),
                    ..Default::default()
                }
                .into(),
                ksni::MenuItem::Separator,
                StandardItem {
                    label: self.quit_label.clone(),
                    activate: Box::new(|tray: &mut Self| (tray.on_quit)()),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    /// Converts PNG to the ARGB32 (network byte order) pixmap the SNI spec expects.
    pub fn decode_icon(data: &[u8]) -> Option<ksni::Icon> {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().ok()?;
        let mut buf = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut buf).ok()?;
        let pixels = &buf[..info.buffer_size()];
        let mut argb = Vec::with_capacity(info.width as usize * info.height as usize * 4);
        match info.color_type {
            png::ColorType::Rgba => {
                for p in pixels.as_chunks::<4>().0 {
                    argb.extend_from_slice(&[p[3], p[0], p[1], p[2]]);
                }
            }
            png::ColorType::Rgb => {
                for p in pixels.as_chunks::<3>().0 {
                    argb.extend_from_slice(&[255, p[0], p[1], p[2]]);
                }
            }
            _ => return None,
        }
        Some(ksni::Icon { width: info.width as i32, height: info.height as i32, data: argb })
    }

    pub struct Tray {
        handle: Arc<Mutex<Option<Handle<Sni>>>>,
        /// Labels chosen before the service finished starting.
        labels: Arc<Mutex<(String, String)>>,
    }

    impl Tray {
        pub fn new(on_show: Action, on_quit: Action) -> Result<Self, slint::PlatformError> {
            let handle: Arc<Mutex<Option<Handle<Sni>>>> = Arc::default();
            let labels = Arc::new(Mutex::new(("Show window".to_string(), "Quit".to_string())));
            let (slot, initial) = (handle.clone(), labels.clone());
            std::thread::Builder::new()
                .name("tray".into())
                .spawn(move || {
                    let (show_label, quit_label) = initial.lock().unwrap().clone();
                    let sni = Sni { icon: decode_icon(ICON_PNG), show_label, quit_label, on_show, on_quit };
                    // `assume_sni_available`: also works when the tray host starts after us (autostart).
                    match sni.assume_sni_available(true).spawn() {
                        Ok(h) => {
                            // Apply a language switch that happened while we were registering.
                            let (show, quit) = initial.lock().unwrap().clone();
                            h.update(|t| {
                                t.show_label = show;
                                t.quit_label = quit;
                            });
                            *slot.lock().unwrap() = Some(h);
                        }
                        Err(err) => eprintln!("Tray unavailable: {err}"),
                    }
                })
                .map_err(|e| slint::PlatformError::Other(e.to_string()))?;
            Ok(Self { handle, labels })
        }

        pub fn set_language(&self, lang: usize) {
            let show = crate::i18n::tray_text(lang, false).to_string();
            let quit = crate::i18n::tray_text(lang, true).to_string();
            *self.labels.lock().unwrap() = (show.clone(), quit.clone());
            if let Some(h) = self.handle.lock().unwrap().as_ref() {
                h.update(|t| {
                    t.show_label = show;
                    t.quit_label = quit;
                });
            }
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn decodes_bundled_icon_to_argb() {
            let icon = super::decode_icon(super::ICON_PNG).expect("icon decodes");
            assert_eq!((icon.width, icon.height), (64, 64));
            assert_eq!(icon.data.len(), 64 * 64 * 4);
            // The icon is opaque: alpha is the first byte of every ARGB pixel.
            assert!(icon.data.as_chunks::<4>().0.iter().all(|p| p[0] == 255));
        }
    }
}
