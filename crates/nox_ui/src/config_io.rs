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

/// Remove several keys (each as path segments) in one write and apply once.
pub fn remove_values(cx: &mut App, keys: &[&[&str]]) {
    let path = nox_config::config_path();
    match nox_config::remove_values_at(&path, keys) {
        Ok(()) => reload_from_disk(cx),
        Err(e) => crate::toast::show(cx, format!("Couldn't update config.toml: {e}")),
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
        for n in crate::actions::bind_keys(cx, &config.keys) {
            crate::toast::show(cx, format!("config.toml: {} — {}", n.key, n.message));
        }
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

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Mutex, MutexGuard};

    use gpui::TestAppContext;

    use super::*;
    use crate::toast::messages;

    /// `config.toml` lives in the process-wide test `NOX_HOME`; tests that write it hold this
    /// lock and start from an empty file.
    static CONFIG_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) fn fresh_config() -> MutexGuard<'static, ()> {
        let guard = CONFIG_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::test_support::nox_home();
        let path = nox_config::config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
        guard
    }

    pub(crate) fn config_text() -> String {
        std::fs::read_to_string(nox_config::config_path()).unwrap_or_default()
    }

    #[gpui::test]
    fn set_value_writes_surgically_and_applies(cx: &mut TestAppContext) {
        let _g = fresh_config();
        std::fs::write(nox_config::config_path(), "# mine\n[font]\nsize = 13 # keep\n").unwrap();
        crate::test_support::init(cx);
        cx.update(|cx| {
            set_value(cx, "font.size", 16);
            set_value(cx, "tabs.position", "bottom");
            let s = Settings::global(cx);
            assert!((s.term_font.size - 16.0).abs() < f32::EPSILON);
            assert_eq!(s.config.tabs.position, nox_config::TabsPosition::Bottom);
        });
        let text = config_text();
        assert!(text.starts_with("# mine\n"), "{text}");
        assert!(text.contains("size = 16 # keep"), "{text}");
        assert!(text.contains("position = \"bottom\""), "{text}");

        cx.update(|cx| {
            remove_value(cx, "font.size");
            assert!((Settings::global(cx).term_font.size - 14.0).abs() < f32::EPSILON);
            remove_values(cx, &[&["tabs", "position"]]);
            assert_eq!(Settings::global(cx).config.tabs.position, nox_config::TabsPosition::Top);
        });
        assert!(!config_text().contains("size ="));
    }

    #[gpui::test]
    fn broken_file_is_never_overwritten(cx: &mut TestAppContext) {
        let _g = fresh_config();
        let broken = "[font\nsize = ";
        std::fs::write(nox_config::config_path(), broken).unwrap();
        crate::test_support::init(cx);
        cx.update(|cx| {
            set_value(cx, "font.size", 20);
            remove_value(cx, "font.size");
            remove_values(cx, &[&["font", "size"]]);
            let m = messages(cx);
            assert!(m.iter().any(|m| m.starts_with("Couldn't write font.size")), "{m:?}");
            assert!(m.iter().any(|m| m.starts_with("Couldn't reset font.size")), "{m:?}");
            assert!(m.iter().any(|m| m.starts_with("Couldn't update config.toml")), "{m:?}");
        });
        assert_eq!(config_text(), broken);

        // Reloading a broken file records the error (the Settings window shows a banner) …
        cx.update(|cx| {
            reload_from_disk(cx);
            assert!(cx.global::<ConfigError>().0.is_some());
            assert!(messages(cx).iter().any(|m| m.starts_with("config.toml: ")));
        });
        // … and a good file clears it.
        std::fs::write(nox_config::config_path(), "[font]\nsize = 18\n").unwrap();
        cx.update(|cx| {
            reload_from_disk(cx);
            assert!(cx.global::<ConfigError>().0.is_none());
            assert!((Settings::global(cx).term_font.size - 18.0).abs() < f32::EPSILON);
        });
    }

    #[gpui::test]
    fn apply_installs_theme_icons_and_keys(cx: &mut TestAppContext) {
        crate::test_support::init(cx);
        let mut config = crate::test_support::config();
        config.ui.theme = "github".into();
        config.ui.mode = nox_config::Mode::Light;
        config.tabs.icons = false;
        config.keys.insert("new_tab".into(), nox_config::KeyBinding::Shortcut("cmd-y".into()));
        let notice = nox_config::Notice { key: "font.size".into(), message: "expected a number".into() };
        cx.update(|cx| {
            cx.default_global::<ConfigError>().0 = Some("old".into());
            apply(cx, config, &[notice]);
            assert!(cx.global::<ConfigError>().0.is_none());
            assert_eq!(crate::theme::Theme::global(cx).name, "github");
            assert!(!crate::theme::Theme::global(cx).dark);
            assert_eq!(cx.global::<crate::theme::ThemeState>().mode, nox_config::Mode::Light);
            assert!(!cx.global::<crate::icons::IconRegistry>().enabled);
            assert!(messages(cx).contains(&"config.toml: font.size — expected a number".to_string()));
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            let new_tab: Vec<_> = keymap.bindings_for_action(&crate::actions::NewTab).collect();
            assert_eq!(new_tab.len(), 1);
            assert_eq!(new_tab[0].keystrokes()[0].inner().key, "y");
        });
    }

    #[gpui::test]
    fn apply_falls_back_on_an_invalid_shortcut(cx: &mut TestAppContext) {
        crate::test_support::init(cx);
        let mut config = crate::test_support::config();
        config.keys.insert("new_tab".into(), nox_config::KeyBinding::Shortcut("hyper-k".into()));
        cx.update(|cx| {
            // A live reload with a typo in [keys] must not panic.
            apply(cx, config, &[]);
            let m = messages(cx);
            assert!(
                m.contains(
                    &"config.toml: keys.new_tab — invalid shortcut \"hyper-k\", using the default".into()
                ),
                "{m:?}"
            );
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            let new_tab: Vec<_> = keymap.bindings_for_action(&crate::actions::NewTab).collect();
            assert_eq!(new_tab.len(), 1);
            assert_eq!(new_tab[0].keystrokes()[0].inner().key, "t");
        });
    }

    #[gpui::test]
    fn apply_reports_an_unknown_theme(cx: &mut TestAppContext) {
        crate::test_support::init(cx);
        let mut config = crate::test_support::config();
        config.ui.theme = "does-not-exist".into();
        cx.update(|cx| {
            apply(cx, config, &[]);
            assert_eq!(crate::theme::Theme::global(cx).name, "nox");
            assert!(messages(cx).iter().any(|m| m.contains("does-not-exist")), "{:?}", messages(cx));
        });
    }
}
