//! Writing settings back to `config.toml` (SPEC §11) and applying configuration changes live.

use std::sync::Arc;

use gpui::App;
use nox_config::{Config, toml_edit};

use crate::settings_state::Settings;

/// Write one key surgically (comments and order preserved) and apply the result at once.
/// Never writes a file that fails to parse.
pub fn set_value(cx: &mut App, key: &str, value: impl Into<toml_edit::Value>) {
    let path = nox_config::config_path();
    match nox_config::set_value(&path, key, value) {
        Ok(()) => reload_from_disk(cx),
        Err(e) => crate::toast::show(cx, format!("Couldn't write {key} to config.toml: {e}")),
    }
}

/// Remove a key (it falls back to its default) and apply.
pub fn remove_value(cx: &mut App, key: &str) {
    let path = nox_config::config_path();
    match nox_config::remove_value(&path, key) {
        Ok(()) => reload_from_disk(cx),
        Err(e) => crate::toast::show(cx, format!("Couldn't reset {key}: {e}")),
    }
}

pub fn reload_from_disk(cx: &mut App) {
    match nox_config::load_from_path(&nox_config::config_path()) {
        Ok((config, notices)) => apply(cx, config, &notices),
        Err(e) => {
            cx.default_global::<ConfigError>().0 = Some(e.to_string());
            crate::toast::show(cx, format!("config.toml: {e}"));
        }
    }
}

/// The last parse error of `config.toml` (the Settings window shows a banner while set).
#[derive(Default)]
pub struct ConfigError(pub Option<String>);
impl gpui::Global for ConfigError {}

/// Install a new configuration: settings, theme, icons, key bindings. Panes observe the
/// `Settings` global for their own options.
pub fn apply(cx: &mut App, config: Config, notices: &[nox_config::Notice]) {
    cx.default_global::<ConfigError>().0 = None;
    for n in notices {
        crate::toast::show(cx, format!("config.toml: {} — {}", n.key, n.message));
    }
    let old = cx.try_global::<Settings>().map(|s| s.config.clone());
    let a11y = cx.try_global::<Settings>().map(|s| s.a11y).unwrap_or_default();
    let changed = |f: &dyn Fn(&Config) -> String| old.as_ref().is_none_or(|o| f(o) != f(&config));

    if changed(&|c| format!("{:?}", c.keys)) {
        crate::actions::bind_keys(cx, &config.keys);
    }
    if changed(&|c| format!("{:?}{:?}{}", c.icons, c.agents.defs, c.tabs.icons)) {
        cx.set_global(crate::icons::IconRegistry::from_config(&config));
    }
    let theme_changed = changed(&|c| format!("{}{:?}", c.ui.theme, c.ui.mode));
    let font_changed = changed(&|c| format!("{}{}", c.font.family, c.ui.font_family));
    if theme_changed {
        let (file, theme_notices) = crate::theme::load_theme_file(&config.ui.theme);
        for n in &theme_notices {
            crate::toast::show(cx, n.clone());
        }
        cx.set_global(crate::theme::ThemeState { file, mode: config.ui.mode, notices: theme_notices });
    }
    if font_changed {
        crate::fonts::ensure_loaded(cx, &config);
    }
    cx.set_global(
        Settings { config: Arc::new(config), ..Settings::new(Config::default(), a11y) }.with_config(cx),
    );
    if theme_changed {
        let appearance = cx.window_appearance();
        let state = cx.global::<crate::theme::ThemeState>().clone();
        crate::theme::apply(cx, &state, appearance);
    }
    cx.refresh_windows();
}

impl Settings {
    /// Recompute derived values (fonts) after replacing `config`.
    fn with_config(self, _cx: &App) -> Self {
        let config = (*self.config).clone();
        Settings::new(config, self.a11y)
    }
}
