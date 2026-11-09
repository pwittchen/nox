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
- **Workspaces**: named groups of tabs, usually tied to a project directory, each with its own
  tabs and agents, switched instantly within the one window (see 4.7, 6.7).
- A great host for **AI coding agents** (Claude Code, Codex, Gemini CLI, …): nox knows which
  panes run an agent and what state it is in, tells the user when one needs them, and lets
  agents in different tabs, panes and workspaces see each other, exchange messages, wait for
  each other and avoid editing the same files (see 4.8, 7.12, 7.13).
- Close nox and open it again (or reboot) and the work is back: every workspace with its tabs
  and splits, and every agent resumed in its conversation (see 6.6).

### Non-Goals (v1)
- Plugins / extensions; a general scripting API beyond the `nox` CLI / control socket for agents
  (see 7.13).
- tmux control mode integration, SSH/profile manager, broadcast input.
- Inline images (Sixel, Kitty graphics, iTerm `imgcat` protocol).
- Font ligatures (the grid is drawn cell by cell).
- Built-in AI: nox runs no model, has no chat, holds no API keys and sends nothing to any AI
  service. It hosts agents; it isn't one.
- Multiple terminal windows (one window; work is organized in workspaces, tabs and panes; the Settings
  window, see 5.9, is the only other window). See ROADMAP.
- Session restoration of running processes: shells start fresh, scrollback is not kept. What
  comes back is all workspaces with their layouts and working directories, and agent
  conversations through each agent's own resume feature (see 6.6).
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
| Auto-update         | Built-in updater (`nox_ui::updater`): GitHub Releases API + the release zip, verified by SHA-256 and Developer ID signature (see 15.3) |
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
  themselves from GitHub Releases (see 15.3).

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
- The window shows one **workspace** (see 4.7) at a time; every tab belongs to a workspace.

### 4.2 Tab
- A tab holds a **layout**: a tree of panes (see 4.3). It always has at least one pane.
- The tab's title is the title of its **focused pane** (see 4.4), prefixed by its index:
  `1 ~`, `2 ~/Development/nox`, `3 vim`. The user can rename a tab (see 6.5); a renamed tab
  keeps its name instead of following the focused pane.
- Status dot: a small dot on the tab tells what its panes are doing (as in the screenshot on
  `#469 - cleanup after migration`):
  - **yellow** (`ansi.yellow`) — **running**: a command is executing in one of the tab's panes.
    Shown on every tab, the active one included, for as long as the command runs.
  - **green** (`ansi.green`) — **done**: a command finished successfully while the tab was not
    being looked at. It stays until the tab is visited (opened or focused, with the window
    active); a command that finishes in the tab being looked at leaves no dot. It may also play
    the attention sound and post a notification (see 7.11).
  - **red** (`ansi.red`) — **failed**: like done, but the command exited with a non-zero status
    (OSC 133 `D` with an exit code; without shell integration the status is unknown and the dot
    is green). Cleared the same way.
  - **blue** (`ansi.blue`) — **waiting**: something needs the user. Either a coding agent in one
    of the tab's panes is blocked on the user (a permission prompt or a question, see 7.12),
    shown on every tab, the active one included, until the agent continues; or a bell rang in
    a background tab (see 7.10), until the tab is visited.
  - Priority when several apply: waiting > running > failed > done. No dot when the tab is
    idle and seen.
- "Running" means, with shell integration (see 4.5), the span between OSC 133 `C` (command
  start) and `D` (command end); without it, the PTY's foreground process (`tcgetpgrp`) is not
  the shell. "Done" is the transition back to the prompt (OSC 133 `D`, or the foreground process
  returning to the shell). Long-lived interactive programs (`vim`, `ssh`) count as running while
  they are in the foreground. Coding agents are the exception: their dot follows the agent state
  (see 7.12) — working = running, turn finished = done, blocked on the user = waiting.

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
  ROADMAP item; v1 uses `xterm-256color`.) The app's executable in the bundle is
  `nox.app/Contents/MacOS/nox`, so tools that name the terminal after the parent process
  (fastfetch, neofetch) show `nox <version>`.
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
     by their arguments, so `node …/claude` resolves to Claude and `java -jar app.jar` to Java;
     an executable named after a version takes the name of its install directory
     (`~/.local/share/claude/versions/2.1.319` → `claude`),
  3. the title set via OSC 0/2, matched by known prefixes (e.g. `✳ Claude Code`),
  4. otherwise the default terminal icon `>_`.
  While an agent is detected in the pane (see 4.8) and running, its icon is kept even when later
  process / title checks don't identify it (e.g. Claude Code's working spinner title).
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
- The status dot (running / done / failed / waiting, see 4.2) overlays the icon's top-right
  corner instead of taking extra space (as in the screenshot on `#469 - cleanup after
  migration`).
- `tabs.icons = false` hides process icons and shows the plain `>_` everywhere.

### 4.7 Workspace
- A workspace is a named group of tabs with its own agents, session state and, optionally, a
  root directory. It is the unit of "one project" or "one task": e.g. `nox`
  (`~/Development/prv/nox`), `kayet`, `scratch`.
- The window shows exactly one workspace at a time, the **active** one. The others keep running
  in the background: their panes keep reading output, their agents keep working, and their
  status (dots, attention sound, notifications) stays live. Switching is instant, like switching
  tabs (see 6.5).
- A workspace has: a name, an optional root directory, an optional color (a small swatch on the
  workspace chip and in the sidebar, one of the theme's ANSI color names, so it follows the
  theme), its own tab list with layouts, active tab and focused pane, its own message bus and
  claims (see 7.13) and its own extra environment variables.
- **Root directory** — usually set: a workspace is typically created from a directory and named
  after it. But it is not required: a workspace without a root starts its first tab in
  `shell.working_directory`. Tabs are never confined to the root (`cd` goes anywhere), and
  several workspaces may share a root (two tasks in one repository; separate worktrees are
  usually better, see 7.12).
- The first tab of a workspace starts in its root. Later tabs and panes follow
  `workspaces.new_tab_cwd`: `"inherit"` (default, the focused pane's cwd, as `shell.inherit_cwd`)
  or `"root"`.
- There is always at least one workspace. On first launch nox creates `default`, without a root.
- **Workspace status**: the aggregate of its tabs' dots with the same priority as 4.2, shown next
  to its name in the switcher and the sidebar, so a background workspace with an agent that
  needs input is visible from anywhere.
- **Project file** (optional): `<root>/.nox/workspace.toml` gives the workspace defaults when it
  is created from that directory — name, color, `[env]`, and startup tabs (titles, split
  layouts and a command per pane, e.g. a tab `agent` running `claude` and a tab `dev` with
  `npm run dev` split next to `git log --oneline`). Commands from a project file run only after
  the user trusts it once: a sheet lists the commands; trust is stored by path + content hash in
  `~/.nox/trusted.toml`, and any change to the file asks again (as `direnv` does). Untrusted,
  the tabs open with plain shells.
- Every pane gets `NOX_WORKSPACE` (name), `NOX_WORKSPACE_ID`, `NOX_WORKSPACE_ROOT` (if set),
  `NOX_TAB_ID` and `NOX_PANE_ID` in its environment, plus the cooperation variables of 7.13.

### 4.8 Agent
- An **agent pane** is a pane whose foreground program is a coding agent: one of the AI coding
  agents of 4.6, or an executable configured in `[agents.<name>]` (see 11). Detection reuses the
  process lookup of 4.6, at no extra cost. The agent **exits** when the shell is back in the
  foreground (or the pane's process exits). An agent nox launched itself (a new agent, a resumed
  session) is an agent pane from the launch on; until its process has shown up in the foreground
  once, the shell in the foreground doesn't count as an exit for the first 10 s (the shell is
  still reading the command).
- An agent belongs to the workspace of its pane. It has a **state** (working, needs input, idle,
  exited — see 7.12), a **name** (the pane's name, set by renaming the pane or by `nox name`,
  otherwise the agent kind, e.g. `claude`, suffixed when it repeats in a workspace: `claude-2`)
  and a position (workspace / tab / pane) by which the user and other agents find it.
- Agents cooperate through nox (see 7.13), never through hidden processes: every agent nox knows
  about runs in a visible pane the user can look at, type into or close.

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
  (`with_animation`); no bounce. Disabled under Reduce Motion. The command palette and sheets
  fade in over 140 ms while sliding down 8 px (a sheet's backdrop fades with it); closing is
  instant. Hover states are instant (no
  transitions), as in native controls.
- No hardcoded colors in components: every color comes from the active `Theme` (a GPUI
  `Global`, see 9).

### 5.2 Window layout — tabs on top (default)

```
┌──────────────────────────────────────────────────────────────────────────┐
│ ● ● ●  [▢ nox]  [>_ 1 ~      ]  [>_ 2 ~            ]           ⚙  ⌘   +  │  title bar (38px)
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
- **Sidebar toggle** `[◧]` (see 5.4).
- **Workspace chip** `[▢ nox]`: the active workspace (see 4.7) — color swatch, name, and the
  workspace status dot when another workspace needs attention (so the user sees it without
  opening anything). Hovering shows the root and the focused pane's cwd. Click → menu: the list
  of workspaces with their status dots (click to switch), New Workspace…, Open Folder as
  Workspace…, New Agent…, New Agent in Worktree…, Rename Workspace…, Set Color ▸, Copy Path, Reveal in
  Finder, Close Workspace. Dropping a folder on it opens that folder as a workspace. While the
  sidebar shows (`left` mode, or toggled on in `top` / `bottom` mode) the chip and the tab strip
  are hidden and move to the sidebar (see 5.4).
- **Tab strip**: one pill per tab, with a process icon (see 4.6; the default `>_` is tinted with
  `ansi.green` at low alpha when the tab is active, muted otherwise), the index and the title. Active tab:
  `bg-elevated` fill with a hairline border; inactive: no fill, `text-muted` label. Pills share
  the available width, between 120 px and 260 px each; when they don't fit, the strip scrolls
  horizontally (trackpad / wheel), keeping the active tab in view (it scrolls to a tab that
  becomes active), and two small arrows `‹ ›` appear right after it: each click scrolls by about
  the strip's width, snapping to a tab edge; an arrow dims when there's nothing more on its side.
  They go away once all tabs fit. Hovering a pill shows its `×`
  in place of the icon; middle-click closes. Drag to reorder (GPUI `on_drag` / `on_drop`): a
  dropped pill takes the place of the pill it's dropped on, and a 2 px `accent` edge on that
  pill marks the side it lands on; dropped on the empty space after the strip it goes last.
  Double-click to rename inline (`Enter` confirms, `Esc` cancels, an empty name restores the
  automatic title).
- Empty space in the title bar drags the window: the bar's background, the gaps between and
  around the pills and the stretch after the strip, which never shrinks below 56 px (when the
  pills don't fit, the strip scrolls instead), so the window can always be grabbed, however
  many tabs are open. A double-click there zooms or minimizes per the system setting. Pills,
  the chip and the buttons never move the window: `AppKit`'s own title-bar dragging is turned
  off for the main window (`-mouseDownCanMoveWindow` → `NO`), and empty space starts the move
  with `performWindowDragWithEvent:`.
- Right side: **⚙** opens Settings (`⌘,`, see 5.9); **⌘** opens the command palette (see 5.6);
  **+** opens a new tab.

Pane card:
- Each tab's content is a card inset 6 px from the window edges, `bg` fill, 1px `border`
  hairline, 10px corners — as in the screenshot.
- **Pane header** (per pane, 28 px, `ui.pane_header = true`): process icon (see 4.6) + pane title on the
  left; on the right, shown on hover of the header (always shown for the focused pane at 60%
  opacity): **split vertically** (◫), **split horizontally** (⊟), **maximize / restore** (⤢),
  **close** (×). With a single pane in the tab the header still shows (it carries the title);
  `ui.pane_header = false` hides all headers, leaving only the hairline dividers.
  After the title, muted: the git branch of the pane's cwd (`⎇ feat/workspaces`,
  `ui.show_git_branch = true`, see 7.12) and, in agent panes, the agent state, unread messages
  (`✉ 2`) and held claims (a small lock) from 7.13.
- Terminal grid padding: `terminal.padding` (default 8 px horizontal, 6 px vertical). Leftover
  space after fitting whole cells goes to the right and bottom edge, filled with `bg`.

### 5.3 Tabs on the bottom

Same as 5.2, with the sidebar toggle, the tab strip, the workspace chip, `⚙`, `⌘` and `+` moved to
a 34 px bar under the pane card. The title bar keeps only the traffic lights and stays draggable;
its height shrinks to 28 px. Toggling the sidebar on hides the chip and the tab strip; the bar
keeps the toggle and the buttons.

### 5.4 Tabs in the left sidebar

```
┌──────────────────────────────────────────────────────────────────────────┐
│ ● ● ●  [◧]                                                     ⚙  ⌘   +  │
│ ┌────────────────┐┌──────────────────────────────────────────────────────┐│
│ │ ▢ nox        ⌄ ││ >_ pw@mbp:~                            ◫  ⊟  ⤢  ×    ││
│ │ ▢ blog      •  ││                                                      ││
│ │ ────────────── ││ ❯ █                                                  ││
│ │ >_ 1  ~        ││                                                      ││
│ │ >_ 2  ~/nox    ││                                                      ││
│ │    vim         ││                                                      ││
│ │ >_ 3  htop  •  ││                                                      ││
│ └────────────────┘└──────────────────────────────────────────────────────┘│
└──────────────────────────────────────────────────────────────────────────┘
```

- `tabs.position = "left"`: tabs are rows in a sidebar on the left (`bg-sidebar`), one row per
  tab: process icon (see 4.6), index, title; a second muted line shows the working directory when it differs from
  the title. The active row has a subtle background pill. Status dot (see 4.2) on the right.
- The top of the sidebar lists all workspaces, one row each: color swatch, workspace name and its
  status dot (see 4.2). The active workspace's row has the background pill and `⌄`; clicking it
  opens the same menu as the workspace chip (see 5.2), clicking another row switches to that
  workspace. Dropping a tab on a row moves the tab there. Below the list, separated by a 1 px
  line, come the tabs of the active workspace.
- `tabs.sidebar_workspaces = "all"` lists every workspace as a collapsible group (header row +
  its tabs) instead: one column showing all projects and all their agents. Clicking a tab of
  another workspace switches to it. Default `"all"`. Hovering a header turns its color swatch
  into a disclosure chevron (`⌄`, `›` when folded); clicking it folds or unfolds the group's
  tabs without switching workspace. A folded header shows its tab count, muted. The folded
  state is per workspace and persisted in the session.
- Pinned to the bottom of the sidebar, under a 1 px line, an **Agents** section (`Agents · n`)
  lists every running agent of every workspace, one row each: process icon, agent name (see
  4.8), a muted second line with its workspace and state (needs input / idle · unseen /
  working / idle), and its status dot. Sorted like the agents panel (see 7.12): needs input
  first, then idle-unseen, working, idle; workspace order within each. The focused agent's row
  has the background pill; clicking a row jumps to that pane (switching workspace and tab). The
  section takes at most 45 % of the sidebar height and scrolls; it is not shown when no agent
  runs, or when `tabs.sidebar_agents = false` (default `true`).
- Drag to reorder, double-click to rename, context menu as for pills (see 5.5). Tab rows drop
  like pills (a 2 px `accent` edge above or below marks where it lands); a row dropped on a
  row of another workspace (`"all"`) moves there, into that row's place.
- Drag a workspace header to reorder workspaces: dropped on another workspace's block (its
  header, plus its tabs in `"all"` mode) it takes that workspace's place, marked by an `accent`
  edge above or below the block. The active workspace stays active.
- Sidebar width: drag its 1px divider, min 160 px, max 360 px, default 220 px; persisted.
- `⌘\` or the sidebar toggle `[◧]` in the title bar hides and shows it; the state is persisted.
  While hidden, `⌘1…9` and the palette still switch tabs.
- **When `tabs.position` is `top` or `bottom`** the sidebar is off by default and the tab bar
  shows. `⌘\` or the toggle `[◧]` (in the title bar, or in the bottom bar for `bottom`) swaps
  them: the sidebar shows and the workspace chip and tab strip hide; toggling again brings the
  tab bar back and hides the sidebar. The state is persisted (`tabs.sidebar_replaces_tabs`),
  separately from `tabs.sidebar_visible` of `left` mode.
- Switching `tabs.position` in the config applies live, without restarting sessions.

### 5.5 Context menus (native)
- Shown as real `NSMenu`s (`popUpMenuPositioningItem:atLocation:inView:` via `objc2`); the
  chosen item is dispatched back as a GPUI action. (GPUI's own drawn menus are not used, so they
  look and behave like every other Mac app.)
- Tab: New Tab, Rename Tab…, Set Icon…, Duplicate Tab (same cwd), Move Left / Right, Move to
  Workspace ▸ (the other workspaces, New Workspace…), Close Tab, Close Other Tabs.
- Pane (right-click in the terminal): Copy, Copy Last Output, Send Selection to… (see 7.12),
  Paste, Select All, Clear Buffer, Split Vertically, Split Horizontally, Maximize Pane, Rename
  Pane…, Close Pane. With a URL or path under the pointer: Open, Copy Link, Reveal in Finder.
  In an agent pane also: Show in Agents Panel, Release Claims (when it holds any, see 7.13).

### 5.6 Command palette
- `⌘K` or the `⌘` title-bar icon opens a floating panel just below the title bar, like kayet's:
  a search field, a filtered list of commands (fuzzy match) with their shortcuts right-aligned,
  `↑` / `↓` navigate, `↵` runs, `Esc` closes, the footer hints the keys.
- Lists every command in 10 (keyboard shortcuts), plus `Switch Theme…`, `Switch Appearance…`,
  `Switch Tab…` (all tabs of all workspaces, filter by title/cwd/workspace), `Switch
  Workspace…`, `Rename Workspace…`, `Set Workspace Color…`, `New Agent in Worktree…`, `Install
  Agent Hooks…`, `Copy MCP Setup Command`, `Install Command Line Tool…` and `Tabs Position: Top /
  Bottom / Left`.
- Theme and appearance pickers live-preview the highlighted choice; `Esc` restores, `↵` applies
  and saves to config.
- Every command is a GPUI action (`actions!` / `Action` derive), so the palette, the menu bar,
  context menus and key bindings all dispatch the same actions.

### 5.7 Dialogs and notices
- In-app sheets as in kayet (bold title, muted message, right-aligned buttons, the default button
  monochrome-filled and focused, `Esc` cancels, `Tab` cycles). Used for: closing a tab / pane /
  the app while a process other than the shell is running (`Cancel` / `Close`); closing a
  workspace (with the number of running processes and working agents); trusting a project file
  (see 4.7); resuming agents on launch (see 7.12). A sheet opens over the window that asked for
  it: one started from Settings (e.g. Install Agent Hooks…) shows in the Settings window, not
  behind it in the main window.
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
  (`[shell]`, `[shell.env]`), Workspaces (`[workspaces]`), Agents (`[agents]`, the configured
  agents and their commands), Icons (`[icons]`), Keys (`[keys]`), plus **About** pinned to the
  bottom of the sidebar (the About window's content, see 5.11) — and a scrollable form on the
  right. Each setting shows a label, a one-line description and the matching config key in muted
  monospace (`tabs.position`), so the UI teaches the file: on the left with the control on the
  right, except for the Keys shortcut list, which has its label above it and takes the full
  width (action name on the left, recorder and buttons on the right).
- Controls by type: switches for booleans, segmented controls / popups for enums, steppers or
  sliders with a numeric field for numbers, text inputs (see 5.8) for strings, font pickers
  (see 5.10), a theme picker with live preview, key-value tables for
  `[icons]` and `[shell.env]` (each entry with **Remove**; a name / value row below adds one —
  `Enter` in the name moves to the value, `Enter` there or **Add** writes it, an existing name
  is overwritten; `[shell.env]` names must be `[A-Za-z_][A-Za-z0-9_]*`, an icon needs a
  value), and a shortcut recorder for `[keys]` (click, press the
  combination; conflicts are flagged). Above the shortcut list, **Reset All to Defaults…** asks
  for confirmation (a sheet with the number of changed shortcuts) and then removes every action
  override from `[keys]` in one write; `{ send = … }` bindings are kept. Disabled while nothing
  is overridden.
- Changes apply immediately (no Save button) and are written to `config.toml` at once.
- Settings that differ from the default show a small "reset" control; resetting removes the key
  from the file, so it falls back to the default again.
  Comment lines above a removed key are kept (moved to the next key or table).
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

- **Terminal font** — default **JetBrains Mono**, 14 pt, 100% line height. Bundled alternatives
  (all monospace): MesloLGS NF (Meslo LG, Apache License 2.0, with Nerd Fonts glyphs, MIT) and,
  under SIL OFL 1.1, Geist Mono, IBM Plex Mono, Fira Code, Cascadia Code, Iosevka, Hack,
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

### 5.11 About window
- nox → **About nox** opens a small single-instance window (a second request brings it to the
  front): the app icon, the name, the tagline ("stay in command."), `Version x.y.z` (the
  crate version), then rows for the author (Piotr Wittchen) and links to the author's website
  (`wittchen.io`) and the app's source repository (`github.com/pwittchen/nox`); links open in
  the default browser. Not resizable; `Esc` or `⌘W` closes it. The same content is the About
  page of Settings (see 5.9).
- Under the version, an update row (also on the Settings About page and under Settings →
  General → Check for updates) shows the updater's state with its action: **Check for
  Updates** → "Checking for updates…" → "nox is up to date." (**Check Again**) or "nox x.y.z is
  available." (**Release Notes**, **Install and Relaunch**) → "Downloading nox x.y.z…" →
  "nox x.y.z is ready." (**Relaunch to Update**); errors show in red with **Try Again** (see 15.3).

## 6. Panes, Tabs and Workspaces Behavior

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
- The window's **close button** hides nox (like `⌘H`) instead of quitting; clicking the Dock
  icon shows the window again.
- **Quitting** (`⌘Q`, nox → Quit nox, the Dock) always asks first
  ("Quit nox?" — `Cancel` / `Quit`; the message says whether the session will be restored), even
  when nothing runs; `app.confirm_quit = false` (Settings → General → Confirm quit) turns that
  off. When processes or agents are running, the `confirm_close` sheet listing them is shown
  instead. Quitting to install an update ("Install and Relaunch") skips the plain quit question.

### 6.5 Tabs
- Tabs, their order and their indices are per workspace: `⌘1` is the first tab of the active
  workspace.
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
Closing nox and opening it again brings back the work as it was left: every workspace, its
tabs and splits, and the agent sessions that were running in them.

**What is saved** (`~/.nox/session.toml`)
- All workspaces: order, name, root, color, extra env, active tab, and which workspace is
  active.
- Per workspace: the tab order, each tab's custom name, layout tree (directions, ratios),
  focused pane and maximize state; per pane its working directory and custom name.
- Per agent pane: the agent kind (`[agents.<name>]`), its name (4.8), the working directory it
  was started in, the command line it was started with, and its **agent session id** when known
  (below).
- The session is written on quit, and also on every structural change (debounced 1 s) and every
  30 s while something changes, atomically (temp file + rename). So it survives a crash, a
  force quit, a logout or a reboot, not only a clean `⌘Q`. Quitting via an update
  (15.3) saves it too.

**What is restored** (on launch, `app.restore_session = true`, default)
- Every workspace with its tabs, splits, names, colors and the active workspace / tab / pane,
  with fresh shells in the saved directories. Workspaces in the background are restored too,
  not only the active one; their shells start lazily, when the workspace is first shown, unless
  they hold agents to resume (`workspaces.lazy_restore = true`), which keeps launch within the
  startup target (12).
- Agent sessions are resumed in the panes they ran in (see 7.12, "Resuming agents"), so each
  conversation continues where it stopped, in the right directory, in the right workspace.
- Messages survive (7.13); claims don't, since their holders restart.
- Not restored: scrollback, and processes other than agents (a `vim`, a dev server). With
  `[[tabs]] command` in a project file (4.7), startup commands of a workspace can be re-run on
  restore (`workspaces.rerun_startup_commands = false` by default).
- If a saved directory no longer exists, the pane starts in the workspace root (or `~`) with a
  notice. If a saved root is gone, the workspace is restored without a root, marked in the
  switcher, and can be pointed at a new folder.
- Starting with `⇧` held, or with `nox --no-restore`, skips restoring once (the session file is
  kept and restored the next time).

### 6.7 Workspaces
- **Create**:
  - **New Workspace** (`⌘⇧N`): a sheet with a name and a root directory (path input with
    completion, or **Choose…** for a folder picker), both optional; an empty name takes the
    folder's name, or `workspace N` without a root.
  - **Open Folder as Workspace…** (`⌘O`): a native folder picker (`NSOpenPanel`); the workspace
    is rooted there and named after the folder (`nox`, then `nox 2` if the name is taken). If a
    workspace with that root already exists, nox switches to it instead; holding `⌥` forces a
    new one.
  - From a terminal: `nox open [path]` / `nox .` (see 7.13) does the same as Open Folder.
    Dropping a folder on the Dock icon or on the workspace chip too.
  - From a project file, when the folder has `.nox/workspace.toml` (see 4.7).
- **Switch**: the workspace chip menu or sidebar header, **Switch Workspace…** (`⌘P`: a palette
  listing workspaces with color, root, status dot and the number of agents per state, fuzzy
  filtered by name and root), `⌃⌘1…⌃⌘9` (workspace N), `⌃⌘[` / `⌃⌘]` (previous / next,
  wrapping), clicking a notification or an entry in the agents panel (7.11, 7.12). The previous
  workspace is one `⌃⌘\` away (toggles between the last two).
- Each workspace remembers its active tab, focused pane, maximize state and scroll positions;
  switching back restores them exactly. Switching never touches processes.
- **Edit**: rename, set color, set or change the root (affects new tabs only; running shells
  stay where they are) — from the chip menu, the palette or the sidebar header's context menu.
- **Move a tab** to another workspace (tab context menu, Move to Workspace ▸, or drag a tab onto
  a workspace in the sidebar / switcher): its panes keep running; agents in it join the target
  workspace's bus (their claims are released).
- **Reorder** workspaces by dragging their headers in the sidebar (both `sidebar_workspaces`
  modes, see 5.4).
- **Close** a workspace (chip menu, palette, `⌃⌘W`): closes all its tabs, asking first when
  processes or agents are running (see 5.7). Closing the last tab of a workspace closes the
  workspace (`workspaces.close_when_empty = true`; otherwise a new tab opens in its root) and
  switches to the most recently used one. Closing the last workspace follows
  `app.quit_when_last_tab_closed`.

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
- Emoji grapheme clusters (`terminal.grapheme_width = "unicode"`, default): a code point that
  continues the emoji cluster in the previous cell is stored in that cell instead of a new one,
  so the whole sequence is one 2-column cell shaped as one glyph — ZWJ sequences (👨‍👩‍👧, 🏳️‍🌈),
  regional-indicator pairs (🇵🇱), skin-tone modifiers (👍🏽) and tag flags. VS16 after a narrow
  base (❤️, ☺️, 1️⃣) widens the cell to 2 columns (wrapping first if it is in the last column).
  Other scripts keep per-code-point widths. Programs that size text with `wcwidth` per code point
  (e.g. zsh's line editor) can disagree about these sequences; `"legacy"` turns the merging off
  and uses alacritty's per-code-point widths. Mode 2027 (grapheme cluster query) isn't
  reported.
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
- `terminal.cursor_style = "block" | "bar" | "underline"`, `terminal.cursor_blink = false`.
  Programs can change style and blinking via DECSCUSR. Blinking stops after 5 s of inactivity
  (no timers when idle) and while the window is unfocused, where the cursor is drawn hollow.

### 7.6 Selection and clipboard
- `alacritty_terminal` selections: click-drag selects (`Simple`); double-click a word
  (`Semantic`, `terminal.word_separators`); triple-click a line (`Lines`); `⌥`-drag a
  rectangular block (`Block`); `⇧`-click extends. With mouse reporting on, `⇧` forces selection.
- Dragging past the top or bottom edge auto-scrolls, faster the further the pointer is (also when
  the pointer leaves the pane or the window while the button is held; releasing it anywhere ends
  the selection).
- `⌘C` copies (when there is a selection; otherwise nothing), `⌘V` pastes (bracketed if the
  program asked). `terminal.copy_on_select = true`: finishing a selection (drag, double- or
  triple-click) copies it and shows a "Copied to clipboard" toast; a plain click copies
  nothing. Trailing whitespace is trimmed on copy.
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
  The case toggle alone decides case (off by default): no smart case, so with it on a lowercase
  query doesn't match uppercase text.
  Search uses `alacritty_terminal`'s `RegexSearch` over the whole scrollback, run off the main
  thread for large buffers. Matches are highlighted with the theme's selection color, the current
  one in accent. `Esc` closes and returns focus to the terminal.

### 7.9 Keyboard and IME
- Key events (`KeyDownEvent` with `Keystroke`) are translated to bytes by nox's key encoder,
  honoring alacritty's mode flags (application cursor keys, application keypad, Kitty
  protocol when on).
- `terminal.option_as_meta = "auto" | "left" | "right" | "both" | "none"` (default `"auto"`): the
  chosen Option key sends `ESC`-prefixed sequences; the other one types special characters.
  `"auto"` is `"left"` on the US / ABC keyboard layouts and `"none"` on every other layout, whose
  Option layer holds letters (Polish Pro `⌥A` → `ą`, …). Which Option key is down comes from the
  device-dependent modifier bits of the key event being dispatched.
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
- Triggers, in a pane the user isn't looking at (a background tab, a pane in another
  workspace, or any pane while the nox window is unfocused):
  - **done**: a command finishes (the "done" transition, see 4.2), if it ran for at least
    `terminal.attention_min_duration` seconds (default 10), so quick commands stay quiet;
  - **attention**: a program asks for it with an OSC 9 / OSC 777 notification (see 7.1), e.g. a
    coding agent like `claude` finishing its turn or waiting for a permission prompt;
  - **agent**: an agent's state changes to *needs input* (always), or to *idle* after a turn that
    lasted at least `terminal.attention_min_duration` (see 7.12), however nox learned about it.
- Nothing fires for the pane the user is looking at (focused window, active workspace, active
  tab), so typing at a prompt never chimes or notifies.

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
  - agent: title `<agent name> needs input` / `<agent name> finished`, body: the agent's last
    message (from `nox status -m`, the notification text or the title) and `workspace · tab`.
- Clicking the notification brings nox to the front, switches to the workspace and the tab and
  focuses the pane.
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

### 7.12 Coding agents
Features for panes that run coding agents (see 4.8). All of them work without any setup;
explicit status reporting (below) makes them precise.

**Agent state**
- States: **working** (the agent is busy on a turn), **needs input** (blocked on the user: a
  permission prompt, a question), **idle** (its turn is over, waiting for the next prompt),
  **exited**. Each carries the time it was entered and an optional short message.
- Sources, the first available wins:
  1. **Explicit report**: `nox status working | needs-input | idle [-m "message"]` from the CLI
     or the MCP server (see 7.13), typically called from the agent's hooks — for Claude Code:
     `UserPromptSubmit` → working, `Notification` → needs input, `Stop` → idle; each report also
     carries the agent's session id, used to resume it later (see "Resuming agents"). **Install Agent
     Hooks…** (palette, Settings → Agents) offers ready-made hooks for Claude Code, Codex and
     Gemini CLI: it shows the exact change to the agent's settings file (user or project level)
     as a diff and writes it only after the user confirms; nothing is ever installed silently.
  2. **OSC 9 / 777 notification** from the agent → needs input, with the text as the message.
  3. **Title**: known title patterns per agent (e.g. Claude Code's spinner prefix while working,
     `✳` when idle), kept in the same built-in table as the icons (see 4.6).
  4. **Activity heuristic** (fallback only): output in the last 1.5 s → working; quiet after
     that → idle.
- Exception to "first wins": hooks miss some turns (Claude Code resumes after a background task
  without `UserPromptSubmit`, and skips `Stop` when interrupted with Esc), so a title that
  clearly contradicts a report corrects it, from 2 s after the report on: an animating working
  title over a reported idle → working, a title changing to idle over a reported working → idle.
  A reported needs input is never overridden by a title.
- The state drives the tab and workspace dots (4.2, 4.7), the attention sound and notifications
  (7.11) and the agents panel. A working dot gently pulses (off under Reduce Motion).

**Agents panel** (`⌘⇧A`)
- A floating panel (like the palette, 5.6) listing every agent of every workspace, grouped by
  workspace: icon, name, tab, state with its duration (`working · 4m 12s`, `needs input · 30s`),
  last message, cwd and git branch, unread messages and claims (7.13).
- Type to filter, `↑` / `↓` to move, `↵` jumps to the agent (switches workspace and tab, focuses
  the pane). Agents that need input are listed first, then idle-unseen, then working.
- A second tab of the panel, **Activity**, shows the cooperation log of the active workspace:
  messages, sends, spawns and claims (see 7.13), newest first.

**Next agent** (`⌘J`)
- Jumps to the agent that has waited longest for the user — needs input first, then idle and
  not yet seen — across all workspaces. Pressing again cycles through the rest; with none
  waiting, a short notice says so. This is the "what needs me now" key when several agents run.

**Git context**
- The git branch of each pane's cwd is shown in the pane header and on the second line of
  sidebar rows (`ui.show_git_branch = true`). It is read from `.git/HEAD` (worktree-aware: a
  `.git` file pointing to `gitdir:` is followed), refreshed together with the cwd (4.4); no `git`
  process is spawned.

**Worktrees for parallel agents**
- **New Agent in Worktree…** (palette, workspace chip menu): asks for a branch name and an agent
  (from `[agents.*]`), runs `git worktree add <worktree_dir>/<repo>-<branch> -b <branch>` from
  the workspace root (`workspaces.worktree_dir`, default the root's parent directory), then opens
  the agent there — in a new tab of the current workspace or in a new workspace rooted at the
  worktree (`workspaces.worktree_opens = "tab" | "workspace"`). If git fails, its output is
  shown in a sheet.
- When that tab or workspace closes and the worktree is clean, nox offers to remove it
  (`git worktree remove`); it never removes a dirty worktree or deletes a branch.

**Launching and resuming agents**
- **New Agent Tab** (`⌥⌘T`): a picker of the configured agents (`[agents.<name>]`, see 11);
  the chosen one starts in a new tab in the workspace root, through the user's shell so their
  `PATH` and aliases apply.

**Resuming agents**
- After nox is closed and opened again (6.6), every pane that ran an agent relaunches it so its
  conversation continues. This starts a fresh process; the conversation itself is restored by
  the agent's own resume feature (nox stores no conversation content).
- **Session id**: to resume the *right* conversation — several agents of the same kind often
  share a directory — nox remembers each agent's session id. It learns it from:
  1. an explicit report: `nox status … --session <id>` (the hooks installed by Install Agent
     Hooks pass it, e.g. Claude Code hooks receive `session_id`),
  2. the agent's command line, when it was started with a session flag (`--resume <id>`,
     `codex resume <id>`),
  3. otherwise none.
- Resume command, per agent in `[agents.<name>]` (11): `resume` with a `{session}` placeholder
  (e.g. `claude --resume {session}`) when the id is known; `resume_last` (e.g.
  `claude --continue`) otherwise. If two agent panes of the same kind in the same directory
  have no session id, `resume_last` would open the same conversation twice, so only the first
  uses it and the others are listed in the resume sheet as "session unknown" (start fresh or
  skip).
- `agents.restore`: `"resume"` (default) relaunches all agents right away, `"ask"` shows one
  sheet listing them per workspace with checkboxes (`Resume` / `Start Shells`), `"off"` restores
  plain shells. A per-pane override is in the pane context menu (Resume Agent on Restore).
- The agent is started through the user's shell in the saved directory, with the pane's
  original env (`NOX_*` ids are new, but its name stays, so other agents and the message bus
  still find it). If the resume command fails (exits non-zero within 5 s, e.g. the session was
  deleted), the pane keeps the output and shows `[resume failed — ↵ start a new session, ⌘R
  retry]`.
- Agents with no `resume` / `resume_last` configured come back as a shell in the saved
  directory with the original command line typed at the prompt but not run.
- **Resume All Agents** (palette) re-runs resumption manually, e.g. after starting with
  `--no-restore` or choosing "Start Shells".

**Feeding context to agents**
- **Copy Last Output** (`⌥⌘C`): copies the output of the last finished command in the focused
  pane (between OSC 133 `C` and `D`, without the prompt); needs shell integration (4.5).
- **Send Selection to…** (`⌘⇧S`): a picker of the panes of the workspace, agents first; sends
  the selection (or the last command output when nothing is selected) to the chosen pane as a
  bracketed paste, **without** pressing `↵`, and focuses it, so the user can add a prompt
  around it before submitting.
- Dropping files from Finder onto a pane pastes their shell-escaped paths, separated by spaces.
- `⇧↵` sends `ESC ↵` (`\x1b\r`), which Claude Code, Codex and other agents read as a newline
  inside the prompt rather than submit; overridable in `[keys]` (`"shift-enter"`).

**Keep awake**
- While any agent is working, nox holds an `IOPMAssertion` (`PreventUserIdleSystemSleep`; the
  display may still sleep), so a long run isn't cut short by system sleep; it is released as
  soon as no agent is working. `agents.keep_awake = true`.

### 7.13 Agent cooperation
Agents running in different tabs, panes and workspaces can see each other, exchange messages,
wait for each other, start helpers and avoid editing the same files — without the user copying
text between them. Everything goes through nox and stays visible to the user.

**Control socket**
- nox listens on a Unix domain socket, `~/.nox/run/nox.sock` (directory `0700`, socket `0600`,
  current user only). Protocol: newline-delimited JSON requests / responses plus subscription
  events, versioned (`"v": 1`). Served on a background thread (`nox_ipc`); requests that touch
  UI state are dispatched to the main thread. It never listens on a network port.
- `agents.cooperation = false` turns the socket off and leaves the variables below unset.

**Identity and scope**
- Every pane's environment carries `NOX_SOCKET`, `NOX_PANE_ID`, `NOX_TAB_ID`,
  `NOX_WORKSPACE_ID` and a random per-pane `NOX_TOKEN`. Requests authenticate with the token,
  so nox knows which pane is calling; child processes inherit it, so an agent's tools act as
  the agent's pane. Without a valid token only `open` and `ls` (workspaces only) are allowed.
- **A workspace is a team**: by default a pane sees and talks only to panes of its own
  workspace (`agents.scope = "workspace"`); `"all"` lets agents reach other workspaces too.
- Panes are addressed by id or by name (4.8); names are unique within a workspace.

**`nox` command-line tool**
- Shipped in the app bundle (`nox.app/Contents/Resources/bin/nox`) and put first on `PATH` in
  every pane; **Install Command Line Tool…** symlinks it into `/usr/local/bin` for use outside
  nox. Every command accepts `--json`; exit codes are documented (`0` ok, `1` error, `2` usage,
  `3` conflict, `4` timeout).

| Command                                              | What it does                                                    |
|------------------------------------------------------|-----------------------------------------------------------------|
| `nox ls [--all]`                                     | workspaces, tabs and panes: id, name, program, agent state, cwd, branch |
| `nox name <name>`                                    | name the calling pane (e.g. `backend`, `reviewer`)              |
| `nox status <state> [-m msg] [--session id]`         | report the calling agent's state and session id (see 7.12)      |
| `nox read <pane> [--lines N \| --scrollback \| --last-output]` | text of another pane: the visible screen by default, plain text without escape sequences |
| `nox send <pane> <text> [--submit]`                  | type text into another pane as a bracketed paste; `--submit` also presses `↵` |
| `nox msg <to> <text>`                                | post a message to a pane (by name / id) or to `all` in the workspace |
| `nox inbox [--wait] [--from <pane>] [--peek]`        | read (and mark read) the calling pane's messages; `--wait` blocks until one arrives |
| `nox wait <pane> [--state idle\|needs-input\|exited] [--timeout s]` | block until the pane reaches that state (default `idle`) |
| `nox claim <path…> [-m reason] [--wait]`             | take advisory claims on files or directories                    |
| `nox release [<path…>]` / `nox claims`               | release own claims (all by default) / list claims in the workspace |
| `nox spawn [--tab \| --split right\|below] [--name n] [--cwd dir] -- <cmd>` | open a new pane running a command in the caller's workspace; prints its id |
| `nox notify <title> [body]`                          | attention sound + notification, as OSC 777 (see 7.11)           |
| `nox open [path]`, `nox .`                           | open or switch to the workspace for a folder (see 6.7)          |
| `nox mcp`                                            | run an MCP server over stdio (below)                            |

**Messages**
- Each workspace has a message bus. `nox msg reviewer "feat/x is ready, please review"` queues
  a message from the calling pane to `reviewer`. The recipient reads it with `nox inbox` (or the
  MCP tool) — the default delivery, which never interferes with the agent's TUI. The pane
  header shows unread messages (`✉ 2`).
- `agents.deliver = "type"` (opt-in) additionally types a message into an **idle** agent's
  prompt as `[from backend] …` and submits it, so an agent that doesn't poll its inbox still
  gets it; messages to a working agent wait until it is idle.
- Messages are kept per workspace (last 1000, `~/.nox/workspaces/<id>/messages.jsonl`), so a
  resumed agent can still read what was sent to it. Shown in the Activity log (7.12).

**Claims**
- A claim is an advisory lock on a path (file or directory, relative to the workspace root or
  absolute) held by a pane, with an optional reason. Claims on overlapping paths conflict:
  `nox claim src/parser/` fails with exit code `3` and the holder's name and reason when
  another pane holds `src/parser/lexer.rs` — the agent can pick other work, or `--wait` for it.
- Claims are released explicitly, when the pane closes or its program exits, when the tab moves
  to another workspace, or after `agents.claim_ttl` seconds (default 1800) without renewal
  (claiming again renews). They are not enforced on the filesystem: they are a convention that
  agent instructions (`CLAUDE.md`, `AGENTS.md`) can rely on ("claim files before editing them").
- Held claims show as a lock in the pane header (hover lists them) and in the agents panel; the
  user can release any claim from there.

**Waiting, handoff, helpers**
- `nox wait backend` lets a reviewer agent sleep until the implementer finishes its turn, then
  `nox read backend --last-output` or its inbox. An orchestrating agent can `nox spawn` helpers
  in splits or tabs of its workspace; they are ordinary visible panes the user can watch, type
  into and close.

**MCP server**
- `nox mcp` exposes the same operations as MCP tools over stdio — `list_panes`, `read_pane`,
  `send_to_pane`, `send_message`, `read_inbox`, `wait_for_pane`, `claim_paths`,
  `release_paths`, `list_claims`, `spawn_pane`, `set_status` — with the same identity and scope
  (the server inherits the agent pane's environment). Setup is one command, e.g.
  `claude mcp add nox -- nox mcp`; **Copy MCP Setup Command** (palette, Settings → Agents) puts
  the right one on the clipboard for each configured agent.

**Safety**
- `send` and `deliver = "type"` only target **agent panes**; typing into a plain shell would run
  commands, so it is refused unless `agents.allow_send_to_shell = true`.
- Typed-in text is never hidden: the target pane header shows `⇠ from backend` for 3 s, and every
  send, spawn, message and claim is logged in the Activity log with its source and time.
- If the user typed in the target pane within the last 2 s, a send waits (up to 10 s, then
  fails) so it never interleaves with the user's typing.
- Loop guard: a pane may send at most 20 messages / sends per 10 s; excess requests fail and the
  user gets a notice naming the pane.
- `read` is limited to the caller's scope; `spawn` runs as the user, like any new tab, in the
  caller's workspace only.

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
  (small writes; large pastes go to a writer thread so the UI never blocks). Input stays in
  order: while the writer thread still has a paste queued, keystrokes queue behind it.
- **Closing a pane** sends SIGHUP to the shell's process group and never blocks: if the shell
  survives (it traps HUP, or was still starting), SIGTERM and then SIGKILL follow, 0.5 s apart,
  on a background thread.
- **Open files**: every pane holds a PTY master, pipes and sockets, and apps started from
  Finder get a soft `ulimit -n` of 256, so at launch nox raises the soft `RLIMIT_NOFILE` to the
  hard limit, capped at macOS's `OPEN_MAX` (10240). Shells inherit the raised limit.
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
| Toggle sidebar / tab bar        | `⌘\`                |
| New workspace                   | `⌘⇧N`               |
| Open folder as workspace        | `⌘O`                |
| Switch workspace…               | `⌘P`                |
| Go to workspace 1–9             | `⌃⌘1`…`⌃⌘9`         |
| Previous / next workspace       | `⌃⌘[` / `⌃⌘]`       |
| Last used workspace             | `⌃⌘\`               |
| Close workspace                 | `⌃⌘W`               |
| New agent tab                   | `⌥⌘T`               |
| Agents panel                    | `⌘⇧A`               |
| Next agent needing attention    | `⌘J`                |
| Copy last command output        | `⌥⌘C`               |
| Send selection to pane…         | `⌘⇧S`               |
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
show_git_branch = true       # branch of the pane's cwd in pane headers and sidebar rows (see 7.12)

[tabs]
position = "top"             # "top" | "bottom" | "left"; top/bottom: tab bar (sidebar via ⌘\)
sidebar_visible = true       # left mode only
sidebar_replaces_tabs = false # top/bottom mode: sidebar shown instead of the tab bar (⌘\)
sidebar_width = 220          # left mode only
show_index = true            # "1 ~" vs "~"
title = "auto"               # "auto" (see 4.4) | "cwd" | "process"
icons = true                 # process icons in tabs and pane headers (see 4.6)
sidebar_workspaces = "all"   # "active" | "all" (every workspace as a group, see 5.4)
sidebar_agents = true        # running agents of all workspaces at the bottom of the sidebar

[workspaces]
new_tab_cwd = "inherit"      # "inherit" (focused pane's cwd) | "root" (workspace root, see 4.7)
close_when_empty = true      # closing a workspace's last tab closes the workspace
worktree_dir = ""            # empty: next to the workspace root; else a path for new worktrees (see 7.12)
worktree_opens = "tab"       # "tab" | "workspace"
lazy_restore = true          # start background workspaces' shells when first shown (agents resume right away)
rerun_startup_commands = false # re-run project file tab commands on session restore (see 6.6)

[agents]
keep_awake = true            # prevent system sleep while an agent is working
restore = "resume"           # resume agent sessions on relaunch: "resume" | "ask" | "off" (see 7.12)
cooperation = true           # control socket, `nox` CLI and MCP server (see 7.13)
scope = "workspace"          # which panes an agent can see and reach: "workspace" | "all"
deliver = "inbox"            # messages: "inbox" | "type" (also typed into idle agents)
allow_send_to_shell = false  # allow `nox send` into panes that aren't agents
claim_ttl = 1800             # s; unrenewed claims expire

# Agents for New Agent Tab / worktrees / resume; the key is also the executable matched for
# detection (4.8), extra executables in `match`.
[agents.claude]
command = "claude"
resume = "claude --resume {session}" # when the session id is known
resume_last = "claude --continue"    # otherwise: latest session in that directory

[agents.codex]
command = "codex"
resume = "codex resume {session}"
resume_last = "codex resume --last"

# [agents.my-agent]
# command = "my-agent --fast"
# match = ["my-agent", "my-agent-dev"]
# icon = "~/.nox/icons/my-agent.svg"

[icons]
# executable = built-in icon name, or a path to an SVG in ~/.nox/icons/
# "my-agent" = "claude"
# "jbang" = "java"
# "work-cli" = "~/.nox/icons/work.svg"

[font]
family = "JetBrains Mono"    # terminal font: a bundled one (see 5.10), "SF Mono", "Menlo" or font names, e.g. "Iosevka, Sarasa Mono J"
size = 14
line_height = 1.0            # × the font's natural line height (1.0 = 100% in iTerm)
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
cursor_blink = false
padding = [8, 6]             # [horizontal, vertical] px
copy_on_select = true
confirm_multiline_paste = true
word_separators = " ()[]{}',\"`│"
option_as_meta = "auto"      # "auto" | "left" | "right" | "both" | "none"
bold_is_bright = false
bell = "visual"              # "visual" | "sound" | "none"
attention_sound = true       # soft chime when a job finishes or waits for input unseen (see 7.11)
attention_sound_volume = 0.4 # 0.0–1.0
notifications = true         # macOS notification when a job finishes or waits for input unseen (see 7.11)
attention_min_duration = 10  # s; shorter commands finish without a chime or notification
close_on_exit = "clean"      # "always" | "clean" | "never"
osc52 = "write"              # "write" | "read-write" | "off"
grapheme_width = "unicode"   # "unicode": emoji sequences are one 2-column cell | "legacy": per code point
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
confirm_quit = true           # ask before quitting nox, even when nothing is running
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
- Per-workspace settings (name, root, color, env, startup tabs) are not in `config.toml`: they
  live in the session (6.6) and, optionally, in the project file `<root>/.nox/workspace.toml`
  (4.7), which uses the same TOML conventions:

```toml
name = "nox"
color = "cyan"               # one of the theme's ANSI color names

[env]
RUST_LOG = "debug"

[[tabs]]
title = "agent"
command = "claude"

[[tabs]]
title = "dev"
layout = "vertical"          # two panes side by side
panes = [{ command = "cargo watch -x check" }, { command = "git log --oneline -20" }]
```
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
  This includes `[keys]`: a shortcut that doesn't parse (e.g. `"hyper-k"`) falls back to the
  action's default binding; a `{ send = … }` entry whose key doesn't parse has no default and
  is skipped.

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
| Switching workspace (10 tabs each)                  | next frame (< 16 ms) |
| Control socket request (`nox ls`, `nox msg`)        | < 5 ms p99        |
| CPU, 5 idle agents across 3 workspaces              | ~0% (no polling while quiet) |
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
│   ├── nox_layout/             # no GPUI: workspaces → tabs → split trees (pure data, heavily unit tested)
│   ├── nox_agents/             # no GPUI: agent detection + state machine, message bus, claims,
│   │                           #   git HEAD reader, worktree helper, project file + trust store
│   ├── nox_ipc/                # no GPUI: control socket server, protocol types, token auth, scope
│   ├── nox_cli/                # binary `nox`: CLI commands + MCP stdio server (depends on nox_ipc only)
│   ├── nox_config/             # no GPUI: config model, load/validate, toml_edit writes, watcher
│   ├── nox_theme/              # no GPUI: theme parsing, bundled themes, color resolution
│   ├── nox_mac/                # objc2: NSMenu context menus, accessibility flags,
│   │                           #   Dock bounce, notifications, NSWindow background color,
│   │                           #   NSOpenPanel, IOPMAssertion (keep awake)
│   └── nox_ui/                 # GPUI views and elements:
│       ├── root.rs             #   window root: title bar, workspace chip, tab bar/sidebar, active workspace
│       ├── workspace.rs        #   one workspace: its tabs, active tab, switching state
│       ├── workspace_switcher.rs # chip menu, Switch Workspace… palette, New Workspace sheet
│       ├── agents_panel.rs     #   agents list + Activity log (see 7.12)
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
│   ├── ci.yml                  # PRs + master: audit, then fmt, lint, test + coverage, bench, bundle
│   └── release.yml             # tag v*: build, sign, notarize, staple, appcast, release, cask
├── assets/
│   ├── icons/                  # SVG line icons (1.5px stroke)
│   ├── fonts/                  # bundled terminal + interface fonts and their OFL licenses (see 5.10)
│   ├── sounds/                 # attention.caf (see 7.11)
│   └── app/                    # .icns, Info.plist, nox.entitlements
├── scripts/
│   ├── ci/                     # audit_summary.sh, coverage_summary.sh (CI run summary)
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

- Entities: `Root` (window root) owns `Vec<Entity<Workspace>>` and the active index; a
  `Workspace` owns `Vec<Tab>`, its root/color/env and its `nox_agents` bus and claims; a `Tab`
  owns a `nox_layout` tree whose leaves are `Entity<Pane>`; a `Pane` owns its
  `TerminalSession` (`nox_terminal`), its agent state and its view state (scroll accumulator,
  find state, hovered link).
- The `nox_ipc` server thread receives socket requests, authenticates them by pane token and
  answers read-only ones from a snapshot; mutations (send, spawn, msg, claim, status) go to the
  main thread over a channel and are applied to the owning `Workspace` / `Pane`. `nox wait` and
  `nox inbox --wait` are held as subscriptions, never as polling.
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
  serialization — property tests (`proptest`) on random split/close sequences; workspaces:
  create, close when empty, move tab between workspaces, session round-trip.
- Session restore: an integration test that opens several workspaces with agent panes (a fake
  agent binary reporting session ids), kills nox with `SIGKILL`, relaunches it and checks that
  the workspaces, layouts and directories come back and each fake agent is started with its own
  `resume` command line; plus the cases of a missing directory, a missing root, two agents
  without session ids in one directory, and a failing resume.
- `nox_agents`: the state machine against recorded sessions of Claude Code, Codex and Gemini
  CLI (title changes, OSC 9/777, output bursts, hook reports), claim overlap / expiry / release
  on exit, message ordering and persistence, `.git/HEAD` parsing incl. worktrees, project file
  trust (hash change re-asks).
- `nox_ipc` / `nox_cli`: protocol round-trips, token auth and scope (a pane cannot reach another
  workspace with `scope = "workspace"`, no token → only `open`), send refused into shells, loop
  guard; an integration test that starts two panes, runs `nox msg` / `nox inbox --wait` /
  `nox wait` between them, and an MCP conformance check of `nox mcp`.
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
- SemVer. A release is cut by pushing a tag `vX.Y.Z`; the tag is the source of truth for the
  version. The release workflow writes it into the workspace `version` in `Cargo.toml` and the
  workspace entries of `Cargo.lock` (`scripts/release/set_version.sh`) before building, and for
  stable tags first commits that bump to `master` (together with the website's download version,
  `scripts/release/set_website_version.sh`), before the build, so `Cargo.toml` and the website
  there always hold the latest released version. A tag that isn't valid SemVer fails the release.
- `CHANGELOG.md`: changes for the next release go into the `## [Unreleased]` section at the top
  (optional). On release, `scripts/release/changelog.sh` takes the notes from that section if it
  has content, else from an existing `## X.Y.Z` section (brackets optional), else from the commit
  subjects since the previous `v*` tag (`Release …` commits excluded), and replaces both sections
  with `## X.Y.Z — YYYY-MM-DD` above older ones (no empty `## [Unreleased]` is added back).
  Re-releasing the current version works the same way (the version bump is then a no-op). Pre-releases get notes but leave `CHANGELOG.md` and `master` untouched.
- `CFBundleShortVersionString` = `X.Y.Z`; `CFBundleVersion` = a monotonically increasing build
  number (`github.run_number` of the release workflow), which is what Sparkle compares.
- A tag with a pre-release suffix (`v1.2.0-beta.1`) produces a GitHub **pre-release** and goes
  to the beta channel only (see 15.3).

### 15.2 Code signing and notarization (GitHub Actions)
nox is distributed outside the Mac App Store, so it is signed with a **Developer ID
Application** certificate, uses the hardened runtime and is notarized by Apple; Gatekeeper opens
it without warnings, also when downloaded from a browser.

Repository secrets, read only by `release.yml` (tag pushes), never by PR workflows or forks.
Names and flow are shared with the author's other macOS apps (kayet, zora):

| Secret                         | Content                                                       |
|--------------------------------|---------------------------------------------------------------|
| `MACOS_CERTIFICATE`            | Developer ID Application certificate + private key, `.p12`, base64 |
| `MACOS_CERTIFICATE_PASSWORD`   | password of the `.p12`                                        |
| `KEYCHAIN_PASSWORD`            | password of the temporary CI keychain                         |
| `APPLE_SIGNING_IDENTITY`       | `Developer ID Application: Piotr Wittchen (TEAMID)`           |
| `APPLE_ID`                     | Apple ID of the developer account, for `notarytool`           |
| `APPLE_PASSWORD`               | app-specific password of that Apple ID                        |
| `APPLE_TEAM_ID`                | 10-character team ID                                          |
| `SPARKLE_ED_PRIVATE_KEY`       | EdDSA private key for signing update archives (see 15.3); not set until public |
| `HOMEBREW_TAP_TOKEN`           | PAT that can start workflows in `pwittchen/homebrew-tap`; not set until public |

`release.yml` (triggered by `push: tags: ['v*']`; a `commit-version` job on Linux, then a
`build` job on `macos-15` arm64, then a `release` job on Linux):

0. Stable tags on `master` only: commit the version bump and the new `CHANGELOG.md` section as
   `Release X.Y.Z` (by `github-actions[bot]`, rebased onto the current `master`) and push it.
   This runs before anything is built, so a rejected push fails the release early; pre-release
   tags and tags not on `master` skip the commit and go straight to the build.
1. Check out, set the workspace version from the tag (15.1), `cargo test --release`.
2. `cargo build --release --target aarch64-apple-darwin`, bundle `nox.app` (Info.plist with
   version numbers from 15.1, `SUFeedURL`, `SUPublicEDKey`), embed `Sparkle.framework` in
   `Contents/Frameworks`.
3. Create a temporary keychain (random password), import the `.p12`, allow `codesign` to use it
   without prompts (`security set-key-partition-list`); the keychain is deleted in an
   `always()` cleanup step.
4. Sign **inside-out**, never with `--deep`: Sparkle's XPC services, `Autoupdate`, `Updater.app`,
   then `Sparkle.framework`, then `nox.app` — each with
   `codesign --force --options runtime --timestamp --sign "$APPLE_SIGNING_IDENTITY"`, the app
   with `--entitlements assets/app/nox.entitlements`.
5. Entitlements are minimal: hardened runtime, **no** App Sandbox (a terminal must run arbitrary
   programs with the user's permissions), no `disable-library-validation`, no JIT. Anything
   added later must be justified in the PR.
6. Notarize the app: zip it (`ditto -c -k --keepParent`), `xcrun notarytool submit … --apple-id
   … --password … --team-id … --wait --timeout 30m`; on rejection, fetch and print
   `notarytool log` and fail.
7. Staple the ticket to `nox.app`, so first launch works offline once it is copied out of the dmg.
8. Verify, fail the release on any error: `stapler validate`, `codesign --verify --strict
   --verbose=2 nox.app`, `spctl --assess --type execute -vv nox.app`.
9. Build `nox-macos-aarch64.dmg` from the stapled app (app + `/Applications` symlink), sign it,
   notarize it (submitted as is), staple it and check `spctl --assess --type open --context
   context:primary-signature`, so a downloaded dmg opens without a Gatekeeper warning.
10. Produce `nox-macos-aarch64.zip` (after stapling), the appcast when the updater is on (15.3),
    and `SHA256SUMS`. The dmg, zip and checksums are kept as the workflow artifact
    `nox-macos-aarch64`.
11. Create the GitHub Release for the tag (notes as in 15.1: the `Unreleased` or matching
    `CHANGELOG.md` section, else the commits since the previous tag) with assets:
    `nox-macos-aarch64.dmg`, `nox-macos-aarch64.zip`, `SHA256SUMS` (+ `appcast.xml` with the
    updater). Created as a
    draft and published only after every asset is uploaded, so the updater never sees a
    half-uploaded release.
12. Start the `update-formulae.yml` workflow of `pwittchen/homebrew-tap` (cask `nox`) with
    `HOMEBREW_TAP_TOKEN` (stable releases of a public repository only).

The same steps live in `scripts/release/*.sh`, so a release can be built and notarized locally
from a Mac with the certificate in the login keychain, if CI is unavailable:
`SIGN_IDENTITY=… scripts/bundle.sh`, `APPLE_ID=… APPLE_PASSWORD=… APPLE_TEAM_ID=…
scripts/release/notarize.sh dist/nox.app`, `SIGN_IDENTITY=… scripts/release/make_dmg.sh`, then
`notarize.sh dist/nox-macos-aarch64.dmg`.

`ci.yml` (PRs and `master`) first runs `cargo audit --deny warnings` (accepted advisories in
`.cargo/audit.toml`); only when that passes does it check formatting, run `cargo lint`
(clippy::pedantic, warnings denied), build and run the tests under `cargo llvm-cov`, and build an
**unsigned** (ad-hoc signed) `nox.app` to check that bundling works; it uploads no artifacts. A
failing audit fails the run. The run's summary page shows the audit result (vulnerabilities,
warnings, ignored advisories) and test coverage per crate (`scripts/ci/*_summary.sh`). A newer
run for the same branch or PR cancels the one still in progress. It has no access to signing
secrets. It doesn't run when a change touches only Markdown files and / or `website/`, or when
the commit message (push) or PR title contains `#no-deploy`.

`deploy-website.yml` builds the website (`website/`, Bun) and rsyncs `website/dist/` to
`/var/www/noxterminal/` on the VPS (secrets `VPS_HOST`, `VPS_USER`, `VPS_SSH_KEY`, `VPS_PORT`).
It runs on pushes to `master` that touch `website/` (skipped when the commit message contains
`#no-deploy`), by hand, and from the release workflow after a stable release is published, so the
site's download version matches the release.

### 15.3 Automatic updates
nox updates itself from GitHub Releases with a small built-in updater (`nox_ui::updater`), not
Sparkle: the release zip that CI already publishes is the update, so no appcast, EdDSA key or
embedded framework is needed. Network and file work runs off the UI thread through the macOS
tools `curl`, `ditto`, `codesign`, `spctl` and `shasum` (no HTTP / TLS stack linked in).

- **Lookup**: `GET https://api.github.com/repos/pwittchen/nox/releases/latest` (unauthenticated,
  `User-Agent: nox/<version>`); the release's `tag_name` (without `v`) is compared with the
  running version by semver precedence (`1.0.0-beta.2` < `1.0.0-beta.10` < `1.0.0`). A release
  without the `nox-macos-aarch64.zip` asset is ignored. `app.update_channel = "beta"`
  (undocumented in the default config) reads `/releases` instead and takes the newest non-draft
  one, pre-releases included.
- **Trust**: the zip's SHA-256 must match the asset's `digest` from the API (or its line in the
  release's `SHA256SUMS`); the unpacked `nox.app` must pass `codesign --verify --deep --strict`,
  have the same bundle identifier, and be signed by the same Developer ID team as the running
  app. A running copy without a team (ad-hoc signed) accepts only an update that passes
  Gatekeeper (`spctl --assess`). Anything else fails with a message and nothing is installed.
- **Install**: the update is downloaded and verified into a temporary folder while nox keeps
  running. On quit, a detached helper waits for the nox process to exit, moves the old bundle
  aside, copies the new one into place with `ditto` (restoring the old one if that fails), and
  with "Install and Relaunch" opens it again. The session is saved on quit, so workspaces, tabs
  and panes come back (with `app.restore_session`, see 6.6).
- A copy that can't replace itself (not inside a `.app`, e.g. `cargo run`, or its folder isn't
  writable) opens the release page in the browser instead, with a notice.
- **Behaviour**:
  - `app.check_for_updates = true` → a check 20 s after launch (never before the first prompt
    is drawn, never on the keystroke-to-pixel path) and then every 24 h while nox runs. Silent
    unless an update is found; then a sheet in the main window ("nox x.y.z is available" —
    `Later` / `Release Notes` / `Install and Relaunch`), at most once per version per run.
  - `app.auto_install_updates = true` → a found update is downloaded and verified silently and
    installed the next time nox quits (a notice says so).
  - **nox → Check for Updates…** (menu bar), the palette command "Check for Updates", and the
    update row in the About window / Settings (see 5.11) trigger a check at any time, also with
    `check_for_updates = false`. From the menu or palette the result is the same sheet, or a
    notice "nox is up to date (x.y.z)." / the error; in About and Settings it shows inline.
  - "Install and Relaunch" quits through the usual quit path: running processes or agents
    still ask first (`app.confirm_close`); the plain quit question (`app.confirm_quit`) is
    skipped. Cancelling leaves the update staged ("Relaunch to Update"); it installs on the
    next quit.
  - Homebrew installs (`brew install --cask`) update the same way; the cask declares
    `auto_updates true`.

### 15.4 Leftovers of the Sparkle plan
The `updater` cargo feature, the `SUFeedURL` / `SUPublicEDKey` Info.plist keys added by
`scripts/bundle.sh` with that feature, and the matching `ci.yml` check date from the earlier
Sparkle plan. The built-in updater doesn't use them (it is always compiled in); they're listed
in ROADMAP for removal.

### 15.5 Testing
- Unit tests cover version precedence, release JSON parsing (latest and beta list), checksum
  lookup, `codesign` output parsing and bundle-path detection.
- Before a release that changes the updater: install release N-1 from its dmg, publish N, check
  for updates → Install and Relaunch → N opens with the session restored; then verify a zip
  with a wrong checksum and an app signed by another team are both rejected.

## 16. Risks & Open Questions

- **First milestone is a prototype** that de-risks GPUI before the UI is built out: one window,
  one pane, `cat` of a 100 MB file, trackpad scrolling with sub-line offset and momentum, live
  window resize. It must meet the scroll, latency and resize rows of 12; known risk areas are
  live-resize rendering and precise-scroll delivery during momentum.
- GPUI upgrades: pinned version, upgrade in a dedicated PR, UI code isolated in `nox_ui`.
- The chip left of the tabs in the reference screenshot is specified as the workspace chip
  (5.2), with Copy Path / Reveal in Finder kept in its menu.
- Multiple windows and "move tab to new window" — v2? With workspaces, a natural model is one
  workspace per window, optionally.
- Agent state heuristics (titles, output activity) differ between agents and change between
  their versions; explicit reporting through hooks (7.12) is the reliable path, and the title
  table needs maintenance like the icon table.
- `nox send` / `deliver = "type"` depend on each agent's TUI handling bracketed paste and
  `ESC ↵` the same way; verify per agent, prefer the inbox.
- MCP is evolving; `nox mcp` pins a protocol version and is tested against the current Claude
  Code and Codex clients.
