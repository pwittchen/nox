# nox — Roadmap

## Not done yet (v1 spec items)

- **Remove Sparkle leftovers** (SPEC §15.4): the built-in updater replaced Sparkle; the
  `updater` cargo feature, the `SUFeedURL` / `SUPublicEDKey` keys in `scripts/bundle.sh` and the
  `ci.yml` check for them can go.
- **Homebrew cask** (§15.2 step 12): `release.yml` starts the tap's `update-formulae.yml` with
  `formula=nox` once the repo is public, but `pwittchen/homebrew-tap` has no `Casks/nox.rb` (nor
  a `nox` choice in that workflow) and the `HOMEBREW_TAP_TOKEN` secret isn't set on nox yet.
- **Kitty keyboard protocol** (§7.1): `terminal.kitty_keyboard` is passed to the emulator, but
  the key encoder still sends legacy sequences.
- **Session-restore integration test** with `SIGKILL` and a fake agent binary (§14).
- **Benchmark regression gate**: CI runs the criterion benches but doesn't yet compare against
  a stored baseline (fail on > 10 %).
- **`latency-trace` feature** (§12): the flag exists; the key-event → frame-presented logging
  doesn't.
- **Codex / Gemini CLI hook installers** in "Install Agent Hooks…" (Claude Code is done).
- **Iosevka** is not bundled (size, see ARCH.md).
- **Mode 2027** (grapheme clustering query, §7.1): emoji clusters are merged, but `DECRQM ?2027`
  isn't answered, so programs can't detect it and adjust their width tables.

## Later

- Multiple windows (one workspace per window), "move tab to new window".
- A `nox` terminfo entry.
- tmux control mode, SSH profiles, broadcast input.
- Inline images (Sixel, Kitty graphics, iTerm `imgcat`).
- Intel / Linux builds.
