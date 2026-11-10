# Changelog

## 0.2.1 — 2026-11-10

- Show only a stack icon in the workspace chip with one workspace (320d1ae)
- Add Google Antigravity and aictl agent icons (089c83c)
- moved design doc for remote agents into docs/design/ dir #no-deploy (6b8d8c1)
- Add design doc for remote agents over SSH #no-deploy (d925d8d)
- Add Datafast analytics to the website (f3beccd)
- Add LinkedIn promo graphic (97712b4)

## 0.2.0 — 2026-11-09

- Make the overlay scrollbar draggable (3a56222)
- Update README.md (80aa3ee)
- Link the website in the README (05547c0)
- Deploy the website to the VPS (07b918f)
- Skip CI for website-only changes #no-deploy (4f51a98)
- Add a contact link to the website menu (eb169ec)
- Update the website version on release (0d4331a)
- Stop adding an empty [Unreleased] section to the changelog on release (373d520)

## 0.1.3 — 2026-11-09

- Let Option type national characters on non-US keyboard layouts (12a9215)
- Lowercase the slogan in the website download section #no-deploy (87a15d4)
- Add a static website built with Bun #no-deploy (a91d9d9)
- Update CHANGELOG.md (050e7b3)

## 0.1.2 — 2026-11-08

- nox updates itself: nox → Check for Updates…, the palette, the About window and Settings check
  GitHub Releases, then download, verify (SHA-256 + Developer ID signature) and install the
  update with a relaunch. A daily automatic check is on by default (`app.check_for_updates`).
- Quitting asks "Quit nox?" first, also when nothing is running; turn it off with
  `app.confirm_quit` (Settings → General). The window's close button hides nox; the
  Dock icon brings it back.

## 0.1.1 — 2026-11-08

- Show the Claude icon while Claude Code is working (757cb49)
- Keep Claude's agent state correct when hooks miss a turn (5436c3e)
- Add install and update instructions to the README (5618d8a)
- Take the release version from the git tag and update the changelog (c6f87d7)

## 0.1.0 — 2026-11-08

First implementation of SPEC.md: GPUI terminal with splits, tabs (top / bottom / sidebar),
workspaces with project files, session restore with agent resume, agent detection and state,
the `nox` CLI and MCP server for agent cooperation (messages, claims, waits, spawn), command
palette, Settings window, 17 bundled themes, 10 bundled font families, attention chime and
notifications. See ROADMAP.md for what's still missing.
