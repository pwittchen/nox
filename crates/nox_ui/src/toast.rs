//! Non-blocking notices (SPEC §5.7): bottom-right of the window, 4 s.

use std::time::Duration;

use gpui::{AnyElement, App, Global, IntoElement, ParentElement, Styled, div, px};

use crate::theme::Theme;

#[derive(Default)]
pub struct Toasts {
    items: Vec<(u64, String)>,
    next: u64,
}

impl Global for Toasts {}

/// Show a notice for 4 seconds.
pub fn show(cx: &mut App, message: impl Into<String>) {
    let message = message.into();
    log::info!("notice: {message}");
    let id = {
        let t = cx.default_global::<Toasts>();
        t.next += 1;
        let id = t.next;
        // Don't stack identical notices.
        t.items.retain(|(_, m)| *m != message);
        t.items.push((id, message));
        if t.items.len() > 4 {
            t.items.remove(0);
        }
        id
    };
    cx.refresh_windows();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(Duration::from_secs(4)).await;
        let _ = cx.update(|cx| {
            cx.default_global::<Toasts>().items.retain(|(i, _)| *i != id);
            cx.refresh_windows();
        });
    })
    .detach();
}

pub fn render(cx: &App) -> Option<AnyElement> {
    let toasts = cx.try_global::<Toasts>()?;
    if toasts.items.is_empty() {
        return None;
    }
    let theme = Theme::global(cx);
    Some(
        div()
            .absolute()
            .bottom(px(16.))
            .right(px(16.))
            .flex()
            .flex_col()
            .items_end()
            .gap_2()
            .children(toasts.items.iter().map(|(_, m)| {
                div()
                    .max_w(px(420.))
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(10.))
                    .bg(theme.bg_elevated)
                    .border_1()
                    .border_color(theme.border)
                    .shadow_md()
                    .text_size(px(12.))
                    .text_color(theme.text)
                    .child(m.clone())
            }))
            .into_any_element(),
    )
}

/// The notices currently shown, oldest first (tests of other modules assert on them).
#[cfg(test)]
pub(crate) fn messages(cx: &App) -> Vec<String> {
    cx.try_global::<Toasts>().map(|t| t.items.iter().map(|(_, m)| m.clone()).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn notices_expire_after_four_seconds(cx: &mut TestAppContext) {
        crate::test_support::init(cx);
        cx.update(|cx| {
            assert!(render(cx).is_none(), "nothing to show");
            show(cx, "first");
        });
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.update(|cx| show(cx, "second"));
        cx.run_until_parked();
        cx.read(|cx| {
            assert_eq!(messages(cx), ["first", "second"]);
            assert!(render(cx).is_some());
        });
        cx.executor().advance_clock(Duration::from_millis(2100));
        cx.read(|cx| assert_eq!(messages(cx), ["second"]));
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.read(|cx| {
            assert!(messages(cx).is_empty());
            assert!(render(cx).is_none());
        });
    }

    #[gpui::test]
    fn identical_notices_dont_stack_and_at_most_four_show(cx: &mut TestAppContext) {
        crate::test_support::init(cx);
        cx.update(|cx| {
            show(cx, "same");
            show(cx, "same");
            assert_eq!(messages(cx), ["same"]);
            for i in 0..5 {
                show(cx, format!("n{i}"));
            }
            assert_eq!(messages(cx), ["n1", "n2", "n3", "n4"]);
        });
        // The re-shown notice keeps only its newest timer.
        cx.executor().advance_clock(Duration::from_secs(5));
        cx.read(|cx| assert!(messages(cx).is_empty()));
    }

    #[gpui::test]
    fn toasts_render_in_a_window(cx: &mut TestAppContext) {
        struct Host;
        impl gpui::Render for Host {
            fn render(&mut self, _: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
                div().size_full().children(render(cx))
            }
        }
        crate::test_support::init(cx);
        let (_, vcx) = cx.add_window_view(|_, _| Host);
        vcx.update(|_, cx| show(cx, "hello"));
        vcx.run_until_parked();
        vcx.read(|cx| assert_eq!(messages(cx), ["hello"]));
    }
}
