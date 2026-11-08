//! The Settings window (SPEC §5.9): a view over `config.toml`, never a separate store. Every
//! change is written to the file at once (surgically) and applied; edits made to the file
//! elsewhere show up here immediately. A file that fails to parse disables the controls.

use std::collections::BTreeMap;

use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable, Global, KeyDownEvent,
    SharedString, Subscription, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, div,
    point, prelude::*, px, size,
};
use nox_config::{Section, SettingKind, SettingMeta, toml_edit};

use crate::config_io::{ConfigError, remove_value, set_value};
use crate::settings_state::Settings;
use crate::text_input::{InputEvent, TextInput};
use crate::theme::Theme;

#[derive(Default)]
struct SettingsWindow(Option<WindowHandle<SettingsView>>);

/// Width of dropdown buttons and their lists.
const DROPDOWN_W: f32 = 240.;
impl Global for SettingsWindow {}

pub fn open_settings_window(cx: &mut App) {
    let existing = cx.try_global::<SettingsWindow>().and_then(|w| w.0);
    if crate::util::activate_if_open(existing, cx) {
        return;
    }
    let bounds = Bounds::centered(None, size(px(820.), px(620.)), cx);
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("nox Settings".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(12.), px(12.))),
        }),
        window_min_size: Some(size(px(620.), px(420.))),
        ..Default::default()
    };
    if let Ok(h) = cx.open_window(opts, |window, cx| cx.new(|cx| SettingsView::new(window, cx))) {
        cx.set_global(SettingsWindow(Some(h)));
    }
}

pub struct SettingsView {
    focus: FocusHandle,
    section: Section,
    search: Entity<TextInput>,
    inputs: BTreeMap<String, Entity<TextInput>>,
    /// The "add entry" name / value fields under each map editor (`[icons]`, `[shell.env]`).
    map_new: BTreeMap<&'static str, (Entity<TextInput>, Entity<TextInput>)>,
    recording: Option<String>,
    expanded: Option<String>,
    /// The About page (bottom of the sidebar) instead of a config section.
    about: bool,
    subs: Vec<Subscription>,
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn section_of_key(key: &str) -> Option<Section> {
    nox_config::find_setting(key).map(|m| m.section)
}

impl SettingsView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::new(cx, "Search settings"));
        let mut subs = vec![cx.subscribe(&search, |_, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Changed) {
                cx.notify();
            }
        })];
        subs.push(cx.observe_global::<Settings>(|this: &mut Self, cx| {
            this.sync_inputs(cx);
            cx.notify();
        }));
        subs.push(cx.observe_global::<ConfigError>(|_, cx| cx.notify()));
        subs.push(cx.observe_global::<Theme>(|_, cx| cx.notify()));
        let _ = window;
        Self {
            focus: cx.focus_handle(),
            section: Section::General,
            search,
            inputs: BTreeMap::new(),
            map_new: BTreeMap::new(),
            recording: None,
            expanded: None,
            about: false,
            subs,
        }
    }

    /// Keep text inputs in sync with external edits (unless focused).
    fn sync_inputs(&mut self, cx: &mut Context<Self>) {
        let config = Settings::global(cx).config.clone();
        for (key, input) in &self.inputs {
            let v = nox_config::get_value(&config, key).map(|v| value_text(&v)).unwrap_or_default();
            input.update(cx, |i, cx| {
                if i.text() != v {
                    i.set_text_cursor_end(v.clone(), cx);
                }
            });
        }
    }

    fn input_for(
        &mut self,
        key: &str,
        list: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        if let Some(i) = self.inputs.get(key) {
            return i.clone();
        }
        let config = Settings::global(cx).config.clone();
        let value = nox_config::get_value(&config, key).map(|v| value_text(&v)).unwrap_or_default();
        let input = cx.new(|cx| {
            let mut i = TextInput::new(cx, "");
            i.set_text_cursor_end(value, cx);
            i
        });
        let k = key.to_string();
        let sub =
            cx.subscribe_in(&input, window, move |_this: &mut Self, input, ev: &InputEvent, _window, cx| {
                if matches!(ev, InputEvent::Submit | InputEvent::Blur) {
                    let text = input.read(cx).text().to_string();
                    let config = Settings::global(cx).config.clone();
                    let current =
                        nox_config::get_value(&config, &k).map(|v| value_text(&v)).unwrap_or_default();
                    if text != current {
                        if list {
                            let mut arr = toml_edit::Array::new();
                            for part in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                                arr.push(part);
                            }
                            set_value(cx, &k, toml_edit::Value::Array(arr));
                        } else {
                            set_value(cx, &k, text.as_str());
                        }
                    }
                }
            });
        self.subs.push(sub);
        self.inputs.insert(key.to_string(), input.clone());
        input
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.expanded.is_some() && ev.keystroke.key == "escape" {
            self.expanded = None;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        let Some(name) = self.recording.clone() else { return };
        let ks = &ev.keystroke;
        if ks.key == "escape" && !ks.modifiers.modified() {
            self.recording = None;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if matches!(ks.key.as_str(), "shift" | "control" | "alt" | "platform" | "function" | "") {
            return;
        }
        let text = crate::util_keys::keystroke_to_config(ks);
        set_value(cx, &format!("keys.{name}"), text.as_str());
        self.recording = None;
        cx.stop_propagation();
        cx.notify();
    }

    fn row(
        &mut self,
        meta: &SettingMeta,
        disabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::global(cx).clone();
        let config = Settings::global(cx).config.clone();
        let key = meta.key;
        let value = nox_config::get_value(&config, key);
        let differs = nox_config::differs_from_default(&config, key);
        let control = self.control(meta, value.clone(), disabled, window, cx);
        let reset = (differs && !matches!(meta.kind, SettingKind::Map | SettingKind::Agents)).then(|| {
            div()
                .id(SharedString::from(format!("reset-{key}")))
                .text_size(px(11.))
                .text_color(theme.accent)
                .cursor_pointer()
                .child("reset")
                .on_click(move |_, _, cx| remove_value(cx, key))
        });
        // The shortcuts editor is a full-width list: its label goes above it, not beside it.
        let stacked = key == "keys";
        div()
            .flex()
            .flex_col()
            .gap_1()
            .py(px(10.))
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .when(stacked, |d| d.flex_col().gap_2())
                    .when(!stacked, |d| d.items_center().justify_between().gap_4())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w(px(0.))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .items_center()
                                    .child(div().text_size(px(13.)).text_color(theme.text).child(meta.label))
                                    .children(reset),
                            )
                            .child(
                                div().text_size(px(11.)).text_color(theme.text_muted).child(meta.description),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(theme.text_muted.opacity(0.8))
                                    .font_family(Settings::global(cx).term_font.family.clone())
                                    .child(key),
                            ),
                    )
                    .child(
                        div()
                            .when(stacked, gpui::Styled::w_full)
                            .when(!stacked, gpui::Styled::flex_none)
                            .when(disabled, |d| d.opacity(0.4))
                            .child(control),
                    ),
            )
            .children(Self::extra_for(key, cx))
            .into_any_element()
    }

    fn extra_for(key: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = Theme::global(cx).clone();
        match key {
            "app.check_for_updates" => {
                Some(div().flex().child(crate::updater::status_row(&theme, cx)).into_any_element())
            }
            "terminal.notifications" if nox_mac::notifications_denied() => Some(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .text_size(px(11.))
                    .text_color(theme.ansi(1))
                    .child("Notifications are turned off for nox in System Settings.")
                    .child(
                        button("open-notif", "Open System Settings", &theme)
                            .on_click(|_, _, _| nox_mac::open_notification_settings()),
                    )
                    .into_any_element(),
            ),
            "terminal.attention_sound_volume" => Some(
                div()
                    .child(button("preview-sound", "Preview", &theme).on_click(|_, _, cx| {
                        let vol = Settings::global(cx).config.terminal.attention_sound_volume as f32;
                        let path = crate::assets::resources_dir().join("sounds/attention.caf");
                        nox_mac::play_sound(&path, vol);
                    }))
                    .into_any_element(),
            ),
            _ => None,
        }
    }

    fn control(
        &mut self,
        meta: &SettingMeta,
        value: Option<toml::Value>,
        disabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::global(cx).clone();
        let key = meta.key;
        match &meta.kind {
            SettingKind::Bool => {
                let on = value.and_then(|v| v.as_bool()).unwrap_or(false);
                div()
                    .id(SharedString::from(format!("sw-{key}")))
                    .w(px(34.))
                    .h(px(20.))
                    .rounded(px(10.))
                    .p(px(2.))
                    .cursor_pointer()
                    .bg(if on { theme.accent } else { theme.text_muted.opacity(0.35) })
                    .flex()
                    .when(on, gpui::Styled::justify_end)
                    .child(div().size(px(16.)).rounded_full().bg(gpui::white()))
                    .when(!disabled, |d| d.on_click(move |_, _, cx| set_value(cx, key, !on)))
                    .into_any_element()
            }
            SettingKind::Enum(opts) if key == "ui.theme" || opts.len() > 5 => self.choice_list(
                key,
                opts.iter().map(std::string::ToString::to_string).collect(),
                value,
                disabled,
                cx,
            ),
            SettingKind::Enum(opts) => {
                let cur = value.and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
                // A track with inset, individually rounded segments: GPUI clips children to a
                // rectangle, so a fill flush with the rounded border would poke out at the corners.
                div()
                    .flex()
                    .gap(px(2.))
                    .p(px(2.))
                    .rounded(px(7.))
                    .border_1()
                    .border_color(theme.border)
                    .children(opts.iter().map(|o| {
                        let on = cur == *o;
                        let o2: &'static str = o;
                        div()
                            .id(SharedString::from(format!("seg-{key}-{o}")))
                            .px(px(8.))
                            .py(px(2.))
                            .rounded(px(5.))
                            .text_size(px(12.))
                            .cursor_pointer()
                            .text_color(if on { theme.bg } else { theme.text })
                            .when(on, |d| d.bg(theme.text))
                            .when(!on, |d| d.hover(|s| s.bg(theme.text.opacity(0.06))))
                            .when(!disabled, |d| d.on_click(move |_, _, cx| set_value(cx, key, o2)))
                            .child(*o)
                    }))
                    .into_any_element()
            }
            SettingKind::Int { min, max, step } => {
                let v = value.and_then(|v| v.as_integer()).unwrap_or(0);
                let (min, max, step) = (*min, *max, *step);
                stepper(key, v.to_string(), &theme, disabled, move |cx, dir| {
                    set_value(cx, key, (v + dir * step).clamp(min, max));
                })
            }
            SettingKind::Float { min, max, step } => {
                let v = value.and_then(|v| v.as_float().or(v.as_integer().map(|i| i as f64))).unwrap_or(0.0);
                let (min, max, step) = (*min, *max, *step);
                stepper(
                    key,
                    format!("{v:.2}").trim_end_matches('0').trim_end_matches('.').to_string(),
                    &theme,
                    disabled,
                    move |cx, dir| {
                        let nv = ((v + dir as f64 * step) * 1000.0).round() / 1000.0;
                        set_value(cx, key, nv.clamp(min, max));
                    },
                )
            }
            SettingKind::IntPair { min, max, .. } => {
                let arr: Vec<i64> = value
                    .and_then(|v| v.as_array().cloned())
                    .map_or_else(|| vec![0, 0], |a| a.iter().filter_map(toml::Value::as_integer).collect());
                let (a, b) = (arr.first().copied().unwrap_or(0), arr.get(1).copied().unwrap_or(0));
                let (min, max) = (*min, *max);
                let write = move |cx: &mut App, a: i64, b: i64| {
                    let mut arr = toml_edit::Array::new();
                    arr.push(a.clamp(min, max));
                    arr.push(b.clamp(min, max));
                    set_value(cx, key, toml_edit::Value::Array(arr));
                };
                div()
                    .flex()
                    .gap_2()
                    .child(stepper("pad-h", a.to_string(), &theme, disabled, move |cx, d| {
                        write(cx, a + d, b);
                    }))
                    .child(stepper("pad-v", b.to_string(), &theme, disabled, move |cx, d| {
                        write(cx, a, b + d);
                    }))
                    .into_any_element()
            }
            SettingKind::String if key == "font.family" => {
                let choices = crate::fonts::terminal_font_choices(cx);
                self.choice_list(key, choices, value, disabled, cx)
            }
            SettingKind::String if key == "ui.font_family" => {
                let choices = crate::fonts::ui_font_choices(cx);
                self.choice_list(key, choices, value, disabled, cx)
            }
            SettingKind::String if key == "ui.theme" => {
                let names = nox_theme::available_theme_names(&nox_config::themes_dir());
                self.choice_list(key, names, value, disabled, cx)
            }
            SettingKind::String | SettingKind::StringList => {
                let list = matches!(meta.kind, SettingKind::StringList);
                let input = self.input_for(key, list, window, cx);
                div()
                    .w(px(220.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.bg)
                    .text_size(px(12.))
                    .child(input)
                    .into_any_element()
            }
            SettingKind::Map => self.map_editor(key, value, window, cx),
            SettingKind::Agents => Self::agents_editor(cx),
        }
    }

    fn choice_list(
        &mut self,
        key: &'static str,
        choices: Vec<String>,
        value: Option<toml::Value>,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::global(cx).clone();
        let cur = value.and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        let expanded = self.expanded.as_deref() == Some(key);
        let is_font = key == "font.family" || key == "ui.font_family";
        let head = div()
            .id(SharedString::from(format!("choice-{key}")))
            .w(px(DROPDOWN_W))
            .h(px(26.))
            .px(px(8.))
            .gap_2()
            .items_center()
            .rounded(px(6.))
            .border_1()
            .border_color(if expanded { theme.accent } else { theme.border })
            .bg(theme.bg)
            .text_size(px(12.))
            .flex()
            .cursor_pointer()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .when(is_font && cur != "system" && !cur.is_empty(), |d| {
                        d.font_family(SharedString::from(cur.clone()))
                    })
                    .child(cur.clone()),
            )
            .child(
                gpui::svg()
                    .path("icons/ui/chevron-down.svg")
                    .size(px(12.))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .when(!disabled, |d| {
                d.on_click(cx.listener(move |this, _, _, cx| {
                    let open = this.expanded.as_deref() != Some(key);
                    // Show each font in its own face: register the bundled families first.
                    if open && key == "font.family" {
                        for (f, _) in crate::assets::BUNDLED_TERMINAL_FONTS {
                            crate::fonts::load_family(cx, f);
                        }
                    } else if open && key == "ui.font_family" {
                        for (f, _) in crate::assets::BUNDLED_UI_FONTS {
                            crate::fonts::load_family(cx, f);
                        }
                    }
                    this.expanded = open.then(|| key.to_string());
                    cx.notify();
                }))
            });
        // The list floats over the form (it never pushes the layout) and stays inside the window.
        let list = expanded.then(|| {
            let selected = choices.iter().position(|c| *c == cur);
            let rows = choices.into_iter().enumerate().map(|(i, c)| {
                let on = Some(i) == selected;
                let c2 = c.clone();
                div()
                    .id(SharedString::from(format!("choice-{key}-{i}")))
                    .h(px(26.))
                    .px(px(8.))
                    .mx(px(4.))
                    .rounded(px(5.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(theme.accent.opacity(0.18)))
                    .hover(|s| s.bg(theme.text.opacity(0.07)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if key == "font.family" || key == "ui.font_family" {
                            crate::fonts::load_family(cx, &c2);
                        }
                        set_value(cx, key, c2.as_str());
                        this.expanded = None;
                        cx.notify();
                    }))
                    .child(div().w(px(12.)).flex_none().text_color(theme.accent).child(if on {
                        "✓"
                    } else {
                        ""
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .when(is_font && c != "system", |d| d.font_family(SharedString::from(c.clone())))
                            .child(c),
                    )
            });
            gpui::deferred(
                gpui::anchored()
                    .anchor(gpui::Corner::TopLeft)
                    .offset(gpui::point(px(0.), px(30.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .id(SharedString::from(format!("choices-{key}")))
                            .occlude()
                            .w(px(DROPDOWN_W))
                            .max_h(px(260.))
                            .py(px(4.))
                            .overflow_y_scroll()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.bg_elevated)
                            .shadow_lg()
                            .text_color(theme.text)
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.expanded = None;
                                cx.notify();
                            }))
                            .children(rows),
                    ),
            )
            .with_priority(1)
        });
        div().flex().flex_col().child(head).children(list).into_any_element()
    }

    /// The name / value inputs of a map editor's "add" row, created on first use. Enter in the
    /// name moves to the value, Enter in the value adds the entry.
    fn map_new_inputs(
        &mut self,
        key: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<TextInput>, Entity<TextInput>) {
        if let Some(pair) = self.map_new.get(key) {
            return pair.clone();
        }
        let (name_hint, value_hint) = match key {
            "shell.env" => ("NAME", "value"),
            _ => ("executable", "icon name or SVG path"),
        };
        let name = cx.new(|cx| TextInput::new(cx, name_hint));
        let value = cx.new(|cx| TextInput::new(cx, value_hint));
        let v = value.clone();
        self.subs.push(cx.subscribe_in(&name, window, move |_, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::Submit) {
                v.read(cx).focus(window);
            }
        }));
        self.subs.push(cx.subscribe_in(
            &value,
            window,
            move |this: &mut Self, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::Submit) {
                    this.add_map_entry(key, window, cx);
                }
            },
        ));
        self.map_new.insert(key, (name.clone(), value.clone()));
        (name, value)
    }

    /// Writes the "add" row of a map editor as `<key>.<name> = "<value>"` (an existing name is
    /// overwritten), then clears the row and puts the cursor back into the name.
    fn add_map_entry(&mut self, key: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((name_in, value_in)) = self.map_new.get(key).cloned() else { return };
        let name = name_in.read(cx).text().trim().to_string();
        let value = value_in.read(cx).text().to_string();
        if let Err(e) = validate_map_entry(key, &name, &value) {
            crate::toast::show(cx, e);
            return;
        }
        let mut parts: Vec<&str> = key.split('.').collect();
        parts.push(&name);
        match nox_config::set_value_at(&nox_config::config_path(), &parts, value.as_str()) {
            Ok(()) => crate::config_io::reload_from_disk(cx),
            Err(e) => {
                crate::toast::show(cx, format!("Couldn't write {key}.{name} to config.toml: {e}"));
                return;
            }
        }
        name_in.update(cx, |i, cx| i.set_text("", cx));
        value_in.update(cx, |i, cx| i.set_text("", cx));
        name_in.read(cx).focus(window);
        cx.notify();
    }

    fn map_editor(
        &mut self,
        key: &'static str,
        value: Option<toml::Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::global(cx).clone();
        if key == "keys" {
            return self.keys_editor(cx);
        }
        let table = value.and_then(|v| v.as_table().cloned()).unwrap_or_default();
        let rows: Vec<AnyElement> = table
            .iter()
            .map(|(k, v)| {
                let full = format!("{key}.{k}");
                let path: Vec<String> =
                    key.split('.').map(str::to_string).chain(std::iter::once(k.clone())).collect();
                div()
                    .flex()
                    .gap_2()
                    .text_size(px(12.))
                    .child(div().w(px(120.)).text_color(theme.text).child(k.clone()))
                    .child(div().w(px(160.)).text_color(theme.text_muted).child(value_text(v)))
                    .child(button(SharedString::from(format!("rm-{full}")), "Remove", &theme).on_click(
                        move |_, _, cx| {
                            let parts: Vec<&str> = path.iter().map(String::as_str).collect();
                            let p = nox_config::config_path();
                            match nox_config::remove_value_at(&p, &parts) {
                                Ok(()) => crate::config_io::reload_from_disk(cx),
                                Err(e) => crate::toast::show(cx, format!("{e}")),
                            }
                        },
                    ))
                    .into_any_element()
            })
            .collect();
        let (name_in, value_in) = self.map_new_inputs(key, window, cx);
        let field = |input: Entity<TextInput>, w: f32| {
            div()
                .w(px(w))
                .px(px(6.))
                .py(px(2.))
                .rounded(px(5.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.bg)
                .text_size(px(12.))
                .child(input)
        };
        let add = div()
            .flex()
            .gap_2()
            .items_center()
            .child(field(name_in, 120.))
            .child(field(value_in, 160.))
            .child(
                button(SharedString::from(format!("add-{key}")), "Add", &theme)
                    .on_click(cx.listener(move |this, _, window, cx| this.add_map_entry(key, window, cx))),
            );
        div().flex().flex_col().gap_1().children(rows).child(add).into_any_element()
    }

    fn agents_editor(cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::global(cx).clone();
        let defs = Settings::global(cx).config.agents.defs.clone();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .w(px(300.))
            .children(defs.into_iter().map(|(name, d)| {
                div()
                    .flex()
                    .flex_col()
                    .text_size(px(12.))
                    .child(div().text_color(theme.text).child(format!("{name}: {}", d.command)))
                    .child(div().text_size(px(11.)).text_color(theme.text_muted).child(format!(
                        "resume: {} · last: {}",
                        d.resume.as_deref().unwrap_or("—"),
                        d.resume_last.as_deref().unwrap_or("—")
                    )))
            }))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .pt_1()
                    .child(
                        button("hooks", "Install Agent Hooks…", &theme)
                            .on_click(|_, window, cx| crate::features::install_agent_hooks(window, cx)),
                    )
                    .child(button("mcp", "Copy MCP Setup", &theme).on_click(|_, _, cx| {
                        if let Some(c) = nox_agents::mcp_setup_command("claude") {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(c.clone()));
                            crate::toast::show(cx, format!("Copied: {c}"));
                        }
                    })),
            )
            .into_any_element()
    }

    fn keys_editor(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::global(cx).clone();
        let keys = Settings::global(cx).config.keys.clone();
        let defaults = crate::actions::default_bindings();
        // Effective shortcut per name, for conflict detection.
        let effective: Vec<(String, String)> = defaults
            .iter()
            .map(|(name, _, def, _)| {
                let s = match keys.get(*name) {
                    Some(nox_config::KeyBinding::Shortcut(s)) => s.clone(),
                    _ => def.to_string(),
                };
                (name.to_string(), s)
            })
            .collect();
        let rows: Vec<AnyElement> = effective
            .iter()
            .map(|(name, shortcut)| {
                let conflict =
                    !shortcut.is_empty() && effective.iter().filter(|(_, s)| s == shortcut).count() > 1;
                let recording = self.recording.as_deref() == Some(name.as_str());
                let custom = keys.contains_key(name);
                let n1 = name.clone();
                let n2 = name.clone();
                let n3 = name.clone();
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .py(px(2.))
                    .text_size(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(theme.text)
                            .child(name.replace('_', " ")),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("rec-{name}")))
                            .w(px(110.))
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(px(5.))
                            .border_1()
                            .border_color(if recording {
                                theme.accent
                            } else if conflict {
                                theme.ansi(1)
                            } else {
                                theme.border
                            })
                            .cursor_pointer()
                            .text_color(if shortcut.is_empty() { theme.text_muted } else { theme.text })
                            .child(if recording {
                                "press keys…".to_string()
                            } else if shortcut.is_empty() {
                                "unbound".to_string()
                            } else {
                                crate::util_keys::pretty_shortcut(shortcut)
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.recording = Some(n1.clone());
                                window.focus(&this.focus);
                                cx.notify();
                            })),
                    )
                    .when(conflict, |d| {
                        d.child(div().text_size(px(11.)).text_color(theme.ansi(1)).child("conflict"))
                    })
                    .child(button(SharedString::from(format!("unbind-{name}")), "Unbind", &theme).on_click(
                        move |_, _, cx| {
                            set_value(cx, &format!("keys.{n2}"), "");
                        },
                    ))
                    // A fixed slot for "Default", so the recorder column lines up on every row.
                    .child(div().w(px(58.)).flex().justify_end().when(custom, |d| {
                        d.child(
                            button(SharedString::from(format!("default-{name}")), "Default", &theme)
                                .on_click(move |_, _, cx| {
                                    remove_value(cx, &format!("keys.{n3}"));
                                }),
                        )
                    }))
                    .into_any_element()
            })
            .collect();
        // Action overrides in `[keys]` (custom shortcuts and unbinds); `{ send = … }` bindings on
        // other keys have no default and are left alone.
        let overrides: Vec<String> =
            keys.keys().filter(|k| defaults.iter().any(|(n, ..)| n == k)).cloned().collect();
        let any = !overrides.is_empty();
        let reset_all = div().flex().justify_end().pb(px(6.)).child(
            button("keys-reset-all", "Reset All to Defaults…", &theme)
                .when(!any, |d| d.opacity(0.4).cursor_default())
                .when(any, |d| {
                    d.on_click(move |_, window, cx| {
                        let names = overrides.clone();
                        let n = names.len();
                        crate::dialog::confirm(
                            window,
                            cx,
                            "Reset all shortcuts?",
                            format!(
                                "{n} changed or unbound shortcut{} will go back to the default. \
                                 Key bindings that send text are kept.",
                                if n == 1 { "" } else { "s" }
                            ),
                            "Reset All",
                            move |_, cx| {
                                let paths: Vec<[&str; 2]> =
                                    names.iter().map(|k| ["keys", k.as_str()]).collect();
                                let refs: Vec<&[&str]> = paths.iter().map(<[&str; 2]>::as_slice).collect();
                                crate::config_io::remove_values(cx, &refs);
                            },
                        );
                    })
                }),
        );
        div().flex().flex_col().w_full().child(reset_all).children(rows).into_any_element()
    }
}

pub(crate) fn button(
    id: impl Into<SharedString>,
    label: &'static str,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .px(px(8.))
        .py(px(2.))
        .rounded(px(5.))
        .border_1()
        .border_color(theme.border)
        .text_size(px(11.))
        .text_color(theme.text)
        .cursor_pointer()
        .hover(|s| s.bg(theme.text.opacity(0.05)))
        .child(label)
}

fn stepper(
    key: &str,
    value: String,
    theme: &Theme,
    disabled: bool,
    f: impl Fn(&mut App, i64) + Clone + 'static,
) -> AnyElement {
    let f2 = f.clone();
    let b = |id: String, label: &'static str| {
        div()
            .id(SharedString::from(id))
            .w(px(22.))
            .h(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|s| s.bg(theme.text.opacity(0.06)))
            .child(label)
    };
    div()
        .flex()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(theme.border)
        .text_size(px(12.))
        .child(b(format!("dec-{key}"), "−").when(!disabled, |d| d.on_click(move |_, _, cx| f(cx, -1))))
        .child(div().min_w(px(44.)).flex().justify_center().child(value))
        .child(b(format!("inc-{key}"), "+").when(!disabled, |d| d.on_click(move |_, _, cx| f2(cx, 1))))
        .into_any_element()
}

/// Checks a new `[icons]` / `[shell.env]` entry from the Settings "add" row.
fn validate_map_entry(key: &str, name: &str, value: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Enter a name first.".into());
    }
    if key == "shell.env" {
        let valid = !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return Err(format!("\"{name}\" isn't a valid environment variable name."));
        }
    } else if value.trim().is_empty() {
        return Err("Enter an icon name or an SVG path.".into());
    }
    Ok(())
}

fn value_text(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Array(a) => a.iter().map(value_text).collect::<Vec<_>>().join(", "),
        toml::Value::Table(t) => match t.get("send") {
            Some(s) => format!("send {}", value_text(s).escape_debug()),
            None => format!("{} entries", t.len()),
        },
        other => other.to_string(),
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx).clone();
        let settings = Settings::global(cx).clone();
        let error = cx.try_global::<ConfigError>().and_then(|e| e.0.clone());
        let disabled = error.is_some();
        let query = self.search.read(cx).text().to_lowercase();
        let schema = nox_config::settings_schema();
        let metas: Vec<SettingMeta> = schema
            .into_iter()
            .filter(|m| {
                if query.is_empty() {
                    m.section == self.section
                } else {
                    m.label.to_lowercase().contains(&query)
                        || m.description.to_lowercase().contains(&query)
                        || m.key.to_lowercase().contains(&query)
                }
            })
            .collect();
        let rows: Vec<AnyElement> = metas.iter().map(|m| self.row(m, disabled, window, cx)).collect();
        let sections = Section::ALL.iter().map(|s| {
            let on = *s == self.section && !self.about && query.is_empty();
            let s2 = *s;
            div()
                .id(SharedString::from(format!("sec-{}", s.label())))
                .px(px(10.))
                .py(px(5.))
                .mx(px(6.))
                .rounded(px(6.))
                .text_size(px(13.))
                .cursor_pointer()
                .text_color(if on { theme.text } else { theme.text_muted })
                .when(on, |d| d.bg(theme.text.opacity(0.08)))
                .hover(|st| st.bg(theme.text.opacity(0.05)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.section = s2;
                    this.about = false;
                    this.expanded = None;
                    this.search.update(cx, |i, cx| i.set_text_cursor_end("", cx));
                    cx.notify();
                }))
                .child(s.label())
        });
        let show_about = self.about && query.is_empty();
        let about_item = div()
            .id("sec-about")
            .px(px(10.))
            .py(px(5.))
            .mx(px(6.))
            .mb(px(10.))
            .rounded(px(6.))
            .text_size(px(13.))
            .cursor_pointer()
            .text_color(if show_about { theme.text } else { theme.text_muted })
            .when(show_about, |d| d.bg(theme.text.opacity(0.08)))
            .hover(|st| st.bg(theme.text.opacity(0.05)))
            .on_click(cx.listener(|this, _, _, cx| {
                this.about = true;
                this.expanded = None;
                this.search.update(cx, |i, cx| i.set_text_cursor_end("", cx));
                cx.notify();
            }))
            .child("About");
        let about_page = show_about.then(|| {
            div()
                .flex()
                .flex_col()
                .items_center()
                .pt(px(36.))
                .text_size(px(12.))
                .child(crate::about::header(&theme))
                .child(div().mt(px(16.)).child(crate::updater::status_row(&theme, cx)))
                .child(div().w(px(340.)).mt(px(24.)).child(crate::about::details(&theme)))
        });
        let _ = section_of_key;
        let dialog = crate::dialog::render_overlay(window, cx);
        div()
            .id("settings-root")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .relative()
            .flex()
            .bg(theme.bg)
            .text_color(theme.text)
            .font_family(settings.ui_font.clone())
            .child(
                div()
                    .w(px(190.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .pt(px(44.))
                    .bg(theme.bg_sidebar)
                    .border_r_1()
                    .border_color(theme.border)
                    .children(sections)
                    .child(div().flex_1())
                    .child(about_item),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(44.))
                            .flex()
                            .items_center()
                            .px(px(20.))
                            .gap_2()
                            .border_b_1()
                            .border_color(theme.border)
                            .on_mouse_down(gpui::MouseButton::Left, |ev, window, _| {
                                if ev.click_count == 2 {
                                    window.titlebar_double_click();
                                }
                            })
                            .child(
                                gpui::svg()
                                    .path("icons/ui/search.svg")
                                    .size(px(13.))
                                    .text_color(theme.text_muted),
                            )
                            .child(div().flex_1().text_size(px(13.)).child(self.search.clone())),
                    )
                    .children(error.map(|e| {
                        div()
                            .mx(px(20.))
                            .mt(px(10.))
                            .p(px(10.))
                            .rounded(px(8.))
                            .bg(theme.ansi(1).opacity(0.12))
                            .text_size(px(12.))
                            .flex()
                            .justify_between()
                            .items_center()
                            .child(format!(
                                "config.toml can't be read: {e}. Fix the file to edit settings here."
                            ))
                            .child(button("err-open", "Open config.toml", &theme).on_click(|_, _, _| {
                                let _ = std::process::Command::new("/usr/bin/open")
                                    .arg("-t")
                                    .arg(nox_config::config_path())
                                    .spawn();
                            }))
                    }))
                    .child(
                        div()
                            .id("settings-scroll")
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_y_scroll()
                            .px(px(20.))
                            .pb(px(20.))
                            .when(!show_about, |d| d.children(rows))
                            .children(about_page),
                    )
                    .child(
                        div()
                            .h(px(44.))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px(px(20.))
                            .border_t_1()
                            .border_color(theme.border)
                            .child(button("open-config", "Open config.toml", &theme).on_click(|_, _, _| {
                                let path = nox_config::config_path();
                                let _ = nox_config::ensure_default_file(&path);
                                let _ =
                                    std::process::Command::new("/usr/bin/open").arg("-t").arg(path).spawn();
                            }))
                            .child(button("reveal-config", "Reveal in Finder", &theme).on_click(
                                |_, _, cx| {
                                    cx.reveal_path(&nox_config::config_path());
                                },
                            )),
                    ),
            )
            .children(dialog)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Keystroke, TestAppContext, VisualTestContext};

    use super::*;
    use crate::config_io::tests::{config_text, fresh_config};

    fn open(cx: &mut TestAppContext) -> (Entity<SettingsView>, &mut VisualTestContext) {
        crate::test_support::init(cx);
        // Start from what's on disk (the window shows config.toml).
        cx.update(crate::config_io::reload_from_disk);
        let (view, cx) = cx.add_window_view(SettingsView::new);
        cx.run_until_parked();
        (view, cx)
    }

    fn set(view: &Entity<SettingsView>, cx: &mut VisualTestContext, f: impl FnOnce(&mut SettingsView)) {
        view.update(cx, |v, cx| {
            f(v);
            cx.notify();
        });
        cx.run_until_parked();
    }

    fn config(cx: &mut VisualTestContext) -> std::sync::Arc<nox_config::Config> {
        cx.read(|cx| Settings::global(cx).config.clone())
    }

    fn key_down(view: &Entity<SettingsView>, cx: &mut VisualTestContext, ks: &str) {
        let ev = KeyDownEvent { keystroke: Keystroke::parse(ks).unwrap(), is_held: false };
        view.update_in(cx, |v, window, cx| v.on_key(&ev, window, cx));
        cx.run_until_parked();
    }

    #[test]
    fn value_texts() {
        assert_eq!(value_text(&toml::Value::String("zsh".into())), "zsh");
        assert_eq!(value_text(&toml::Value::Integer(3)), "3");
        assert_eq!(value_text(&toml::Value::Boolean(true)), "true");
        let arr = toml::Value::Array(vec!["-l".into(), "-i".into()]);
        assert_eq!(value_text(&arr), "-l, -i");
        let mut send = toml::Table::new();
        send.insert("send".into(), "\x1bb".into());
        assert_eq!(value_text(&toml::Value::Table(send)), "send \\u{1b}b");
        let mut t = toml::Table::new();
        t.insert("a".into(), 1.into());
        t.insert("b".into(), 2.into());
        assert_eq!(value_text(&toml::Value::Table(t)), "2 entries");
    }

    #[test]
    fn every_setting_has_a_section() {
        for m in nox_config::settings_schema() {
            assert_eq!(section_of_key(m.key), Some(m.section), "{}", m.key);
        }
        assert_eq!(section_of_key("nope.nope"), None);
    }

    #[gpui::test]
    fn every_section_and_the_about_page_render(cx: &mut TestAppContext) {
        let _g = fresh_config();
        std::fs::write(
            nox_config::config_path(),
            "[icons]\nhtop = \"python\"\n[shell.env]\nFOO = \"1\"\n[agents.mine]\ncommand = \"mine --x\"\nresume = \"mine -r {session}\"\n",
        )
        .unwrap();
        let (view, cx) = open(cx);
        assert_eq!(config(cx).icons.get("htop").map(String::as_str), Some("python"));
        for s in Section::ALL {
            set(&view, cx, |v| v.section = *s);
        }
        // Text inputs are created for the string settings that were shown.
        view.read_with(cx, |v, _| {
            assert!(v.inputs.contains_key("shell.program"));
            assert!(v.inputs.contains_key("shell.args"));
        });
        set(&view, cx, |v| v.about = true);
        // A section click leaves the About page.
        set(&view, cx, |v| {
            v.about = false;
            v.section = Section::Font;
        });
    }

    #[gpui::test]
    fn dropdowns_render_open(cx: &mut TestAppContext) {
        let _g = fresh_config();
        let (view, cx) = open(cx);
        for (section, key) in [
            (Section::Appearance, "ui.theme"),
            (Section::Appearance, "ui.font_family"),
            (Section::Font, "font.family"),
        ] {
            set(&view, cx, |v| {
                v.section = section;
                v.expanded = Some(key.into());
            });
            // Esc closes an open list first.
            key_down(&view, cx, "escape");
            assert_eq!(view.read_with(cx, |v, _| v.expanded.clone()), None);
        }
    }

    #[gpui::test]
    fn search_spans_sections(cx: &mut TestAppContext) {
        let _g = fresh_config();
        let (view, cx) = open(cx);
        let search = view.read_with(cx, |v, _| v.search.clone());
        search.update_in(cx, |i, window, _| i.focus(window));
        cx.simulate_input("sidebar");
        assert_eq!(search.read_with(cx, |i, _| i.text().to_string()), "sidebar");
        // While searching, the About page doesn't show.
        set(&view, cx, |v| v.about = true);
        cx.simulate_input("zzzz-nothing");
    }

    #[gpui::test]
    fn text_fields_write_on_enter_and_blur(cx: &mut TestAppContext) {
        let _g = fresh_config();
        let (view, cx) = open(cx);
        set(&view, cx, |v| v.section = Section::Shell);
        let program = view.read_with(cx, |v, _| v.inputs["shell.program"].clone());
        program.update_in(cx, |i, window, cx| {
            i.set_text("/bin/zsh", cx);
            i.focus(window);
        });
        cx.simulate_keystrokes("enter");
        assert_eq!(config(cx).shell.program, "/bin/zsh");
        assert!(config_text().contains("program = \"/bin/zsh\""), "{}", config_text());

        // Lists are comma-separated; empty parts are dropped. Blur commits too.
        let args = view.read_with(cx, |v, _| v.inputs["shell.args"].clone());
        cx.update(|window, _| window.activate_window());
        args.update_in(cx, |i, window, cx| {
            i.set_text("-l, , -i ", cx);
            i.focus(window);
        });
        cx.run_until_parked();
        cx.update(|window, _| window.blur());
        cx.run_until_parked();
        assert_eq!(config(cx).shell.args, ["-l", "-i"]);

        // Unchanged text writes nothing.
        let before = config_text();
        program.update_in(cx, |i, window, _| i.focus(window));
        cx.simulate_keystrokes("enter");
        assert_eq!(config_text(), before);
    }

    #[test]
    fn map_entry_validation() {
        assert!(validate_map_entry("shell.env", "FOO_1", "").is_ok());
        assert!(validate_map_entry("shell.env", "", "x").is_err());
        assert!(validate_map_entry("shell.env", "1FOO", "x").is_err());
        assert!(validate_map_entry("shell.env", "A-B", "x").is_err());
        assert!(validate_map_entry("icons", "my.tool", "python").is_ok());
        assert!(validate_map_entry("icons", "htop", " ").is_err());
    }

    #[gpui::test]
    fn map_entries_are_added_inline(cx: &mut TestAppContext) {
        let _g = fresh_config();
        let (view, cx) = open(cx);
        set(&view, cx, |v| v.section = Section::Shell);
        let (name, value) = view.read_with(cx, |v, _| v.map_new["shell.env"].clone());
        cx.update(|window, _| window.activate_window());
        name.update_in(cx, |i, window, cx| {
            i.set_text("FOO", cx);
            i.focus(window);
        });
        // Enter in the name moves to the value; Enter there adds the entry and clears the row.
        cx.simulate_keystrokes("enter");
        cx.simulate_input("bar baz");
        cx.simulate_keystrokes("enter");
        assert_eq!(config(cx).shell.env.get("FOO").map(String::as_str), Some("bar baz"));
        assert_eq!(name.read_with(cx, |i, _| i.text().to_string()), "");
        assert_eq!(value.read_with(cx, |i, _| i.text().to_string()), "");

        // An invalid name writes nothing.
        let before = config_text();
        name.update_in(cx, |i, window, cx| {
            i.set_text("NOT VALID", cx);
            i.focus(window);
        });
        cx.simulate_keystrokes("enter enter");
        assert_eq!(config_text(), before);

        // Icon names may contain dots (one key, not a nested table).
        set(&view, cx, |v| v.section = Section::Icons);
        let (name, value) = view.read_with(cx, |v, _| v.map_new["icons"].clone());
        name.update(cx, |i, cx| i.set_text("my.tool", cx));
        value.update(cx, |i, cx| i.set_text("python", cx));
        view.update_in(cx, |v, window, cx| v.add_map_entry("icons", window, cx));
        cx.run_until_parked();
        assert_eq!(config(cx).icons.get("my.tool").map(String::as_str), Some("python"));
    }

    #[gpui::test]
    fn external_edits_show_up(cx: &mut TestAppContext) {
        let _g = fresh_config();
        let (view, cx) = open(cx);
        set(&view, cx, |v| v.section = Section::Shell);
        std::fs::write(nox_config::config_path(), "[shell]\nprogram = \"/bin/fish\"\n").unwrap();
        cx.update(|_, cx| crate::config_io::reload_from_disk(cx));
        cx.run_until_parked();
        let program = view.read_with(cx, |v, cx| v.inputs["shell.program"].read(cx).text().to_string());
        assert_eq!(program, "/bin/fish");
    }

    #[gpui::test]
    fn recording_a_shortcut(cx: &mut TestAppContext) {
        let _g = fresh_config();
        let (view, cx) = open(cx);
        set(&view, cx, |v| {
            v.section = Section::Keys;
            v.recording = Some("new_tab".into());
        });
        // Modifier presses alone wait for the key.
        key_down(&view, cx, "shift");
        assert!(view.read_with(cx, |v, _| v.recording.is_some()));
        key_down(&view, cx, "cmd-shift-y");
        assert!(view.read_with(cx, |v, _| v.recording.is_none()));
        assert_eq!(
            config(cx).keys.get("new_tab"),
            Some(&nox_config::KeyBinding::Shortcut("shift-cmd-y".into()))
        );
        assert!(config_text().contains("new_tab = \"shift-cmd-y\""), "{}", config_text());

        // Esc stops recording without writing; other keys without recording do nothing.
        set(&view, cx, |v| v.recording = Some("close_tab".into()));
        key_down(&view, cx, "escape");
        assert!(view.read_with(cx, |v, _| v.recording.is_none()));
        key_down(&view, cx, "cmd-j");
        assert!(!config(cx).keys.contains_key("close_tab"));
    }

    #[gpui::test]
    fn keys_editor_shows_conflicts_and_unbound(cx: &mut TestAppContext) {
        let _g = fresh_config();
        std::fs::write(
            nox_config::config_path(),
            "[keys]\nnew_tab = \"cmd-d\"\nclose_tab = \"\"\n\"alt-left\" = { send = \"\\u001bb\" }\n",
        )
        .unwrap();
        let (view, cx) = open(cx);
        set(&view, cx, |v| {
            v.section = Section::Keys;
            v.recording = Some("find".into());
        });
        assert!(
            matches!(config(cx).keys.get("close_tab"), Some(nox_config::KeyBinding::Shortcut(s)) if s.is_empty())
        );
    }

    #[gpui::test]
    fn a_broken_file_disables_controls(cx: &mut TestAppContext) {
        let _g = fresh_config();
        let (view, cx) = open(cx);
        std::fs::write(nox_config::config_path(), "[ui\n").unwrap();
        cx.update(|_, cx| crate::config_io::reload_from_disk(cx));
        cx.run_until_parked();
        assert!(cx.read(|cx| cx.global::<ConfigError>().0.is_some()));
        for s in Section::ALL {
            set(&view, cx, |v| v.section = *s);
        }
        set(&view, cx, |v| {
            v.section = Section::Appearance;
            v.expanded = Some("ui.theme".into());
        });
    }

    #[gpui::test]
    fn a_sheet_opened_from_settings_renders_there(cx: &mut TestAppContext) {
        let _g = fresh_config();
        let (_, cx) = open(cx);
        cx.update(|window, cx| crate::dialog::confirm(window, cx, "Reset?", "", "Reset", |_, _| {}));
        cx.run_until_parked();
        assert!(cx.update(|window, cx| crate::dialog::is_open(window, cx)));
        cx.simulate_keystrokes("escape");
    }

    #[gpui::test]
    fn one_settings_window(cx: &mut TestAppContext) {
        let _g = fresh_config();
        crate::test_support::init(cx);
        cx.update(open_settings_window);
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
        cx.update(open_settings_window);
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
    }
}
