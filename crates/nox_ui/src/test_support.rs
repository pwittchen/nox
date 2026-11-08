//! Shared setup for GPUI tests (`#[gpui::test]` on the headless test platform): an isolated
//! `NOX_HOME` and the globals `main.rs` installs before opening a window.

use std::path::PathBuf;
use std::sync::OnceLock;

use gpui::{App, TestAppContext, WindowAppearance};
use nox_config::Config;

use crate::settings_state::Settings;

/// A per-process scratch `NOX_HOME`, so tests never touch `~/.nox` (sessions, sockets, trust).
pub fn nox_home() -> PathBuf {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        raise_fd_limit();
        let dir = tempfile::Builder::new().prefix("noxt").tempdir().expect("temp NOX_HOME").keep();
        // SAFETY: set once, before any test reads it (every test goes through here first).
        unsafe { std::env::set_var("NOX_HOME", &dir) };
        dir
    })
    .clone()
}

/// Tests run in parallel in one process and each pane holds a PTY, pipes and a socket or two,
/// so macOS's default soft `ulimit -n` of 256 runs out (`EMFILE`). Raise the soft limit to the
/// hard one, capped at 10240 (macOS's `OPEN_MAX`: `setrlimit` refuses more, e.g. `unlimited`).
fn raise_fd_limit() {
    // SAFETY: plain libc calls on a stack-allocated `rlimit`.
    unsafe {
        let mut lim: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut lim) == 0 {
            lim.rlim_cur = lim.rlim_max.min(10_240);
            libc::setrlimit(libc::RLIMIT_NOFILE, &raw const lim);
        }
    }
}

/// The test default config: defaults, but no IPC socket (tests opt in) and a plain `/bin/sh`
/// started in the scratch home, so the user's shell rc files never run.
#[must_use]
pub fn config() -> Config {
    let mut c = Config::default();
    c.agents.cooperation = false;
    c.shell.program = "/bin/sh".into();
    c.shell.args = vec![];
    c.shell.working_directory = nox_home().to_string_lossy().into_owned();
    c
}

/// Install the app globals for `config` (settings, theme, icons, text-input keys).
pub fn init_app(cx: &mut App, config: Config) {
    nox_home();
    cx.set_global(crate::config_io::ConfigError(None));
    cx.set_global(crate::icons::IconRegistry::from_config(&config));
    let (file, notices) = crate::theme::load_theme_file(&config.ui.theme);
    let state = crate::theme::ThemeState { file, mode: config.ui.mode, notices };
    cx.set_global(state.clone());
    crate::theme::apply(cx, &state, WindowAppearance::Dark);
    cx.set_global(Settings::new(config, nox_mac::Accessibility::default()));
    crate::text_input::bind_keys(cx);
}

/// `init_app` with the test default config.
pub fn init(cx: &mut TestAppContext) {
    cx.update(|cx| init_app(cx, config()));
}
