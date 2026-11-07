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
