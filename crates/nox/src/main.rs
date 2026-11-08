//! nox — stay in command. The app binary: setup, menus, global actions, the window.

// `cargo lint` runs clippy::pedantic. These lints are allowed on purpose: numeric casts are
// bounded grid / pixel / color math, `# Errors` / `# Panics` sections would only restate the
// signature, hex color literals read better without separators, and long UI builders and
// flag-heavy config structs are clearer kept whole; `assert!(x.is_empty())` reads fine in tests
// and `(n..).find(..)` searches are meant to be unbounded.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::unreadable_literal,
    clippy::too_many_lines,
    clippy::struct_excessive_bools,
    clippy::similar_names,
    clippy::many_single_char_names,
    clippy::assert_is_empty,
    clippy::maybe_infinite_iter
)]

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    App, AppContext, Application, Bounds, Entity, Focusable, TitlebarOptions, WindowBackgroundAppearance,
    WindowBounds, WindowOptions, point, px, size,
};
use nox_ui::actions::{About, CheckForUpdates, HideApp, HideOthers, OpenSettings, Quit, ShowAll};
use nox_ui::root::{Initial, Root};
use nox_ui::settings_state::Settings;

fn init_logging() {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,gpui=warn"));
    if !cfg!(debug_assertions) {
        let path = nox_config::nox_dir().join("nox.log");
        if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            builder.target(env_logger::Target::Pipe(Box::new(file)));
        }
    }
    let _ = builder.try_init();
}

fn main() {
    init_logging();
    let args: Vec<String> = std::env::args().collect();
    // ⇧ held at launch or --no-restore skips restoring once (SPEC §6.6).
    let shift_held = nox_mac::shift_key_down();
    let no_restore = args.iter().any(|a| a == "--no-restore") || shift_held;
    let open_paths: Vec<PathBuf> = args
        .iter()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .collect();

    let app = Application::new().with_assets(nox_ui::assets::Assets);
    app.on_open_urls(|urls| {
        for u in urls {
            if let Some(p) = u.strip_prefix("file://") {
                let p = PathBuf::from(nox_ui::links_decode(p.trim_end_matches('/')));
                if p.is_dir() {
                    OPEN_QUEUE.with(|q| q.borrow_mut().push(p));
                }
            }
        }
    });
    app.run(move |cx: &mut App| {
        // The app icon (Assets.xcassets) also when running as a bare binary.
        nox_mac::set_app_icon(include_bytes!("../../../Assets.xcassets/AppIcon.appiconset/1024-mac.png"));

        // Configuration.
        let config_path = nox_config::config_path();
        if let Err(e) = nox_config::ensure_default_file(&config_path) {
            log::error!("creating {}: {e}", config_path.display());
        }
        let (config, mut notices, parse_error) = match nox_config::load_from_path(&config_path) {
            Ok((c, n)) => (c, n, None),
            Err(e) => (nox_config::Config::default(), vec![], Some(e.to_string())),
        };
        let a11y = nox_mac::accessibility();
        cx.set_global(Settings::new(config.clone(), a11y));
        cx.set_global(nox_ui::config_io::ConfigError(parse_error.clone()));
        cx.set_global(nox_ui::icons::IconRegistry::from_config(&config));

        // Theme.
        let (file, theme_notices) = nox_ui::theme::load_theme_file(&config.ui.theme);
        let state = nox_ui::theme::ThemeState { file, mode: config.ui.mode, notices: theme_notices.clone() };
        cx.set_global(state.clone());
        let appearance = cx.window_appearance();
        nox_ui::theme::apply(cx, &state, appearance);

        // Fonts, shell integration, keys, menus.
        nox_ui::fonts::ensure_loaded(cx, &config);
        let integration_dir = nox_terminal::shell::default_dir(&nox_config::nox_dir());
        if let Err(e) = nox_terminal::shell::install(&integration_dir) {
            log::warn!("shell integration: {e}");
        }
        notices.extend(nox_ui::actions::bind_keys(cx, &config.keys));
        cx.set_menus(nox_ui::actions::menus());

        // Restore or start fresh.
        let initial = if config.app.restore_session && !no_restore {
            nox_ui::persist::load_plan().map_or(Initial::Fresh, |p| Initial::Restored(Box::new(p)))
        } else {
            Initial::Fresh
        };

        let w = &config.window;
        let wsize = size(px(w.width.max(480) as f32), px(w.height.max(320) as f32));
        let bounds = if w.x == 0 && w.y == 0 {
            Bounds::centered(None, wsize, cx)
        } else {
            Bounds::new(point(px(w.x as f32), px(w.y as f32)), wsize)
        };
        let titlebar_h = match config.tabs.position {
            nox_config::TabsPosition::Bottom => nox_ui::root::TITLEBAR_SLIM_PX,
            _ => nox_ui::root::TITLEBAR_TOP_PX,
        };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("nox".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(13.), px((titlebar_h - 14.) / 2.))),
            }),
            window_background: WindowBackgroundAppearance::Opaque,
            window_min_size: Some(size(px(420.), px(260.))),
            app_id: Some("nox".into()),
            ..Default::default()
        };
        let window = cx
            .open_window(options, |window, cx| {
                nox_ui::window_bg::apply(window, nox_ui::theme::Theme::global(cx));
                nox_ui::window_bg::take_over_window_drag(window);
                cx.new(|cx| Root::new(initial, window, cx))
            })
            .expect("open the nox window");
        let root: Entity<Root> = window.update(cx, |_, _, cx| cx.entity()).expect("root entity");

        // Notices collected during startup.
        for n in notices {
            nox_ui::toast::show(cx, format!("config.toml: {} — {}", n.key, n.message));
        }
        for n in theme_notices {
            nox_ui::toast::show(cx, n);
        }
        if let Some(e) = parse_error {
            nox_ui::toast::show(cx, format!("config.toml: {e} — using defaults until it's fixed."));
        }

        // Live reload of config.toml (SPEC §11).
        let (cfg_tx, cfg_rx) = async_channel::unbounded();
        match nox_config::watch(&config_path, move |res| {
            let _ = cfg_tx.try_send(res);
        }) {
            Ok(handle) => std::mem::forget(handle),
            Err(e) => log::warn!("watching config.toml: {e}"),
        }
        cx.spawn(async move |cx| {
            while let Ok(res) = cfg_rx.recv().await {
                let _ = cx.update(|cx| match res {
                    Ok((config, notices)) => nox_ui::config_io::apply(cx, config, &notices),
                    Err(e) => {
                        cx.set_global(nox_ui::config_io::ConfigError(Some(e.to_string())));
                        nox_ui::toast::show(cx, format!("config.toml: {e}"));
                        cx.refresh_windows();
                    }
                });
            }
        })
        .detach();

        // Accessibility flags (Reduce Motion, …) follow the system live.
        let (a11y_tx, a11y_rx) = async_channel::unbounded();
        nox_mac::observe_accessibility(move |a| {
            let _ = a11y_tx.try_send(a);
        });
        cx.spawn(async move |cx| {
            while let Ok(a) = a11y_rx.recv().await {
                let _ = cx.update(|cx| {
                    let mut s = Settings::global(cx).clone();
                    s.a11y = a;
                    cx.set_global(s);
                    cx.refresh_windows();
                });
            }
        })
        .detach();

        // Clicking a notification focuses its pane.
        let (note_tx, note_rx) = async_channel::unbounded::<String>();
        nox_mac::init_notifications(move |id| {
            let _ = note_tx.try_send(id);
        });
        {
            let _root = root.clone();
            cx.spawn(async move |cx| {
                while let Ok(id) = note_rx.recv().await {
                    let Some(pane) = nox_ui::attention::pane_from_notification(&id) else { continue };
                    let _ = window.update(cx, |r, window, cx| {
                        cx.activate(true);
                        window.activate_window();
                        r.reveal_pane(pane, window, cx);
                    });
                }
            })
            .detach();
        }

        // Folders dropped on the Dock icon / `open -a nox <dir>` (queued by `on_open_urls`)
        // open as workspaces.
        {
            let _root = root.clone();
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor().timer(Duration::from_millis(300)).await;
                    let paths: Vec<PathBuf> = OPEN_QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()));
                    if paths.is_empty() {
                        continue;
                    }
                    let _ = window.update(cx, |r, window, cx| {
                        for p in paths {
                            r.open_folder(p, false, window, cx);
                        }
                    });
                }
            })
            .detach();
        }
        for p in open_paths {
            let _ = window.update(cx, |r, window, cx| {
                r.open_folder(p, false, window, cx);
            });
        }

        // App-level actions that must work from any window.
        {
            let _root = root.clone();
            cx.on_action(move |_: &Quit, cx| {
                let _ = window.update(cx, nox_ui::root_actions::request_quit);
            });
        }
        cx.on_action(|_: &OpenSettings, cx| nox_ui::settings::open_settings_window(cx));
        cx.on_action(|_: &HideApp, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
        cx.on_action(|_: &About, cx| nox_ui::about::open_about_window(cx));
        cx.on_action(|_: &CheckForUpdates, cx| {
            nox_ui::toast::show(cx, "Updates are disabled in this build (SPEC §15.4).");
        });

        // Save the session and window geometry on quit.
        {
            let root = root.clone();
            cx.on_app_quit(move |cx| {
                root.update(cx, nox_ui::root::Root::save_session_now);
                if let Ok(b) = window.update(cx, |_, window, _| window.bounds()) {
                    let path = nox_config::config_path();
                    let _ =
                        nox_config::set_value(&path, "window.width", f32::from(b.size.width).round() as i64);
                    let _ = nox_config::set_value(
                        &path,
                        "window.height",
                        f32::from(b.size.height).round() as i64,
                    );
                    let _ = nox_config::set_value(&path, "window.x", f32::from(b.origin.x).round() as i64);
                    let _ = nox_config::set_value(&path, "window.y", f32::from(b.origin.y).round() as i64);
                }
                nox_mac::set_keep_awake(false, "");
                async {}
            })
            .detach();
        }

        let _ = window.update(cx, |root, window, cx| {
            window.focus(&root.focus_handle(cx));
            root.focus_active(window, cx);
        });
        cx.activate(true);
    });
}

thread_local! {
    static OPEN_QUEUE: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}
