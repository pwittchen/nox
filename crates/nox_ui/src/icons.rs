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

pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(p)
}

pub fn icon_path(icon: &Icon) -> String {
    match icon {
        Icon::Builtin(name) => format!("icons/process/{name}.svg"),
        Icon::Custom(p) => p.to_string_lossy().into_owned(),
    }
}

/// The icon tinted with its brand color blended towards `base` (low saturation, readable in
/// both light and dark mode), or `base` when the brand has no tint.
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
