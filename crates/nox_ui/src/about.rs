//! The About window (SPEC §5.11): app icon, name, version, the author and links; the same content
//! is the About page of Settings (SPEC §5.9). One instance;
//! a second "About nox" brings it to the front. `Esc` or `⌘W` closes it.

use std::sync::{Arc, LazyLock};

use gpui::{
    App, AppContext as _, Bounds, Context, FocusHandle, Focusable, Global, Image, ImageFormat, KeyDownEvent,
    SharedString, Subscription, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, div, img,
    point, prelude::*, px, size,
};

use crate::settings_state::Settings;
use crate::theme::Theme;

pub const AUTHOR: &str = "Piotr Wittchen";
pub const AUTHOR_WEBSITE: &str = "https://wittchen.io";
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

static ICON: LazyLock<Arc<Image>> = LazyLock::new(|| {
    Arc::new(Image::from_bytes(
        ImageFormat::Png,
        include_bytes!("../../../Assets.xcassets/AppIcon.appiconset/256-mac.png").to_vec(),
    ))
});

#[derive(Default)]
struct AboutWindow(Option<WindowHandle<AboutView>>);
impl Global for AboutWindow {}

pub fn open_about_window(cx: &mut App) {
    let existing = cx.try_global::<AboutWindow>().and_then(|w| w.0);
    if crate::util::activate_if_open(existing, cx) {
        return;
    }
    let bounds = Bounds::centered(None, size(px(340.), px(400.)), cx);
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("About nox".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(12.), px(12.))),
        }),
        is_resizable: false,
        is_minimizable: false,
        ..Default::default()
    };
    if let Ok(h) = cx.open_window(opts, |window, cx| cx.new(|cx| AboutView::new(window, cx))) {
        cx.set_global(AboutWindow(Some(h)));
    }
}

pub struct AboutView {
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl Focusable for AboutView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl AboutView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let subs = vec![cx.observe_global::<Theme>(|_, cx| cx.notify())];
        Self { focus, _subs: subs }
    }
}

/// A clickable link: muted label on the left, the accent-colored target on the right.
fn link_row(
    id: &'static str,
    label: &'static str,
    text: String,
    url: &'static str,
    theme: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .justify_between()
        .gap_4()
        .w_full()
        .child(div().text_color(theme.text_muted).child(label))
        .child(
            div()
                .id(id)
                .text_color(theme.accent)
                .cursor_pointer()
                .hover(gpui::Styled::underline)
                .on_click(move |_, _, cx| cx.open_url(url))
                .child(text),
        )
}

fn strip_scheme(url: &str) -> String {
    url.trim_start_matches("https://").trim_start_matches("http://").to_string()
}

/// Icon, name, tagline and version, centered (the About window and Settings → About).
#[must_use]
pub fn header(theme: &Theme) -> gpui::Div {
    let version: SharedString = format!("Version {}", env!("CARGO_PKG_VERSION")).into();
    div()
        .flex()
        .flex_col()
        .items_center()
        // The icon PNG has the macOS grid margin built in; 112 px shows the square at ~90 px.
        .child(img(ICON.clone()).size(px(112.)))
        .child(div().mt(px(4.)).text_size(px(22.)).font_weight(gpui::FontWeight::BOLD).child("nox"))
        .child(div().mt(px(2.)).text_color(theme.text_muted).child("stay in command."))
        .child(div().mt(px(10.)).text_color(theme.text_muted).child(version))
}

/// Author, website and source rows under a hairline.
#[must_use]
pub fn details(theme: &Theme) -> gpui::Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(8.))
        .pt(px(14.))
        .border_t_1()
        .border_color(theme.border)
        .child(
            div()
                .flex()
                .justify_between()
                .w_full()
                .child(div().text_color(theme.text_muted).child("Author"))
                .child(AUTHOR),
        )
        .child(link_row("about-website", "Website", strip_scheme(AUTHOR_WEBSITE), AUTHOR_WEBSITE, theme))
        .child(link_row("about-source", "Source", strip_scheme(REPOSITORY), REPOSITORY, theme))
}

impl Render for AboutView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx).clone();
        let settings = Settings::global(cx);
        div()
            .id("about-root")
            .track_focus(&self.focus)
            .on_key_down(|ev: &KeyDownEvent, window, _| {
                let ks = &ev.keystroke;
                if (ks.key == "escape" && !ks.modifiers.modified())
                    || (ks.key == "w" && ks.modifiers.platform)
                {
                    window.remove_window();
                }
            })
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(40.))
            .px(px(28.))
            .pb(px(24.))
            .bg(theme.bg)
            .text_color(theme.text)
            .font_family(settings.ui_font.clone())
            .text_size(px(12.))
            .child(header(&theme))
            .child(div().flex_1())
            .child(details(&theme))
    }
}
