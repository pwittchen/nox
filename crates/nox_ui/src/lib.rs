//! nox_ui — GPUI views and elements (SPEC §13).

pub mod actions;
pub mod assets;
pub mod attention;
pub mod chrome;
pub mod config_io;
pub mod coop;
pub mod dialog;
pub mod features;
pub mod fonts;
pub mod icons;
pub mod links;
pub mod menus;
pub mod palette;
pub mod pane;
pub mod persist;
pub mod root;
pub mod root_actions;
pub mod scroll;
pub mod settings;
pub mod settings_state;
pub mod terminal_element;
pub mod text_input;
pub mod theme;
pub mod toast;
pub mod util;
pub mod util_keys;
pub mod window_bg;

/// Percent-decode a `file://` URL path.
pub fn links_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
