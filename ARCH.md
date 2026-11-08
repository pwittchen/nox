# nox — Architecture

How the code implements [SPEC.md](SPEC.md). The spec is the source of truth for behaviour; this
file explains where things live and the few places where the implementation differs.

## Overview

At runtime nox is one process, `nox-app`. GPUI owns the main thread; every pane's PTY has its own
I/O threads; agents running inside panes talk back to the app through the `nox` CLI over a
Unix control socket.

```
+---------------------------------- nox-app (one process) ---------------------------------+
|                                                                                          |
|  main thread: GPUI                                                                       |
|  +------------------------------------------------------------------------------------+  |
|  | Root  (root.rs, chrome.rs, root_actions.rs)                                        |  |
|  |   workspaces -> tabs -> split trees (nox_layout)    palette, dialogs, toasts       |  |
|  |                                                     Settings + About windows       |  |
|  |   +----------------------+  +----------------------+                               |  |
|  |   | Pane                 |  | Pane                 |  Coop per workspace (coop.rs):|  |
|  |   |  TerminalElement     |  |  TerminalElement     |  message bus, claims, waiters |  |
|  |   |  agent tracker       |  |  agent tracker       |                               |  |
|  |   +----------^-----------+  +----------^-----------+                ^              |  |
|  +--------------|-------------------------|----------------------------|--------------+  |
|                 | wakeups / events        |                            | requests        |
|  +--------------+-----------+  +----------+-----------+  +-------------+------------+    |
|  | Session  (nox_terminal)  |  | Session              |  | IPC server  (nox_ipc)    |    |
|  | reader / writer / waiter |  | ...                  |  | token auth, deferred     |    |
|  | / resize threads, Term   |  |                      |  | responders               |    |
|  +--------------+-----------+  +----------+-----------+  +-------------^------------+    |
|                 |                         |                            |                 |
|  config.toml <-> watcher (nox_config); session file (nox_layout)       |                 |
+-----------------|-------------------------|----------------------------|-----------------+
                  | PTY                     | PTY                        | Unix socket
           +------v------+          +-------v-------+                    |
           | zsh         |          | claude        |-- nox msg / wait --+
           |             |          | (an agent)    |   claim / status   (nox CLI, nox_cli;
           +-------------+          +---------------+                     also `nox mcp`)
```

Crates, top to bottom; arrows point at dependencies. Only `nox` and `nox_ui` use GPUI, and only
`nox_mac` touches AppKit (through objc2); the rest is plain Rust, tested without either.
`nox-app` also uses `nox_config`, `nox_theme`, `nox_terminal`, `nox_mac` and `nox_layout`
directly (setup, menus, the config watcher), left out of the drawing.

```
                nox-app  (crates/nox)                                         nox  (crates/nox_cli)
                    |                                                                     |
                    v                                                                     |
                 nox_ui   (GPUI)                                                          |
                    |                                                                     |
       +------------+-------------+------------+-----------+---------+-----------+        |
       v            v             v            v           v         v           v        |
 nox_terminal  nox_layout    nox_agents   nox_config   nox_theme  nox_mac     nox_ipc <---+
 PTY, Term     tabs, splits  agents, bus  config.toml  colors     objc2 glue  socket
```

## Crates

```
crates/
├── nox/            binary `nox-app`: app setup, menus, config watcher, global actions, the window
├── nox_cli/        binary `nox`: CLI for agents + `nox mcp` (MCP over stdio); depends on nox_ipc only
├── nox_terminal/   PTY session (reader / waiter / writer / resize threads), OSC sniffer, key and
│                   mouse encoders, resize coalescing, process inspection, shell integration,
│                   grapheme-aware input (emoji clusters)
├── nox_layout/     workspaces → tabs → split trees, geometry, navigation, session file (pure data)
├── nox_agents/     process icons + agent detection, agent state machine, message bus, claims,
│                   rate limiter, git HEAD / worktrees, project files + trust, hooks, resume plans
├── nox_ipc/        control-socket protocol, server (token auth, deferred responders), client
├── nox_config/     config model, lenient loading, comment-preserving writes, watcher, schema
├── nox_theme/      colors, theme files (bundled from themes/), contrast helpers, 256-color table
├── nox_mac/        objc2: NSMenu context menus, notifications, accessibility, Dock bounce, NSSound,
│                   IOPMAssertion, NSWindow background + window dragging, Option-key side
└── nox_ui/         GPUI views and elements (below)
```

Only `nox_ui` and `nox` depend on GPUI; everything else builds and tests without it.

### nox_ui

| File                  | What                                                                          |
|-----------------------|-------------------------------------------------------------------------------|
| `root.rs`             | `Root` entity: all workspaces, `PaneId → Entity<Pane>`, per-workspace `Coop` (bus, claims, activity), pane/tab/workspace operations, pane events |
| `chrome.rs`           | title bar, workspace chip, tab pills (top/bottom), sidebar, pane card, dividers |
| `root_actions.rs`     | every app action handler + the palette pickers (commands, tabs, workspaces, agents, themes, appearance, send-to) |
| `pane.rs`             | `Pane` entity: session, title/cwd/icon/branch, status, agent tracker, keyboard/IME, mouse, selection, links, find, scrolling, header |
| `terminal_element.rs` | the grid as a custom `Element`: snapshot under a short lock, merged background quads, style runs snapped to the cell grid, cursor, selection, search, scrollbar |
| `coop.rs`             | control-socket bridge: IPC requests on the main thread; `wait` / `inbox --wait` / `claim --wait` / deferred `send` are held waiters, re-checked on state changes |
| `attention.rs`        | chime + notification rules (dedupe, once per pane until visited) |
| `persist.rs`          | session snapshot / restore, resume planning, resume sheet |
| `features.rs`         | open folder as workspace, project files + trust, worktrees, tab moves, hooks installer, CLI symlink |
| `palette.rs`, `dialog.rs`, `toast.rs`, `text_input.rs` | shared UI components |
| `settings.rs`         | the Settings window (a view over `config.toml`) |
| `config_io.rs`        | surgical writes + applying a new config live |
| `menus.rs`            | native context menus (tab, pane, workspace chip) |

#### Tests

`nox_ui` tests run on GPUI's headless test platform (`gpui` `test-support` dev-dependency,
`#[gpui::test]` with `TestAppContext`); a `Root` built in a test window spawns real panes.
`test_support.rs` installs the globals `main.rs` sets up (settings, theme, icons, input keys),
points `NOX_HOME` at a per-process temp dir and uses a plain `/bin/sh` (no `-l`, no rc files)
with IPC off. Test windows have no native handle, so `window_bg::apply` is a no-op under
`cfg(test)` and `window_bg::start_move` only counts window moves, and `menus.rs` hands context menus to the test instead of showing an `NSMenu`.
Tests that write the shared `config.toml` or `session.toml` take the lock in
`config_io::tests` / `persist::tests` (PTY creation is serialized by
`TerminalSession::spawn` itself, see below). What stays untested is native
glue (`nox_mac`, `nox/main.rs`) and paths the test platform leaves `unimplemented!` (file
pickers, character palette, minimize / zoom).

## Data flow

```
key ─▶ GPUI bindings (app actions) ─▶ else Pane::on_key_down ─▶ keys::encode ─▶ write(PTY)
       plain text ─▶ IME input handler (EntityInputHandler on Pane) ─▶ write(PTY)

PTY ─▶ reader thread: poll + read 64 KiB ─▶ OscSniffer (OSC 7/133/9/777, offsets)
                     ─▶ vte Processor under FairMutex, segment by segment (marks get the
                        cursor line at the exact point), into `GraphemeInput` (forwards every
                        Handler call to the Term, merges emoji clusters in `input`)
                     ─▶ one Wakeup if none pending
     ─▶ async channel ─▶ Pane::handle_event ─▶ cx.notify() ─▶ TerminalElement paints next frame
```

- Prompt marks (OSC 133) are recorded with the absolute line (`history_size + cursor line`) at
  the byte where the sequence ends; "Copy Last Output", `nox read --last-output` and ⌘↑/⌘↓ use
  them. Once scrollback is full the marks drift by the number of dropped lines (accepted).
- The child is reaped by a waiter thread (`waitpid`); if a background process keeps the PTY
  open, the exit is still reported after 300 ms.
- PTY creation + spawn is serialized by a process-wide lock in `TerminalSession::spawn` (macOS
  `openpty` uses the static `ptsname` buffer, and the master's `FD_CLOEXEC` is set after the
  fact, so concurrent spawns could fail or leak a master fd into another child).
- Dropping a session sends SIGHUP and hands the `Pty` to a reaper thread: alacritty's
  `Pty::drop` waits on the child without a timeout, so the reaper first escalates to SIGTERM /
  SIGKILL (0.5 s grace each, only while the child is unreaped) and drops the `Pty` after that.
- PTY resizes go through `ResizeCoalescer` (≤ 1 per 33 ms, final one 50 ms after the last change)
  on a thread that is idle unless the size changes; `Term::resize` happens in the same frame.

## Concurrency rules in the UI

- A palette's item source must not read `Root` (palettes are created inside `Root` updates):
  pickers snapshot their items first.
- Native menus pop up from `dispatch_async` on the main queue, outside any GPUI callback; the
  chosen tag comes back through a channel to a foreground task.
- IPC requests are handled with `cx.spawn_in(window, …)` so handlers can spawn panes.

## Build notes

- GPUI 0.2.2 from crates.io, pinned, with `runtime_shaders`: Metal shaders compile at runtime,
  so building doesn't need Xcode's separate Metal toolchain
  (`xcodebuild -downloadComponent MetalToolchain` enables precompiled shaders if wanted).
- `scripts/bundle.sh` builds `dist/nox.app`; `scripts/release/*` sign, notarize, build the dmg and
  appcast; the workflows in `.github/workflows` call them.
- The app icon lives in `Assets.xcassets` (transparent outside the rounded square;
  `scripts/icon_deborder.swift` removes a white rim from a new source image and scales the
  square to Apple's icon grid, 824 px with a 100 px margin on 1024, so it matches other Dock
  icons in size; regenerate the smaller sizes from its 1024 px output). The bundle script
  builds `nox.icns` from it, and the binary also sets it as the Dock icon at runtime.
- The app's name outside `dist/nox.app`: `crates/nox/build.rs` embeds a minimal `Info.plist`
  (`__TEXT,__info_plist`) so even a bare `cargo run` shows "nox" in the menu bar. The Dock
  still names a bare executable after its file (`nox-app`), so `scripts/run.sh` runs the debug
  build from a dev bundle, `target/debug/nox.app` (hard links to `nox-app` and the `nox` CLI,
  no bundle identifier, so it doesn't share defaults or notification permissions with the
  release app).
- The cargo binary is `nox-app` (the CLI owns the name `nox` in `target/`), but both bundles
  install it as `Contents/MacOS/nox` (`CFBundleExecutable = nox`): fastfetch and similar tools
  name the terminal after the parent process's executable and would otherwise print
  `nox-app <version>`. The CLI lives in `Contents/Resources/bin/nox`, so the names don't clash.

## Differences from the spec

- **Iosevka is not bundled**: its four faces are ~45 MB and would break the `.dmg < 20 MB`
  target. It works when installed. The other nine families are bundled (`assets/fonts`).
- **Fonts load lazily** (`CTFontManager`-equivalent via GPUI's `add_fonts`): the configured
  terminal / interface fonts at startup, others when picked in Settings.
- **Unix socket length**: when `~/.nox/run/nox.sock` would exceed the 104-byte `sun_path` limit
  (e.g. a long `NOX_HOME`), nox uses `/tmp/nox-<uid>/nox.sock`; panes get the path in
  `NOX_SOCKET` either way.
- **PATH in panes**: rc files often reset `PATH`, so the shell-integration prompt hook re-adds
  `NOX_BIN_DIR` (the directory of the bundled `nox` CLI) when it's missing.
- **⌘+ / ⌘−** change the font size for the session (all panes, or the focused pane with
  `font.size_applies_to = "pane"`); ⌘0 resets to `font.size`. They don't write the config.
- **Agent hooks**: "Install Agent Hooks…" writes Claude Code hooks; Codex and Gemini CLI hook
  installers are not implemented yet.
- **Window dragging**: GPUI 0.2.2's `start_window_move` does nothing on macOS, and `AppKit`
  moves a transparent-titlebar window for any drag in the title-bar band, which swallowed tab
  drags. `window_bg::take_over_window_drag` switches the main window's content view to a runtime
  subclass whose `-mouseDownCanMoveWindow` returns `NO` (Settings and About keep the native
  behaviour); the empty title-bar space calls `window_bg::start_move`
  (`performWindowDragWithEvent:`), and interactive items stop the mouse-down.
- **Window close button** quits like `⌘Q` (`on_window_should_close` → `request_quit`), so the
  quit confirmations (`app.confirm_quit`, `app.confirm_close`) apply to it too.
- See [ROADMAP.md](ROADMAP.md) for features not implemented yet.
