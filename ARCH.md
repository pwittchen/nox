# nox — Architecture

How the code implements [SPEC.md](SPEC.md). The spec is the source of truth for behaviour; this
file explains where things live and the few places where the implementation differs.

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
│                   IOPMAssertion, NSWindow background, Option-key side
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
  `scripts/icon_deborder.swift` removes a white rim from a new source image). The bundle script
  builds `nox.icns` from it, and the binary also sets it as the Dock icon at runtime.

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
- See [ROADMAP.md](ROADMAP.md) for features not implemented yet.
