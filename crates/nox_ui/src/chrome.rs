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
    /// Its index when the drag started (where the drop marker goes).
    pub index: usize,
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

/// Dragged workspace payload (reorder in the sidebar).
#[derive(Clone)]
pub struct DraggedWorkspace {
    pub ws: usize,
    pub label: SharedString,
    pub swatch: Hsla,
}

impl Render for DraggedWorkspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx);
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border)
            .text_size(px(12.))
            .text_color(theme.text)
            .child(div().size(px(9.)).rounded(px(2.)).bg(self.swatch))
            .child(self.label.clone())
    }
}

/// Where a dragged item lands relative to the drop target: before it (dragged from further
/// down / right, or from another list) or after it.
fn lands_after(from_same_list: bool, from: usize, target: usize) -> bool {
    from_same_list && from < target
}

/// A 2 px accent edge on the side of a drop target where the dragged item will land; the
/// negative margin cancels the border so nothing shifts.
fn drop_marker(s: gpui::StyleRefinement, vertical: bool, after: bool, accent: Hsla) -> gpui::StyleRefinement {
    let s = s.border_color(accent);
    match (vertical, after) {
        (false, false) => s.border_l_2().ml(px(-2.)),
        (false, true) => s.border_r_2().mr(px(-2.)),
        (true, false) => s.border_t_2().mt(px(-2.)),
        (true, true) => s.border_b_2().mb(px(-2.)),
    }
}

/// Mouse-downs on interactive title-bar items stop here, so they don't move the window.
fn keep_press(_: &MouseDownEvent, _: &mut Window, cx: &mut gpui::App) {
    cx.stop_propagation();
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
        .on_mouse_down(MouseButton::Left, keep_press)
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
        let el = div().absolute().left(px(left)).w(px(w)).child(p.clone());
        crate::util::fade_slide_in("palette-appear", settings.reduce_motion(), el, move |el, dy| {
            el.top(px(top + dy))
        })
    });
    let dialog = crate::dialog::render_overlay(window, cx);
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

/// Empty title-bar space (the bar's background, the gaps between and around the pills, and this
/// grab area) moves the window; a double-click zooms or minimizes per the system setting. The
/// interactive items on the bar keep their presses ([`keep_press`]).
fn on_titlebar_press(ev: &MouseDownEvent, window: &mut Window, _: &mut gpui::App) {
    if ev.click_count == 2 {
        window.titlebar_double_click();
    } else {
        crate::window_bg::start_move(window);
    }
}

/// Smallest empty stretch kept next to the tab strip, so the window can still be grabbed when
/// the tabs overflow (the strip scrolls instead).
pub const MIN_GRAB_PX: f32 = 56.;

/// The flexible space after the tab strip: a tab dropped here goes to the end. In the title bar
/// it never shrinks below [`MIN_GRAB_PX`], which keeps a place to grab the window.
fn strip_tail(root: &Root, id: &'static str, grab: bool, cx: &mut Context<Root>) -> AnyElement {
    let ws = root.ws.active;
    let n = root.ws.active().tabs.len();
    div()
        .id(id)
        .debug_selector(|| id.to_string())
        .flex_1()
        .h_full()
        .when(grab, |d| d.min_w(px(MIN_GRAB_PX)))
        .on_drop(cx.listener(move |root, d: &DraggedTab, window, cx| root.drop_tab(d, ws, n, window, cx)))
        .into_any_element()
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
        .bg(theme.bg_sidebar)
        .on_mouse_down(MouseButton::Left, on_titlebar_press);
    match pos {
        nox_config::TabsPosition::Bottom => {
            bar = bar.child(div().id("titlebar-drag").flex_1().h_full());
        }
        nox_config::TabsPosition::Top | nox_config::TabsPosition::Left => {
            bar = bar.child(sidebar_toggle(root, &theme, cx));
            let tabs = root.tab_bar_shown(cx);
            if tabs {
                bar = bar.child(render_workspace_chip(root, cx)).child(render_tab_strip(root, cx));
            }
            bar = bar.child(strip_tail(root, "titlebar-drag", tabs, cx)).child(title_bar_buttons(cx));
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
        TabStatus::NeedsInput | TabStatus::Bell | TabStatus::Done | TabStatus::Failed => {
            status_dot(&theme, others)
        }
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
    // With a single workspace the chip collapses to a stack icon (same tooltip, menu and drop).
    let single = root.ws.list.len() < 2;
    div()
        .id("workspace-chip")
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(26.))
        .px(px(if single { 6. } else { 8. }))
        .rounded(px(6.))
        .border_1()
        .border_color(theme.border)
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|s| s.bg(theme.text.opacity(0.05)))
        .on_mouse_down(MouseButton::Left, keep_press)
        .when(single, |d| {
            d.child(svg().path("icons/ui/stack.svg").size(px(14.)).text_color(theme.text_muted))
        })
        .when(!single, |d| {
            d.child(div().size(px(9.)).rounded(px(2.)).bg(swatch))
                .child(
                    div()
                        .max_w(px(140.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(ws.name.clone()),
                )
                .children(other_dot.map(|c| div().size(px(6.)).rounded_full().bg(c)))
        })
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
        TabStatus::Done | TabStatus::Failed | TabStatus::Bell if active && root.window_active => None,
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

    let pill = div()
        .id(SharedString::from(format!("tab-pill-{tab_id}")))
        .group(group)
        .flex()
        .size_full()
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
        .on_mouse_down(MouseButton::Left, keep_press)
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
        .on_drag(DraggedTab { ws: ws_idx, index, tab: tab_id, label }, |d, _, _, cx| cx.new(|_| d.clone()));
    // The drop target wraps the pill, so its marker sits at the pill's edge.
    div()
        .id(SharedString::from(format!("tab-drop-{tab_id}")))
        .debug_selector(|| format!("tab-pill-{index}"))
        .flex()
        .flex_1()
        .min_w(px(120.))
        .max_w(px(260.))
        .h(px(28.))
        .child(pill)
        .drag_over::<DraggedTab>(move |s, d, _, cx| {
            if d.tab == tab_id {
                return s;
            }
            drop_marker(s, false, lands_after(d.ws == ws_idx, d.index, index), Theme::global(cx).accent)
        })
        .on_drop(
            cx.listener(move |root, d: &DraggedTab, window, cx| root.drop_tab(d, ws_idx, index, window, cx)),
        )
        .into_any_element()
}

/// What the strip's arrows show: whether the tabs overflow, and whether there is more to the left
/// / right. Read from the scroll state of the last layout.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StripOverflow {
    pub overflows: bool,
    pub more_left: bool,
    pub more_right: bool,
}

impl StripOverflow {
    #[must_use]
    pub fn of(handle: &gpui::ScrollHandle) -> Self {
        let max = f32::from(handle.max_offset().width);
        let x = f32::from(handle.offset().x);
        Self { overflows: max > 0.5, more_left: x < -0.5, more_right: max > 0.5 && x > -max + 0.5 }
    }
}

/// Scrolls the strip by about its visible width (`dir` −1 = left, 1 = right), keeping a sliver of
/// the previous view for context, then snaps so the tab nearest the leading edge (left when going
/// left, right when going right) lines up with it rather than ending half cut off.
pub fn scroll_tab_strip(handle: &gpui::ScrollHandle, dir: f32) {
    let max = f32::from(handle.max_offset().width);
    let strip = handle.bounds();
    let (left, right) = (f32::from(strip.left()), f32::from(strip.right()));
    let page = (f32::from(strip.size.width) - 48.).max(80.);
    let off = handle.offset();
    let target = f32::from(off.x) - dir * page;
    let mut best: Option<f32> = None;
    for ix in 0.. {
        let Some(item) = handle.bounds_for_item(ix) else { break };
        // The offset that puts this tab's edge on the strip's edge.
        let x = if dir < 0. { left - f32::from(item.left()) } else { right - f32::from(item.right()) };
        if best.is_none_or(|b| (x - target).abs() < (b - target).abs()) {
            best = Some(x);
        }
    }
    let x = best.unwrap_or(target).clamp(-max, 0.);
    handle.set_offset(gpui::point(px(x), off.y));
}

/// Scrolls the strip so tab `ix` is fully visible, measured on the current layout. True when the
/// offset changed.
fn reveal_tab(handle: &gpui::ScrollHandle, ix: usize) -> bool {
    let Some(item) = handle.bounds_for_item(ix) else { return false };
    let strip = handle.bounds();
    let max = handle.max_offset().width;
    let off = handle.offset();
    let mut x = off.x;
    if item.left() + x < strip.left() {
        x = strip.left() - item.left();
    } else if item.right() + x > strip.right() {
        x = strip.right() - item.right();
    }
    let x = x.clamp(-max, px(0.));
    if (x - off.x).abs() < px(0.5) {
        return false;
    }
    handle.set_offset(gpui::point(x, off.y));
    true
}

fn strip_arrow(
    id: &'static str,
    dir: f32,
    enabled: bool,
    handle: &gpui::ScrollHandle,
    theme: &Theme,
    cx: &mut Context<Root>,
) -> AnyElement {
    let angle = if dir < 0. { std::f32::consts::FRAC_PI_2 } else { -std::f32::consts::FRAC_PI_2 };
    let handle = handle.clone();
    div()
        .id(id)
        .debug_selector(|| id.to_string())
        .size(px(22.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .on_mouse_down(MouseButton::Left, keep_press)
        .child(
            svg()
                .path("icons/ui/chevron-down.svg")
                .size(px(12.))
                .text_color(if enabled { theme.text_muted } else { theme.text_muted.opacity(0.35) })
                .with_transformation(gpui::Transformation::rotate(gpui::radians(angle))),
        )
        .when(enabled, |d| {
            d.cursor_pointer().hover(|s| s.bg(theme.text.opacity(0.07))).on_click(cx.listener(
                move |_, _, _, cx| {
                    scroll_tab_strip(&handle, dir);
                    cx.notify();
                },
            ))
        })
        .into_any_element()
}

fn render_tab_strip(root: &mut Root, cx: &mut Context<Root>) -> AnyElement {
    let theme = Theme::global(cx).clone();
    let w = root.ws.active;
    let active = root.ws.active().active_tab;
    let n = root.ws.active().tabs.len();
    let pills: Vec<AnyElement> = (0..n).map(|i| tab_pill(root, w, i, i == active, cx)).collect();
    let handle = root.tab_strip.clone();
    let state = StripOverflow::of(&handle);
    // Keep the active tab in view when it changes (new tab, ⌘1…9, switching workspaces), and
    // when the arrows come or go (they change the strip's width).
    let active_id = root.ws.active().active().map(|t| t.id);
    let reveal = active_id != root.tab_strip_revealed || state.overflows != root.tab_strip_arrows;
    root.tab_strip_revealed = active_id;
    root.tab_strip_arrows = state.overflows;
    let strip = div()
        .id("tab-strip")
        .flex()
        .flex_shrink()
        .min_w(px(0.))
        .gap(px(4.))
        .items_center()
        .overflow_x_scroll()
        .track_scroll(&handle)
        .children(pills);
    // The scroll state above is from the last layout. Once this one is laid out (the strip
    // prepaints before this canvas), reveal the active tab and redraw if the offset or the arrows
    // would change (tabs added or closed, window resized, wheel scrolled). GPUI's own
    // `scroll_to_item` would measure against the previous frame's strip width.
    let check = {
        let handle = handle.clone();
        gpui::canvas(
            move |_, window, _| {
                if reveal && reveal_tab(&handle, active) {
                    window.request_animation_frame();
                }
                if StripOverflow::of(&handle) != state {
                    window.request_animation_frame();
                }
            },
            |_, (), _, _| {},
        )
        .absolute()
        .size_0()
    };
    div()
        .flex()
        .flex_shrink()
        .min_w(px(0.))
        .items_center()
        .gap(px(2.))
        .relative()
        .child(strip)
        .when(state.overflows, |d| {
            d.child(strip_arrow("tab-strip-left", -1., state.more_left, &handle, &theme, cx))
                .child(strip_arrow("tab-strip-right", 1., state.more_right, &handle, &theme, cx))
        })
        .child(check)
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
    bar.child(strip_tail(root, "bottom-bar-tail", false, cx)).child(title_bar_buttons(cx)).into_any_element()
}

// ---- sidebar ---------------------------------------------------------------------------------

/// One tab row in the sidebar; `inset` adds the side margin rows inside a workspace block get
/// from the block.
fn sidebar_row(root: &Root, ws_idx: usize, index: usize, inset: bool, cx: &mut Context<Root>) -> AnyElement {
    let theme = Theme::global(cx).clone();
    let ws = &root.ws.list[ws_idx];
    let tab = &ws.tabs[index];
    let tab_id = tab.id;
    let active = ws_idx == root.ws.active && index == ws.active_tab;
    let title = root.tab_title(tab, index, cx);
    let status = root.tab_status(tab, cx);
    let dot = match status {
        TabStatus::Done | TabStatus::Failed | TabStatus::Bell if active && root.window_active => None,
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
    let row = div()
        .id(SharedString::from(format!("side-row-{tab_id}")))
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
        .on_drag(DraggedTab { ws: ws_idx, index, tab: tab_id, label }, |d, _, _, cx| cx.new(|_| d.clone()));
    div()
        .id(SharedString::from(format!("side-drop-{tab_id}")))
        .when(inset, |d| d.mx(px(6.)))
        .child(row)
        .drag_over::<DraggedTab>(move |s, d, _, cx| {
            if d.tab == tab_id {
                return s;
            }
            drop_marker(s, true, lands_after(d.ws == ws_idx, d.index, index), Theme::global(cx).accent)
        })
        .on_drop(
            cx.listener(move |root, d: &DraggedTab, window, cx| root.drop_tab(d, ws_idx, index, window, cx)),
        )
        .into_any_element()
}

/// A workspace's row in the sidebar. With `groups` (the all-workspaces list) the swatch turns
/// into a disclosure chevron on hover that folds the group's tabs; a folded group shows its tab
/// count. The row drags to reorder workspaces.
fn sidebar_ws_header(root: &Root, ws_idx: usize, groups: bool, cx: &mut Context<Root>) -> AnyElement {
    let theme = Theme::global(cx).clone();
    let ws = &root.ws.list[ws_idx];
    let swatch = ws.color.as_deref().and_then(|c| theme.named(c)).unwrap_or(theme.text_muted.opacity(0.5));
    let status = root.workspace_status(ws, cx);
    let dot = status_dot(&theme, status);
    let active = ws_idx == root.ws.active;
    let collapsed = groups && ws.collapsed;
    let group = SharedString::from(format!("side-ws-{}", ws.id.0));
    let label: SharedString = ws.name.clone().into();

    let swatch_el = div().size(px(9.)).rounded(px(2.)).bg(swatch);
    let lead: AnyElement = if groups {
        let chevron = svg()
            .path("icons/ui/chevron-down.svg")
            .size(px(12.))
            .text_color(theme.text_muted)
            .when(collapsed, |s| {
                s.with_transformation(gpui::Transformation::rotate(gpui::radians(
                    -std::f32::consts::FRAC_PI_2,
                )))
            });
        div()
            .id(("side-ws-fold", ws_idx))
            .relative()
            .size(px(14.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(3.))
            .hover(|s| s.bg(theme.text.opacity(0.1)))
            .child(div().group_hover(group.clone(), gpui::Styled::invisible).child(swatch_el))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .invisible()
                    .group_hover(group.clone(), gpui::Styled::visible)
                    .child(chevron),
            )
            .debug_selector(|| format!("side-ws-fold-{ws_idx}"))
            .on_mouse_down(MouseButton::Left, keep_press)
            .on_click(cx.listener(move |root, _, _, cx| root.toggle_workspace_collapsed(ws_idx, cx)))
            .into_any_element()
    } else {
        swatch_el.flex_none().into_any_element()
    };
    let count = collapsed.then(|| div().text_color(theme.text_muted).child(ws.tabs.len().to_string()));

    div()
        .id(("side-ws", ws_idx))
        .debug_selector(|| format!("side-ws-{ws_idx}"))
        .group(group)
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
        .child(lead)
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(ws.name.clone()),
        )
        .children(count)
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
        .on_drag(DraggedWorkspace { ws: ws_idx, label, swatch }, |d, _, _, cx| cx.new(|_| d.clone()))
        .drag_over::<DraggedTab>(move |s, d, _, cx| {
            if d.ws == ws_idx { s } else { s.bg(Theme::global(cx).accent.opacity(0.12)) }
        })
        .on_drop(cx.listener(move |root, d: &DraggedTab, window, cx| {
            root.move_tab_to_workspace(d.ws, d.tab, ws_idx, window, cx);
        }))
        .into_any_element()
}

/// A workspace's block in the sidebar (its header, plus its tabs in the all-workspaces list):
/// the drop target for reordering workspaces, marked on the side the dragged one lands.
fn sidebar_ws_group(ws_idx: usize, rows: Vec<AnyElement>, pad: f32, cx: &mut Context<Root>) -> AnyElement {
    div()
        .id(("side-ws-group", ws_idx))
        .mx(px(6.))
        .flex()
        .flex_col()
        .gap(px(2.))
        .pb(px(pad))
        .children(rows)
        .drag_over::<DraggedWorkspace>(move |s, d, _, cx| {
            if d.ws == ws_idx {
                return s;
            }
            drop_marker(s, true, lands_after(true, d.ws, ws_idx), Theme::global(cx).accent)
        })
        .on_drop(cx.listener(move |root, d: &DraggedWorkspace, window, cx| {
            root.reorder_workspace(d.ws, ws_idx, window, cx);
        }))
        .into_any_element()
}

/// One running agent in the sidebar's bottom section: icon, name, workspace and state.
fn sidebar_agent_row(root: &Root, ws_idx: usize, pane: PaneId, cx: &mut Context<Root>) -> Option<AnyElement> {
    let theme = Theme::global(cx).clone();
    let e = root.panes.get(&pane)?.read(cx);
    let state = match e.agent_state()? {
        nox_agents::AgentState::NeedsInput => "needs input",
        nox_agents::AgentState::Working => "working",
        nox_agents::AgentState::Idle if e.done_unseen => "idle · unseen",
        _ => "idle",
    };
    let dot = status_dot(&theme, e.status());
    let icon = e.icon.clone();
    let focused = ws_idx == root.ws.active && root.ws.active().active().is_some_and(|t| t.focused == pane);
    let second = format!("{} · {state}", root.ws.list[ws_idx].name);
    Some(
        div()
            .id(SharedString::from(format!("side-agent-{pane}")))
            .mx(px(6.))
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .flex()
            .gap_2()
            .items_start()
            .cursor_pointer()
            .text_size(px(12.))
            .when(focused, |d| d.bg(theme.text.opacity(0.08)).text_color(theme.text))
            .when(!focused, |d| d.text_color(theme.text_muted).hover(|s| s.bg(theme.text.opacity(0.04))))
            .child(div().pt(px(1.)).child(icon_element(
                &icon,
                14.,
                if focused { theme.text } else { theme.text_muted },
                cx,
            )))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .child(
                        div()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(root.pane_name(pane, cx)),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.text_muted)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(second),
                    ),
            )
            .children(dot.map(|c| div().mt(px(5.)).size(px(7.)).rounded_full().bg(c).flex_none()))
            .on_click(cx.listener(move |root, _, window, cx| root.reveal_pane(pane, window, cx)))
            .into_any_element(),
    )
}

/// The sidebar's bottom section: every running agent across all workspaces (SPEC 5.4); none
/// when no agent runs.
fn render_sidebar_agents(root: &Root, cx: &mut Context<Root>) -> Option<AnyElement> {
    if !Settings::global(cx).config.tabs.sidebar_agents {
        return None;
    }
    let theme = Theme::global(cx).clone();
    let rows: Vec<AnyElement> =
        root.live_agents(cx).into_iter().filter_map(|(w, p)| sidebar_agent_row(root, w, p, cx)).collect();
    if rows.is_empty() {
        return None;
    }
    let header = div()
        .px(px(14.))
        .pt(px(8.))
        .pb(px(4.))
        .text_size(px(11.))
        .text_color(theme.text_muted)
        .child(format!("Agents · {}", rows.len()));
    Some(
        div()
            .flex_none()
            .flex()
            .flex_col()
            .max_h(gpui::relative(0.45))
            .border_t_1()
            .border_color(theme.border)
            .child(header)
            .child(
                div()
                    .id("sidebar-agents-scroll")
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .pb(px(8.))
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element(),
    )
}

fn render_sidebar(root: &mut Root, cx: &mut Context<Root>) -> AnyElement {
    let theme = Theme::global(cx).clone();
    let width = root.sidebar_width(cx);
    let all = Settings::global(cx).config.tabs.sidebar_workspaces == nox_config::SidebarWorkspaces::All;
    let mut rows: Vec<AnyElement> = Vec::new();
    if all {
        // Every workspace as a group: its header row followed by its tabs (unless folded).
        for w in 0..root.ws.len() {
            let mut group = vec![sidebar_ws_header(root, w, true, cx)];
            if !root.ws.list[w].collapsed {
                for i in 0..root.ws.list[w].tabs.len() {
                    group.push(sidebar_row(root, w, i, false, cx));
                }
            }
            rows.push(sidebar_ws_group(w, group, 6., cx));
        }
    } else {
        // All workspaces on top, a separator line, then the active workspace's tabs.
        for w in 0..root.ws.len() {
            let header = sidebar_ws_header(root, w, false, cx);
            rows.push(sidebar_ws_group(w, vec![header], 0., cx));
        }
        rows.push(div().mx(px(14.)).my(px(4.)).h(px(1.)).bg(theme.border).into_any_element());
        let w = root.ws.active;
        for i in 0..root.ws.list[w].tabs.len() {
            rows.push(sidebar_row(root, w, i, true, cx));
        }
    }
    let agents = render_sidebar_agents(root, cx);
    div()
        .flex()
        .flex_none()
        .w(px(width))
        .h_full()
        .relative()
        .bg(theme.bg_sidebar)
        .child(
            div()
                .flex()
                .flex_col()
                .size_full()
                .child(
                    div()
                        .id("sidebar-scroll")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.))
                        .gap(px(2.))
                        .w_full()
                        .pb(px(8.))
                        .overflow_y_scroll()
                        .children(rows),
                )
                .children(agents),
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

#[cfg(test)]
mod tests {
    use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point};
    use nox_config::{SidebarWorkspaces, TabsPosition};

    use super::*;
    use crate::root::{DIVIDER_HIT_PX, DIVIDER_PX, Initial, RenameTarget};

    fn root_with(
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut nox_config::Config),
    ) -> (Entity<Root>, &mut VisualTestContext) {
        let mut config = crate::test_support::config();
        f(&mut config);
        cx.update(|cx| crate::test_support::init_app(cx, config));
        let (root, cx) = cx.add_window_view(|window, cx| Root::new(Initial::Fresh, window, cx));
        cx.run_until_parked();
        (root, cx)
    }

    /// Two workspaces (the second colored, with two tabs) and a split in the first.
    fn populate(root: &Entity<Root>, cx: &mut VisualTestContext) {
        root.update_in(cx, |r, window, cx| {
            r.new_tab(window, cx);
            r.split(SplitDirection::Vertical, None, window, cx).expect("room to split");
            let w = r.create_workspace(Some("second".into()), None, window, cx);
            r.ws.list[w].color = Some("cyan".into());
            r.ensure_started(w, window, cx);
            r.new_tab(window, cx);
            r.switch_workspace(0, window, cx);
        });
        cx.run_until_parked();
    }

    fn redraw(root: &Entity<Root>, cx: &mut VisualTestContext) {
        root.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
    }

    fn divider(root: &Entity<Root>, cx: &mut VisualTestContext) -> (nox_layout::Divider, Rect) {
        cx.update(|window, cx| {
            let r = root.read(cx);
            let area = r.layout_area(window, cx);
            let d = r.active_tab().unwrap().layout.dividers(area, DIVIDER_PX).remove(0);
            (d, area)
        })
    }

    fn first_pane_width(root: &Entity<Root>, cx: &mut VisualTestContext) -> f32 {
        cx.update(|window, cx| {
            let r = root.read(cx);
            let area = r.layout_area(window, cx);
            r.active_tab().unwrap().visible_rects(area, DIVIDER_PX)[0].1.w
        })
    }

    fn center(r: Rect) -> gpui::Point<gpui::Pixels> {
        point(px(r.x + r.w / 2.), px(r.y + r.h / 2.))
    }

    #[gpui::test]
    fn tabs_on_top(cx: &mut TestAppContext) {
        let (root, cx) = root_with(cx, |_| {});
        populate(&root, cx);
        root.read_with(cx, |r, cx| {
            assert!(r.tab_bar_shown(cx));
            assert!(!r.sidebar_shown(cx));
            assert_eq!(r.ws.list.len(), 2);
            assert_eq!(r.ws.active().tabs.len(), 2);
        });
        // Inactive window: finished tabs keep their dots; a zoomed pane shows its marker.
        root.update_in(cx, |r, window, cx| {
            r.window_active = false;
            window.dispatch_action(Box::new(crate::actions::ToggleMaximizePane), cx);
        });
        cx.run_until_parked();
        assert!(root.read_with(cx, |r, _| r.active_tab().unwrap().maximized.is_some()));
        // Renaming the active tab puts an input in its pill.
        root.update_in(cx, |r, window, cx| {
            let id = r.active_tab().unwrap().id;
            r.start_rename(RenameTarget::Tab(id), window, cx);
        });
        cx.run_until_parked();
        cx.simulate_input("renamed");
        cx.simulate_keystrokes("enter");
        root.read_with(cx, |r, _| {
            assert!(r.rename.is_none());
            assert_eq!(r.active_tab().unwrap().custom_name.as_deref(), Some("renamed"));
        });
    }

    #[gpui::test]
    fn tabs_on_the_bottom(cx: &mut TestAppContext) {
        let (root, cx) = root_with(cx, |c| c.tabs.position = TabsPosition::Bottom);
        populate(&root, cx);
        cx.update(|window, cx| {
            let r = root.read(cx);
            assert_eq!(r.titlebar_height(cx), crate::root::TITLEBAR_SLIM_PX);
            let area = r.layout_area(window, cx);
            let vp = window.viewport_size();
            assert!(area.y + area.h < f32::from(vp.height) - crate::root::BOTTOM_BAR_PX + 1.);
        });
        // The sidebar can replace the tab strip at the bottom too.
        cx.update(|_, cx| {
            let mut c = (*Settings::global(cx).config).clone();
            c.tabs.sidebar_replaces_tabs = true;
            crate::config_io::apply(cx, c, &[]);
        });
        redraw(&root, cx);
        root.read_with(cx, |r, cx| {
            assert!(r.sidebar_shown(cx));
            assert!(!r.tab_bar_shown(cx));
        });
    }

    #[gpui::test]
    fn sidebar_with_all_workspaces(cx: &mut TestAppContext) {
        let (root, cx) = root_with(cx, |c| {
            c.tabs.position = TabsPosition::Left;
            c.tabs.sidebar_workspaces = SidebarWorkspaces::All;
        });
        populate(&root, cx);
        root.read_with(cx, |r, cx| {
            assert!(r.sidebar_shown(cx));
            assert!(!r.tab_bar_shown(cx));
        });
        root.update_in(cx, |r, window, cx| {
            let id = r.active_tab().unwrap().id;
            r.start_rename(RenameTarget::Tab(id), window, cx);
        });
        redraw(&root, cx);
        cx.simulate_keystrokes("escape");
        assert!(root.read_with(cx, |r, _| r.rename.is_none()));
    }

    #[gpui::test]
    fn sidebar_with_the_active_workspace(cx: &mut TestAppContext) {
        let (root, cx) = root_with(cx, |c| {
            c.tabs.position = TabsPosition::Left;
            c.tabs.sidebar_workspaces = SidebarWorkspaces::Active;
            c.tabs.sidebar_agents = false;
            c.ui.show_git_branch = false;
        });
        populate(&root, cx);
        root.update_in(cx, |r, window, cx| r.switch_workspace(1, window, cx));
        redraw(&root, cx);
        // Hidden sidebar in left mode: no tabs anywhere but the card.
        cx.update(|_, cx| {
            let mut c = (*Settings::global(cx).config).clone();
            c.tabs.sidebar_visible = false;
            crate::config_io::apply(cx, c, &[]);
        });
        redraw(&root, cx);
        root.read_with(cx, |r, cx| {
            assert!(!r.sidebar_shown(cx));
            assert!(!r.tab_bar_shown(cx));
        });
    }

    #[gpui::test]
    fn overlays_render_over_the_card(cx: &mut TestAppContext) {
        let (root, cx) = root_with(cx, |_| {});
        cx.update(|window, cx| {
            window.dispatch_action(Box::new(crate::actions::CommandPalette), cx);
            crate::toast::show(cx, "a notice");
        });
        cx.run_until_parked();
        assert!(root.read_with(cx, |r, _| r.palette.is_some()));
        cx.simulate_keystrokes("escape");
        assert!(root.read_with(cx, |r, _| r.palette.is_none()));
        cx.update(|window, cx| crate::dialog::confirm(window, cx, "Sure?", "", "OK", |_, _| {}));
        cx.run_until_parked();
        assert!(cx.update(|window, cx| crate::dialog::is_open(window, cx)));
    }

    #[gpui::test]
    fn dragging_a_divider_resizes_and_double_click_equalizes(cx: &mut TestAppContext) {
        let (root, cx) = root_with(cx, |_| {});
        root.update_in(cx, |r, window, cx| {
            r.split(SplitDirection::Vertical, None, window, cx).unwrap();
        });
        cx.run_until_parked();
        let (d, area) = divider(&root, cx);
        assert_eq!(d.dir, SplitDirection::Vertical);
        let w0 = first_pane_width(&root, cx);
        let hit = d.hit_rect(DIVIDER_HIT_PX);

        cx.simulate_mouse_down(center(hit), MouseButton::Left, Modifiers::none());
        assert_eq!(root.read_with(cx, |r, _| r.drag_divider.clone()), Some(d.path.clone()));
        let target = point(px(area.x + area.w * 0.3), px(area.y + 50.));
        cx.simulate_mouse_move(target, Some(MouseButton::Left), Modifiers::none());
        let w1 = first_pane_width(&root, cx);
        assert!(w1 < w0 - 50., "{w1} < {w0}");
        cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::none());
        assert!(root.read_with(cx, |r, _| r.drag_divider.is_none()));

        // A move without the button held ends a drag that lost its mouse up.
        root.update(cx, |r, _| r.drag_divider = Some(d.path.clone()));
        cx.simulate_mouse_move(target, None, Modifiers::none());
        assert!(root.read_with(cx, |r, _| r.drag_divider.is_none()));

        let (d, _) = divider(&root, cx);
        cx.simulate_event(gpui::MouseDownEvent {
            button: MouseButton::Left,
            position: center(d.hit_rect(DIVIDER_HIT_PX)),
            modifiers: Modifiers::none(),
            click_count: 2,
            first_mouse: false,
        });
        let w2 = first_pane_width(&root, cx);
        assert!((w2 - w0).abs() <= 1., "equalized back: {w2} vs {w0}");
    }

    #[gpui::test]
    fn dragging_the_sidebar_edge_writes_its_width(cx: &mut TestAppContext) {
        let _g = crate::config_io::tests::fresh_config();
        let (root, cx) = root_with(cx, |c| c.tabs.position = TabsPosition::Left);
        let width = root.read_with(cx, Root::sidebar_width);
        let y = 300.;
        cx.simulate_mouse_down(point(px(width), px(y)), MouseButton::Left, Modifiers::none());
        assert!(root.read_with(cx, |r, _| r.drag_sidebar));
        cx.simulate_mouse_move(point(px(250.4), px(y)), Some(MouseButton::Left), Modifiers::none());
        assert!(crate::config_io::tests::config_text().contains("sidebar_width = 250"));
        assert!((root.read_with(cx, Root::sidebar_width) - 250.).abs() < f32::EPSILON);
        // Clamped to 160–360.
        cx.simulate_mouse_move(point(px(900.), px(y)), Some(MouseButton::Left), Modifiers::none());
        assert!((root.read_with(cx, Root::sidebar_width) - 360.).abs() < f32::EPSILON);
        cx.simulate_mouse_up(point(px(900.), px(y)), MouseButton::Left, Modifiers::none());
        assert!(!root.read_with(cx, |r, _| r.drag_sidebar));
        // Released elsewhere: a move without the button stops the drag.
        root.update(cx, |r, _| r.drag_sidebar = true);
        cx.simulate_mouse_move(point(px(200.), px(y)), None, Modifiers::none());
        assert!(!root.read_with(cx, |r, _| r.drag_sidebar));
    }

    fn bounds(cx: &mut VisualTestContext, sel: &'static str) -> gpui::Bounds<gpui::Pixels> {
        cx.debug_bounds(sel).unwrap_or_else(|| panic!("{sel} is rendered"))
    }

    /// Press at `from`, move past GPUI's drag threshold to `to`, release.
    fn drag(cx: &mut VisualTestContext, from: gpui::Point<gpui::Pixels>, to: gpui::Point<gpui::Pixels>) {
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(from + point(px(5.), px(0.)), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
    }

    fn tab_ids(root: &Entity<Root>, cx: &mut VisualTestContext, w: usize) -> Vec<TabId> {
        root.read_with(cx, |r, _| r.ws.list[w].tabs.iter().map(|t| t.id).collect())
    }

    #[gpui::test]
    fn dragging_tab_pills_reorders_and_empty_title_bar_moves_the_window(cx: &mut TestAppContext) {
        let (root, cx) = root_with(cx, |_| {});
        root.update_in(cx, |r, window, cx| {
            r.new_tab(window, cx);
            r.new_tab(window, cx);
        });
        redraw(&root, cx);
        let ids = tab_ids(&root, cx, 0);
        let moves = crate::window_bg::tests::moves();

        // First pill onto the third: it takes the third's place.
        let (p0, p2) = (bounds(cx, "tab-pill-0"), bounds(cx, "tab-pill-2"));
        drag(cx, p0.center(), p2.center());
        assert_eq!(tab_ids(&root, cx, 0), vec![ids[1], ids[2], ids[0]]);
        // Last onto the first: back to the start.
        let (p0, p2) = (bounds(cx, "tab-pill-0"), bounds(cx, "tab-pill-2"));
        drag(cx, p2.center(), p0.center());
        assert_eq!(tab_ids(&root, cx, 0), ids);
        // Onto the empty space after the strip: to the end.
        let (p0, tail) = (bounds(cx, "tab-pill-0"), bounds(cx, "titlebar-drag"));
        drag(cx, p0.center(), tail.center());
        assert_eq!(tab_ids(&root, cx, 0), vec![ids[1], ids[2], ids[0]]);
        assert_eq!(crate::window_bg::tests::moves(), moves, "pills never move the window");

        // A press on empty title bar space (after the strip, or between pills) moves the window.
        let tail = bounds(cx, "titlebar-drag");
        cx.simulate_mouse_down(tail.center(), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(tail.center(), MouseButton::Left, Modifiers::none());
        let (p0, p1) = (bounds(cx, "tab-pill-0"), bounds(cx, "tab-pill-1"));
        let gap = point(p0.right() + (p1.left() - p0.right()) / 2., p0.center().y);
        cx.simulate_mouse_down(gap, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(gap, MouseButton::Left, Modifiers::none());
        assert_eq!(crate::window_bg::tests::moves(), moves + 2);
    }

    #[gpui::test]
    fn overflowing_tabs_leave_room_to_grab_the_window(cx: &mut TestAppContext) {
        let (root, cx) = root_with(cx, |_| {});
        root.update_in(cx, |r, window, cx| {
            for _ in 0..30 {
                r.new_tab(window, cx);
            }
        });
        redraw(&root, cx);
        let tail = bounds(cx, "titlebar-drag");
        assert!(f32::from(tail.size.width) >= MIN_GRAB_PX - 0.5, "{tail:?}");
        let moves = crate::window_bg::tests::moves();
        cx.simulate_mouse_down(tail.center(), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(tail.center(), MouseButton::Left, Modifiers::none());
        assert_eq!(crate::window_bg::tests::moves(), moves + 1);

        // The newest (active) tab was scrolled into view: more to the left, nothing to the right.
        redraw(&root, cx);
        let state = |cx: &mut VisualTestContext| root.read_with(cx, |r, _| StripOverflow::of(&r.tab_strip));
        assert_eq!(state(cx), StripOverflow { overflows: true, more_left: true, more_right: false });
        let offset =
            |cx: &mut VisualTestContext| root.read_with(cx, |r, _| f32::from(r.tab_strip.offset().x));
        let x0 = offset(cx);
        // ‹ pages left, › back; neither moves the window.
        let arrow = bounds(cx, "tab-strip-left").center();
        cx.simulate_click(arrow, Modifiers::none());
        redraw(&root, cx);
        let x1 = offset(cx);
        assert!(x1 > x0 + 100., "{x0} -> {x1}");
        assert!(state(cx).more_right);
        let arrow = bounds(cx, "tab-strip-right").center();
        cx.simulate_click(arrow, Modifiers::none());
        redraw(&root, cx);
        assert!((offset(cx) - x0).abs() < 1., "back at the end");
        assert_eq!(crate::window_bg::tests::moves(), moves + 1);
        // Activating the first tab scrolls it into view.
        root.update_in(cx, |r, window, cx| r.activate_tab(0, window, cx));
        redraw(&root, cx);
        redraw(&root, cx);
        assert!(offset(cx).abs() < 1.);
        assert!(!state(cx).more_left);
    }

    #[gpui::test]
    fn few_tabs_show_no_strip_arrows(cx: &mut TestAppContext) {
        let (root, cx) = root_with(cx, |_| {});
        redraw(&root, cx);
        assert!(cx.debug_bounds("tab-strip-left").is_none());
        assert!(!root.read_with(cx, |r, _| StripOverflow::of(&r.tab_strip).overflows));
    }

    #[gpui::test]
    fn sidebar_workspaces_drag_to_reorder_and_fold(cx: &mut TestAppContext) {
        let (root, cx) = root_with(cx, |c| {
            c.tabs.position = TabsPosition::Left;
            c.tabs.sidebar_workspaces = SidebarWorkspaces::All;
            c.tabs.sidebar_agents = false;
        });
        populate(&root, cx);
        redraw(&root, cx);
        let names = |root: &Entity<Root>, cx: &mut VisualTestContext| {
            root.read_with(cx, |r, _| r.ws.list.iter().map(|w| w.name.clone()).collect::<Vec<_>>())
        };
        let before = names(&root, cx);
        // The first workspace's header onto the second's group.
        let (h0, h1) = (bounds(cx, "side-ws-0"), bounds(cx, "side-ws-1"));
        drag(cx, h0.center(), h1.center());
        assert_eq!(names(&root, cx), vec![before[1].clone(), before[0].clone()]);
        assert_eq!(root.read_with(cx, |r, _| r.ws.active().name.clone()), before[0], "still active");

        // The fold control collapses the group: its tabs leave the list.
        let rows = |cx: &mut VisualTestContext| {
            root.read_with(cx, |r, _| r.ws.list[0].tabs.iter().map(|t| t.id).collect::<Vec<_>>())
        };
        assert!(!rows(cx).is_empty());
        let fold = bounds(cx, "side-ws-fold-0");
        cx.simulate_click(fold.center(), Modifiers::none());
        redraw(&root, cx);
        root.read_with(cx, |r, _| {
            assert!(r.ws.list[0].collapsed);
            assert_ne!(r.ws.active, 0, "folding doesn't switch workspaces");
        });
        let h0 = bounds(cx, "side-ws-0");
        let h1 = bounds(cx, "side-ws-1");
        assert!(h1.top() - h0.bottom() < px(16.), "nothing between a folded header and the next");
        cx.simulate_click(fold.center(), Modifiers::none());
        assert!(!root.read_with(cx, |r, _| r.ws.list[0].collapsed));
    }

    #[gpui::test]
    fn dragged_tab_preview(cx: &mut TestAppContext) {
        crate::test_support::init(cx);
        let (d, cx) =
            cx.add_window_view(|_, _| DraggedTab { ws: 0, index: 0, tab: TabId(1), label: "zsh".into() });
        d.read_with(cx, |d, _| assert_eq!(d.label.as_ref(), "zsh"));
        let swatch = cx.read(|cx| Theme::global(cx).accent);
        let (w, cx) = cx.add_window_view(|_, _| DraggedWorkspace { ws: 0, label: "nox".into(), swatch });
        w.read_with(cx, |w, _| assert_eq!(w.label.as_ref(), "nox"));
    }

    #[gpui::test]
    fn status_dots_follow_the_status(cx: &mut TestAppContext) {
        crate::test_support::init(cx);
        cx.read(|cx| {
            let theme = Theme::global(cx);
            assert_eq!(status_dot(theme, TabStatus::None), None);
            assert!(status_dot(theme, TabStatus::NeedsInput).is_some());
            assert_ne!(status_dot(theme, TabStatus::Done), status_dot(theme, TabStatus::Failed));
        });
    }
}
