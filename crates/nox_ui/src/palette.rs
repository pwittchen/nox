//! The command palette and pickers (SPEC §5.6): a floating panel below the title bar with a
//! search field, a fuzzy-filtered list with right-aligned hints, `↑` / `↓` to move, `↵` to run,
//! `Esc` to close. Theme / appearance pickers live-preview the highlighted entry. The same view
//! backs Switch Tab…, Switch Workspace…, the agents panel and the other pickers.

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla,
    KeyBinding, ScrollStrategy, SharedString, Subscription, UniformListScrollHandle, Window, actions, div,
    prelude::*, px, uniform_list,
};

use crate::text_input::{InputEvent, TextInput};
use crate::theme::Theme;
use crate::util::fuzzy_score;

actions!(nox_palette, [SelectPrev, SelectNext, Confirm, Dismiss, NextTab, PrevTab]);

const CONTEXT: &str = "NoxPalette";

/// Width of the floating panel.
pub const PALETTE_WIDTH: f32 = 560.;
/// Rows shown before the list scrolls.
const MAX_ROWS: usize = 12;
const ROW_H: f32 = 32.;
/// Search field + list + footer at the most rows; used to center the panel.
pub const PALETTE_MAX_HEIGHT: f32 = 42. + 8. + MAX_ROWS as f32 * ROW_H + 28.;

pub fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", SelectPrev, c),
        KeyBinding::new("down", SelectNext, c),
        KeyBinding::new("ctrl-p", SelectPrev, c),
        KeyBinding::new("ctrl-n", SelectNext, c),
        KeyBinding::new("enter", Confirm, c),
        KeyBinding::new("escape", Dismiss, c),
        KeyBinding::new("tab", NextTab, c),
        KeyBinding::new("shift-tab", PrevTab, c),
    ]);
}

pub type Action = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone, Default)]
pub struct PaletteItem {
    pub label: SharedString,
    /// Muted text after the label (cwd, workspace, last message).
    pub detail: Option<SharedString>,
    /// Right-aligned hint (shortcut, state, duration).
    pub hint: Option<SharedString>,
    pub icon: Option<SharedString>,
    pub icon_color: Option<Hsla>,
    /// A status dot / color swatch before the label.
    pub dot: Option<Hsla>,
    /// Extra text matched by the filter but not shown.
    pub keywords: Option<String>,
    /// A non-selectable group header.
    pub header: bool,
    pub on_select: Option<Action>,
    pub on_highlight: Option<Action>,
}

impl PaletteItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self { label: label.into(), ..Default::default() }
    }
    pub fn header(label: impl Into<SharedString>) -> Self {
        Self { label: label.into(), header: true, ..Default::default() }
    }
    #[must_use]
    pub fn detail(mut self, d: impl Into<SharedString>) -> Self {
        self.detail = Some(d.into());
        self
    }
    #[must_use]
    pub fn hint(mut self, h: impl Into<SharedString>) -> Self {
        self.hint = Some(h.into());
        self
    }
    #[must_use]
    pub fn icon(mut self, path: impl Into<SharedString>, color: Option<Hsla>) -> Self {
        self.icon = Some(path.into());
        self.icon_color = color;
        self
    }
    #[must_use]
    pub fn dot(mut self, c: Option<Hsla>) -> Self {
        self.dot = c;
        self
    }
    #[must_use]
    pub fn keywords(mut self, k: impl Into<String>) -> Self {
        self.keywords = Some(k.into());
        self
    }
    #[must_use]
    pub fn on_select(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
    #[must_use]
    pub fn on_highlight(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_highlight = Some(Rc::new(f));
        self
    }
}

pub type Source = Rc<dyn Fn(usize, &mut App) -> Vec<PaletteItem>>;

pub enum PaletteEvent {
    Dismissed,
}

pub struct Palette {
    input: Entity<TextInput>,
    source: Source,
    tabs: Vec<SharedString>,
    tab: usize,
    items: Vec<PaletteItem>,
    filtered: Vec<usize>,
    selected: usize,
    on_cancel: Option<Action>,
    footer: SharedString,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    keep_order: bool,
    _sub: Subscription,
}

impl EventEmitter<PaletteEvent> for Palette {}

impl Focusable for Palette {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

pub struct PaletteOptions {
    pub placeholder: SharedString,
    pub tabs: Vec<SharedString>,
    pub on_cancel: Option<Action>,
    pub initial_query: String,
    pub footer: SharedString,
    /// Keep the source's order when filtering (pickers sorted by relevance already).
    pub keep_order: bool,
    pub preselect: Option<usize>,
}

impl Default for PaletteOptions {
    fn default() -> Self {
        Self {
            placeholder: "Type a command…".into(),
            tabs: vec![],
            on_cancel: None,
            initial_query: String::new(),
            footer: "↑↓ navigate   ↵ run   esc close".into(),
            keep_order: false,
            preselect: None,
        }
    }
}

impl Palette {
    pub fn new(source: Source, opts: PaletteOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            let mut i = TextInput::new(cx, opts.placeholder.clone());
            i.set_text_cursor_end(opts.initial_query.clone(), cx);
            i
        });
        let sub =
            cx.subscribe_in(&input, window, |this: &mut Self, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Changed => {
                    this.refilter(cx);
                    this.highlight(window, cx);
                }
                InputEvent::Submit => this.confirm(&Confirm, window, cx),
                InputEvent::Cancel => this.dismiss(&Dismiss, window, cx),
                InputEvent::Blur => {}
            });
        input.read(cx).focus(window);
        let mut this = Self {
            input,
            source,
            tabs: opts.tabs,
            tab: 0,
            items: vec![],
            filtered: vec![],
            selected: 0,
            on_cancel: opts.on_cancel,
            footer: opts.footer,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            keep_order: opts.keep_order,
            _sub: sub,
        };
        this.reload(cx);
        if let Some(i) = opts.preselect
            && let Some(pos) = this.filtered.iter().position(|&f| f == i)
        {
            this.selected = pos;
        }
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.items = (self.source)(self.tab, cx);
        self.refilter(cx);
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let q = self.input.read(cx).text().to_string();
        let mut scored: Vec<(i32, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| {
                if it.header {
                    return q.is_empty().then_some((0, i));
                }
                let hay = match &it.keywords {
                    Some(k) => {
                        format!(
                            "{} {} {}",
                            it.label,
                            it.detail.as_ref().map_or("", std::convert::AsRef::as_ref),
                            k
                        )
                    }
                    None => {
                        format!("{} {}", it.label, it.detail.as_ref().map_or("", std::convert::AsRef::as_ref))
                    }
                };
                fuzzy_score(&q, &hay).map(|s| (s, i))
            })
            .collect();
        if !q.is_empty() && !self.keep_order {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        self.filtered = scored.into_iter().map(|(_, i)| i).collect();
        self.selected = self.filtered.iter().position(|&i| !self.items[i].header).unwrap_or(0);
        cx.notify();
    }

    fn highlight(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(&i) = self.filtered.get(self.selected)
            && let Some(f) = self.items[i].on_highlight.clone()
        {
            f(window, cx);
        }
        self.scroll.scroll_to_item(self.selected, ScrollStrategy::Top);
    }

    fn move_by(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.filtered.len();
        if n == 0 {
            return;
        }
        let mut idx = self.selected as i32;
        for _ in 0..n {
            idx = (idx + delta).rem_euclid(n as i32);
            if !self.items[self.filtered[idx as usize]].header {
                break;
            }
        }
        self.selected = idx as usize;
        self.highlight(window, cx);
        cx.notify();
    }

    fn select_prev(&mut self, _: &SelectPrev, window: &mut Window, cx: &mut Context<Self>) {
        self.move_by(-1, window, cx);
    }
    fn select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        self.move_by(1, window, cx);
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let Some(&i) = self.filtered.get(self.selected) else { return };
        let Some(f) = self.items[i].on_select.clone() else { return };
        cx.emit(PaletteEvent::Dismissed);
        f(window, cx);
    }

    fn run_index(&mut self, pos: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = pos;
        self.confirm(&Confirm, window, cx);
    }

    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(f) = self.on_cancel.clone() {
            f(window, cx);
        }
        cx.emit(PaletteEvent::Dismissed);
    }

    fn next_tab(&mut self, _: &NextTab, _: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() > 1 {
            self.tab = (self.tab + 1) % self.tabs.len();
            self.reload(cx);
        }
    }
    fn prev_tab(&mut self, _: &PrevTab, _: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() > 1 {
            self.tab = (self.tab + self.tabs.len() - 1) % self.tabs.len();
            self.reload(cx);
        }
    }

    fn render_row(&self, pos: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::global(cx).clone();
        let item = &self.items[self.filtered[pos]];
        if item.header {
            return div()
                .id(("palette-header", pos))
                .h(px(28.))
                .px(px(12.))
                .pt(px(8.))
                .text_size(px(11.))
                .text_color(theme.text_muted)
                .child(item.label.clone())
                .into_any_element();
        }
        let selected = pos == self.selected;
        div()
            .id(("palette-row", pos))
            .h(px(32.))
            .w_full()
            .mx(px(6.))
            .px(px(8.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .when(selected, |d| d.bg(theme.text.opacity(0.08)))
            .hover(|s| s.bg(theme.text.opacity(0.05)))
            .on_click(cx.listener(move |this, _, window, cx| this.run_index(pos, window, cx)))
            .children(item.dot.map(|c| div().size(px(8.)).rounded_full().bg(c).flex_none()))
            .children(item.icon.clone().map(|p| {
                gpui::svg()
                    .path(p)
                    .size(px(14.))
                    .flex_none()
                    .text_color(item.icon_color.unwrap_or(theme.text_muted))
            }))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap_2()
                    .items_baseline()
                    .overflow_hidden()
                    .child(div().flex_none().text_color(theme.text).child(item.label.clone()))
                    .children(item.detail.clone().map(|d| {
                        div()
                            .text_color(theme.text_muted)
                            .text_size(px(11.))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(d)
                    })),
            )
            .children(
                item.hint
                    .clone()
                    .map(|h| div().flex_none().text_size(px(11.)).text_color(theme.text_muted).child(h)),
            )
            .into_any_element()
    }
}

impl Render for Palette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx).clone();
        let count = self.filtered.len();
        let list_h = (count.min(MAX_ROWS) as f32 * ROW_H).max(ROW_H);
        let tabs = (!self.tabs.is_empty()).then(|| {
            div().flex().gap_1().children(self.tabs.iter().enumerate().map(|(i, t)| {
                let on = i == self.tab;
                div()
                    .id(("palette-tab", i))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(6.))
                    .text_size(px(11.))
                    .cursor_pointer()
                    .text_color(if on { theme.text } else { theme.text_muted })
                    .when(on, |d| d.bg(theme.text.opacity(0.08)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = i;
                        this.reload(cx);
                    }))
                    .child(t.clone())
            }))
        });
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            // A click anywhere outside closes it, like Esc (pickers restore their preview).
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.dismiss(&Dismiss, window, cx)))
            .w_full()
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border)
            .shadow_lg()
            .text_size(px(13.))
            .text_color(theme.text)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px(px(14.))
                    .h(px(42.))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(gpui::svg().path("icons/ui/search.svg").size(px(14.)).text_color(theme.text_muted))
                    .child(self.input.clone())
                    .children(tabs),
            )
            .child(div().py(px(4.)).child(if count == 0 {
                div()
                    .h(px(32.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .text_color(theme.text_muted)
                    .child("No matches")
                    .into_any_element()
            } else {
                uniform_list(
                    "palette-list",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range.map(|i| this.render_row(i, cx)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(self.scroll.clone())
                .w_full()
                .pr(px(12.))
                .h(px(list_h))
                .into_any_element()
            }))
            .child(
                div()
                    .h(px(28.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_color(theme.border)
                    .text_size(px(11.))
                    .text_color(theme.text_muted)
                    .child(self.footer.clone()),
            )
    }
}
