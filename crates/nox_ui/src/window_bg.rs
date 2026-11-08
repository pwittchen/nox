//! Native `NSWindow` glue for the main window: the background follows the theme's `bg`, so areas
//! exposed during live resize never flash white (SPEC §7.4, §9), and window moves are driven by
//! the app's own title-bar drag regions instead of `AppKit`'s (SPEC §5.2).

use std::ffi::c_void;

use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::theme::Theme;

/// The window's content `NSView`; `None` in tests (GPUI's test windows have no native handle and
/// asking for one panics).
fn ns_view(window: &Window) -> Option<*mut c_void> {
    if cfg!(test) {
        return None;
    }
    let handle = HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr()),
        _ => None,
    }
}

pub fn apply(window: &Window, theme: &Theme) {
    let Some(view) = ns_view(window) else { return };
    let c: gpui::Rgba = theme.bg.into();
    unsafe {
        nox_mac::set_window_background(view, f64::from(c.r), f64::from(c.g), f64::from(c.b), 1.0);
    }
}

/// `AppKit` would move the window for any drag that starts in the transparent title bar, tab
/// pills included, so tabs could never be dragged. Turn that off for this window; the title bar's
/// empty space moves the window through [`start_move`] instead.
pub fn take_over_window_drag(window: &Window) {
    let Some(view) = ns_view(window) else { return };
    unsafe { nox_mac::disable_native_window_drag(view) };
}

/// Moves the window with the pointer, for the left mouse-down being handled. On macOS GPUI's
/// `start_window_move` does nothing, so this goes through `AppKit` directly.
pub fn start_move(window: &Window) {
    #[cfg(test)]
    {
        let _ = window;
        tests::MOVES.with(|m| m.set(m.get() + 1));
    }
    #[cfg(not(test))]
    match ns_view(window) {
        Some(view) => unsafe { nox_mac::start_window_drag(view) },
        None => window.start_window_move(),
    }
}

#[cfg(test)]
pub mod tests {
    use std::cell::Cell;

    thread_local! {
        /// Window moves started (GPUI's test windows can't move).
        pub static MOVES: Cell<usize> = const { Cell::new(0) };
    }

    #[must_use]
    pub fn moves() -> usize {
        MOVES.with(Cell::get)
    }
}
