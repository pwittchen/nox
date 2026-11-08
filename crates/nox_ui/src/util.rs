//! Small shared helpers.

use std::time::Duration;

use gpui::{
    AnyView, App, AppContext as _, Context, IntoElement, ParentElement, Render, Styled, Window, WindowHandle,
    div, px,
};

use crate::theme::Theme;

/// For single-instance windows (Settings, About): brings `handle`'s window to the front if it is
/// still open and returns whether it was. Activation is deferred, because the action that asks
/// for it may be dispatched while that very window is being updated (it's the key window), and
/// a nested update would fail.
pub fn activate_if_open<V: 'static>(handle: Option<WindowHandle<V>>, cx: &mut App) -> bool {
    let Some(h) = handle else { return false };
    if !cx.windows().iter().any(|w| w.window_id() == h.window_id()) {
        return false;
    }
    cx.defer(move |cx| {
        let _ = h.update(cx, |_, window, _| window.activate_window());
    });
    true
}

/// `4m 12s`, `30s`, `2h 5m`.
#[must_use]
pub fn short_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    }
}

struct Tooltip {
    text: String,
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx);
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.text)
            .children(self.text.lines().map(|l| div().child(l.to_string())).collect::<Vec<_>>())
    }
}

pub fn tooltip(text: String, _window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|_| Tooltip { text }).into()
}

/// Fuzzy subsequence match; higher is better, `None` = no match.
#[must_use]
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0i32;
    let mut ti = 0usize;
    let mut last_match: Option<usize> = None;
    for qc in q {
        let mut found = false;
        while ti < t.len() {
            if t[ti] == qc {
                score += 10;
                if last_match.is_some_and(|l| l + 1 == ti) {
                    score += 15; // consecutive
                }
                if ti == 0 || matches!(t[ti - 1], ' ' | '/' | '-' | '_' | '.' | ':') {
                    score += 20; // word start
                }
                last_match = Some(ti);
                ti += 1;
                found = true;
                break;
            }
            ti += 1;
        }
        if !found {
            return None;
        }
    }
    score -= t.len() as i32 / 4;
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy() {
        assert!(fuzzy_score("spv", "Split Vertically").is_some());
        assert!(fuzzy_score("xyz", "Split Vertically").is_none());
        assert!(
            fuzzy_score("new tab", "New Tab").unwrap() > fuzzy_score("new tab", "New Agent Tab").unwrap()
        );
    }

    #[test]
    fn durations() {
        assert_eq!(short_duration(Duration::from_secs(30)), "30s");
        assert_eq!(short_duration(Duration::from_secs(252)), "4m 12s");
        assert_eq!(short_duration(Duration::from_mins(125)), "2h 5m");
    }
}
