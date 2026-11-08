//! Font loading (SPEC §5.10): bundled fonts are registered with the process when first used
//! (the configured terminal and interface fonts at startup, others when picked), never
//! installed system-wide. A family that isn't found falls back to the default with a notice.

use std::borrow::Cow;
use std::collections::HashSet;

use gpui::{App, Global};
use nox_config::Config;

use crate::assets::{BUNDLED_TERMINAL_FONTS, BUNDLED_UI_FONTS, SYSTEM_TERMINAL_FONTS, font_files};
use crate::settings_state::{DEFAULT_TERMINAL_FONT, term_font, ui_font};

#[derive(Default)]
struct LoadedFonts(HashSet<String>);
impl Global for LoadedFonts {}

/// Register a bundled family (no-op if not bundled or already loaded).
pub fn load_family(cx: &mut App, family: &str) {
    let key = family.to_ascii_lowercase();
    if cx.default_global::<LoadedFonts>().0.contains(&key) {
        return;
    }
    let files = font_files(family);
    if files.is_empty() {
        return;
    }
    let data: Vec<Cow<'static, [u8]>> =
        files.iter().filter_map(|p| std::fs::read(p).ok()).map(Cow::Owned).collect();
    match cx.text_system().add_fonts(data) {
        Ok(()) => {
            cx.default_global::<LoadedFonts>().0.insert(key);
        }
        Err(e) => log::warn!("loading font {family}: {e}"),
    }
}

/// Make sure the configured fonts are available; notice when one isn't.
pub fn ensure_loaded(cx: &mut App, config: &Config) {
    load_family(cx, DEFAULT_TERMINAL_FONT);
    let tf = term_font(config);
    load_family(cx, &tf.family);
    for f in &tf.fallbacks {
        load_family(cx, f);
    }
    let uf = ui_font(config);
    if uf.as_ref() != crate::settings_state::SYSTEM_UI_FONT {
        load_family(cx, &uf);
    }
    let names = cx.text_system().all_font_names();
    let has = |f: &str| names.iter().any(|n| n.eq_ignore_ascii_case(f));
    if !has(&tf.family) {
        crate::toast::show(cx, format!("Font “{}” not found; using {DEFAULT_TERMINAL_FONT}.", tf.family));
    }
    if uf.as_ref() != crate::settings_state::SYSTEM_UI_FONT && !has(&uf) {
        crate::toast::show(cx, format!("Font “{uf}” not found; using the system font."));
    }
}

/// Families for the terminal font picker: bundled first, then installed monospace-looking ones.
pub fn terminal_font_choices(cx: &mut App) -> Vec<String> {
    let mut v: Vec<String> = BUNDLED_TERMINAL_FONTS.iter().map(|(n, _)| n.to_string()).collect();
    v.extend(SYSTEM_TERMINAL_FONTS.iter().map(std::string::ToString::to_string));
    let mut installed: Vec<String> = cx
        .text_system()
        .all_font_names()
        .into_iter()
        .filter(|n| {
            let l = n.to_ascii_lowercase();
            l.contains("mono") || l.contains("code") || l.contains("courier") || l.contains("consol")
        })
        .filter(|n| !v.iter().any(|x| x.eq_ignore_ascii_case(n)))
        .collect();
    installed.sort();
    installed.dedup();
    v.extend(installed);
    v
}

/// Families for the interface font picker: system, bundled, then all installed.
pub fn ui_font_choices(cx: &mut App) -> Vec<String> {
    let mut v: Vec<String> = vec!["system".into()];
    v.extend(BUNDLED_UI_FONTS.iter().map(|(n, _)| n.to_string()));
    let mut installed: Vec<String> = cx
        .text_system()
        .all_font_names()
        .into_iter()
        .filter(|n| !n.starts_with('.') && !v.iter().any(|x| x.eq_ignore_ascii_case(n)))
        .collect();
    installed.sort();
    installed.dedup();
    v.extend(installed);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn loaded(cx: &App) -> Vec<String> {
        let mut v: Vec<String> =
            cx.try_global::<LoadedFonts>().map(|l| l.0.iter().cloned().collect()).unwrap_or_default();
        v.sort();
        v
    }

    #[gpui::test]
    fn bundled_families_register_once(cx: &mut TestAppContext) {
        crate::test_support::init(cx);
        cx.update(|cx| {
            load_family(cx, "Hack");
            load_family(cx, "hack");
            assert_eq!(loaded(cx), ["hack"]);
            // Not bundled: nothing to register.
            load_family(cx, "Menlo");
            load_family(cx, "Comic Sans");
            assert_eq!(loaded(cx), ["hack"]);
        });
    }

    #[gpui::test]
    fn ensure_loaded_registers_configured_fonts_and_notices_missing(cx: &mut TestAppContext) {
        crate::test_support::init(cx);
        let mut config = crate::test_support::config();
        config.font.family = "Fira Code, Hack".into();
        config.ui.font_family = "Inter".into();
        cx.update(|cx| {
            ensure_loaded(cx, &config);
            assert_eq!(loaded(cx), ["fira code", "hack", "inter", "jetbrains mono"]);
            // The headless text system knows no font names, so both are reported as missing.
            let notices = crate::toast::messages(cx);
            assert!(notices.iter().any(|m| m.contains("“Fira Code” not found")), "{notices:?}");
            assert!(notices.iter().any(|m| m.contains("“Inter” not found")), "{notices:?}");
        });
    }

    #[gpui::test]
    fn system_ui_font_is_never_loaded_or_reported(cx: &mut TestAppContext) {
        crate::test_support::init(cx);
        let config = crate::test_support::config();
        cx.update(|cx| {
            ensure_loaded(cx, &config);
            assert_eq!(loaded(cx), ["jetbrains mono"]);
            let notices = crate::toast::messages(cx);
            assert!(notices.iter().all(|m| !m.contains("system font")), "{notices:?}");
        });
    }

    #[gpui::test]
    fn picker_choices(cx: &mut TestAppContext) {
        crate::test_support::init(cx);
        cx.update(|cx| {
            let t = terminal_font_choices(cx);
            assert_eq!(t[0], "JetBrains Mono");
            let n = BUNDLED_TERMINAL_FONTS.len() + SYSTEM_TERMINAL_FONTS.len();
            assert_eq!(t[n - 1], "Menlo");
            // Installed families come after, without repeats.
            let mut seen = std::collections::HashSet::new();
            assert!(t.iter().all(|f| seen.insert(f.to_ascii_lowercase())), "{t:?}");
            let u = ui_font_choices(cx);
            assert_eq!(u[0], "system");
            let bundled: Vec<String> = BUNDLED_UI_FONTS.iter().map(|(n, _)| (*n).to_string()).collect();
            assert_eq!(&u[1..=bundled.len()], bundled.as_slice());
            assert!(u.iter().all(|f| !f.starts_with('.')), "hidden system fonts are left out: {u:?}");
        });
    }
}
