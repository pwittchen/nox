# CLAUDE.md

nox is a native macOS terminal in Rust + GPUI. **SPEC.md is the source of truth** for behaviour;
ARCH.md explains how the code maps to it and lists deliberate differences; ROADMAP.md lists
what's not done. When behaviour changes, update SPEC.md (and ARCH.md / ROADMAP.md) in the same
change.

## Commands

```sh
cargo build                                       # whole workspace (debug)
cargo run -p nox --bin nox-app                    # run the app
cargo test --workspace                            # all tests (incl. real-shell PTY tests)
cargo test -p nox_layout                          # one crate
cargo clippy --workspace --all-targets -- -D warnings   # must be clean (CI enforces)
cargo fmt --all                                   # rustfmt.toml: max_width 110
cargo bench -p nox_terminal                       # parser / encoder benches
scripts/bundle.sh                                 # dist/nox.app (ad-hoc signed)
scripts/release/make_dmg.sh                       # dist/nox-macos-aarch64.dmg
```

Before finishing a change: `cargo fmt --all`, clippy with `-D warnings`, and the tests of the
touched crates.

## Layout

- `crates/nox` — app binary `nox-app` (setup, menus, config watcher, global actions).
- `crates/nox_cli` — binary `nox` (agent CLI + `nox mcp`); depends only on `nox_ipc`.
- `crates/nox_terminal`, `nox_layout`, `nox_agents`, `nox_ipc`, `nox_config`, `nox_theme` —
  no GPUI; pure logic, heavily unit-tested. Keep it that way.
- `crates/nox_mac` — objc2 / AppKit glue (menus, notifications, sounds, icon, power).
- `crates/nox_ui` — all GPUI views and elements (`root.rs` owns workspaces and panes,
  `pane.rs`, `terminal_element.rs`, `chrome.rs`, `palette.rs`, `settings.rs`, …).
- `themes/` bundled themes (compiled in), `assets/` fonts, icons, sounds, app bundle files.

## Running the app safely

- Use `NOX_HOME=<scratch dir>` to run with a separate config, session and socket, never the
  user's `~/.nox`.
- The user may have their own `nox-app` running. Don't `pkill nox-app`; kill only the PID you
  started, and only send synthetic keystrokes / clicks to that PID (check the frontmost
  process's PID, not its name).

## GPUI pitfalls (each of these has crashed the app before)

- `window.request_animation_frame()` is only allowed during prepaint / paint. In event handlers
  use `cx.notify()`.
- Never update or read an entity that is already being updated: inside
  `window.update(cx, |root, window, cx| …)` use the `root` argument, don't call
  `root_entity.update(...)` again. Palette item sources run while `Root` is updating, so build
  their items up front (snapshot), don't read `Root` inside the source closure.
- `on_open_urls` is on `Application`, before `run`.
- Native `NSMenu`s are shown via `dispatch_async` (`nox_mac::show_context_menu`) and report back
  through a channel; never run a nested AppKit loop inside a GPUI callback.
- Popovers / dropdowns must float (`deferred(anchored()...)`) and close on `on_mouse_down_out`,
  not expand inline.
- GPUI is pinned (`gpui = "=0.2.2"`, `runtime_shaders` because the Metal toolchain isn't
  installed); check APIs in `~/.cargo/registry/src/*/gpui-0.2.2` rather than guessing.

## Conventions

- Config lives in `nox_config` (model, lenient loader, `settings_schema()` for the Settings
  window, `default_config.toml` template). A new setting needs all of these plus SPEC.md §11.
  UI writes go through `config_io::set_value` (surgical `toml_edit`, never rewrite the file).
- Colors come only from the `Theme` global; no hardcoded colors in components.
- Defaults: terminal font JetBrains Mono 14 pt, `line_height` is a multiple of the font's
  natural line height (1.0 = 100 %).
- Commit messages: English, imperative, capitalized subject without a prefix (see `git log`).
- CI skips Markdown-only changes and commits / PR titles containing `#no-deploy`.
- Never commit `.macos-signing/` (private signing keys; it's gitignored).
