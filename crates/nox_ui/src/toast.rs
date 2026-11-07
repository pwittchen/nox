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
