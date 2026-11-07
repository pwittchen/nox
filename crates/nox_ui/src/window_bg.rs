//! The `NSWindow` background follows the theme's `bg`, so areas exposed during live resize
//! never flash white (SPEC §7.4, §9).

use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::theme::Theme;

pub fn apply(window: &Window, theme: &Theme) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    if let RawWindowHandle::AppKit(h) = handle.as_raw() {
        let c: gpui::Rgba = theme.bg.into();
        unsafe {
            nox_mac::set_window_background(h.ns_view.as_ptr(), c.r as f64, c.g as f64, c.b as f64, 1.0);
        }
    }
}
