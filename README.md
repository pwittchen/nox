<p align="center">
  <img src="logo.png" alt="nox" width="128">
</p>

<h1 align="center">nox</h1>

<p align="center"><a href="https://noxterminal.app"><strong>noxterminal.app</strong></a></p>

<p align="center">
  <a href="https://github.com/pwittchen/nox/actions/workflows/ci.yml"><img src="https://github.com/pwittchen/nox/actions/workflows/ci.yml/badge.svg?branch=master" alt="CI"></a>
</p>

<p align="center">a minimalistic, fast, native terminal for macOS, built in Rust with GPUI.</p>

## Install

Requires macOS 13 or newer on Apple Silicon.

```sh
brew install --cask pwittchen/tap/nox
```

This installs `nox.app` and the `nox` agent CLI. Alternatively, download
`nox-macos-aarch64.dmg` from the [latest release](https://github.com/pwittchen/nox/releases/latest)
and drag nox into Applications.

## Update

```sh
brew upgrade --cask nox
```

If you installed from the dmg, download the latest release and replace the app in Applications.

## Features

- Splits and tabs like iTerm; tabs on top, bottom or in a sidebar
- Workspaces: named groups of tabs per project, switched instantly, restored on relaunch
- A host for AI coding agents: knows which panes run an agent and what state it's in, tells you
  when one needs you, and lets agents message each other, wait for each other and claim files
  through the `nox` CLI or MCP server
- `~/.nox/config.toml`, applied live; a Settings window over the same file
- 17 bundled themes, JetBrains Mono by default, plus other bundled fonts

Terminal emulation (VT parsing, grid, scrollback) is based on
[alacritty_terminal](https://github.com/alacritty/alacritty), the core of the Alacritty
terminal; nox adds its own GPUI rendering, UI and agent features on top.

The full behaviour is specified in [SPEC.md](SPEC.md); [ARCH.md](ARCH.md) explains the code and
[ROADMAP.md](ROADMAP.md) lists what's left.

## Build

Requires macOS 13+ on Apple Silicon and Rust 1.99 (`rust-toolchain.toml`).

```sh
scripts/run.sh                      # build and run from the checkout (as "nox")
cargo run -p nox --bin nox-app      # run the bare binary
cargo test --workspace              # tests
scripts/test.sh                     # audit, fmt check, lint, tests + summary table
cargo bench -p nox_terminal         # parser / encoder benches
scripts/bundle.sh                   # dist/nox.app (ad-hoc signed)
scripts/release/make_dmg.sh         # dist/nox-macos-aarch64.dmg
```

`scripts/run.sh` builds the app and the `nox` CLI and runs them from a small dev bundle,
`target/debug/nox.app`, so the menu bar and the Dock show "nox" (a bare `cargo run` shows up in
the Dock as `nox-app`). Arguments are passed through to the app.

`scripts/test.sh` runs the checks CI runs (`cargo audit`, `cargo fmt --check`, `cargo lint`,
`cargo test --workspace`) and prints a table with each step's result, time and details; it
exits non-zero if any step failed. Name steps to run only those (`scripts/test.sh fmt lint`),
`-v` streams their output, and `--coverage` runs the tests under `cargo llvm-cov` and adds a
per-crate coverage table. Logs go to `target/test-logs/`.

`NOX_HOME=/some/dir` runs nox with a separate config, session and socket directory.

## Agents

Every pane has the `nox` CLI on its `PATH` and its own identity:

```sh
nox name backend                        # name this pane
nox status working -m "refactoring"     # report state (hooks do this for Claude Code)
nox msg reviewer "feat/x is ready"      # message another pane; it reads `nox inbox`
nox wait backend                        # block until backend finishes its turn
nox claim src/parser/ -m "rewriting"    # advisory file claims (exit 3 on conflict)
nox spawn --split right -- cargo test   # open a helper pane
claude mcp add nox -- nox mcp           # the same operations as MCP tools
```

"Install Agent Hooks…" in the command palette (⌘K) adds Claude Code hooks that report state
and session ids, so nox can show when an agent needs you and resume the right conversation
after a restart.
