# Changelog

## [Unreleased]

- Quitting asks "Quit nox?" first, also when nothing is running; turn it off with
  `app.confirm_quit` (Settings → General). The window's close button now quits like `⌘Q`.

## [0.1.0] — unreleased

First implementation of SPEC.md: GPUI terminal with splits, tabs (top / bottom / sidebar),
workspaces with project files, session restore with agent resume, agent detection and state,
the `nox` CLI and MCP server for agent cooperation (messages, claims, waits, spawn), command
palette, Settings window, 17 bundled themes, 10 bundled font families, attention chime and
notifications. See ROADMAP.md for what's still missing.
