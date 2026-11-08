//! The `Theme` global (SPEC §9): the resolved palette as GPUI colors, the 16 ANSI colors and
//! the precomputed 256-color table. Replacing it refreshes every window, live.

use std::sync::Arc;

use gpui::{App, Global, Hsla, Rgba, WindowAppearance};
use nox_config::Mode;
use nox_theme::{Rgba as TRgba, Theme as ThemeFile, ThemeVariant};

#[derive(Clone)]
pub struct Theme {
    pub name: String,
    pub dark: bool,
    pub single_variant: bool,
    pub bg: Hsla,
    pub bg_sidebar: Hsla,
    pub bg_elevated: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub border: Hsla,
    pub accent: Hsla,
    pub selection: Hsla,
    pub caret: Hsla,
    pub caret_text: Hsla,
    /// 256 indexed colors (16 ANSI + cube + grays).
    pub palette: Arc<[Hsla; 256]>,
    /// The same as raw sRGB, for answering terminal color queries and contrast math.
    pub palette_rgb: Arc<[TRgba; 256]>,
    pub raw: ThemeVariant,
}

impl Global for Theme {}

#[must_use]
pub fn to_hsla(c: TRgba) -> Hsla {
    Rgba { r: c.r, g: c.g, b: c.b, a: c.a }.into()
}

impl Theme {
    #[must_use]
    pub fn from_variant(name: &str, v: &ThemeVariant, dark: bool, single_variant: bool) -> Self {
        let rgb = nox_theme::palette256(v);
        let mut pal = [Hsla::default(); 256];
        for (i, c) in rgb.iter().enumerate() {
            pal[i] = to_hsla(*c);
        }
        Self {
            name: name.to_string(),
            dark,
            single_variant,
            bg: to_hsla(v.bg),
            bg_sidebar: to_hsla(v.bg_sidebar),
            bg_elevated: to_hsla(v.bg_elevated),
            text: to_hsla(v.text),
            text_muted: to_hsla(v.text_muted),
            border: to_hsla(v.border),
            accent: to_hsla(v.accent),
            selection: to_hsla(v.selection),
            caret: to_hsla(v.caret),
            caret_text: to_hsla(v.caret_text),
            palette: Arc::new(pal),
            palette_rgb: Arc::new(rgb),
            raw: *v,
        }
    }

    /// One of the ANSI color names (`"cyan"`, `"bright-blue"`) → color; used for workspace
    /// swatches so they follow the theme.
    #[must_use]
    pub fn named(&self, name: &str) -> Option<Hsla> {
        let idx = nox_theme::ANSI_NAMES.iter().position(|n| *n == name)?;
        Some(self.palette[idx])
    }

    #[must_use]
    pub fn ansi(&self, i: usize) -> Hsla {
        self.palette[i.min(255)]
    }

    pub fn global(cx: &App) -> &Theme {
        cx.global::<Theme>()
    }
}

/// Theme selection state: the loaded file plus the mode; the active `Theme` global is derived
/// from it and the window appearance.
#[derive(Clone)]
pub struct ThemeState {
    pub file: ThemeFile,
    pub mode: Mode,
    pub notices: Vec<String>,
}

impl Global for ThemeState {}

#[must_use]
pub fn is_dark(mode: Mode, appearance: WindowAppearance) -> bool {
    match mode {
        Mode::Dark => true,
        Mode::Light => false,
        Mode::System => matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark),
    }
}

#[must_use]
pub fn load_theme_file(name: &str) -> (ThemeFile, Vec<String>) {
    nox_theme::load_theme(name, &nox_config::themes_dir())
}

/// Resolve and install the `Theme` global. Returns true if it changed.
pub fn apply(cx: &mut App, state: &ThemeState, appearance: WindowAppearance) -> bool {
    let dark = is_dark(state.mode, appearance);
    let single = state.file.is_single_variant();
    let variant = state.file.variant(dark);
    let actual_dark = if single { state.file.dark.is_some() } else { dark };
    let theme = Theme::from_variant(&state.file.name, variant, actual_dark, single);
    let changed = !cx.has_global::<Theme>()
        || cx.global::<Theme>().name != theme.name
        || cx.global::<Theme>().dark != theme.dark
        || cx.global::<Theme>().raw != theme.raw;
    cx.set_global(theme);
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn state(name: &str, mode: Mode) -> ThemeState {
        // User themes are looked up in the scratch home, never in ~/.nox.
        crate::test_support::nox_home();
        let (file, notices) = load_theme_file(name);
        ThemeState { file, mode, notices }
    }

    #[test]
    fn mode_resolution() {
        assert!(is_dark(Mode::Dark, WindowAppearance::Light));
        assert!(!is_dark(Mode::Light, WindowAppearance::Dark));
        assert!(is_dark(Mode::System, WindowAppearance::Dark));
        assert!(is_dark(Mode::System, WindowAppearance::VibrantDark));
        assert!(!is_dark(Mode::System, WindowAppearance::Light));
        assert!(!is_dark(Mode::System, WindowAppearance::VibrantLight));
    }

    #[test]
    fn variant_colors_and_palette() {
        crate::test_support::nox_home();
        let (file, notices) = load_theme_file("nox");
        assert!(notices.is_empty());
        let v = file.variant(true);
        let t = Theme::from_variant("nox", v, true, false);
        assert_eq!(t.name, "nox");
        assert_eq!(t.bg, to_hsla(v.bg));
        assert_eq!(t.accent, to_hsla(v.accent));
        assert_eq!(t.palette_rgb[1], v.ansi[1]);
        assert_eq!(t.ansi(1), to_hsla(v.ansi[1]));
        // Out-of-range indices clamp to the last gray instead of panicking.
        assert_eq!(t.ansi(10_000), t.palette[255]);
        assert_eq!(t.named("red"), Some(t.palette[1]));
        assert_eq!(t.named("bright-blue"), Some(t.palette[12]));
        assert_eq!(t.named("chartreuse"), None);
    }

    #[test]
    fn to_hsla_keeps_alpha() {
        let c = to_hsla(TRgba { r: 1.0, g: 0.0, b: 0.0, a: 0.5 });
        assert!((c.a - 0.5).abs() < 1e-6);
        assert!(c.h.abs() < 1e-6);
    }

    #[test]
    fn unknown_theme_falls_back_with_notice() {
        let s = state("no-such-theme-xyz", Mode::Dark);
        assert_eq!(s.file.name, "nox");
        assert!(!s.notices.is_empty());
    }

    #[gpui::test]
    fn apply_reports_changes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let dark = state("nox", Mode::System);
            assert!(apply(cx, &dark, WindowAppearance::Dark), "first install is a change");
            assert!(Theme::global(cx).dark);
            assert!(!apply(cx, &dark, WindowAppearance::Dark), "same theme, same appearance");
            assert!(apply(cx, &dark, WindowAppearance::Light), "system mode follows the appearance");
            assert!(!Theme::global(cx).dark);
            assert!(!Theme::global(cx).single_variant);

            let forced = state("nox", Mode::Dark);
            assert!(apply(cx, &forced, WindowAppearance::Light));
            assert!(Theme::global(cx).dark);

            let other = state("nord", Mode::Dark);
            assert!(apply(cx, &other, WindowAppearance::Light), "another theme is a change");
            assert_eq!(Theme::global(cx).name, "nord");
        });
    }

    #[gpui::test]
    fn single_variant_theme_ignores_mode(cx: &mut TestAppContext) {
        crate::test_support::nox_home();
        let (nox, _) = load_theme_file("nox");
        let file = ThemeFile { name: "only-dark".into(), light: None, dark: nox.dark };
        let s = ThemeState { file, mode: Mode::Light, notices: vec![] };
        cx.update(|cx| {
            apply(cx, &s, WindowAppearance::Light);
            let t = Theme::global(cx);
            assert!(t.single_variant);
            assert!(t.dark, "a dark-only theme stays dark in light mode");
            assert_eq!(t.raw, nox.dark.unwrap());
        });
    }
}
