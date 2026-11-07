//! nox_theme: theme model, the built-in `nox` palette, bundled themes and theme-file parsing
//! (SPEC §9). No GPUI here: colors are plain [`Rgba`]; the UI converts them.

mod color;

use std::path::Path;

pub use color::{Rgba, contrast_ratio, ensure_contrast};

/// One light or dark palette: UI tokens plus the 16 terminal colors.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ThemeVariant {
    /// Terminal background and pane card.
    pub bg: Rgba,
    /// Sidebar, title bar.
    pub bg_sidebar: Rgba,
    /// Active tab pill, floating panels.
    pub bg_elevated: Rgba,
    /// Terminal foreground and UI text.
    pub text: Rgba,
    pub text_muted: Rgba,
    pub border: Rgba,
    pub accent: Rgba,
    pub selection: Rgba,
    /// Terminal cursor.
    pub caret: Rgba,
    /// Text under a block cursor.
    pub caret_text: Rgba,
    /// black red green yellow blue magenta cyan white, then the bright-* ones.
    pub ansi: [Rgba; 16],
}

/// Theme-file names of the 16 ANSI colors, in [`ThemeVariant::ansi`] order.
pub const ANSI_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright-black",
    "bright-red",
    "bright-green",
    "bright-yellow",
    "bright-blue",
    "bright-magenta",
    "bright-cyan",
    "bright-white",
];

/// `bg-elevated` derived from `bg`: slightly lighter in dark variants, darker in light ones.
pub fn derive_bg_elevated(bg: Rgba, dark: bool) -> Rgba {
    if dark { bg.mix(Rgba::WHITE, 0.07).with_alpha(bg.a) } else { bg.mix(Rgba::BLACK, 0.06).with_alpha(bg.a) }
}

/// A resolved theme with at least one variant.
#[derive(Clone, PartialEq, Debug)]
pub struct Theme {
    pub name: String,
    pub light: Option<ThemeVariant>,
    pub dark: Option<ThemeVariant>,
}

impl Theme {
    /// The variant for the given appearance; a single-variant theme always returns its only
    /// variant.
    pub fn variant(&self, dark: bool) -> &ThemeVariant {
        let (preferred, other) = if dark { (&self.dark, &self.light) } else { (&self.light, &self.dark) };
        preferred.as_ref().or(other.as_ref()).expect("a Theme always has at least one variant")
    }

    /// Only one of light / dark exists (the appearance toggle is disabled).
    pub fn is_single_variant(&self) -> bool {
        self.light.is_none() || self.dark.is_none()
    }
}

fn hex(s: &str) -> Rgba {
    Rgba::from_hex(s).expect("valid built-in hex color")
}

fn variant_from_hex(ui: [&str; 9], ansi: [&str; 16]) -> ThemeVariant {
    let bg = hex(ui[0]);
    ThemeVariant {
        bg,
        bg_sidebar: hex(ui[1]),
        bg_elevated: hex(ui[2]),
        text: hex(ui[3]),
        text_muted: hex(ui[4]),
        border: hex(ui[5]),
        accent: hex(ui[6]),
        selection: hex(ui[7]),
        caret: hex(ui[8]),
        caret_text: bg,
        ansi: ansi.map(hex),
    }
}

/// The built-in `nox` dark palette.
pub fn nox_dark() -> ThemeVariant {
    variant_from_hex(
        ["#18191B", "#1C1D20", "#2A2B2F", "#E6E6E8", "#8A8A8F", "#2A2A2E", "#22D3EE", "#22D3EE33", "#D7DED7"],
        [
            "#3A3B40", "#F07178", "#8BD49C", "#E6C07B", "#6CB6FF", "#F075C1", "#5CCFE6",
            "#C8C8CC", // normal
            "#6E6F76", "#FF8A90", "#A3E6B2", "#F2D296", "#8FC8FF", "#FF94D2", "#85DDF0",
            "#F4F4F6", // bright
        ],
    )
}

/// The built-in `nox` light palette.
pub fn nox_light() -> ThemeVariant {
    variant_from_hex(
        ["#FFFFFF", "#F7F7F8", "#EFEFF1", "#1D1D1F", "#8A8A8E", "#E6E6E8", "#0891B2", "#0891B233", "#1D1D1F"],
        [
            "#1D1D1F", "#C4262E", "#18792F", "#8A6A00", "#0B63CE", "#A5307F", "#0A7A8C",
            "#6E6E73", // normal
            "#57575C", "#D7373F", "#1F8A3A", "#9C7800", "#2A78E0", "#B9418F", "#12899C",
            "#9A9AA0", // bright
        ],
    )
}

/// The built-in default theme `nox` (SPEC §9).
pub fn builtin_nox() -> Theme {
    Theme { name: "nox".into(), light: Some(nox_light()), dark: Some(nox_dark()) }
}

macro_rules! bundled {
    ($($name:literal),+ $(,)?) => {
        const BUNDLED: &[(&str, &str)] = &[
            $(($name, include_str!(concat!("../../../themes/", $name, ".toml"))),)+
        ];
    };
}

bundled!(
    "1984",
    "ayu",
    "bluloco",
    "catppuccin",
    "everforest",
    "flexoki",
    "github",
    "gruvbox",
    "kanagawa",
    "modus",
    "nord",
    "rose-pine",
    "solarized",
    "tokyo-night",
    "vitesse",
    "white",
    "zenwritten",
);

/// Names of the themes compiled into the binary (not including `nox`).
pub fn bundled_theme_names() -> &'static [&'static str] {
    static NAMES: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| BUNDLED.iter().map(|(n, _)| *n).collect())
}

/// The TOML source of a bundled theme.
pub fn bundled_theme_source(name: &str) -> Option<&'static str> {
    BUNDLED.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

/// Names reserved for `ui.theme` / `ui.mode` and never resolved as theme files.
pub const RESERVED_NAMES: &[&str] = &["nox", "system", "light", "dark"];

/// Theme file names: letters, digits, `_`, `-`.
pub fn is_valid_theme_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// All selectable theme names: `nox`, bundled ones, then user files in `user_themes_dir`
/// (sorted, deduplicated).
pub fn available_theme_names(user_themes_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::iter::once("nox".to_string())
        .chain(bundled_theme_names().iter().map(|s| s.to_string()))
        .collect();
    if let Ok(entries) = std::fs::read_dir(user_themes_dir) {
        let mut user: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let p = e.path();
                (p.extension()? == "toml").then(|| p.file_stem()?.to_str().map(str::to_string))?
            })
            .filter(|n| is_valid_theme_name(n) && !RESERVED_NAMES.contains(&n.as_str()))
            .collect();
        user.sort();
        for n in user {
            if !names.contains(&n) {
                names.push(n);
            }
        }
    }
    names
}

/// Resolves `ui.theme`: `nox` is built in; otherwise `~/.nox/themes/<name>.toml` overrides a
/// bundled theme of the same name. Anything unusable falls back to `nox` with a notice; invalid
/// keys inside a theme are reported as notices too.
pub fn load_theme(name: &str, user_themes_dir: &Path) -> (Theme, Vec<String>) {
    if name == "nox" {
        return (builtin_nox(), Vec::new());
    }
    let mut notices = Vec::new();
    if !is_valid_theme_name(name) || RESERVED_NAMES.contains(&name) {
        notices.push(format!(
            "theme \"{name}\": invalid theme name (letters, digits, _ and - only; nox, system, light and dark are reserved); using nox"
        ));
        return (builtin_nox(), notices);
    }
    let user_path = user_themes_dir.join(format!("{name}.toml"));
    if user_path.is_file() {
        match std::fs::read_to_string(&user_path) {
            Ok(src) => match parse_theme(name, &src) {
                Ok((theme, n)) => {
                    notices.extend(n);
                    return (theme, notices);
                }
                Err(e) => notices.push(format!("theme \"{name}\" ({}): {e}", user_path.display())),
            },
            Err(e) => notices.push(format!("theme \"{name}\": cannot read {}: {e}", user_path.display())),
        }
    }
    if let Some(src) = bundled_theme_source(name) {
        match parse_theme(name, src) {
            Ok((theme, n)) => {
                notices.extend(n);
                return (theme, notices);
            }
            Err(e) => notices.push(format!("theme \"{name}\": {e}")),
        }
    } else if !user_path.is_file() {
        notices.push(format!("theme \"{name}\" not found; using nox"));
        return (builtin_nox(), notices);
    }
    notices.push(format!("theme \"{name}\": using nox"));
    (builtin_nox(), notices)
}

/// Parses a theme file. `Err` for a TOML syntax error or a file without `[dark]` / `[light]`;
/// otherwise missing keys fall back to the `nox` palette of the same variant and invalid ones
/// are reported in the notices.
pub fn parse_theme(name: &str, src: &str) -> Result<(Theme, Vec<String>), String> {
    let table: toml::Table = src.parse().map_err(|e: toml::de::Error| {
        let line = e
            .span()
            .map(|sp| src.as_bytes()[..sp.start.min(src.len())].iter().filter(|&&b| b == b'\n').count() + 1);
        match line {
            Some(l) => format!("line {l}: {}", e.message().trim()),
            None => e.message().trim().to_string(),
        }
    })?;
    let mut notices = Vec::new();
    let mut theme = Theme { name: name.to_string(), light: None, dark: None };
    for (key, value) in &table {
        let dark = match key.as_str() {
            "dark" => true,
            "light" => false,
            _ => {
                notices.push(format!("theme \"{name}\": unknown key \"{key}\"; ignored"));
                continue;
            }
        };
        let Some(t) = value.as_table() else {
            notices.push(format!("theme \"{name}\": [{key}] must be a table; ignored"));
            continue;
        };
        let v = parse_variant(name, key, t, dark, &mut notices);
        if dark {
            theme.dark = Some(v);
        } else {
            theme.light = Some(v);
        }
    }
    if theme.dark.is_none() && theme.light.is_none() {
        return Err("no [dark] or [light] section".into());
    }
    Ok((theme, notices))
}

fn parse_variant(
    name: &str,
    section: &str,
    t: &toml::Table,
    dark: bool,
    notices: &mut Vec<String>,
) -> ThemeVariant {
    let mut v = if dark { nox_dark() } else { nox_light() };
    let color = |key: &str, val: &toml::Value, notices: &mut Vec<String>| -> Option<Rgba> {
        match val.as_str().and_then(Rgba::from_hex) {
            Some(c) => Some(c),
            None => {
                notices.push(format!(
                    "theme \"{name}\": [{section}] {key} = {val} is not a #rgb / #rrggbb / #rrggbbaa color; ignored"
                ));
                None
            }
        }
    };
    let (mut bg_elevated, mut caret_text) = (None, None);
    let mut bg_set = false;
    for (key, val) in t {
        let slot = match key.as_str() {
            "bg" => {
                bg_set = true;
                &mut v.bg
            }
            "bg-sidebar" => &mut v.bg_sidebar,
            "text" => &mut v.text,
            "text-muted" => &mut v.text_muted,
            "border" => &mut v.border,
            "accent" => &mut v.accent,
            "selection" => &mut v.selection,
            "caret" => &mut v.caret,
            "bg-elevated" => {
                if let Some(c) = color(key, val, notices) {
                    bg_elevated = Some(c);
                }
                continue;
            }
            "caret-text" => {
                if let Some(c) = color(key, val, notices) {
                    caret_text = Some(c);
                }
                continue;
            }
            "ansi" => {
                match val.as_table() {
                    Some(a) => parse_ansi(name, section, a, &mut v.ansi, notices),
                    None => {
                        notices.push(format!("theme \"{name}\": [{section}] ansi must be a table; ignored"))
                    }
                }
                continue;
            }
            k if k.starts_with("hl-") => continue, // kayet syntax colors: accepted, unused
            _ => {
                notices.push(format!("theme \"{name}\": [{section}] unknown key \"{key}\"; ignored"));
                continue;
            }
        };
        if let Some(c) = color(key, val, notices) {
            *slot = c;
        } else if key == "bg" {
            bg_set = false;
        }
    }
    v.bg_elevated = match bg_elevated {
        Some(c) => c,
        None if bg_set => derive_bg_elevated(v.bg, dark),
        None => v.bg_elevated,
    };
    v.caret_text = caret_text.unwrap_or(v.bg);
    v
}

fn parse_ansi(name: &str, section: &str, t: &toml::Table, ansi: &mut [Rgba; 16], notices: &mut Vec<String>) {
    for (key, val) in t {
        let Some(i) = ANSI_NAMES.iter().position(|n| n == key) else {
            notices.push(format!("theme \"{name}\": [{section}.ansi] unknown key \"{key}\"; ignored"));
            continue;
        };
        match val.as_str().and_then(Rgba::from_hex) {
            Some(c) => ansi[i] = c,
            None => notices.push(format!(
                "theme \"{name}\": [{section}.ansi] {key} = {val} is not a valid color; ignored"
            )),
        }
    }
}

/// The xterm 256-color palette: the variant's 16 ANSI colors, the 6×6×6 cube and the
/// 24-step grayscale ramp.
pub fn palette256(v: &ThemeVariant) -> [Rgba; 256] {
    let mut p = [Rgba::BLACK; 256];
    p[..16].copy_from_slice(&v.ansi);
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    for i in 0..216 {
        let (r, g, b) = (i / 36, (i / 6) % 6, i % 6);
        p[16 + i] = Rgba::from_u8(LEVELS[r], LEVELS[g], LEVELS[b], 255);
    }
    for i in 0..24 {
        let l = (8 + 10 * i) as u8;
        p[232 + i] = Rgba::from_u8(l, l, l, 255);
    }
    p
}

#[cfg(test)]
mod tests;
