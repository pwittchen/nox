//! Process icons (SPEC §4.6): the detection table as a global, and rendering helpers.

use std::collections::BTreeMap;
use std::path::PathBuf;

use gpui::{AnyElement, App, Global, Hsla, IntoElement, Styled, px, svg};
use nox_agents::{Icon, IconTable};
use nox_config::Config;

pub struct IconRegistry {
    pub table: IconTable,
    pub login_shell: String,
    pub enabled: bool,
}

impl Global for IconRegistry {}

impl IconRegistry {
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        let mut user_icons: BTreeMap<String, String> = config.icons.clone();
        let mut matches: Vec<(String, Vec<String>)> = Vec::new();
        for (name, def) in &config.agents.defs {
            let mut exes = def.match_.clone();
            exes.push(name.clone());
            if let Some(first) = def.command.split_whitespace().next() {
                let base = first.rsplit('/').next().unwrap_or(first).to_string();
                if !exes.contains(&base) {
                    exes.push(base);
                }
            }
            matches.push((name.clone(), exes));
            if let Some(icon) = &def.icon {
                user_icons.insert(name.clone(), icon.clone());
            }
        }
        Self {
            table: IconTable::new(&user_icons, &matches),
            login_shell: nox_terminal::process::login_shell(),
            enabled: config.tabs.icons,
        }
    }

    /// A user-set icon (Set Icon…): a built-in name or a path.
    #[must_use]
    pub fn custom_icon(&self, name: &str) -> Icon {
        if name.starts_with('/') || name.starts_with('~') {
            return Icon::Custom(expand_tilde(name));
        }
        match nox_agents::builtin_icon_names().iter().find(|n| **n == name) {
            Some(n) => Icon::Builtin(n),
            None => Icon::TERMINAL,
        }
    }
}

#[must_use]
pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(p)
}

#[must_use]
pub fn icon_path(icon: &Icon) -> String {
    match icon {
        Icon::Builtin(name) => format!("icons/process/{name}.svg"),
        Icon::Custom(p) => p.to_string_lossy().into_owned(),
    }
}

/// The icon tinted with its brand color blended towards `base` (low saturation, readable in
/// both light and dark mode), or `base` when the brand has no tint.
#[must_use]
pub fn icon_color(icon: &Icon, base: Hsla) -> Hsla {
    if let Icon::Builtin(name) = icon
        && let Some(rgb) = nox_agents::brand_tint(name)
    {
        let c: Hsla = gpui::rgb(rgb).into();
        let mut c = c;
        c.s = (c.s * 0.55).min(0.6);
        c.l = (c.l * 0.5 + base.l * 0.5).clamp(0.35, 0.75);
        c.a = base.a;
        return c;
    }
    base
}

pub fn icon_element(icon: &Icon, size: f32, base: Hsla, cx: &App) -> AnyElement {
    let enabled = cx.try_global::<IconRegistry>().is_none_or(|r| r.enabled);
    let icon = if enabled { icon.clone() } else { Icon::TERMINAL };
    let color = icon_color(&icon, base);
    svg().path(icon_path(&icon)).size(px(size)).flex_none().text_color(color).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nox_agents::ProcessInfo;
    use nox_config::AgentDef;

    fn config_with_agent() -> Config {
        let mut c = Config::default();
        c.agents.defs.insert(
            "myagent".into(),
            AgentDef {
                command: "/opt/bin/myagent-cli --fast".into(),
                resume: None,
                resume_last: None,
                match_: vec!["ma".into()],
                icon: Some("claude".into()),
            },
        );
        c.icons.insert("htop".into(), "codex".into());
        c
    }

    #[test]
    fn registry_from_config_maps_agents_and_user_icons() {
        let r = IconRegistry::from_config(&config_with_agent());
        assert!(r.enabled);
        let icon =
            |exe: &str| r.table.icon_for(Some(&ProcessInfo::new(exe, Vec::<String>::new())), None, "zsh");
        // The agent's name, its `match` executables and its command's basename all match.
        assert_eq!(icon("myagent"), Icon::Builtin("claude"));
        assert_eq!(icon("ma"), Icon::Builtin("claude"));
        assert_eq!(icon("myagent-cli"), Icon::Builtin("claude"));
        // `[icons]` entries.
        assert_eq!(icon("htop"), Icon::Builtin("codex"));
        // The login shell is the plain terminal icon.
        assert_eq!(icon("zsh"), Icon::TERMINAL);

        let mut c = Config::default();
        c.tabs.icons = false;
        assert!(!IconRegistry::from_config(&c).enabled);
    }

    #[test]
    fn custom_icons() {
        let r = IconRegistry::from_config(&Config::default());
        assert_eq!(r.custom_icon("claude"), Icon::Builtin("claude"));
        assert_eq!(r.custom_icon("not-an-icon"), Icon::TERMINAL);
        assert_eq!(r.custom_icon("/tmp/x.svg"), Icon::Custom(PathBuf::from("/tmp/x.svg")));
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
        assert_eq!(r.custom_icon("~/i/x.svg"), Icon::Custom(home.join("i/x.svg")));
    }

    #[test]
    fn tilde_expansion() {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
        assert_eq!(expand_tilde("~/a/b"), home.join("a/b"));
        assert_eq!(expand_tilde("/abs"), PathBuf::from("/abs"));
        assert_eq!(expand_tilde("~user/x"), PathBuf::from("~user/x"));
    }

    #[test]
    fn paths_for_icons() {
        assert_eq!(icon_path(&Icon::Builtin("claude")), "icons/process/claude.svg");
        assert_eq!(icon_path(&Icon::Custom(PathBuf::from("/x/y.svg"))), "/x/y.svg");
        // Every built-in icon has its SVG compiled in.
        for name in nox_agents::builtin_icon_names() {
            let p = icon_path(&Icon::Builtin(name));
            assert!(gpui::AssetSource::load(&crate::assets::Assets, &p).unwrap().is_some(), "{p}");
        }
    }

    #[test]
    fn brand_tint_is_muted_and_bounded() {
        let base = gpui::hsla(0., 0., 0.9, 0.7);
        let c = icon_color(&Icon::Builtin("claude"), base);
        assert_ne!(c, base);
        assert!(c.s <= 0.6);
        assert!((0.35..=0.75).contains(&c.l));
        assert!((c.a - 0.7).abs() < 1e-6, "keeps the base alpha");
        // No tint: the base color as is.
        assert_eq!(icon_color(&Icon::TERMINAL, base), base);
        assert_eq!(icon_color(&Icon::Custom(PathBuf::from("/a.svg")), base), base);
    }

    #[gpui::test]
    fn icon_element_respects_disabled_icons(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            // Without a registry (and with one that's enabled) it builds.
            let _ = icon_element(&Icon::Builtin("claude"), 14., gpui::white(), cx);
            let mut c = Config::default();
            c.tabs.icons = false;
            cx.set_global(IconRegistry::from_config(&c));
            let _ = icon_element(&Icon::Builtin("claude"), 14., gpui::white(), cx);
        });
    }
}
