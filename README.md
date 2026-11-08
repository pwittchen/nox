<p align="center">
  <img src="logo.png" alt="nox" width="128">
</p>

<h1 align="center">nox</h1>

<p align="center">
  <a href="https://github.com/pwittchen/nox/actions/workflows/ci.yml"><img src="https://github.com/pwittchen/nox/actions/workflows/ci.yml/badge.svg?branch=master" alt="CI"></a>
</p>

<p align="center">a minimalistic, fast, native terminal for macOS, built in Rust with GPUI.</p>

- Splits and tabs like iTerm; tabs on top, bottom or in a sidebar
- Workspaces: named groups of tabs per project, switched instantly, restored on relaunch
- A host for AI coding agents: knows which panes run an agent and what state it's in, tells you
  when one needs you, and lets agents message each other, wait for each other and claim files
  through the `nox` CLI or MCP server
- `~/.nox/config.toml`, applied live; a Settings window over the same file
- 17 bundled themes, JetBrains Mono by default, plus other bundled fonts

The full behaviour is specified in [SPEC.md](SPEC.md); [ARCH.md](ARCH.md) explains the code and
[ROADMAP.md](ROADMAP.md) lists what's left.

## Build

Requires macOS 13+ on Apple Silicon and Rust 1.99 (`rust-toolchain.toml`).

```sh
scripts/run.sh                      # build and run from the checkout (as "nox")
cargo run -p nox --bin nox-app      # run the bare binary
cargo test --workspace              # tests
cargo bench -p nox_terminal         # parser / encoder benches
scripts/bundle.sh                   # dist/nox.app (ad-hoc signed)
scripts/release/make_dmg.sh         # dist/nox-macos-aarch64.dmg
```

`scripts/run.sh` builds the app and the `nox` CLI and runs them from a small dev bundle,
`target/debug/nox.app`, so the menu bar and the Dock show "nox" (a bare `cargo run` shows up in
the Dock as `nox-app`). Arguments are passed through to the app.

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
