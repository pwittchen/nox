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

    pub fn new(config: Config, a11y: nox_mac::Accessibility) -> Self {
        let term_font = term_font(&config);
        let ui_font = ui_font(&config);
        Self { config: Arc::new(config), term_font, ui_font, a11y }
    }

    pub fn reduce_motion(&self) -> bool {
        self.a11y.reduce_motion
    }
}

pub const DEFAULT_TERMINAL_FONT: &str = "JetBrains Mono";
pub const SYSTEM_UI_FONT: &str = ".SystemUIFont";

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

/// `font.family` is a family or a comma-separated list, always followed by JetBrains Mono.
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

pub fn ui_font(config: &Config) -> SharedString {
    let f = config.ui.font_family.trim();
    if f.is_empty() || f.eq_ignore_ascii_case("system") {
        SYSTEM_UI_FONT.into()
    } else {
        f.to_string().into()
    }
}
