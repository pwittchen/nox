//! In-app sheets (SPEC §5.7): bold title, muted message, right-aligned buttons, the default
//! button monochrome-filled and focused, `Esc` cancels, `Tab` cycles. Optional text fields
//! (New Workspace, New Agent in Worktree…) and checkboxes (resume sheet).

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, Global, KeyBinding,
    SharedString, Window, actions, div, prelude::*, px,
};

use crate::text_input::{InputEvent, TextInput};
use crate::theme::Theme;

actions!(nox_dialog, [DialogCancel, DialogConfirm, DialogNextButton, DialogPrevButton]);

const CONTEXT: &str = "NoxDialog";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", DialogCancel, Some(CONTEXT)),
        KeyBinding::new("enter", DialogConfirm, Some(CONTEXT)),
        KeyBinding::new("tab", DialogNextButton, Some(CONTEXT)),
        KeyBinding::new("shift-tab", DialogPrevButton, Some(CONTEXT)),
    ]);
}

/// Values entered in the sheet: field texts and checkbox states.
#[derive(Debug, Clone, Default)]
pub struct DialogValues {
    pub fields: Vec<String>,
    pub checks: Vec<bool>,
}

pub type DialogCallback = Rc<dyn Fn(DialogValues, &mut Window, &mut App)>;

/// Extra content rendered below the fields.
pub type ExtraContent = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

pub struct Button {
    pub label: SharedString,
    pub on_click: Option<DialogCallback>,
}

pub struct Field {
    pub label: SharedString,
    pub input: Entity<TextInput>,
}

pub struct Check {
    pub label: SharedString,
    pub detail: Option<SharedString>,
    pub on: bool,
}

pub struct DialogView {
    pub title: SharedString,
    pub message: Option<SharedString>,
    pub detail: Option<String>,
    pub fields: Vec<Field>,
    pub checks: Vec<Check>,
    pub buttons: Vec<Button>,
    pub default_button: usize,
    pub focused_button: usize,
    pub extra: Option<ExtraContent>,
    focus: FocusHandle,
    restore_focus: Option<FocusHandle>,
}

#[derive(Default)]
pub struct ActiveDialog(pub Option<Entity<DialogView>>);
impl Global for ActiveDialog {}

impl Focusable for DialogView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

pub struct DialogBuilder {
    title: SharedString,
    message: Option<SharedString>,
    detail: Option<String>,
    fields: Vec<(SharedString, String, SharedString)>,
    checks: Vec<Check>,
    buttons: Vec<Button>,
    default_button: usize,
}

impl DialogBuilder {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            message: None,
            detail: None,
            fields: vec![],
            checks: vec![],
            buttons: vec![],
            default_button: 0,
        }
    }
    #[must_use]
    pub fn message(mut self, m: impl Into<SharedString>) -> Self {
        self.message = Some(m.into());
        self
    }
    /// Monospace detail text (git output, a diff, a list of commands).
    #[must_use]
    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = Some(d.into());
        self
    }
    #[must_use]
    pub fn field(
        mut self,
        label: impl Into<SharedString>,
        value: impl Into<String>,
        placeholder: impl Into<SharedString>,
    ) -> Self {
        self.fields.push((label.into(), value.into(), placeholder.into()));
        self
    }
    #[must_use]
    pub fn check(mut self, label: impl Into<SharedString>, detail: Option<String>, on: bool) -> Self {
        self.checks.push(Check { label: label.into(), detail: detail.map(Into::into), on });
        self
    }
    #[must_use]
    pub fn cancel(mut self, label: impl Into<SharedString>) -> Self {
        self.buttons.push(Button { label: label.into(), on_click: None });
        self
    }
    #[must_use]
    pub fn button(
        mut self,
        label: impl Into<SharedString>,
        f: impl Fn(DialogValues, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.buttons.push(Button { label: label.into(), on_click: Some(Rc::new(f)) });
        self
    }
    /// The last added button is the default.
    #[must_use]
    pub fn default_last(mut self) -> Self {
        self.default_button = self.buttons.len().saturating_sub(1);
        self
    }

    pub fn show(self, window: &mut Window, cx: &mut App) {
        let restore = window.focused(cx);
        let view = cx.new(|cx: &mut Context<DialogView>| {
            let focus = cx.focus_handle();
            let fields = self
                .fields
                .into_iter()
                .map(|(label, value, placeholder)| {
                    let input = cx.new(|cx| {
                        let mut i = TextInput::new(cx, placeholder);
                        i.set_text(value, cx);
                        i
                    });
                    cx.subscribe_in(
                        &input,
                        window,
                        |this: &mut DialogView, _, ev: &InputEvent, window, cx| match ev {
                            InputEvent::Submit => this.confirm(&DialogConfirm, window, cx),
                            InputEvent::Cancel => this.cancel(&DialogCancel, window, cx),
                            _ => {}
                        },
                    )
                    .detach();
                    Field { label, input }
                })
                .collect();
            DialogView {
                title: self.title,
                message: self.message,
                detail: self.detail,
                fields,
                checks: self.checks,
                buttons: self.buttons,
                default_button: self.default_button,
                focused_button: self.default_button,
                extra: None,
                focus,
                restore_focus: restore,
            }
        });
        {
            let v = view.read(cx);
            if let Some(f) = v.fields.first() {
                f.input.read(cx).focus(window);
            } else {
                window.focus(&v.focus);
            }
        }
        cx.set_global(ActiveDialog(Some(view)));
        window.refresh();
    }
}

/// A two-button confirmation (`Cancel` / `<label>`).
pub fn confirm(
    window: &mut Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    message: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on_confirm: impl Fn(&mut Window, &mut App) + 'static,
) {
    DialogBuilder::new(title)
        .message(message)
        .cancel("Cancel")
        .button(label, move |_, w, cx| on_confirm(w, cx))
        .default_last()
        .show(window, cx);
}

pub fn is_open(cx: &App) -> bool {
    cx.try_global::<ActiveDialog>().is_some_and(|d| d.0.is_some())
}

fn close(window: &mut Window, cx: &mut App, restore: Option<FocusHandle>) {
    cx.set_global(ActiveDialog(None));
    if let Some(f) = restore {
        window.focus(&f);
    }
    window.refresh();
}

impl DialogView {
    fn values(&self, cx: &App) -> DialogValues {
        DialogValues {
            fields: self.fields.iter().map(|f| f.input.read(cx).text().to_string()).collect(),
            checks: self.checks.iter().map(|c| c.on).collect(),
        }
    }

    fn press(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let values = self.values(cx);
        let cb = self.buttons.get(index).and_then(|b| b.on_click.clone());
        let restore = self.restore_focus.clone();
        close(window, cx, restore);
        if let Some(cb) = cb {
            cb(values, window, cx);
        }
    }

    fn cancel(&mut self, _: &DialogCancel, window: &mut Window, cx: &mut Context<Self>) {
        let restore = self.restore_focus.clone();
        close(window, cx, restore);
    }

    fn confirm(&mut self, _: &DialogConfirm, window: &mut Window, cx: &mut Context<Self>) {
        let idx = if self.focus.is_focused(window) { self.focused_button } else { self.default_button };
        self.press(idx, window, cx);
    }

    fn next(&mut self, _: &DialogNextButton, window: &mut Window, cx: &mut Context<Self>) {
        if !self.buttons.is_empty() {
            self.focused_button = (self.focused_button + 1) % self.buttons.len();
            window.focus(&self.focus);
            cx.notify();
        }
    }

    fn prev(&mut self, _: &DialogPrevButton, window: &mut Window, cx: &mut Context<Self>) {
        if !self.buttons.is_empty() {
            self.focused_button = (self.focused_button + self.buttons.len() - 1) % self.buttons.len();
            window.focus(&self.focus);
            cx.notify();
        }
    }
}

impl Render for DialogView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx).clone();
        let buttons_focused = self.focus.is_focused(window);
        let fields: Vec<_> = self
            .fields
            .iter()
            .map(|f| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_size(px(11.)).text_color(theme.text_muted).child(f.label.clone()))
                    .child(
                        div()
                            .px(px(8.))
                            .py(px(5.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.bg)
                            .text_size(px(13.))
                            .child(f.input.clone()),
                    )
            })
            .collect();
        let checks: Vec<_> = self
            .checks
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let on = c.on;
                div()
                    .id(("dialog-check", i))
                    .flex()
                    .items_start()
                    .gap_2()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(c) = this.checks.get_mut(i) {
                            c.on = !c.on;
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .mt(px(2.))
                            .size(px(14.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(if on { theme.accent } else { theme.border })
                            .when(on, |d| d.bg(theme.accent))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(10.))
                            .text_color(theme.bg)
                            .child(if on { "✓" } else { "" }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(13.)).child(c.label.clone()))
                            .children(
                                c.detail
                                    .clone()
                                    .map(|d| div().text_size(px(11.)).text_color(theme.text_muted).child(d)),
                            ),
                    )
            })
            .collect();
        let buttons: Vec<_> = self
            .buttons
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let is_default = i == self.default_button;
                let ring = buttons_focused && i == self.focused_button;
                div()
                    .id(("dialog-button", i))
                    .px(px(14.))
                    .py(px(5.))
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .cursor_pointer()
                    .when(is_default, |d| d.bg(theme.text).text_color(theme.bg))
                    .when(!is_default, |d| d.border_1().border_color(theme.border).text_color(theme.text))
                    .when(ring, |d| d.border_2().border_color(theme.accent))
                    .hover(|s| s.opacity(0.9))
                    .on_click(cx.listener(move |this, _, window, cx| this.press(i, window, cx)))
                    .child(b.label.clone())
            })
            .collect();
        let extra = self.extra.clone().map(|f| f(window, cx));
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::next))
            .on_action(cx.listener(Self::prev))
            .w(px(440.))
            .max_h(px(560.))
            .flex()
            .flex_col()
            .gap_3()
            .p(px(20.))
            .rounded(px(12.))
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border)
            .shadow_lg()
            .text_color(theme.text)
            .child(div().text_size(px(14.)).font_weight(gpui::FontWeight::BOLD).child(self.title.clone()))
            .children(
                self.message.clone().map(|m| div().text_size(px(13.)).text_color(theme.text_muted).child(m)),
            )
            .children(self.detail.clone().map(|d| {
                div()
                    .id("dialog-detail")
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .p(px(8.))
                    .rounded(px(6.))
                    .bg(theme.bg)
                    .border_1()
                    .border_color(theme.border)
                    .font_family(crate::settings_state::Settings::global(cx).term_font.family.clone())
                    .text_size(px(11.))
                    .children(d.lines().map(|l| div().child(l.to_string())).collect::<Vec<_>>())
            }))
            .children(fields)
            .children(checks)
            .children(extra)
            .child(div().flex().justify_end().gap_2().pt(px(4.)).children(buttons))
    }
}

/// The overlay (dimmed backdrop + centered sheet) for the root view.
pub fn render_overlay(cx: &App) -> Option<AnyElement> {
    let view = cx.try_global::<ActiveDialog>()?.0.clone()?;
    let theme = Theme::global(cx);
    Some(
        div()
            .id("dialog-backdrop")
            .absolute()
            .inset_0()
            .bg(gpui::black().opacity(if theme.dark { 0.35 } else { 0.18 }))
            .flex()
            .items_start()
            .justify_center()
            .pt(px(80.))
            .occlude()
            .child(view)
            .into_any_element(),
    )
}
