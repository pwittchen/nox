# nox — Specification

> stay in command — a minimalistic, fast, native terminal for macOS, built in Rust with GPUI.

## 1. Vision

nox is a quiet, fast terminal. The prompt and its output are the only things that matter on
screen; chrome is thin, neutral and out of the way. It splits panes and manages tabs the way
iTerm does, scrolls and resizes as smoothly as iTerm, and looks like its siblings **kayet** and
**zora**: macOS-native + Linear — soft neutral palette, 1px hairlines, crisp typography,
restrained motion, no decoration that hasn't earned its place.

It is fully native: no web view. The UI and the terminal grid are drawn by GPUI on Metal, text
is shaped and rasterized with Core Text, and macOS integration (menus, context menus,
accessibility settings, notifications) goes through AppKit.

## 2. Goals & Non-Goals

### Goals
- Instant startup: a usable prompt in well under 150 ms after launch.
- Low input latency and high output throughput (see 12 for targets).
- Panes split vertically and horizontally, nested arbitrarily, like iTerm.
- Tabs, each holding its own pane layout; tab bar on the **top**, **bottom** or in a **left
  sidebar** (configurable).
- Smooth, pixel-precise scrolling with trackpad momentum; smooth live window resize without
  flicker, stretching or garbled content.
- Configuration in `~/.nox/config.toml`, applied live on save; editable by hand in any editor
  or in the built-in Settings window, both always in sync.
- Every kayet theme bundled, plus the built-in `nox` palette; light/dark following the system.
- JetBrains Mono as the default terminal font, with a curated set of other terminal fonts and
  interface fonts bundled to choose from (see 5.10).

### Non-Goals (v1)
- Plugins / extensions / scripting API.
- tmux control mode integration, SSH/profile manager, broadcast input.
- Inline images (Sixel, Kitty graphics, iTerm `imgcat` protocol).
- Font ligatures (the grid is drawn cell by cell).
- AI features.
- Multiple terminal windows (one window; work is organized in tabs and panes; the Settings
  window, see 5.9, is the only other window). See ROADMAP.
- Session restoration of running processes (layout + working directories only, see 6.6).
- Intel Macs, Linux, Windows builds. GPUI and `alacritty_terminal` don't preclude them, but they
  are not built or tested.

## 3. Tech Stack

| Layer               | Choice                                                                  |
|---------------------|-------------------------------------------------------------------------|
| Language            | Rust (edition 2024), stable toolchain pinned in `rust-toolchain.toml`   |
| UI framework        | **GPUI** (Zed's GPU UI framework: Metal renderer, Core Text text system, flexbox layout via `taffy`) |
| UI components       | Own small set (tab pill, icon button, divider, text input, list); `gpui-component` only where it saves real work (text input) |
| Terminal emulation  | `alacritty_terminal`: VT parser, grid, scrollback, selection, regex search, modes |
| PTY                 | `alacritty_terminal::tty` (openpty, spawn, `TIOCSWINSZ`), own read loop (see 8) |
| macOS integration   | `objc2`, `objc2-app-kit`, `objc2-foundation`, `objc2-user-notifications`: native context menus, accessibility settings, Dock bounce, notifications |
| Process info        | `libproc` (foreground process name, cwd)                                |
| Config              | `serde` + `toml` (read), `toml_edit` (comment-preserving writes)        |
| File watching       | `notify` (+ `notify-debouncer-mini`)                                    |
| Regex (links)       | `regex`                                                                 |
| Paths               | `dirs`                                                                  |
| Logging             | `log` + `env_logger` (`~/.nox/nox.log` in release)                      |
| Packaging           | `cargo-bundle` / `cargo-packager` → `.app`, `.dmg`; `codesign` + `notarytool` (Developer ID, see 15) |
| Auto-update         | **Sparkle 2** (`Sparkle.framework`, driven via `objc2` from `nox_mac`), appcast + EdDSA-signed archives on GitHub Releases |
| CI                  | GitHub Actions, macOS arm64 runners; signing, notarization and releases on tag push (see 15) |

- **GPUI dependency**: pinned to an exact version (a crates.io `gpui` release, or a Zed git
  revision if needed for a fix), upgraded deliberately — its API changes between versions.
  All GPUI-facing code lives in the UI crates (see 13), so an upgrade touches as little as
  possible.
- Target: **macOS on Apple Silicon only** (`aarch64-apple-darwin`), macOS 13 Ventura or newer.
  Release profile: `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`, `strip = true`,
  `panic = "abort"`.
- Distribution: signed + notarized `nox.app` in `nox-macos-aarch64.dmg`, published as a GitHub
  Release, and a Homebrew cask in `pwittchen/tap`, as for kayet. Installed copies update
  themselves from GitHub Releases once the repository is public (see 15).

### Why GPUI + alacritty_terminal
The terminal grid is the product, so it is drawn by the same GPU renderer as the rest of the UI,
with no web view, IPC or JavaScript in the keystroke-to-pixel path. GPUI renders on Metal, paces
frames with the display link (120 Hz on ProMotion), only redraws when something changed, and
shapes text with Core Text, so fonts look as they do in other Mac apps. `alacritty_terminal` is
a mature, fast emulator core. Zed's built-in terminal uses exactly this pair, which proves the
approach and gives reference code (`crates/terminal`, `crates/terminal_view` in Zed).

Fallback if GPUI blocks us (API churn, missing platform hooks): `winit` + `wgpu` + `glyphon`
with an own small UI layer. Terminal, layout, config and theme crates have no GPUI dependency,
so they carry over unchanged.

## 4. Core Concepts

### 4.1 Window
- One window. Native traffic lights in a transparent title bar
  (`TitlebarOptions { appears_transparent: true, traffic_light_position }`); nox draws its own
  content (tabs, chip, buttons) in the title bar area, edge to edge.
- Window size and position persisted and restored. Full screen (`⌃⌘F`) supported.

### 4.2 Tab
- A tab holds a **layout**: a tree of panes (see 4.3). It always has at least one pane.
- The tab's title is the title of its **focused pane** (see 4.4), prefixed by its index:
  `1 ~`, `2 ~/Development/nox`, `3 vim`. The user can rename a tab (see 6.5); a renamed tab
  keeps its name instead of following the focused pane.
- Status dot: a small dot on the tab tells what its panes are doing (as in the screenshot on
  `#469 - cleanup after migration`):
  - **yellow** (`ansi.yellow`) — **running**: a command is executing in one of the tab's panes.
    Shown on every tab, the active one included, for as long as the command runs.
  - **blue** (`ansi.blue`) — **done**: a command finished while the tab was not active. It stays
    until the tab is visited; a command that finishes in the active tab leaves no dot. It may
    also play the attention sound and post a notification (see 7.11).
  - **accent** — **bell**: a bell rang in a background tab (see 7.10); until the tab is visited.
  - Priority when several apply: bell > running > done. No dot when the tab is idle and seen.
- "Running" means, with shell integration (see 4.5), the span between OSC 133 `C` (command
  start) and `D` (command end); without it, the PTY's foreground process (`tcgetpgrp`) is not
  the shell. "Done" is the transition back to the prompt (OSC 133 `D`, or the foreground process
  returning to the shell). Long-lived interactive programs (`vim`, `claude`, `ssh`) count as
  running while they are in the foreground.

### 4.3 Pane
- A pane is one terminal session: one PTY, one shell process, one `alacritty_terminal::Term`.
- Layout model: a binary tree. A leaf is a pane; an inner node is a **split** with a direction
  (`vertical` = side by side, `horizontal` = stacked) and a ratio (0.0–1.0). Splitting a pane
  replaces its leaf with a split node holding the old pane and the new one.
- Exactly one pane per tab is **focused** (GPUI `FocusHandle`); it receives keyboard input.
  Unfocused panes are dimmed slightly (`ui.dim_inactive_panes`, default 0.15: an overlay of
  `bg` at that opacity).

### 4.4 Titles
- A pane's title is, in order of preference:
  1. a title the user set (rename),
  2. the title set by the program via OSC 0 / OSC 2 (alacritty `Event::Title`),
  3. the foreground process name if it isn't the shell (`vim`, `htop`, `ssh`),
  4. `user@host:cwd`, with `$HOME` shortened to `~` (as in the screenshot: `pw@mbp:~`).
- The working directory comes from OSC 7 when the shell reports it, otherwise from the
  foreground process (`tcgetpgrp` on the PTY + `libproc` `proc_pidinfo` vnode path), refreshed
  on prompt marks / focus and at most every 500 ms while output flows.

### 4.5 Shell
- Default: the user's login shell (`$SHELL`, then the passwd entry), started as a login shell
  (`-l`). Overridable in config (`shell.program`, `shell.args`).
- Environment: inherits the app's environment plus `TERM=xterm-256color`,
  `COLORTERM=truecolor`, `TERM_PROGRAM=nox`, `TERM_PROGRAM_VERSION=<version>`, `LANG` defaulted
  to `en_US.UTF-8` if unset. Extra variables from `[shell.env]`. (A `nox` terminfo entry is a
  ROADMAP item; v1 uses `xterm-256color`.)
- New tabs and new panes start in the working directory of the focused pane
  (`shell.inherit_cwd = true`), otherwise in `shell.working_directory` (default `~`).
- Shell integration (optional, `shell.integration = true`): nox injects a small snippet for zsh
  (`ZDOTDIR` shim), bash (`--rcfile` shim) and fish (`XDG_DATA_DIRS` vendor conf) that emits
  OSC 7 (cwd) and OSC 133 (prompt marks). Prompt marks enable "jump to previous/next prompt"
  (`⌘↑` / `⌘↓`). Without it everything still works; titles fall back to process inspection.

### 4.6 Process icons
- Every tab and every pane header shows an icon of **what runs in it**, so tabs can be told apart
  at a glance (as in the screenshot: the Claude mark on `aif claude`, `>_` on `aif terminal`).
- The icon follows the **focused pane** of the tab and updates live when the foreground program
  changes (`claude` starts → Claude icon; it exits → back to `>_`).
- Detection, in order:
  1. a user override for the tab / pane (context menu: Set Icon…, also in the palette),
  2. the foreground process of the PTY (`tcgetpgrp` + `proc_pidinfo` / `proc_pidpath`), matched
     by executable name; for interpreters (`node`, `python`, `java`, `bun`, `deno`, `ruby`) also
     by their arguments, so `node …/claude` resolves to Claude and `java -jar app.jar` to Java,
  3. the title set via OSC 0/2, matched by known prefixes (e.g. `✳ Claude Code`),
  4. otherwise the default terminal icon `>_`.
  Refreshed together with the title (see 4.4): on prompt marks, focus and at most every 500 ms
  while output flows; a lookup is a map hit, never a filesystem scan.
- Built-in set (bundled SVGs, monochrome line style matching 5.1, optionally tinted with the
  tool's brand color at low saturation so it reads in both light and dark mode):
  - **AI coding agents**: Claude Code, Codex, Gemini CLI, GitHub Copilot CLI, Aider, Cursor
    Agent, opencode, Amp, Goose, Ollama.
  - **Languages / runtimes / REPLs**: Python (`python`, `ipython`, `uv`, `poetry`), Java (`java`,
    `jshell`), Kotlin, Scala (`sbt`), Node.js (`node`, `npm`, `pnpm`, `yarn`, `npx`), Bun, Deno,
    Ruby (`ruby`, `irb`, `rails`), Go, Rust (`cargo`, `rustc`), Swift (`swift`, `xcodebuild`),
    PHP, Elixir (`iex`, `mix`), Haskell (`ghci`), Lua, R, Julia, .NET (`dotnet`).
  - **Build tools**: Gradle, Maven, Make, CMake, Bazel.
  - **Version control**: Git (`git`, `lazygit`, `tig`, `gh`), Jujutsu (`jj`).
  - **Containers / cloud / infra**: Docker (`docker`, `docker-compose`, `lazydocker`),
    Kubernetes (`kubectl`, `k9s`, `helm`), Terraform / OpenTofu, AWS CLI, gcloud, Azure CLI,
    Fly (`fly`, `flyctl`).
  - **Editors**: Vim / Neovim, Emacs, Helix, nano, micro.
  - **Databases**: PostgreSQL (`psql`, `pgcli`), MySQL, SQLite, Redis (`redis-cli`), MongoDB
    (`mongosh`).
  - **Remote / network**: SSH (`ssh`, `mosh`, `et`), with the host shown in the title.
  - **Monitoring / tools**: htop / btop / top, tmux, zellij, man / less, curl / wget,
    ffmpeg, Homebrew (`brew`).
  - **Shells**: zsh, bash, fish, nu — all shown as the default `>_` unless a nested shell
    differs from the login shell.
- An icon for an unknown process is the default `>_`; it is never blank.
- User mappings in config (`[icons]`, see 11) add executables to a built-in icon or point to a
  custom SVG in `~/.nox/icons/`; they take precedence over the built-in table.
- The status dot (running / done / bell, see 4.2) overlays the icon's top-right corner instead
  of taking extra space (as in the screenshot on `#469 - cleanup after migration`).
- `tabs.icons = false` hides process icons and shows the plain `>_` everywhere.

## 5. UI

### 5.1 Look
Follows the kayet / zora design language (zora `docs/design.md` "App Direction" takes
precedence where details differ):

- Native Apple feel: system font (`.SystemUIFont`, SF Pro — the same as zora and kayet) for
  chrome by default, JetBrains Mono for the terminal (both changeable, see 5.10), native context
  menus (see 5.5), standard macOS shortcuts and menu bar. Respect
  Reduce Motion, Reduce Transparency and Increase Contrast — read from
  `NSWorkspace.accessibilityDisplayShould…` via `objc2` and re-read on
  `NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification`.
- Content first: the terminal grid takes nearly all of the window. Regions are separated by
  whitespace and 1px hairlines (`border`), not boxes or heavy shadows.
- Monochrome controls: icon buttons are 1.5px-stroke line icons (SVG assets rendered by GPUI's
  `svg()` and tinted) in `text-muted`, `text` on hover, with a subtle neutral fill on hover.
  The accent color only marks small signals: focus, selection, bell, the cursor.
- Corners: 6px on interactive elements (tab pills, buttons), 10px on the pane card, 12px on
  floating panels. Content inside a rounded card is kept within its padding (GPUI clips to
  rectangles), so the grid never touches a rounded corner.
- Motion: 120–150 ms ease-out fades and slides via GPUI's `Animation` API
  (`with_animation`); no bounce. Disabled under Reduce Motion. Hover states are instant (no
  transitions), as in native controls.
- No hardcoded colors in components: every color comes from the active `Theme` (a GPUI
  `Global`, see 9).

### 5.2 Window layout — tabs on top (default)

```
┌──────────────────────────────────────────────────────────────────────────┐
│ ● ● ●  [▢ ~]  [>_ 1 ~        ]  [>_ 2 ~            ]              ⌘   +  │  title bar (38px)
│ ┌──────────────────────────────────────────────────────────────────────┐ │
│ │ >_ pw@mbp:~                                          ◫  ⊟  ⤢  ×      │ │  pane header (28px)
│ │                                                                      │ │
│ │ ~                                                                    │ │
│ │ ❯ █                                                                  │ │  terminal grid
│ │                                                                      │ │
│ └──────────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────────┘
```

Title bar (left to right):
- Traffic lights (native).
- **Directory chip** `[▢ ~]`: the working directory of the focused pane, shortened. Click → menu:
  Copy Path, Reveal in Finder. In `left` mode this slot holds the sidebar toggle instead.
- **Tab strip**: one pill per tab, with a process icon (see 4.6; the default `>_` is tinted with
  `ansi.green` at low alpha when the tab is active, muted otherwise), the index and the title. Active tab:
  `bg-elevated` fill with a hairline border; inactive: no fill, `text-muted` label. Pills share
  the available width, between 120 px and 260 px each; when they don't fit, the strip scrolls
  horizontally (trackpad / wheel), keeping the active tab in view. Hovering a pill shows its `×`
  in place of the icon; middle-click closes. Drag to reorder (GPUI `on_drag` / `on_drop`);
  double-click to rename inline (`Enter` confirms, `Esc` cancels, an empty name restores the
  automatic title).
- Empty space in the title bar drags the window (`window.start_window_move()`); double-click
  zooms or minimizes per the system setting.
- Right side: **⌘** opens the command palette (see 5.6); **+** opens a new tab.

Pane card:
- Each tab's content is a card inset 6 px from the window edges, `bg` fill, 1px `border`
  hairline, 10px corners — as in the screenshot.
- **Pane header** (per pane, 28 px, `ui.pane_header = true`): process icon (see 4.6) + pane title on the
  left; on the right, shown on hover of the header (always shown for the focused pane at 60%
  opacity): **split vertically** (◫), **split horizontally** (⊟), **maximize / restore** (⤢),
  **close** (×). With a single pane in the tab the header still shows (it carries the title);
  `ui.pane_header = false` hides all headers, leaving only the hairline dividers.
- Terminal grid padding: `terminal.padding` (default 8 px horizontal, 6 px vertical). Leftover
  space after fitting whole cells goes to the right and bottom edge, filled with `bg`.

### 5.3 Tabs on the bottom

Same as 5.2, with the tab strip, the directory chip, `⌘` and `+` moved to a 34 px bar under the
pane card. The title bar keeps only the traffic lights and stays draggable; its height shrinks to
28 px. No sidebar.

### 5.4 Tabs in the left sidebar

```
┌──────────────────────────────────────────────────────────────────────────┐
│ ● ● ●  [◧]                                                        ⌘   +  │
│ ┌────────────────┐┌──────────────────────────────────────────────────────┐│
│ │ >_ 1  ~        ││ >_ pw@mbp:~                            ◫  ⊟  ⤢  ×    ││
│ │ >_ 2  ~/nox    ││                                                      ││
│ │    vim         ││ ❯ █                                                  ││
│ │ >_ 3  htop  •  ││                                                      ││
│ │                ││                                                      ││
│ └────────────────┘└──────────────────────────────────────────────────────┘│
└──────────────────────────────────────────────────────────────────────────┘
```

- `tabs.position = "left"`: tabs are rows in a sidebar on the left (`bg-sidebar`), one row per
  tab: process icon (see 4.6), index, title; a second muted line shows the working directory when it differs from
  the title. The active row has a subtle background pill. Status dot (see 4.2) on the right.
- Drag to reorder, double-click to rename, context menu as for pills (see 5.5).
- Sidebar width: drag its 1px divider, min 160 px, max 360 px, default 220 px; persisted.
- `⌘\` or the sidebar toggle `[◧]` in the title bar hides and shows it; the state is persisted.
  While hidden, `⌘1…9` and the palette still switch tabs.
- **When `tabs.position` is `top` or `bottom` there is no sidebar**: it is not rendered, `⌘\`
  does nothing and the toggle is not shown.
- Switching `tabs.position` in the config applies live, without restarting sessions.

### 5.5 Context menus (native)
- Shown as real `NSMenu`s (`popUpMenuPositioningItem:atLocation:inView:` via `objc2`); the
  chosen item is dispatched back as a GPUI action. (GPUI's own drawn menus are not used, so they
  look and behave like every other Mac app.)
- Tab: New Tab, Rename Tab…, Set Icon…, Duplicate Tab (same cwd), Move Left / Right, Close Tab, Close Other
  Tabs.
- Pane (right-click in the terminal): Copy, Paste, Select All, Clear Buffer, Split Vertically,
  Split Horizontally, Maximize Pane, Rename Pane…, Close Pane. With a URL or path under the
  pointer: Open, Copy Link, Reveal in Finder.

### 5.6 Command palette
- `⌘K` or the `⌘` title-bar icon opens a floating panel just below the title bar, like kayet's:
  a search field, a filtered list of commands (fuzzy match) with their shortcuts right-aligned,
  `↑` / `↓` navigate, `↵` runs, `Esc` closes, the footer hints the keys.
- Lists every command in 10 (keyboard shortcuts), plus `Switch Theme…`, `Switch Appearance…`,
  `Switch Tab…` (all tabs, filter by title/cwd) and `Tabs Position: Top / Bottom / Left`.
- Theme and appearance pickers live-preview the highlighted choice; `Esc` restores, `↵` applies
  and saves to config.
- Every command is a GPUI action (`actions!` / `Action` derive), so the palette, the menu bar,
  context menus and key bindings all dispatch the same actions.

### 5.7 Dialogs and notices
- In-app sheets as in kayet (bold title, muted message, right-aligned buttons, the default button
  monochrome-filled and focused, `Esc` cancels, `Tab` cycles). Used for: closing a tab / pane /
  the app while a process other than the shell is running (`Cancel` / `Close`).
- Non-blocking notices (toast, bottom-right of the window, 4 s): config parse errors, invalid
  values, unknown theme, shell failed to start (with the error), font not found.

### 5.8 Text inputs
- The palette search, find bar and inline rename use one small single-line text input
  component (from `gpui-component`, or own on top of GPUI's `EntityInputHandler`): caret,
  selection, `⌥`/`⌘` word and line movement, copy / paste, undo, IME marked text.

### 5.9 Settings window
- `⌘,` / nox → Settings… opens a separate, single-instance Settings window (a second `⌘,`
  focuses it). It is a view over `config.toml` (see 11), not a separate store: the file stays
  the single source of truth.
- Layout: a sidebar of sections mirroring the config tables — General (`[app]`), Appearance
  (`[ui]`, theme, mode), Tabs (`[tabs]`), Font (`[font]`), Terminal (`[terminal]`), Shell
  (`[shell]`, `[shell.env]`), Icons (`[icons]`), Keys (`[keys]`) — and a scrollable form on the
  right. Each setting shows a label, a one-line description and the matching config key in muted
  monospace (`tabs.position`), so the UI teaches the file.
- Controls by type: switches for booleans, segmented controls / popups for enums, steppers or
  sliders with a numeric field for numbers, text inputs (see 5.8) for strings, font pickers
  (see 5.10), a theme picker with live preview, key-value tables for
  `[icons]` and `[shell.env]`, and a shortcut recorder for `[keys]` (click, press the
  combination; conflicts are flagged).
- Changes apply immediately (no Save button) and are written to `config.toml` at once.
- Settings that differ from the default show a small "reset" control; resetting removes the key
  from the file, so it falls back to the default again.
- A search field at the top filters settings across all sections by label, description and key.
- Footer: **Open config.toml** (opens the file in the external editor, see 11) and **Reveal in
  Finder**.
- Edits made to the file outside the window (vim, any editor) show up in the open window right
  away (same file watcher as live reload). If the file fails to parse, the window shows a banner
  with the line and error plus **Open config.toml**, and its controls are disabled until the
  file is valid again — nox never overwrites a file it couldn't parse.

### 5.10 Fonts
Two independent settings: the terminal font (`font.family`) and the interface font
(`ui.font_family`, used for tabs, sidebar, pane headers, palette, dialogs and Settings).

- **Terminal font** — default **JetBrains Mono**. Bundled alternatives (all monospace, all
  under SIL OFL 1.1): Geist Mono, IBM Plex Mono, Fira Code, Cascadia Code, Iosevka, Hack,
  Source Code Pro, Commit Mono. The system fonts SF Mono and Menlo are also offered (not
  bundled, always present on macOS). Ligatures stay off regardless of the font (see 2).
- **Interface font** — default **system** (`.SystemUIFont`, SF Pro), the same as in zora and
  kayet. Bundled alternatives (SIL OFL 1.1): Geist, Inter, IBM Plex Sans.
- Bundled fonts ship inside the `.app` (`assets/fonts/`, Regular / Bold / Italic / Bold Italic
  for terminal fonts; Regular / Medium / Semibold for interface fonts) and are registered for
  the process only at startup (`CTFontManagerRegisterFontsForURL` with
  `kCTFontManagerScopeProcess`) — nothing is installed system-wide. Their OFL license texts are
  included in the bundle and in About → Acknowledgements.
- Any installed family can be used too, by name in `config.toml` or in the picker.
- Settings → Font has two pickers: terminal (bundled fonts first, then installed monospace
  families) and interface (bundled fonts first, then all installed families). Each entry is
  rendered in its own face; the choice applies live, like every other setting.
- A family that isn't found falls back to the default (JetBrains Mono / SF Pro) with a notice
  (see 5.7).

## 6. Panes and Tabs Behavior

### 6.1 Splitting
- **Split vertically** (`⌘D`): the focused pane is split into two side by side; the new pane
  opens on the right and is focused. **Split horizontally** (`⌘⇧D`): the new pane opens below.
  (Same keys and semantics as iTerm.)
- Splits nest without limit, but a pane is never smaller than `20 cols × 5 rows`; a split that
  would make either side smaller is refused with a short shake of the focused pane header (a
  notice instead under Reduce Motion).
- The new pane starts at a 0.5 ratio of the pane it was split from.

### 6.2 Focus and navigation
- Click a pane to focus it (the click is not forwarded to the program unless the pane was
  already focused, or mouse reporting is on and `ui.focus_click_through = true`).
- `⌘⌥←/→/↑/↓`: focus the nearest pane in that direction (geometric, by overlap with the focused
  pane's edges, like iTerm). `⌘[` / `⌘]`: previous / next pane in tree order, wrapping.
- `ui.focus_follows_mouse = false` (default); when true, hovering focuses.

### 6.3 Resizing panes
- Drag the divider between panes (a 1px hairline with a 6 px invisible hit area; the cursor
  changes to `ResizeLeftRight` / `ResizeUpDown`; the hairline turns `accent` while dragging).
- `⌘⌃←/→/↑/↓` moves the nearest divider of the focused pane by one cell.
- Double-click a divider equalizes its two sides. `Equalize Panes` (palette, `⌘⌃=`) equalizes the
  whole tab.
- Dragging is live: panes relayout and redraw every frame (see 7.3 for how the PTY is resized).

### 6.4 Maximize and close
- **Maximize pane** (`⌘⇧↵`): the focused pane fills the tab; others keep running, hidden. The
  header's ⤢ turns into ⤡ and the tab pill shows a small "zoomed" mark. Same key or ⤡ restores.
  Focusing another pane via navigation keys restores first.
- **Close pane** (`⌘W`): closes the focused pane; its sibling takes its space and focus. Closing
  the last pane closes the tab. Closing the last tab closes the window and quits the app
  (`app.quit_when_last_tab_closed = true`; otherwise a new tab opens).
- When the shell of a pane exits, the pane closes (`terminal.close_on_exit = "always" |
  "clean" | "never"`; default `"clean"`: closes on exit status 0, otherwise keeps the output
  with a muted `[process exited with code N — press ↵ to close, ⌘R to restart]` line).
- A process other than the shell running in the pane (or any pane of the tab / app) asks first
  (see 5.7); `app.confirm_close = false` disables it.

### 6.5 Tabs
- `⌘T` new tab (after the current one), `⌘⇧W` close tab, `⌘1…⌘8` go to tab N, `⌘9` last tab,
  `⌘⇧[` / `⌘⇧]` (and `⌃⇧⇥` / `⌃⇥`) previous / next, wrapping. `⌘⇧←` / `⌘⇧→` move the tab.
- **Renaming**: the user can edit a tab's name at any time — double-click the pill / sidebar row,
  `⌘⇧I`, the tab context menu (Rename Tab…) or the palette. The title turns into an inline text
  input (see 5.8) prefilled with the current name and fully selected; `Enter` or clicking away
  confirms, `Esc` cancels, an empty name restores the automatic title (see 4.2). The index
  prefix stays and is not editable. A custom name is kept until changed or cleared, and
  survives moving the tab and session restore.
- Each tab keeps its own layout, focus, maximize state and per-pane scroll position.
- Switching tabs is instant: panes of inactive tabs keep reading and parsing output but are not
  rendered; the shown tab draws from its current grid in the next frame.

### 6.6 Session restore
- On quit, the tab order, each tab's layout tree (directions, ratios), each pane's working
  directory and custom name, each tab's custom name, and the active tab are saved to `~/.nox/session.toml`.
- On launch with `app.restore_session = true` (default), that layout is re-created with fresh
  shells in the saved directories. Scrollback and running programs are not restored.

## 7. Terminal Behavior

### 7.1 Emulation
- Provided by `alacritty_terminal`: xterm-256color compatible, 24-bit color, bold / italic / dim
  / inverse / hidden / strikethrough, underline styles (single, double, curly, dotted, dashed)
  with colored underlines, alternate screen, bracketed paste, focus reporting, mouse reporting
  (normal, button, any-event; SGR and UTF-8 encodings), synchronized output (DEC 2026), OSC 8
  hyperlinks, OSC 52 clipboard (alacritty `ClipboardStore` / `ClipboardLoad` events; nox allows
  write only by default, `terminal.osc52 = "write" | "read-write" | "off"`).
- Handled by nox before the parser (see 8, OSC sniffer): OSC 7 (cwd), OSC 133 (prompt marks),
  OSC 9 / OSC 777 notifications (trigger the attention sound and a macOS notification, see
  7.11).
- Unicode: wide characters and emoji per alacritty's width tables; combining characters kept in
  the cell. Glyphs missing from the configured font fall back through Core Text's cascade list
  (so CJK and emoji render without configuration).
- Kitty keyboard protocol: supported by `alacritty_terminal`; enabled with
  `terminal.kitty_keyboard = true` (default `false` in v1).

### 7.2 Rendering
- The pane view is a custom GPUI `Element` (like Zed's `TerminalElement`):
  1. On paint, lock the `Term` briefly and copy the visible cells (`renderable_content()`) into a
     reusable frame buffer, then release the lock — the reader thread is never blocked for the
     whole paint.
  2. Merge adjacent cells with the same background into rectangles (`paint_quad`), and adjacent
     cells with the same style into text runs.
  3. Shape each run with GPUI's text system (Core Text), positioned on the cell grid
     (monospace advance from the font's `M`/`0` width, so wide and fallback glyphs never shift
     the grid), and paint the glyphs from GPUI's glyph atlas.
  4. Paint cursor, selection, search matches, hovered link underline and the scrollbar on top.
- Shaped lines are cached per row by content hash and reused between frames, so a frame where
  only the cursor or one line changed re-shapes only that line.
- Bold text uses the bold face (`font.weight_bold`); "bold is bright" is off by default
  (`terminal.bold_is_bright = false`).
- Frames are paced by GPUI (display link): output notifies the view at most once per frame,
  however many bytes arrived.

### 7.3 Scrolling
Goal: feel like iTerm on a MacBook trackpad.
- **Pixel-smooth trackpad scrolling** with native momentum: GPUI `ScrollWheelEvent`s with
  `ScrollDelta::Pixels` (precise deltas, including the momentum phase macOS synthesizes)
  accumulate into a pixel offset. Whole lines go to alacritty's `display_offset`
  (`Term::scroll_display`); the remaining fraction of a line is applied as a sub-line paint
  offset, with one extra row drawn above/below so the viewport moves continuously.
- `ScrollDelta::Lines` (mouse wheels) scroll `terminal.scroll_multiplier` (default 3) lines per
  notch, animated over `terminal.smooth_scroll_duration` (default 100 ms, ease-out; 0 disables)
  using the same sub-line offset.
- Rendering runs at the display refresh rate (120 Hz on ProMotion).
- Scrollback: `terminal.scrollback = 10000` lines per pane (max 100 000).
- When output arrives while scrolled up, the viewport stays put (no jump to bottom); a small
  pill "↓ New output" appears at the bottom of the pane; click or `⌘End` jumps to the bottom.
  Typing jumps to the bottom (`terminal.scroll_on_input = true`).
- Alternate screen (vim, less, htop): scrolling becomes mouse reports when the program enables
  mouse mode, otherwise arrow keys (alacritty's alternate scroll mode); fractional pixel deltas
  accumulate until they amount to a whole line.
- Keys: `⇧PgUp` / `⇧PgDn` page, `⌘Home` / `⌘End` top / bottom, `⌘↑` / `⌘↓` previous / next
  prompt (with shell integration; otherwise one line).
- Overlay scrollbar, macOS style: hidden until scrolling, 6 px thumb in `text-muted` at 40%,
  fades out after 800 ms; draggable.

### 7.4 Resizing
Goal: no flicker, no stretched or torn frames, no prompt duplication while dragging the window
corner or a divider.
- The grid size (cols × rows) is computed from the pane's bounds, cell metrics and padding in
  layout, every frame during a resize; `Term::resize` runs in the same frame, so content is
  always drawn at the right size (text reflows on width change; the cursor line stays anchored
  to the bottom when the viewport is at the bottom).
- The PTY resize (`TIOCSWINSZ` → `SIGWINCH`) is **coalesced**: sent at most once per 33 ms while
  the size keeps changing, and once more with the final size 50 ms after it stops — so shells and
  full-screen programs redraw a few times, not on every pixel.
- Live window resize draws synchronously with the resize (GPUI renders from the view's
  `setFrameSize` / display link during live resize); the `NSWindow` background color is set to
  the theme's `bg` so newly exposed areas never flash white. **This is verified in the first
  prototype** (see 16); if GPUI shows stretched frames, patch the Metal layer setup
  (`presentsWithTransaction` during live resize) in the pinned GPUI revision.
- Font size changes (`⌘+`, `⌘-`, `⌘0`) resize the grid, not the window, and apply to all panes
  (`font.size_applies_to = "all" | "pane"`).

### 7.5 Cursor
- `terminal.cursor_style = "block" | "bar" | "underline"`, `terminal.cursor_blink = true`.
  Programs can change style and blinking via DECSCUSR. Blinking stops after 5 s of inactivity
  (no timers when idle) and while the window is unfocused, where the cursor is drawn hollow.

### 7.6 Selection and clipboard
- `alacritty_terminal` selections: click-drag selects (`Simple`); double-click a word
  (`Semantic`, `terminal.word_separators`); triple-click a line (`Lines`); `⌥`-drag a
  rectangular block (`Block`); `⇧`-click extends. With mouse reporting on, `⇧` forces selection.
- Dragging past the top or bottom edge auto-scrolls, faster the further the pointer is.
- `⌘C` copies (when there is a selection; otherwise nothing), `⌘V` pastes (bracketed if the
  program asked). `terminal.copy_on_select = false`. Trailing whitespace is trimmed on copy.
- Paste safety: pasting text with several lines into a shell outside bracketed mode asks first
  (`terminal.confirm_multiline_paste = true`).
- `⌘A` selects all scrollback.

### 7.7 Links
- OSC 8 hyperlinks (from the cell's hyperlink attribute) and plain URLs (regex over the hovered
  line, wrapped lines joined) are underlined on hover with `⌘` held; `⌘`-click opens them with
  `cx.open_url`. File paths relative to the pane's cwd are detected too; `⌘`-click opens them
  with the default app, `⌘⌥`-click reveals them in Finder (`cx.reveal_path`).

### 7.8 Find
- `⌘F` opens a compact find bar at the top-right of the focused pane: query field, match count
  (`3/17`), previous / next (`⇧↵` / `↵`, `⌘G` / `⌘⇧G`), case-sensitive and regex toggles.
  Search uses `alacritty_terminal`'s `RegexSearch` over the whole scrollback, run off the main
  thread for large buffers. Matches are highlighted with the theme's selection color, the current
  one in accent. `Esc` closes and returns focus to the terminal.

### 7.9 Keyboard and IME
- Key events (`KeyDownEvent` with `Keystroke`) are translated to bytes by nox's key encoder,
  honoring alacritty's mode flags (application cursor keys, application keypad, Kitty
  protocol when on).
- `terminal.option_as_meta = "left" | "right" | "both" | "none"` (default `"left"`): the chosen
  Option key sends `ESC`-prefixed sequences; the other one types special characters.
- `⌥←` / `⌥→` send word-left / word-right (`ESC b` / `ESC f`), `⌘←` / `⌘→` send line start / end
  (`^A` / `^E`), `⌥⌫` deletes a word, `⌘⌫` deletes the line (`^U`) — iTerm "Natural Text
  Editing" defaults, overridable in `[keys]`.
- IME, dead keys and the emoji picker (`⌃⌘Space`) go through GPUI's platform input handler
  (`EntityInputHandler` on the pane): marked (composing) text is drawn inline at the cursor
  with an underline, committed text is written to the PTY.
- Every app shortcut that isn't a standard macOS one can be rebound or unbound in `[keys]`
  (see 11), loaded as GPUI key bindings; an unbound key is passed to the program.

### 7.10 Bell
- `terminal.bell = "visual" | "sound" | "none"` (default `"visual"`: the pane header flashes
  `accent` for 150 ms; `"sound"` plays `NSBeep`). A bell in a background tab marks the tab (see
  4.2); with the window unfocused it bounces the Dock icon once
  (`NSApp.requestUserAttention`, `app.dock_bounce_on_bell = true`).

### 7.11 Attention: sound and notifications
- When a job in a pane has finished, or stopped and waits for the user to act, nox lets the user
  know in two independent ways, each of which can be turned off: a soft chime and a macOS system
  notification. So the user can look away while a job runs and come back when needed.
- Triggers, in a pane the user isn't looking at (a background tab, or any pane while the nox
  window is unfocused):
  - **done**: a command finishes (the "done" transition, see 4.2), if it ran for at least
    `terminal.attention_min_duration` seconds (default 10), so quick commands stay quiet;
  - **attention**: a program asks for it with an OSC 9 / OSC 777 notification (see 7.1), e.g. a
    coding agent like `claude` finishing its turn or waiting for a permission prompt.
- Nothing fires for the pane the user is looking at (focused window, active tab), so typing at a
  prompt never chimes or notifies.

**Sound**
- A bundled, soft and light tone (`assets/sounds/attention.caf`, ~0.3 s, gentle attack, no sharp
  transients, quiet by default), played through `NSSound` at `terminal.attention_sound_volume`
  (0.0–1.0, default 0.4). It respects the system output device and mute.
- Not irritating: several triggers within 3 s (e.g. several panes finishing together) play one
  chime; the same pane chimes at most once until it is visited again. It is independent of
  `terminal.bell` (7.10), which only reacts to the BEL character.

**System notification**
- Posted through `UNUserNotificationCenter` (`nox_mac`), so it shows in Notification Center and
  follows the user's Focus / Do Not Disturb and per-app notification settings.
- Content:
  - done: title `<command> finished` (e.g. `cargo build finished`), or `<command> failed (exit
    1)` when the exit status from OSC 133 `D` is non-zero; body: the tab title and the running
    time (`2 ~/Development/nox · 2m 14s`). Without shell integration the command is the process
    name and no exit status is shown.
  - attention: the title and body the program sent (OSC 777 `notify;title;body`, OSC 9 body
    only, titled with the process name, e.g. `claude`).
- Clicking the notification brings nox to the front, switches to the tab and focuses the pane.
- One notification per pane at a time (`threadIdentifier` per pane, a new one replaces the old);
  a pane's notifications are removed from Notification Center when it is visited or closed.
- Notifications are silent at the system level; the sound is nox's own chime above, so the two
  never play twice and each toggle does what it says.
- Permission is requested the first time a notification would be posted, not at launch. If the
  user has denied it, Settings → Terminal shows that next to the switch with an **Open System
  Settings** button (Notifications pane).

**Settings**
- Both are on by default and turned off separately: `terminal.attention_sound = false`,
  `terminal.notifications = false`, in Settings → Terminal (two switches, a volume slider with a
  preview button, a "notify for commands longer than" stepper) or in `config.toml` by hand. Like
  every setting, the change applies immediately from either place (see 5.9 and 11).

## 8. Performance Design

- **One reader thread per pane** (std thread, blocking `read` on the PTY master into a 64 KiB
  buffer). Each chunk goes through the **OSC sniffer** (a tiny state machine that extracts OSC
  7 / 133 / 9 / 777 without copying the rest), then is fed to the parser
  (`vte::ansi::Processor::advance(&mut term, bytes)`) under the `Term`'s `FairMutex`.
- **Lock discipline**: the reader holds the lock for at most ~1 MiB of input (or 4 ms) at a time,
  then releases it so the UI can paint; the UI holds it only to copy visible cells.
- **Wakeups**: after parsing, the reader sends one "dirty" signal through a channel; the pane
  view coalesces them into a single `cx.notify()` per frame.
- **Backpressure**: if a pane produces output faster than it is parsed (e.g. `cat huge.log`),
  the reader simply keeps parsing at full speed — rendering only ever shows the latest state,
  once per frame, so no queue builds up. `^C` is written to the PTY directly from the UI thread
  and is never stuck behind output.
- **Input path**: `KeyDownEvent` → key encoder → `write(2)` on the PTY master on the UI thread
  (small writes; large pastes go to a writer thread so the UI never blocks).
- **Only visible panes render.** Hidden tabs keep parsing but don't paint.
- **Idle is idle**: GPUI doesn't redraw without `notify`; the cursor-blink timer stops after 5 s;
  the cwd poll runs only while output flows. Target ~0% CPU idle.

## 9. Theming

Same model as kayet (kayet `SPEC.md` §9), extended with terminal colors.

- `theme` picks the palette, `mode` picks when light or dark is used (`"system"` default,
  following macOS appearance live via `window.appearance()` / `observe_window_appearance`;
  `"light"` / `"dark"` pin a variant).
- `theme = "nox"` (default) is the built-in palette (below); any other value is the name of a
  theme file, `~/.nox/themes/<name>.toml` (letters, digits, `_`, `-`; `nox`, `system`, `light`,
  `dark` are reserved). Bundled themes are compiled into the binary (`include_str!`) and always
  available; a user file with the same name overrides a bundled one.
- **Bundled themes** — all of kayet's, with the same names, light/dark variants and upstream
  sources: `1984`, `ayu`, `bluloco`, `catppuccin`, `everforest`, `flexoki`, `github`, `gruvbox`,
  `kanagawa`, `modus`, `nord` (dark only), `rose-pine`, `solarized`, `tokyo-night`, `vitesse`,
  `white`, `zenwritten`. Their UI tokens are copied from kayet's files; their ANSI colors are
  taken from the same upstream source each file cites (e.g. Catppuccin's terminal palette,
  nord's `nord-iterm2`, `kanagawa.nvim` `extras/`, `tokyonight.nvim` `extras/`, the VS Code
  themes' `terminal.ansi*` colors for the VS Code ports). Each file cites its sources in a
  header comment, like kayet's.
- A single-variant theme is always shown in its variant; the appearance toggle is disabled
  while it is applied.
- A theme file holds, per variant, kayet's UI tokens and the terminal tokens (hex `#rgb`,
  `#rrggbb`, `#rrggbbaa`):

```toml
# Catppuccin for nox: Mocha (dark) and Latte (light).
# Source of truth for the colors: https://catppuccin.com/palette

[dark]
# UI (same keys as kayet)
bg = "#1e1e2e"            # terminal background and pane card
bg-sidebar = "#181825"    # sidebar, title bar
bg-elevated = "#313244"   # active tab pill, floating panels (optional, derived from bg)
text = "#cdd6f4"          # terminal foreground and UI text
text-muted = "#9399b2"
border = "#313244"
accent = "#cba6f7"
selection = "#9399b240"
caret = "#f5e0dc"         # terminal cursor
caret-text = "#1e1e2e"    # text under a block cursor (optional, defaults to bg)

[dark.ansi]
black = "#45475a"
red = "#f38ba8"
green = "#a6e3a1"
yellow = "#f9e2af"
blue = "#89b4fa"
magenta = "#f5c2e7"
cyan = "#94e2d5"
white = "#bac2de"
bright-black = "#585b70"
bright-red = "#f38ba8"
bright-green = "#a6e3a1"
bright-yellow = "#f9e2af"
bright-blue = "#89b4fa"
bright-magenta = "#f5c2e7"
bright-cyan = "#94e2d5"
bright-white = "#a6adc8"

[light]
# ...
[light.ansi]
# ...
```

- Missing keys fall back to the built-in `nox` palette of the same variant; `[ansi]` missing
  entirely → built-in ANSI colors. kayet's `hl-*` keys are accepted and ignored, so a kayet theme
  file dropped into `~/.nox/themes/` works (with built-in ANSI colors). Unknown keys and invalid
  values are ignored with a notice.
- At runtime the resolved theme is a `Theme` struct (all colors as GPUI `Hsla`, the 16 ANSI
  colors plus the 256-color cube and grayscale ramp precomputed) stored as a GPUI `Global`.
  Changing it replaces the global and refreshes all windows — live, for UI and every pane; the
  `NSWindow` background follows.
- `terminal.minimum_contrast` (default `1.0`, off; e.g. `4.5`): when drawing, nox lightens or
  darkens a cell's foreground that is too close to its background.
- `Switch Theme…` and `Switch Appearance…` (`⌘⇧L`) pickers in the palette, with live preview
  (see 5.6).

### Built-in palette `nox` (indicative)

| Token          | Light       | Dark        |
|----------------|-------------|-------------|
| `bg`           | `#FFFFFF`   | `#18191B`   |
| `bg-sidebar`   | `#F7F7F8`   | `#1C1D20`   |
| `bg-elevated`  | `#EFEFF1`   | `#2A2B2F`   |
| `text`         | `#1D1D1F`   | `#E6E6E8`   |
| `text-muted`   | `#8A8A8E`   | `#8A8A8F`   |
| `border`       | `#E6E6E8`   | `#2A2A2E`   |
| `accent`       | `#0891B2`   | `#22D3EE`   |
| `selection`    | `#0891B233` | `#22D3EE33` |
| `caret`        | `#1D1D1F`   | `#D7DED7`   |

ANSI (dark): a restrained set in the same spirit — e.g. red `#F07178`, green `#8BD49C`,
yellow `#E6C07B`, blue `#6CB6FF`, magenta `#F075C1`, cyan `#5CCFE6`; final values chosen
for WCAG AA against `bg` in both variants.

## 10. Keyboard Shortcuts (defaults)

| Action                          | Shortcut            |
|---------------------------------|---------------------|
| New tab                         | `⌘T`                |
| Close pane (or tab if last)     | `⌘W`                |
| Close tab                       | `⌘⇧W`               |
| Go to tab 1–8 / last            | `⌘1`…`⌘8` / `⌘9`    |
| Previous / next tab             | `⌘⇧[` / `⌘⇧]`, `⌃⇧⇥` / `⌃⇥` |
| Move tab left / right           | `⌘⇧←` / `⌘⇧→`       |
| Rename tab                      | `⌘⇧I`               |
| Split vertically (right)        | `⌘D`                |
| Split horizontally (below)      | `⌘⇧D`               |
| Focus pane in direction         | `⌘⌥←/→/↑/↓`         |
| Previous / next pane            | `⌘[` / `⌘]`         |
| Resize pane                     | `⌘⌃←/→/↑/↓`         |
| Equalize panes                  | `⌘⌃=`               |
| Maximize / restore pane         | `⌘⇧↵`               |
| Toggle sidebar (left mode only) | `⌘\`                |
| Command palette                 | `⌘K`                |
| Find                            | `⌘F`                |
| Clear buffer                    | `⌘⇧K`               |
| Copy / Paste / Select all       | `⌘C` / `⌘V` / `⌘A`  |
| Font bigger / smaller / reset   | `⌘+` / `⌘-` / `⌘0`  |
| Previous / next prompt          | `⌘↑` / `⌘↓`         |
| Restart exited pane             | `⌘R`                |
| Switch appearance               | `⌘⇧L`               |
| Settings window                 | `⌘,`                |
| Open config.toml in editor      | `⌥⌘,`               |
| Full screen                     | `⌃⌘F`               |
| Quit                            | `⌘Q`                |

All of them are also in the native menu bar (`cx.set_menus`: nox, Shell, Edit, View, Window,
Help), so they show up in macOS's Help search and display their shortcuts.

## 11. Configuration

Location: `~/.nox/config.toml` (directory created on first launch). Missing file or keys →
defaults; the file is written with defaults and comments on first launch only.

All settings live in this one plain-text file, and there are two equal ways to change them:
- **By hand**, in any editor (vim, an external GUI editor, …). nox picks the change up on save
  (see live reload below).
- **In the Settings window** (see 5.9), or through UI shortcuts that change a value (theme
  picker, appearance, tabs position, sidebar visibility / width, font size reset).

Whatever the UI changes is written back to `config.toml` immediately. Writes are surgical
(`toml_edit`): only the affected key is added, changed or removed; comments, ordering, blank
lines and unknown keys are preserved. Writes are atomic (temp file + rename) and nox ignores the
file-watch event caused by its own write. nox never writes to a file that currently fails to
parse.

```toml
[ui]
theme = "nox"                # palette: "nox" or a theme from ~/.nox/themes/ or bundled (see 9)
mode = "system"              # "system" | "light" | "dark"
font_family = "system"       # interface font: "system" (SF Pro) | "Geist" | "Inter" | "IBM Plex Sans" | any installed family
pane_header = true           # title + split/maximize/close controls above each pane
dim_inactive_panes = 0.15    # 0.0 disables
focus_follows_mouse = false
focus_click_through = false
window_padding = 6           # px between the window edge and the pane card

[tabs]
position = "top"             # "top" | "bottom" | "left"; top/bottom: no sidebar
sidebar_visible = true       # left mode only
sidebar_width = 220          # left mode only
show_index = true            # "1 ~" vs "~"
title = "auto"               # "auto" (see 4.4) | "cwd" | "process"
icons = true                 # process icons in tabs and pane headers (see 4.6)

[icons]
# executable = built-in icon name, or a path to an SVG in ~/.nox/icons/
# "my-agent" = "claude"
# "jbang" = "java"
# "work-cli" = "~/.nox/icons/work.svg"

[font]
family = "JetBrains Mono"    # terminal font: a bundled one (see 5.10), "SF Mono", "Menlo" or font names, e.g. "Iosevka, Sarasa Mono J"
size = 13
line_height = 1.2
letter_spacing = 0
weight = "normal"
weight_bold = "bold"
size_applies_to = "all"      # "all" | "pane"

[terminal]
scrollback = 10000
scroll_multiplier = 3
smooth_scroll_duration = 100 # ms, mouse wheels only; 0 disables
scroll_on_input = true
cursor_style = "block"       # "block" | "bar" | "underline"
cursor_blink = true
padding = [8, 6]             # [horizontal, vertical] px
copy_on_select = false
confirm_multiline_paste = true
word_separators = " ()[]{}',\"`│"
option_as_meta = "left"      # "left" | "right" | "both" | "none"
bold_is_bright = false
bell = "visual"              # "visual" | "sound" | "none"
attention_sound = true       # soft chime when a job finishes or waits for input unseen (see 7.11)
attention_sound_volume = 0.4 # 0.0–1.0
notifications = true         # macOS notification when a job finishes or waits for input unseen (see 7.11)
attention_min_duration = 10  # s; shorter commands finish without a chime or notification
close_on_exit = "clean"      # "always" | "clean" | "never"
osc52 = "write"              # "write" | "read-write" | "off"
kitty_keyboard = false
minimum_contrast = 1.0

[shell]
program = ""                 # empty: login shell from $SHELL
args = ["-l"]
working_directory = "~"
inherit_cwd = true
integration = true           # inject OSC 7 / OSC 133 snippets for zsh, bash, fish

[shell.env]
# EXAMPLE = "value"

[app]
restore_session = true
confirm_close = true
quit_when_last_tab_closed = true
dock_bounce_on_bell = true
check_for_updates = true      # daily check against GitHub Releases (see 15)
auto_install_updates = false  # true: download in the background, install on quit

[keys]
# action = "shortcut"; "" unbinds and passes the key to the program
# split_vertical = "cmd-d"
# split_horizontal = "cmd-shift-d"
# command_palette = "cmd-k"
# send text: map a key to bytes written to the PTY
# "alt-left" = { send = "\u001bb" }

[window]
width = 1000
height = 680
x = 0
y = 0
```

- `font.family`: a family or comma-separated list, always followed by JetBrains Mono, so a font
  that isn't installed falls back to it (with a notice). Default `"JetBrains Mono"`.
- `ui.font_family`: `"system"` is SF Pro (`.SystemUIFont`); any other value is a family, falling
  back to SF Pro (with a notice) if it isn't found. Bundled fonts: see 5.10.
- Key syntax follows GPUI keystrokes (`cmd-shift-d`, `ctrl-tab`, `alt-left`).
- `⌘,` / nox → Settings… opens the Settings window (see 5.9). `⌥⌘,` / nox → Open Config File
  (also in the palette and the Settings window footer) opens `config.toml` in a new tab with
  `$VISUAL` / `$EDITOR` (fallback: `open -t`, i.e. the default text editor — kayet if
  installed).
- **Live reload**: the file is watched (`notify`, debounced 150 ms); valid changes apply
  immediately to the UI and every pane (font, theme, padding, tabs position, cursor, keys,
  sounds).
  Shell settings apply to new panes only. Window geometry and the session are owned by the
  running app and not reloaded.
- A file that fails to parse keeps the last good config and shows a notice with the line and
  error; invalid individual values fall back to their default with a notice.

## 12. Performance Targets

Measured on an M1 MacBook Air (the slowest supported machine), release build:

| Metric                                              | Target            |
|-----------------------------------------------------|-------------------|
| Launch to first prompt drawn (warm)                 | < 150 ms          |
| Keypress to glyph on screen (typing in zsh)         | < 8 ms p50, < 12 ms p99 |
| Scrolling a 10 000-line scrollback                  | 120 fps on ProMotion, no dropped frames at p95 |
| `cat` of a 100 MB text file                         | < 2 s, UI responsive, `^C` stops within 50 ms |
| Window live resize with 4 panes                     | ≥ 60 fps, no blank, stretched or torn frames |
| New tab / split to prompt                           | < 50 ms           |
| Memory, 1 tab, 1 pane, idle                         | < 60 MB total     |
| Each additional pane (10k scrollback, idle)         | < 10 MB           |
| CPU, idle with 10 panes                             | ~0%               |
| `.dmg` size                                         | < 20 MB           |

- `cargo bench` (criterion) covers the hot paths: parsing throughput (`Processor::advance` on
  recorded streams: `cat` of source code, `htop`, `vim` scrolling, colored `ls`), visible-cell
  copy, run merging and row shaping. Regressions over 10% fail CI.
- An instrumented build (`--features latency-trace`) logs key-event → frame-presented times and
  frame times during scripted scroll / resize, for the latency, scroll and resize rows.

## 13. Architecture

Cargo workspace, split so that GPUI is only a dependency of the UI crates (faster rebuilds of
the rest, and the fallback in 3 stays possible):

```
nox/
├── Cargo.toml                  # workspace, release profile, pinned gpui
├── rust-toolchain.toml
├── crates/
│   ├── nox/                    # binary: main.rs, app setup, menus, global actions
│   ├── nox_terminal/           # no GPUI: PTY spawn/resize/kill, reader thread, OSC sniffer,
│   │                           #   Term wrapper, key encoder, selection/search helpers,
│   │                           #   process info (libproc), shell integration snippets
│   ├── nox_layout/             # no GPUI: tabs + split trees (pure data, heavily unit tested)
│   ├── nox_config/             # no GPUI: config model, load/validate, toml_edit writes, watcher
│   ├── nox_theme/              # no GPUI: theme parsing, bundled themes, color resolution
│   ├── nox_mac/                # objc2: NSMenu context menus, accessibility flags,
│   │                           #   Dock bounce, notifications, NSWindow background color,
│   │                           #   Sparkle updater controller (feature `updater`)
│   └── nox_ui/                 # GPUI views and elements:
│       ├── workspace.rs        #   window root: title bar, tab bar/sidebar, active tab
│       ├── tab_bar.rs          #   pills (top/bottom) and sidebar rows (left)
│       ├── pane_group.rs       #   renders a split tree, dividers, drag-resize
│       ├── pane.rs             #   pane header + terminal view, focus, find bar
│       ├── terminal_element.rs #   custom Element: cells → quads + shaped runs
│       ├── scroll.rs           #   pixel accumulator, sub-line offset, overlay scrollbar
│       ├── palette.rs          #   command palette + pickers
│       ├── settings.rs         #   Settings window: sections, controls, search (see 5.9)
│       ├── text_input.rs
│       ├── dialog.rs, toast.rs
│       └── theme.rs            #   Theme global (Hsla), applying nox_theme output
├── .github/workflows/
│   ├── ci.yml                  # PRs + master: fmt, clippy, test, bench, unsigned bundle
│   └── release.yml             # tag v*: build, sign, notarize, staple, appcast, release, cask
├── assets/
│   ├── icons/                  # SVG line icons (1.5px stroke)
│   ├── fonts/                  # bundled terminal + interface fonts and their OFL licenses (see 5.10)
│   ├── sounds/                 # attention.caf (see 7.11)
│   └── app/                    # .icns, Info.plist, nox.entitlements
├── scripts/
│   └── release/                # sign.sh, notarize.sh, make_dmg.sh, appcast.sh (run by CI and locally)
├── themes/                     # bundled theme files (see 9)
├── SPEC.md
├── ARCH.md
└── ROADMAP.md
```

### Data flow
```
keyboard ─▶ GPUI KeyDownEvent ─▶ key encoder ─▶ PTY master (write)
                                                    │
shell ◀──────────────────────────── PTY slave ◀─────┘
  │
  └─▶ PTY master (read, reader thread) ─▶ OSC sniffer ─▶ vte Processor ─▶ Term (FairMutex)
                                              │                              │
                                   cwd / prompt marks / notify      dirty signal (channel)
                                              ▼                              ▼
                                         pane state ◀──────────── cx.notify() once per frame
                                                                             │
                                          TerminalElement::paint ◀───────────┘
                                          (copy visible cells, quads + shaped runs → Metal)
```

- Entities: `Workspace` (window root) owns `Vec<Tab>`; a `Tab` owns a `nox_layout` tree whose
  leaves are `Entity<Pane>`; a `Pane` owns its `TerminalSession` (`nox_terminal`) and its view
  state (scroll accumulator, find state, hovered link).
- Config and theme are GPUI `Global`s; `nox_config`'s watcher sends changes to the main thread,
  which updates the globals and refreshes the window.
- All user commands are GPUI actions dispatched through the focused element's key context
  (`Workspace` > `Pane` > `Terminal`), so the same action works from keys, menu bar, palette and
  context menus.

## 14. Testing

- `nox_terminal`: unit tests for the OSC sniffer (split sequences across reads), key encoder
  (modes, option-as-meta, natural text editing), resize coalescing; an integration test that
  spawns a real shell, writes `echo`, and reads it back from the grid.
- `nox_layout`: split, close, navigate in every direction, min sizes, equalize, maximize,
  serialization — property tests (`proptest`) on random split/close sequences.
- `nox_config` / `nox_theme`: defaults, partial files, invalid values, comment-preserving
  writes, every bundled theme parses and meets WCAG AA for `text` on `bg` in both variants.
- `nox_ui`: GPUI tests (`#[gpui::test]`, `TestAppContext`) for tab and pane actions, tabs
  position switching, palette filtering and theme live-preview/restore, Settings window
  controls writing the expected key to the file and reflecting external edits.
- Benches in CI (see 12).
- Manual checklist before release: vim, htop, less, tmux, ssh, `fzf`, emoji/CJK, IME (Japanese,
  Polish dead keys), live resize with 4 panes, ProMotion scrolling with momentum, each tabs
  position, every bundled theme in both variants, Reduce Motion / Increase Contrast.

## 15. Release, Signing & Updates

### 15.1 Versioning
- SemVer. The workspace `version` in `Cargo.toml` is the single source of truth; a release is
  cut by pushing a tag `vX.Y.Z` that matches it (the release workflow fails on a mismatch).
- `CFBundleShortVersionString` = `X.Y.Z`; `CFBundleVersion` = a monotonically increasing build
  number (`github.run_number` of the release workflow), which is what Sparkle compares.
- A tag with a pre-release suffix (`v1.2.0-beta.1`) produces a GitHub **pre-release** and goes
  to the beta channel only (see 15.3).

### 15.2 Code signing and notarization (GitHub Actions)
nox is distributed outside the Mac App Store, so it is signed with a **Developer ID
Application** certificate, uses the hardened runtime and is notarized by Apple; Gatekeeper opens
it without warnings, also when downloaded from a browser.

Repository secrets, available only to the `release` GitHub environment (protected: tags `v*`
on `master`, required reviewer = owner), never to PR workflows or forks:

| Secret                         | Content                                                       |
|--------------------------------|---------------------------------------------------------------|
| `MACOS_CERTIFICATE_P12`        | Developer ID Application certificate + private key, `.p12`, base64 |
| `MACOS_CERTIFICATE_PASSWORD`   | password of the `.p12`                                        |
| `MACOS_SIGNING_IDENTITY`       | `Developer ID Application: Piotr Wittchen (TEAMID)`           |
| `APPLE_TEAM_ID`                | 10-character team ID                                          |
| `APPSTORE_API_KEY_P8`          | App Store Connect API key (`.p8`, base64), role Developer, for `notarytool` |
| `APPSTORE_API_KEY_ID`, `APPSTORE_API_ISSUER_ID` | key and issuer IDs of that API key           |
| `SPARKLE_ED_PRIVATE_KEY`       | EdDSA private key for signing update archives (see 15.3)      |
| `HOMEBREW_TAP_TOKEN`           | fine-grained PAT with write access to `pwittchen/homebrew-tap` only |

An App Store Connect API key is used instead of Apple ID + app-specific password: no 2FA, can
be revoked independently, not tied to a personal login.

`release.yml` (runs on `macos-15` arm64, triggered by `push: tags: ['v*']`):

1. Check out, verify tag == workspace version, `cargo test --release`.
2. `cargo build --release --target aarch64-apple-darwin`, bundle `nox.app` (Info.plist with
   version numbers from 15.1, `SUFeedURL`, `SUPublicEDKey`), embed `Sparkle.framework` in
   `Contents/Frameworks`.
3. Create a temporary keychain (random password), import the `.p12`, allow `codesign` to use it
   without prompts (`security set-key-partition-list`); the keychain is deleted in an
   `always()` cleanup step.
4. Sign **inside-out**, never with `--deep`: Sparkle's XPC services, `Autoupdate`, `Updater.app`,
   then `Sparkle.framework`, then `nox.app` — each with
   `codesign --force --options runtime --timestamp --sign "$MACOS_SIGNING_IDENTITY"`, the app
   with `--entitlements assets/app/nox.entitlements`.
5. Entitlements are minimal: hardened runtime, **no** App Sandbox (a terminal must run arbitrary
   programs with the user's permissions), no `disable-library-validation`, no JIT. Anything
   added later must be justified in the PR.
6. Build `nox-macos-aarch64.dmg` (app + `/Applications` symlink, background, icon layout), sign
   the dmg.
7. Notarize: `xcrun notarytool submit nox-macos-aarch64.dmg --key … --key-id … --issuer …
   --wait --timeout 30m`; on rejection, fetch and print `notarytool log` and fail.
8. Staple: `xcrun stapler staple` on `nox.app` and the dmg, so first launch works offline.
9. Verify, fail the release on any error: `codesign --verify --strict --verbose=2 nox.app`,
   `spctl --assess --type execute -vv nox.app`, `spctl --assess --type open
   --context context:primary-signature -vv nox-macos-aarch64.dmg`, `stapler validate`.
10. Produce the update archive and appcast (15.3), `SHA256SUMS`.
11. Create the GitHub Release for the tag (notes from the matching `CHANGELOG.md` section) with
    assets: `nox-macos-aarch64.dmg`, `nox-macos-aarch64.zip`, `appcast.xml`, `SHA256SUMS`.
    Created as a draft and published only after every asset is uploaded, so the updater never
    sees a half-uploaded release.
12. Bump the cask in `pwittchen/tap` (version + sha256) through a commit with
    `HOMEBREW_TAP_TOKEN` (stable releases only).

The same steps live in `scripts/release/*.sh`, so a release can be built and notarized locally
from a Mac with the certificate in the login keychain, if CI is unavailable.

`ci.yml` (PRs and `master`) builds and tests and produces an **unsigned** (ad-hoc signed)
`nox.app` artifact; it has no access to signing secrets.

### 15.3 Automatic updates
Updates are delivered with **Sparkle 2**, the de-facto standard for non-App-Store Mac apps:
native AppKit UI, EdDSA-signed archives, atomic replace + relaunch,
and it does not fight with Homebrew (the cask declares `auto_updates true`).

- **Feed**: `SUFeedURL = https://github.com/pwittchen/nox/releases/latest/download/appcast.xml`.
  GitHub redirects `latest/download` to the newest non-prerelease release, so no extra hosting
  (no Pages, no server) is needed.
- **Appcast**: generated in CI by Sparkle's `generate_appcast` (or `sign_update` + a template)
  from the notarized `nox-macos-aarch64.zip` (`ditto -c -k --keepParent nox.app`, made *after*
  stapling). Each item has `sparkle:version` (= `CFBundleVersion`), `sparkle:shortVersionString`,
  `sparkle:minimumSystemVersion` (`13.0`), `sparkle:edSignature`, length, and
  `sparkle:releaseNotesLink` pointing to the GitHub Release page. The appcast carries the full
  history of the last few stable releases (CI downloads the previous `appcast.xml` and appends).
- **Trust**: two independent checks — Sparkle verifies the EdDSA signature against
  `SUPublicEDKey` in Info.plist, and that the new app is signed by the same Developer ID team.
  The EdDSA private key exists only as a GitHub secret plus an offline backup (losing it means
  users must reinstall manually; leaking it is mitigated by the Developer ID check).
- **Behaviour**:
  - `app.check_for_updates = true` → Sparkle checks once a day (`SUScheduledCheckInterval =
    86400`) and on launch if the last check was more than a day ago; never on the
    keystroke-to-pixel path and never before the first prompt is drawn (startup target in 12).
  - A found update shows Sparkle's standard native window with release notes;
    "Install and Relaunch" quits nox. Before quitting, the usual close confirmation applies
    (`app.confirm_close`, running processes), and the session is saved so tabs, splits and
    working directories come back after relaunch (see 6.6).
  - `app.auto_install_updates = true` → downloaded silently, installed on the next quit.
  - **nox → Check for Updates…** (menu bar) and the palette command "Check for Updates" trigger
    a user-initiated check, which also reports "nox is up to date".
  - No system profiling (`SUEnableSystemProfiling = false`); the only request is an HTTPS GET to
    github.com.
- **Channels**: stable by default. `app.update_channel = "beta"` (undocumented in the default
  config) also accepts pre-releases: CI publishes a separate `appcast-beta.xml` asset and nox
  sets `feedURLString` from the delegate accordingly.

### 15.4 Before the repository is public
Release assets of a private repository are not downloadable anonymously, so the updater cannot
work until the repo is public. Until then:

- The `updater` cargo feature is **off** in release builds: Sparkle is not embedded, the menu
  item and palette command are hidden, `check_for_updates` is ignored. Signing and notarization
  work from day one, so private builds are already Gatekeeper-clean.
- GitHub Releases are still created (private), the dmg is shared manually; the cask bump step is
  skipped (`if: github.event.repository.private == false`).
- Going public = flip the repo visibility, enable the `updater` feature in `release.yml`, cut a
  release. That release is the first one able to update itself; users of earlier private builds
  install it once manually (or via `brew install --cask pwittchen/tap/nox`).

### 15.5 Testing
- `ci.yml` checks that `Info.plist` contains `SUFeedURL` / `SUPublicEDKey` when the `updater`
  feature is on, and that the bundled Sparkle version matches the pinned one.
- Before the first public release, and after each Sparkle upgrade: install release N-1 from its
  dmg, publish N as a pre-release on the beta channel, verify update → relaunch → session
  restored, then verify a tampered archive (bad EdDSA signature) is rejected.

## 16. Risks & Open Questions

- **First milestone is a prototype** that de-risks GPUI before the UI is built out: one window,
  one pane, `cat` of a 100 MB file, trackpad scrolling with sub-line offset and momentum, live
  window resize. It must meet the scroll, latency and resize rows of 12; known risk areas are
  live-resize rendering and precise-scroll delivery during momentum.
- GPUI upgrades: pinned version, upgrade in a dedicated PR, UI code isolated in `nox_ui`.
- The directory chip (`[▢ ~]`) left of the tabs in the reference screenshot: specified here as
  the focused pane's cwd with Copy Path / Reveal in Finder. Confirm or replace (e.g. workspace
  switcher, new-window button).
- Multiple windows and "move tab to new window" — v2?
