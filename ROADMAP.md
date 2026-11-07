# nox — Roadmap

## Not done yet (v1 spec items)

- **Sparkle auto-update** (SPEC §15.3): the `updater` cargo feature, Info.plist keys, appcast
  script and release workflow are in place, but Sparkle.framework is not embedded or driven
  yet. Off by design until the repository is public (§15.4).
- **Kitty keyboard protocol** (§7.1): `terminal.kitty_keyboard` is passed to the emulator, but
  the key encoder still sends legacy sequences.
- **GPUI UI tests** (§14): `#[gpui::test]` coverage for tab / pane actions, tabs position
  switching, palette filtering, theme preview and Settings controls. Pure parts of `nox_ui`
  (scrolling, links, attention, fuzzy matching) have unit tests.
- **Session-restore integration test** with `SIGKILL` and a fake agent binary (§14).
- **Benchmark regression gate**: CI runs the criterion benches but doesn't yet compare against
  a stored baseline (fail on > 10 %).
- **`latency-trace` feature** (§12): the flag exists; the key-event → frame-presented logging
  doesn't.
- **Codex / Gemini CLI hook installers** in "Install Agent Hooks…" (Claude Code is done).
- **Workspace reordering by drag** and collapsible workspace groups in the sidebar
  (`tabs.sidebar_workspaces = "all"`).
- **Draggable overlay scrollbar**; the tab strip doesn't auto-scroll to keep the active tab in
  view when it overflows.
- **Settings window**: `[icons]` / `[shell.env]` entries can be removed but not added inline
  (edit `config.toml`); fade/slide animations of palette and sheets.
- **Iosevka** is not bundled (size, see ARCH.md).

## Later

- Multiple windows (one workspace per window), "move tab to new window".
- A `nox` terminfo entry.
- tmux control mode, SSH profiles, broadcast input.
- Inline images (Sixel, Kitty graphics, iTerm `imgcat`).
- Intel / Linux builds.
