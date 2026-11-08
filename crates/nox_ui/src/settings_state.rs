//! The live configuration as a GPUI global, plus values derived from it (resolved fonts) and
//! the macOS accessibility flags.

use std::sync::Arc;

use gpui::{App, FontWeight, Global, SharedString};
use nox_config::Config;

use crate::terminal_element::TermFont;

#[derive(Clone)]
pub struct Settings {
    pub config: Arc<Config>,
    pub term_font: TermFont,
    pub ui_font: SharedString,
    pub a11y: nox_mac::Accessibility,
}

impl Global for Settings {}

impl Settings {
    pub fn global(cx: &App) -> &Settings {
        cx.global::<Settings>()
    }

    #[must_use]
    pub fn new(config: Config, a11y: nox_mac::Accessibility) -> Self {
        let term_font = term_font(&config);
        let ui_font = ui_font(&config);
        Self { config: Arc::new(config), term_font, ui_font, a11y }
    }

    #[must_use]
    pub fn reduce_motion(&self) -> bool {
        self.a11y.reduce_motion
    }
}

pub const DEFAULT_TERMINAL_FONT: &str = "JetBrains Mono";
pub const SYSTEM_UI_FONT: &str = ".SystemUIFont";

#[must_use]
pub fn parse_weight(s: &str) -> FontWeight {
    match s.trim().to_ascii_lowercase().as_str() {
        "thin" | "100" => FontWeight::THIN,
        "extralight" | "extra-light" | "200" => FontWeight::EXTRA_LIGHT,
        "light" | "300" => FontWeight::LIGHT,
        "medium" | "500" => FontWeight::MEDIUM,
        "semibold" | "semi-bold" | "600" => FontWeight::SEMIBOLD,
        "bold" | "700" => FontWeight::BOLD,
        "extrabold" | "extra-bold" | "800" => FontWeight::EXTRA_BOLD,
        "black" | "900" => FontWeight::BLACK,
        _ => FontWeight::NORMAL,
    }
}

/// `font.family` is a family or a comma-separated list, always followed by `JetBrains` Mono.
#[must_use]
pub fn term_font(config: &Config) -> TermFont {
    let mut families: Vec<String> =
        config.font.family.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if families.is_empty() {
        families.push(DEFAULT_TERMINAL_FONT.to_string());
    }
    let primary = families.remove(0);
    if !families.iter().any(|f| f == DEFAULT_TERMINAL_FONT) && primary != DEFAULT_TERMINAL_FONT {
        families.push(DEFAULT_TERMINAL_FONT.to_string());
    }
    TermFont {
        family: primary.into(),
        fallbacks: families,
        size: config.font.size.clamp(6.0, 72.0) as f32,
        line_height: config.font.line_height.clamp(0.8, 3.0) as f32,
        letter_spacing: config.font.letter_spacing as f32,
        weight: parse_weight(&config.font.weight),
        weight_bold: parse_weight(&config.font.weight_bold),
    }
}

#[must_use]
pub fn ui_font(config: &Config) -> SharedString {
    let f = config.ui.font_family.trim();
    if f.is_empty() || f.eq_ignore_ascii_case("system") {
        SYSTEM_UI_FONT.into()
    } else {
        f.to_string().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_by_name_and_number() {
        for (s, w) in [
            ("thin", FontWeight::THIN),
            ("100", FontWeight::THIN),
            ("ExtraLight", FontWeight::EXTRA_LIGHT),
            ("extra-light", FontWeight::EXTRA_LIGHT),
            ("200", FontWeight::EXTRA_LIGHT),
            ("light", FontWeight::LIGHT),
            ("300", FontWeight::LIGHT),
            (" medium ", FontWeight::MEDIUM),
            ("500", FontWeight::MEDIUM),
            ("semibold", FontWeight::SEMIBOLD),
            ("semi-bold", FontWeight::SEMIBOLD),
            ("600", FontWeight::SEMIBOLD),
            ("BOLD", FontWeight::BOLD),
            ("700", FontWeight::BOLD),
            ("extrabold", FontWeight::EXTRA_BOLD),
            ("extra-bold", FontWeight::EXTRA_BOLD),
            ("800", FontWeight::EXTRA_BOLD),
            ("black", FontWeight::BLACK),
            ("900", FontWeight::BLACK),
            ("normal", FontWeight::NORMAL),
            ("400", FontWeight::NORMAL),
            ("heavy-ish", FontWeight::NORMAL),
            ("", FontWeight::NORMAL),
        ] {
            assert_eq!(parse_weight(s), w, "{s:?}");
        }
    }

    #[test]
    fn term_font_family_list_ends_with_default() {
        let mut c = Config::default();
        c.font.family = " Fira Code , Hack ,, ".into();
        let f = term_font(&c);
        assert_eq!(f.family.as_ref(), "Fira Code");
        assert_eq!(f.fallbacks, vec!["Hack".to_string(), DEFAULT_TERMINAL_FONT.to_string()]);

        // Already listed: not added twice.
        c.font.family = "Hack, JetBrains Mono".into();
        assert_eq!(term_font(&c).fallbacks, vec![DEFAULT_TERMINAL_FONT.to_string()]);

        // The default itself needs no fallback; an empty family means the default.
        c.font.family = "JetBrains Mono".into();
        assert!(term_font(&c).fallbacks.is_empty());
        c.font.family = " , ".into();
        let f = term_font(&c);
        assert_eq!(f.family.as_ref(), DEFAULT_TERMINAL_FONT);
        assert!(f.fallbacks.is_empty());
    }

    #[test]
    fn term_font_clamps_size_and_line_height() {
        let mut c = Config::default();
        c.font.size = 2.0;
        c.font.line_height = 10.0;
        c.font.letter_spacing = 0.5;
        c.font.weight = "light".into();
        c.font.weight_bold = "800".into();
        let f = term_font(&c);
        assert!((f.size - 6.0).abs() < f32::EPSILON);
        assert!((f.line_height - 3.0).abs() < f32::EPSILON);
        assert!((f.letter_spacing - 0.5).abs() < f32::EPSILON);
        assert_eq!(f.weight, FontWeight::LIGHT);
        assert_eq!(f.weight_bold, FontWeight::EXTRA_BOLD);
        c.font.size = 500.0;
        c.font.line_height = 0.1;
        let f = term_font(&c);
        assert!((f.size - 72.0).abs() < f32::EPSILON);
        assert!((f.line_height - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn ui_font_system_aliases() {
        let mut c = Config::default();
        for s in ["", "  ", "system", "System"] {
            c.ui.font_family = s.into();
            assert_eq!(ui_font(&c).as_ref(), SYSTEM_UI_FONT, "{s:?}");
        }
        c.ui.font_family = " Inter ".into();
        assert_eq!(ui_font(&c).as_ref(), "Inter");
    }

    #[test]
    fn settings_new_derives_fonts_and_a11y() {
        let mut c = Config::default();
        c.font.family = "Hack".into();
        c.ui.font_family = "Geist".into();
        let a11y = nox_mac::Accessibility { reduce_motion: true, ..Default::default() };
        let s = Settings::new(c, a11y);
        assert_eq!(s.term_font.family.as_ref(), "Hack");
        assert_eq!(s.ui_font.as_ref(), "Geist");
        assert!(s.reduce_motion());
        assert!(!Settings::new(Config::default(), nox_mac::Accessibility::default()).reduce_motion());
    }
}
