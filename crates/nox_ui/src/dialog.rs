//! In-app sheets (SPEC §5.7): bold title, muted message, right-aligned buttons, the default
//! button monochrome-filled and focused, `Esc` cancels, `Tab` cycles. Optional text fields
//! (New Workspace, New Agent in Worktree…) and checkboxes (resume sheet).

use std::rc::Rc;

use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Context, Entity, FocusHandle, Focusable, Global,
    KeyBinding, SharedString, Window, actions, div, prelude::*, px,
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

/// The open sheet and the window it belongs to (the one that opened it, e.g. Settings), so it
/// renders there and not behind it in the main window.
#[derive(Default)]
pub struct ActiveDialog(pub Option<(Entity<DialogView>, AnyWindowHandle)>);
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
        cx.set_global(ActiveDialog(Some((view, window.window_handle()))));
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

/// Whether a sheet is open in `window`.
pub fn is_open(window: &Window, cx: &App) -> bool {
    cx.try_global::<ActiveDialog>()
        .and_then(|d| d.0.as_ref())
        .is_some_and(|(_, w)| *w == window.window_handle())
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
                // The focus ring is a separate outline 2 px outside the button (always laid out,
                // transparent when unfocused), so focus doesn't resize the button and the fill
                // never sits under a differently colored border.
                div()
                    .id(("dialog-button", i))
                    .p(px(2.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(if ring { theme.accent } else { gpui::transparent_black() })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| this.press(i, window, cx)))
                    .child(
                        div()
                            .px(px(14.))
                            .py(px(4.))
                            .rounded(px(6.))
                            .border_1()
                            .text_size(px(13.))
                            .when(is_default, |d| {
                                d.bg(theme.text).border_color(theme.text).text_color(theme.bg)
                            })
                            .when(!is_default, |d| {
                                d.border_color(theme.border)
                                    .text_color(theme.text)
                                    .hover(|s| s.bg(theme.text.opacity(0.05)))
                            })
                            .when(is_default, |d| d.hover(|s| s.opacity(0.9)))
                            .child(b.label.clone()),
                    )
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
            .child(div().flex().justify_end().gap_1().pt(px(1.)).mr(px(-3.)).children(buttons))
    }
}

/// The overlay (dimmed backdrop + centered sheet) for a window's root view; `None` unless the
/// open sheet belongs to `window`.
pub fn render_overlay(window: &Window, cx: &App) -> Option<AnyElement> {
    let (view, handle) = cx.try_global::<ActiveDialog>()?.0.clone()?;
    if handle != window.window_handle() {
        return None;
    }
    let theme = Theme::global(cx);
    let backdrop = gpui::black().opacity(if theme.dark { 0.35 } else { 0.18 });
    // The backdrop fades in with the sheet; the sheet also slides down a little (SPEC §5.1).
    let sheet = crate::util::fade_slide_in(
        "dialog-appear",
        crate::settings_state::Settings::global(cx).reduce_motion(),
        div().child(view),
        |el, dy| el.mt(px(dy)),
    );
    let fade = crate::util::fade_slide_in(
        "dialog-backdrop-appear",
        crate::settings_state::Settings::global(cx).reduce_motion(),
        div().absolute().inset_0().bg(backdrop),
        |el, _| el,
    );
    Some(
        div()
            .id("dialog-backdrop")
            .absolute()
            .inset_0()
            .child(fade)
            .child(div().absolute().inset_0().flex().items_start().justify_center().pt(px(80.)).child(sheet))
            .occlude()
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui::{TestAppContext, VisualTestContext};

    use super::*;

    type Log = Rc<RefCell<Vec<String>>>;

    /// A window with something focused that renders the sheet overlay, like `Root`.
    struct Host {
        focus: FocusHandle,
    }
    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().track_focus(&self.focus).children(render_overlay(window, cx))
        }
    }

    fn host(cx: &mut TestAppContext) -> (Entity<Host>, &mut VisualTestContext) {
        crate::test_support::init(cx);
        cx.update(bind_keys);
        let (host, cx) = cx.add_window_view(|_, cx| Host { focus: cx.focus_handle() });
        host.update_in(cx, |h, window, _| window.focus(&h.focus));
        (host, cx)
    }

    fn show(cx: &mut VisualTestContext, build: impl FnOnce() -> DialogBuilder) {
        let b = build();
        cx.update(|window, cx| b.show(window, cx));
        cx.run_until_parked();
    }

    fn view(cx: &mut VisualTestContext) -> Option<Entity<DialogView>> {
        cx.read(|cx| cx.try_global::<ActiveDialog>().and_then(|d| d.0.as_ref().map(|(v, _)| v.clone())))
    }

    fn is_open_here(cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| is_open(window, cx))
    }

    fn host_focused(host: &Entity<Host>, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| host.read(cx).focus.is_focused(window))
    }

    fn recorder(log: &Log, name: &'static str) -> impl Fn(DialogValues, &mut Window, &mut App) + 'static {
        let log = log.clone();
        move |v, _, _| log.borrow_mut().push(format!("{name} {:?} {:?}", v.fields, v.checks))
    }

    #[gpui::test]
    fn confirm_runs_on_enter(cx: &mut TestAppContext) {
        let (host, cx) = host(cx);
        let log: Log = Rc::default();
        let l = log.clone();
        cx.update(|window, cx| {
            confirm(window, cx, "Close tab?", "A process is running.", "Close", move |_, _| {
                l.borrow_mut().push("closed".into());
            });
        });
        cx.run_until_parked();
        assert!(is_open_here(cx));
        let v = view(cx).unwrap();
        v.read_with(cx, |v, _| {
            assert_eq!(v.title.as_ref(), "Close tab?");
            assert_eq!(v.message.as_ref().map(AsRef::as_ref), Some("A process is running."));
            let labels: Vec<_> = v.buttons.iter().map(|b| b.label.to_string()).collect();
            assert_eq!(labels, ["Cancel", "Close"]);
            assert_eq!(v.default_button, 1);
            assert_eq!(v.focused_button, 1);
        });
        cx.simulate_keystrokes("enter");
        assert_eq!(*log.borrow(), ["closed"]);
        assert!(!is_open_here(cx));
        assert!(host_focused(&host, cx), "focus goes back to where it was");
    }

    #[gpui::test]
    fn escape_cancels_without_running_anything(cx: &mut TestAppContext) {
        let (host, cx) = host(cx);
        let log: Log = Rc::default();
        let l = log.clone();
        cx.update(|window, cx| {
            confirm(window, cx, "Quit?", "", "Quit", move |_, _| l.borrow_mut().push("quit".into()));
        });
        cx.run_until_parked();
        assert!(!host_focused(&host, cx), "the sheet takes focus");
        cx.simulate_keystrokes("escape");
        assert!(log.borrow().is_empty());
        assert!(view(cx).is_none());
        assert!(host_focused(&host, cx));
    }

    #[gpui::test]
    fn tab_cycles_buttons_and_enter_presses_the_focused_one(cx: &mut TestAppContext) {
        let (_, cx) = host(cx);
        let log: Log = Rc::default();
        let l = log.clone();
        show(cx, || {
            DialogBuilder::new("Resume agents?")
                .detail("claude\ncodex")
                .cancel("Not Now")
                .button("Some", recorder(&l, "some"))
                .button("All", recorder(&l, "all"))
                .default_last()
        });
        let v = view(cx).unwrap();
        cx.simulate_keystrokes("tab");
        assert_eq!(v.read_with(cx, |v, _| v.focused_button), 0, "wraps to the first");
        cx.simulate_keystrokes("tab");
        assert_eq!(v.read_with(cx, |v, _| v.focused_button), 1);
        cx.simulate_keystrokes("shift-tab shift-tab");
        assert_eq!(v.read_with(cx, |v, _| v.focused_button), 2);
        cx.simulate_keystrokes("shift-tab enter");
        assert_eq!(*log.borrow(), ["some [] []"]);
        assert!(view(cx).is_none());
    }

    #[gpui::test]
    fn fields_and_checks_are_passed_to_the_button(cx: &mut TestAppContext) {
        let (_, cx) = host(cx);
        let log: Log = Rc::default();
        let l = log.clone();
        show(cx, || {
            DialogBuilder::new("New Workspace")
                .field("Name", "untitled", "name")
                .field("Folder", "", "~/src")
                .check("Open a tab", Some("in the folder".into()), true)
                .check("Start an agent", None, false)
                .cancel("Cancel")
                .button("Create", recorder(&l, "create"))
                .default_last()
        });
        let v = view(cx).unwrap();
        // The first field has focus with its text selected: typing replaces it.
        cx.simulate_input("api");
        v.update(cx, |v, cx| {
            v.checks[1].on = true;
            cx.notify();
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        assert_eq!(*log.borrow(), [r#"create ["api", ""] [true, true]"#]);
    }

    #[gpui::test]
    fn escape_in_a_field_cancels(cx: &mut TestAppContext) {
        let (_, cx) = host(cx);
        let log: Log = Rc::default();
        let l = log.clone();
        show(cx, || DialogBuilder::new("Rename").field("Name", "x", "").button("OK", recorder(&l, "ok")));
        cx.simulate_keystrokes("escape");
        assert!(view(cx).is_none());
        assert!(log.borrow().is_empty());
    }

    #[gpui::test]
    fn tab_then_enter_from_a_field_presses_the_focused_button(cx: &mut TestAppContext) {
        let (_, cx) = host(cx);
        let log: Log = Rc::default();
        let l = log.clone();
        show(cx, || {
            DialogBuilder::new("Rename")
                .field("Name", "x", "")
                .cancel("Cancel")
                .button("OK", recorder(&l, "ok"))
                .default_last()
        });
        // In a field, enter is the default button; on the buttons row the focused one.
        cx.simulate_keystrokes("tab enter");
        assert!(log.borrow().is_empty(), "Cancel was focused: {:?}", log.borrow());
        assert!(view(cx).is_none());
    }

    #[gpui::test]
    fn a_sheet_only_shows_in_its_window(cx: &mut TestAppContext) {
        let (_, vcx) = host(cx);
        show(vcx, || DialogBuilder::new("Mine").cancel("OK"));
        let other = cx.add_empty_window();
        other.update(|window, cx| {
            assert!(!is_open(window, cx));
            assert!(render_overlay(window, cx).is_none());
        });
    }

    #[gpui::test]
    fn extra_content_renders_and_buttonless_sheets_stay_put(cx: &mut TestAppContext) {
        let (_, cx) = host(cx);
        let rendered: Log = Rc::default();
        show(cx, || DialogBuilder::new("Info").message("Nothing to press"));
        let v = view(cx).unwrap();
        let r = rendered.clone();
        v.update(cx, |v, cx| {
            v.extra = Some(Rc::new(move |_, _| {
                r.borrow_mut().push("extra".into());
                div().child("extra").into_any_element()
            }));
            cx.notify();
        });
        cx.run_until_parked();
        assert!(!rendered.borrow().is_empty());
        // Tab with no buttons does nothing; enter presses nothing but closes.
        cx.simulate_keystrokes("tab shift-tab");
        assert!(view(cx).is_some());
        cx.simulate_keystrokes("enter");
        assert!(view(cx).is_none());
    }
}
