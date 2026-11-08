# Changelog

## [Unreleased]

- nox updates itself: nox → Check for Updates…, the palette, the About window and Settings check
  GitHub Releases, then download, verify (SHA-256 + Developer ID signature) and install the
  update with a relaunch. A daily automatic check is on by default (`app.check_for_updates`).
- Quitting asks "Quit nox?" first, also when nothing is running; turn it off with
  `app.confirm_quit` (Settings → General). The window's close button hides nox; the
  Dock icon brings it back.

## [0.1.1] — 2026-11-08

- Show the Claude icon while Claude Code is working (757cb49)
- Keep Claude's agent state correct when hooks miss a turn (5436c3e)
- Add install and update instructions to the README (5618d8a)
- Take the release version from the git tag and update the changelog (c6f87d7)

## [0.1.0] — unreleased

First implementation of SPEC.md: GPUI terminal with splits, tabs (top / bottom / sidebar),
workspaces with project files, session restore with agent resume, agent detection and state,
the `nox` CLI and MCP server for agent cooperation (messages, claims, waits, spawn), command
palette, Settings window, 17 bundled themes, 10 bundled font families, attention chime and
notifications. See ROADMAP.md for what's still missing.
