//! Window chrome (SPEC §5.2–5.4): title bar, workspace chip, tab strip (top / bottom), the left
//! sidebar, the pane card with the split layout and its dividers, and overlays.

use gpui::{
    AnyElement, AppContext as _, ClickEvent, Context, CursorStyle, Div, Hsla, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, SharedString, Stateful, Window, div, prelude::*, px, svg,
};
use nox_layout::{PaneId, Rect, SplitDirection, TabId, TabStatus};

use crate::actions::ROOT;
use crate::icons::icon_element;
use crate::root::{Root, px_rect};
use crate::settings_state::Settings;
use crate::theme::Theme;

/// Dragged tab payload (reorder within the strip, move onto a workspace).
#[derive(Clone)]
pub struct DraggedTab {
    pub ws: usize,
    pub tab: TabId,
    pub label: SharedString,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx);
        div()
            .px(px(10.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border)
            .text_size(px(12.))
            .text_color(theme.text)
            .child(self.label.clone())
    }
}

#[must_use]
pub fn status_dot(theme: &Theme, status: TabStatus) -> Option<Hsla> {
    crate::pane::status_color(theme, status)
}

fn icon_button(id: impl Into<SharedString>, icon: &'static str, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id.into())
        .size(px(24.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .text_color(theme.text_muted)
        .hover(|s| s.bg(theme.text.opacity(0.07)).text_color(theme.text))
        .child(svg().path(icon).size(px(14.)).text_color(theme.text_muted))
}

pub fn render_root(root: &mut Root, window: &mut Window, cx: &mut Context<Root>) -> AnyElement {
    let theme = Theme::global(cx).clone();
    let settings = Settings::global(cx).clone();
    let pos = settings.config.tabs.position;
    let pad = settings.config.ui.window_padding as f32;

    let titlebar = render_titlebar(root, window, cx);
    let sidebar = root.sidebar_shown(cx).then(|| render_sidebar(root, cx));
    let bottom = (pos == nox_config::TabsPosition::Bottom).then(|| render_bottom_bar(root, cx));
    let card = render_card(root, window, cx);

    // Centered over the terminal area; the top is placed so a full-height palette is centered,
    // so it doesn't jump while the list shrinks with filtering.
    let palette = root.palette.as_ref().map(|(p, _)| {
        let area = root.layout_area(window, cx);
        let w = crate::palette::PALETTE_WIDTH.min(area.w - 16.).max(280.);
        let left = (area.x + (area.w - w) / 2.).max(8.);
        let top = (area.y + (area.h - crate::palette::PALETTE_MAX_HEIGHT) / 2.).max(area.y + 8.);
        div().absolute().top(px(top)).left(px(left)).w(px(w)).child(p.clone())
    });
    let dialog = crate::dialog::render_overlay(cx);
    let toasts = crate::toast::render(cx);

    let mut el = div()
        .id("nox-root")
        .key_context(ROOT)
        .track_focus(&root.focus_handle)
        .size_full()
        .relative()
        .flex()
        .flex_col()
        .bg(theme.bg_sidebar)
        .text_color(theme.text)
        .font_family(settings.ui_font.clone())
        .text_size(px(13.))
        .on_mouse_move(cx.listener(on_root_mouse_move))
        .on_mouse_up(MouseButton::Left, cx.listener(on_root_mouse_up))
        .on_mouse_up_out(MouseButton::Left, cx.listener(on_root_mouse_up));
    el = crate::root_actions::register(el, cx);
    el.child(titlebar)
        .child(
            div()
                .flex()
                .flex_1()
                .min_h(px(0.))
                .children(sidebar)
                .child(div().flex_1().min_w(px(0.)).px(px(pad)).pb(px(pad)).flex().flex_col().child(card)),
        )
        .children(bottom)
        .children(palette)
        .children(dialog)
        .children(toasts)
        .into_any_element()
}

fn on_root_mouse_move(root: &mut Root, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Root>) {
    if let Some(path) = root.drag_divider.clone() {
        if ev.pressed_button != Some(MouseButton::Left) {
            root.drag_divider = None;
            cx.notify();
            return;
        }
        let area = root.layout_area(window, cx);
        let min = root.min_pane(window, cx);
        let pos = nox_layout::Point::new(f32::from(ev.position.x), f32::from(ev.position.y));
        if let Some(tab) = root.ws.active_mut().active_mut()
            && let Some(r) = tab.layout.ratio_for_drag(&path, pos, area, crate::root::DIVIDER_PX)
        {
            tab.layout.set_ratio(&path, r, area, crate::root::DIVIDER_PX, min);
            cx.notify();
        }
    }
    if root.drag_sidebar {
        if ev.pressed_button != Some(MouseButton::Left) {
            root.drag_sidebar = false;
            return;
        }
        let w = f32::from(ev.position.x).clamp(160., 360.).round() as i64;
        crate::config_io::set_value(cx, "tabs.sidebar_width", w);
    }
}

fn on_root_mouse_up(root: &mut Root, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Root>) {
    if root.drag_divider.take().is_some() {
        root.session_changed(cx);
        cx.notify();
    }
    root.drag_sidebar = false;
}

// ---- title bar -------------------------------------------------------------------------------

fn drag_region(id: &'static str) -> Stateful<Div> {
    div().id(id).flex_1().h_full().on_mouse_down(MouseButton::Left, |ev: &MouseDownEvent, window, _| {
        if ev.click_count == 2 {
            window.titlebar_double_click();
        } else {
            window.start_window_move();
        }
    })
}

fn render_titlebar(root: &mut Root, window: &mut Window, cx: &mut Context<Root>) -> AnyElement {
    let theme = Theme::global(cx).clone();
    let pos = Settings::global(cx).config.tabs.position;
    let h = root.titlebar_height(cx);
    let full_screen = window.is_fullscreen();
    let lights = if full_screen { 10. } else { crate::root::TRAFFIC_LIGHTS_PX };
    let mut bar = div()
        .id("titlebar")
        .h(px(h))
        .flex_none()
        .flex()
        .items_center()
        .pl(px(lights))
        .pr(px(8.))
        .gap_2()
        .bg(theme.bg_sidebar);
    match pos {
        nox_config::TabsPosition::Bottom => {
            bar = bar.child(drag_region("titlebar-drag"));
        }
        nox_config::TabsPosition::Top | nox_config::TabsPosition::Left => {
            bar = bar.child(sidebar_toggle(root, &theme, cx));
            if root.tab_bar_shown(cx) {
                bar = bar.child(render_workspace_chip(root, cx)).child(render_tab_strip(root, cx));
            }
            bar = bar.child(drag_region("titlebar-drag")).child(title_bar_buttons(cx));
        }
    }
    bar.into_any_element()
}

/// The `[◧]` button: highlighted while the sidebar shows (SPEC 5.4).
fn sidebar_toggle(root: &Root, theme: &Theme, cx: &mut Context<Root>) -> AnyElement {
    let shown = root.sidebar_shown(cx);
    icon_button("sidebar-toggle", "icons/ui/sidebar.svg", theme)
        .when(shown, |d| d.text_color(theme.text))
        .on_click(cx.listener(|_, _, window, cx| {
            window.dispatch_action(Box::new(crate::actions::ToggleSidebar), cx);
        }))
        .into_any_element()
}

fn title_bar_buttons(cx: &mut Context<Root>) -> AnyElement {
    let theme = Theme::global(cx).clone();
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap_1()
        .child(
            icon_button("settings-button", "icons/ui/settings.svg", &theme)
                .tooltip(|window, cx| crate::util::tooltip("Settings  ⌘,".into(), window, cx))
                .on_click(cx.listener(|_, _, window, cx| {
                    window.dispatch_action(Box::new(crate::actions::OpenSettings), cx);
                })),
        )
        .child(
            icon_button("palette-button", "icons/ui/command.svg", &theme)
                .tooltip(|window, cx| crate::util::tooltip("Command Palette  ⌘K".into(), window, cx))
                .on_click(cx.listener(|_, _, window, cx| {
                    window.dispatch_action(Box::new(crate::actions::CommandPalette), cx);
                })),
        )
        .child(
            icon_button("new-tab-button", "icons/ui/plus.svg", &theme)
                .tooltip(|window, cx| crate::util::tooltip("New Tab  ⌘T".into(), window, cx))
                .on_click(cx.listener(|_, _, window, cx| {
                    window.dispatch_action(Box::new(crate::actions::NewTab), cx);
                })),
        )
        .into_any_element()
}

fn render_workspace_chip(root: &mut Root, cx: &mut Context<Root>) -> AnyElement {
    let theme = Theme::global(cx).clone();
    let ws = root.ws.active();
    let swatch = ws.color.as_deref().and_then(|c| theme.named(c)).unwrap_or(theme.text_muted.opacity(0.5));
    // A dot when another workspace needs attention.
    let others = nox_layout::aggregate_status(
        root.ws
            .list
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != root.ws.active)
            .map(|(_, w)| root.workspace_status(w, cx)),
    );
    let other_dot = match others {
        TabStatus::NeedsInput | TabStatus::Bell | TabStatus::Done => status_dot(&theme, others),
        _ => None,
    };
    let tip = {
        let root_dir =
            ws.root.as_ref().map_or_else(|| "no root folder".into(), |r| nox_terminal::process::tildify(r));
        let cwd = root
            .focused_pane()
            .and_then(|p| p.read(cx).cwd.clone())
            .map(|c| nox_terminal::process::tildify(&c))
            .unwrap_or_default();
        format!("{}\nroot: {root_dir}\ncwd: {cwd}", ws.name)
    };
    div()
        .id("workspace-chip")
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(26.))
        .px(px(8.))
        .rounded(px(6.))
        .border_1()
        .border_color(theme.border)
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|s| s.bg(theme.text.opacity(0.05)))
        .child(div().size(px(9.)).rounded(px(2.)).bg(swatch))
        .child(
            div()
                .max_w(px(140.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(ws.name.clone()),
        )
        .children(other_dot.map(|c| div().size(px(6.)).rounded_full().bg(c)))
        .tooltip(move |window, cx| crate::util::tooltip(tip.clone(), window, cx))
        .on_click(
            cx.listener(|root, _: &ClickEvent, window, cx| crate::menus::workspace_menu(root, window, cx)),
        )
        .on_drop(cx.listener(|root, paths: &gpui::ExternalPaths, window, cx| {
            for p in paths.paths() {
                if p.is_dir() {
                    root.open_folder(p.clone(), false, window, cx);
                }
            }
        }))
        .into_any_element()
}

fn tab_pill(root: &Root, ws_idx: usize, index: usize, active: bool, cx: &mut Context<Root>) -> AnyElement {
    let theme = Theme::global(cx).clone();
    let ws = &root.ws.list[ws_idx];
    let tab = &ws.tabs[index];
    let tab_id = tab.id;
    let title = root.tab_title(tab, index, cx);
    let status = root.tab_status(tab, cx);
    let dot = match status {
        // A finished command in the active tab leaves no dot.
        TabStatus::Done | TabStatus::Bell if active && root.window_active => None,
        s => status_dot(&theme, s),
    };
    let icon = root.panes.get(&tab.focused).map_or(nox_agents::Icon::TERMINAL, |p| p.read(cx).icon.clone());
    let is_terminal = icon == nox_agents::Icon::TERMINAL;
    let icon_color = if active && is_terminal {
        theme.ansi(2).opacity(0.85)
    } else if active {
        theme.text
    } else {
        theme.text_muted
    };
    let renaming = matches!(&root.rename, Some(r) if matches!(r.target, crate::root::RenameTarget::Tab(t) if t == tab_id));
    let zoomed = tab.maximized.is_some();
    let group = SharedString::from(format!("tab-{tab_id}"));
    let label: SharedString = title.clone().into();

    let icon_slot = div()
        .relative()
        .size(px(14.))
        .flex_none()
        .child(
            div()
                .group_hover(group.clone(), gpui::Styled::invisible)
                .child(icon_element(&icon, 14., icon_color, cx)),
        )
        .child(
            div()
                .id(SharedString::from(format!("tab-close-{tab_id}")))
                .absolute()
                .inset_0()
                .invisible()
                .group_hover(group.clone(), gpui::Styled::visible)
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .hover(|s| s.bg(theme.text.opacity(0.1)))
                .child(svg().path("icons/ui/close.svg").size(px(11.)).text_color(theme.text_muted))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |root, _, window, cx| {
                    root.request_close_tab(ws_idx, tab_id, window, cx);
                })),
        )
        .children(dot.map(|c| {
            div()
                .absolute()
                .top(px(-2.))
                .right(px(-3.))
                .size(px(7.))
                .rounded_full()
                .bg(c)
                .border_1()
                .border_color(if active { theme.bg_elevated } else { theme.bg_sidebar })
        }));

    let content: AnyElement = if renaming {
        let input = root.rename.as_ref().unwrap().input.clone();
        div().flex_1().min_w(px(0.)).child(input).into_any_element()
    } else {
        div()
            .flex_1()
            .min_w(px(0.))
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
            .child(title)
            .into_any_element()
    };

    div()
        .id(SharedString::from(format!("tab-pill-{tab_id}")))
        .group(group)
        .flex()
        .flex_1()
        .min_w(px(120.))
        .max_w(px(260.))
        .h(px(28.))
        .px(px(9.))
        .gap(px(7.))
        .items_center()
        .rounded(px(6.))
        .text_size(px(12.))
        .cursor_pointer()
        .when(active, |d| {
            d.bg(theme.bg_elevated).border_1().border_color(theme.border).text_color(theme.text)
        })
        .when(!active, |d| d.text_color(theme.text_muted).hover(|s| s.bg(theme.text.opacity(0.04))))
        .child(icon_slot)
        .child(content)
        .when(zoomed, |d| {
            d.child(svg().path("icons/ui/maximize.svg").size(px(10.)).text_color(theme.text_muted))
        })
        .on_click(cx.listener(move |root, ev: &ClickEvent, window, cx| {
            if ev.click_count() == 2 {
                root.ws.list[ws_idx].select_tab(index);
                root.start_rename(crate::root::RenameTarget::Tab(tab_id), window, cx);
            } else {
                if ws_idx != root.ws.active {
                    root.switch_workspace(ws_idx, window, cx);
                }
                root.activate_tab(index, window, cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Middle,
            cx.listener(move |root, _, window, cx| root.request_close_tab(ws_idx, tab_id, window, cx)),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |root, _, window, cx| {
                crate::menus::tab_context_menu(root, ws_idx, tab_id, window, cx);
            }),
        )
        .on_drag(DraggedTab { ws: ws_idx, tab: tab_id, label }, |d, _, _, cx| cx.new(|_| d.clone()))
        .drag_over::<DraggedTab>(move |s, _, _, cx| s.bg(Theme::global(cx).accent.opacity(0.12)))
        .on_drop(
            cx.listener(move |root, d: &DraggedTab, window, cx| root.drop_tab(d, ws_idx, index, window, cx)),
        )
        .into_any_element()
}

fn render_tab_strip(root: &mut Root, cx: &mut Context<Root>) -> AnyElement {
    let w = root.ws.active;
    let active = root.ws.active().active_tab;
    let n = root.ws.active().tabs.len();
    let pills: Vec<AnyElement> = (0..n).map(|i| tab_pill(root, w, i, i == active, cx)).collect();
    div()
        .id("tab-strip")
        .flex()
        .flex_shrink()
        .min_w(px(0.))
        .gap(px(4.))
        .items_center()
        .overflow_x_scroll()
        .children(pills)
        .into_any_element()
}

fn render_bottom_bar(root: &mut Root, cx: &mut Context<Root>) -> AnyElement {
    let theme = Theme::global(cx).clone();
    let tabs = root.tab_bar_shown(cx);
    let mut bar = div()
        .h(px(crate::root::BOTTOM_BAR_PX))
        .flex_none()
        .flex()
        .items_center()
        .px(px(8.))
        .gap_2()
        .bg(theme.bg_sidebar)
        .child(sidebar_toggle(root, &theme, cx));
    if tabs {
        bar = bar.child(render_workspace_chip(root, cx)).child(render_tab_strip(root, cx));
    }
    bar.child(div().flex_1()).child(title_bar_buttons(cx)).into_any_element()
}

// ---- sidebar ---------------------------------------------------------------------------------

fn sidebar_row(root: &Root, ws_idx: usize, index: usize, cx: &mut Context<Root>) -> AnyElement {
    let theme = Theme::global(cx).clone();
    let ws = &root.ws.list[ws_idx];
    let tab = &ws.tabs[index];
    let tab_id = tab.id;
    let active = ws_idx == root.ws.active && index == ws.active_tab;
    let title = root.tab_title(tab, index, cx);
    let status = root.tab_status(tab, cx);
    let dot = match status {
        TabStatus::Done | TabStatus::Bell if active && root.window_active => None,
        s => status_dot(&theme, s),
    };
    let pane = root.panes.get(&tab.focused).map(|p| p.read(cx));
    let icon = pane.map_or(nox_agents::Icon::TERMINAL, |p| p.icon.clone());
    let cwd = pane.map(super::pane::Pane::cwd_display).unwrap_or_default();
    let branch =
        pane.and_then(|p| p.branch.clone()).filter(|_| Settings::global(cx).config.ui.show_git_branch);
    let second = {
        let mut parts = Vec::new();
        if !title.ends_with(&cwd) {
            parts.push(cwd.clone());
        }
        if let Some(b) = branch {
            parts.push(format!("⎇ {b}"));
        }
        (!parts.is_empty()).then(|| parts.join("  "))
    };
    let renaming = matches!(&root.rename, Some(r) if matches!(r.target, crate::root::RenameTarget::Tab(t) if t == tab_id));
    let label: SharedString = title.clone().into();
    let content: AnyElement = if renaming {
        root.rename.as_ref().unwrap().input.clone().into_any_element()
    } else {
        div().whitespace_nowrap().overflow_hidden().text_ellipsis().child(title).into_any_element()
    };
    div()
        .id(SharedString::from(format!("side-row-{tab_id}")))
        .mx(px(6.))
        .px(px(8.))
        .py(px(5.))
        .rounded(px(6.))
        .flex()
        .gap_2()
        .items_start()
        .cursor_pointer()
        .text_size(px(12.))
        .when(active, |d| d.bg(theme.text.opacity(0.08)).text_color(theme.text))
        .when(!active, |d| d.text_color(theme.text_muted).hover(|s| s.bg(theme.text.opacity(0.04))))
        .child(div().pt(px(1.)).child(icon_element(
            &icon,
            14.,
            if active { theme.text } else { theme.text_muted },
            cx,
        )))
        .child(div().flex().flex_col().flex_1().min_w(px(0.)).child(content).children(second.map(|s| {
            div()
                .text_size(px(11.))
                .text_color(theme.text_muted)
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(s)
        })))
        .children(dot.map(|c| div().mt(px(5.)).size(px(7.)).rounded_full().bg(c).flex_none()))
        .on_click(cx.listener(move |root, ev: &ClickEvent, window, cx| {
            if ws_idx != root.ws.active {
                root.switch_workspace(ws_idx, window, cx);
            }
            if ev.click_count() == 2 {
                root.ws.list[ws_idx].select_tab(index);
                root.start_rename(crate::root::RenameTarget::Tab(tab_id), window, cx);
            } else {
                root.activate_tab(index, window, cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |root, _, window, cx| {
                crate::menus::tab_context_menu(root, ws_idx, tab_id, window, cx);
            }),
        )
        .on_drag(DraggedTab { ws: ws_idx, tab: tab_id, label }, |d, _, _, cx| cx.new(|_| d.clone()))
        .drag_over::<DraggedTab>(move |s, _, _, cx| s.bg(Theme::global(cx).accent.opacity(0.12)))
        .on_drop(
            cx.listener(move |root, d: &DraggedTab, window, cx| root.drop_tab(d, ws_idx, index, window, cx)),
        )
        .into_any_element()
}

fn sidebar_ws_header(root: &Root, ws_idx: usize, cx: &mut Context<Root>) -> AnyElement {
    let theme = Theme::global(cx).clone();
    let ws = &root.ws.list[ws_idx];
    let swatch = ws.color.as_deref().and_then(|c| theme.named(c)).unwrap_or(theme.text_muted.opacity(0.5));
    let status = root.workspace_status(ws, cx);
    let dot = status_dot(&theme, status);
    let active = ws_idx == root.ws.active;
    div()
        .id(("side-ws", ws_idx))
        .mx(px(6.))
        .px(px(8.))
        .h(px(30.))
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(6.))
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .cursor_pointer()
        .when(active, |d| d.bg(theme.text.opacity(0.08)).text_color(theme.text))
        .when(!active, |d| d.text_color(theme.text_muted).hover(|s| s.bg(theme.text.opacity(0.04))))
        .child(div().size(px(9.)).rounded(px(2.)).bg(swatch).flex_none())
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(ws.name.clone()),
        )
        .children(dot.map(|c| div().size(px(6.)).rounded_full().bg(c)))
        .when(active, |d| {
            d.child(svg().path("icons/ui/chevron-down.svg").size(px(12.)).text_color(theme.text_muted))
        })
        .on_click(cx.listener(move |root, _, window, cx| {
            if ws_idx == root.ws.active {
                crate::menus::workspace_menu(root, window, cx);
            } else {
                root.switch_workspace(ws_idx, window, cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(|root, _, window, cx| crate::menus::workspace_menu(root, window, cx)),
        )
        .drag_over::<DraggedTab>(move |s, _, _, cx| s.bg(Theme::global(cx).accent.opacity(0.12)))
        .on_drop(cx.listener(move |root, d: &DraggedTab, window, cx| {
            root.move_tab_to_workspace(d.ws, d.tab, ws_idx, window, cx);
        }))
        .into_any_element()
}

fn render_sidebar(root: &mut Root, cx: &mut Context<Root>) -> AnyElement {
    let theme = Theme::global(cx).clone();
    let width = root.sidebar_width(cx);
    let all = Settings::global(cx).config.tabs.sidebar_workspaces == nox_config::SidebarWorkspaces::All;
    let mut rows: Vec<AnyElement> = Vec::new();
    if all {
        // Every workspace as a group: its header row followed by its tabs.
        for w in 0..root.ws.len() {
            rows.push(sidebar_ws_header(root, w, cx));
            for i in 0..root.ws.list[w].tabs.len() {
                rows.push(sidebar_row(root, w, i, cx));
            }
            rows.push(div().h(px(6.)).into_any_element());
        }
    } else {
        // All workspaces on top, a separator line, then the active workspace's tabs.
        for w in 0..root.ws.len() {
            rows.push(sidebar_ws_header(root, w, cx));
        }
        rows.push(div().mx(px(14.)).my(px(4.)).h(px(1.)).bg(theme.border).into_any_element());
        let w = root.ws.active;
        for i in 0..root.ws.list[w].tabs.len() {
            rows.push(sidebar_row(root, w, i, cx));
        }
    }
    div()
        .flex()
        .flex_none()
        .w(px(width))
        .h_full()
        .relative()
        .bg(theme.bg_sidebar)
        .child(
            div()
                .id("sidebar-scroll")
                .flex()
                .flex_col()
                .gap(px(2.))
                .w_full()
                .pb(px(8.))
                .overflow_y_scroll()
                .children(rows),
        )
        .child(
            div()
                .id("sidebar-divider")
                .absolute()
                .top_0()
                .bottom_0()
                .right(px(-3.))
                .w(px(6.))
                .cursor(CursorStyle::ResizeLeftRight)
                .child(div().absolute().top_0().bottom_0().left(px(3.)).w(px(1.)).bg(theme.border))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|root, _, _, cx| {
                        root.drag_sidebar = true;
                        cx.stop_propagation();
                    }),
                ),
        )
        .into_any_element()
}

// ---- pane card & split layout ----------------------------------------------------------------

fn render_card(root: &mut Root, window: &mut Window, cx: &mut Context<Root>) -> AnyElement {
    let theme = Theme::global(cx).clone();
    let area = root.layout_area(window, cx);
    // The card's content box starts at (area.x, area.y); children are placed relative to it.
    let origin = (area.x, area.y);
    let rel = |r: Rect| Rect::new(r.x - origin.0, r.y - origin.1, r.w, r.h);
    let mut children: Vec<AnyElement> = Vec::new();
    let ws_idx = root.ws.active;
    if let Some(tab) = root.ws.list[ws_idx].active().cloned() {
        for (pane, r) in tab.visible_rects(area, crate::root::DIVIDER_PX) {
            let rr = rel(r);
            let el: AnyElement = match root.panes.get(&pane) {
                Some(p) => p.clone().into_any_element(),
                None => div().size_full().bg(theme.bg).into_any_element(),
            };
            children.push(
                div()
                    .absolute()
                    .left(px(rr.x))
                    .top(px(rr.y))
                    .w(px(rr.w))
                    .h(px(rr.h))
                    .overflow_hidden()
                    .child(el)
                    .into_any_element(),
            );
        }
        if tab.maximized.is_none() {
            for d in tab.layout.dividers(area, crate::root::DIVIDER_PX) {
                let line = rel(d.rect);
                let hit = rel(d.hit_rect(crate::root::DIVIDER_HIT_PX));
                let dragging = root.drag_divider.as_ref() == Some(&d.path);
                let vertical = d.dir == SplitDirection::Vertical;
                let path = d.path.clone();
                let path2 = d.path.clone();
                children.push(
                    div()
                        .absolute()
                        .left(px(line.x))
                        .top(px(line.y))
                        .w(px(line.w))
                        .h(px(line.h))
                        .bg(if dragging { theme.accent } else { theme.border })
                        .into_any_element(),
                );
                children.push(
                    div()
                        .id(SharedString::from(format!("divider-{:?}", d.path.0)))
                        .absolute()
                        .left(px(hit.x))
                        .top(px(hit.y))
                        .w(px(hit.w))
                        .h(px(hit.h))
                        .cursor(if vertical {
                            CursorStyle::ResizeLeftRight
                        } else {
                            CursorStyle::ResizeUpDown
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |root, ev: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                if ev.click_count == 2 {
                                    let area = root.layout_area(window, cx);
                                    let min = root.min_pane(window, cx);
                                    if let Some(tab) = root.ws.active_mut().active_mut() {
                                        tab.layout.equalize_at(&path2);
                                        tab.layout.clamp_to_min_sizes(area, crate::root::DIVIDER_PX, min);
                                    }
                                    root.session_changed(cx);
                                } else {
                                    root.drag_divider = Some(path.clone());
                                }
                                cx.notify();
                            }),
                        )
                        .into_any_element(),
                );
            }
        }
    }
    let _ = px_rect;
    let _: Option<PaneId> = None;
    div()
        .flex_1()
        .min_h(px(0.))
        .relative()
        .rounded(px(10.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.bg)
        .overflow_hidden()
        .children(children)
        .into_any_element()
}
