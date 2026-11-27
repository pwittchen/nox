---
name: security-deps-fix
description: Run `cargo audit` on the nox workspace and fix what it reports — vulnerabilities, yanked crates and unmaintained / unsound warnings — by updating dependencies, or, when no compatible fix exists, by documenting an ignore in .cargo/audit.toml. Also prunes ignore entries that are no longer reported. Use when the user asks to fix audit / security / dependency advisories, or runs /security-deps-fix.
argument-hint: "[optional: advisory ID or crate to focus on]"
allowed-tools: Bash(cargo audit:*), Bash(cargo-audit:*), Bash(cargo update:*), Bash(cargo tree:*), Bash(cargo build:*), Bash(cargo test:*), Bash(cargo lint:*), Bash(cargo fmt:*), Bash(cargo search:*), Bash(cargo info:*), Bash(cargo install cargo-audit:*), Bash(git status:*), Bash(git diff:*), Bash(git log:*), Bash(jq:*), Bash(mv:*), Read, Edit, Write
---

# Security deps fix

Make `cargo-audit audit --deny warnings` (what CI runs, `.github/workflows/ci.yml`) pass with the
smallest, safest change set. Focus (may be empty): $ARGUMENTS

## Hard rules

1. **Never** hide a real vulnerability (`kind: vulnerability`) affecting code compiled into the
   macOS app just to make the audit green. Fix it, or stop and report to the user.
2. **GPUI stays pinned** (`gpui = "=0.2.2"` in the workspace `Cargo.toml`). Don't bump it or other
   `=`-pinned crates; if only a GPUI upgrade would fix something, report it instead.
3. Prefer `cargo update -p <crate>` (lockfile only, semver-compatible) over editing `Cargo.toml`.
   Bump a version requirement in `Cargo.toml` only for direct dependencies of nox crates, and
   only when needed. Never run a blanket `cargo update` unless the user asks.
4. Don't commit or push. Leave the changes for the user (they can use /commit).
5. Every ignore entry needs an inline comment (crate, kind, which dependency pulls it in) and the
   header comment of `.cargo/audit.toml` must explain why it's acceptable.

## Step 1: Audit

```sh
command -v cargo-audit || cargo install cargo-audit --locked
cargo-audit audit --deny warnings          # human-readable, exit code like CI
cargo-audit audit --no-fetch --json > "$TMPDIR/audit.json"   # machine-readable, for triage
```

From the JSON, list `vulnerabilities.list[]` and every entry in `warnings` (unmaintained, unsound,
yanked, notice): advisory ID, crate, installed version, `patched_versions`, kind, title.

Then find stale ignores: run the audit once without the config and compare with the
`[advisories] ignore` list in `.cargo/audit.toml` — an ignored ID that no longer shows up is stale.

```sh
mv .cargo/audit.toml "$TMPDIR/audit.toml.bak"
cargo-audit audit --no-fetch --json > "$TMPDIR/audit-all.json"; mv "$TMPDIR/audit.toml.bak" .cargo/audit.toml
```

Always restore the file (check `git status` afterwards — `.cargo/audit.toml` must be unchanged).

If nothing is reported and no ignore is stale, say so and stop.

## Step 2: Triage each finding

For each advisory find who pulls it in:

```sh
cargo tree -i <crate>@<version> --workspace --edges normal,build,dev --target all
cargo tree -i <crate>@<version> --workspace --target aarch64-apple-darwin   # in the app?
```

Classify:

- **A. Lockfile fix available** — a patched version satisfies every dependent's requirement →
  `cargo update -p <crate>@<version> --precise <patched>` (or just `-p <crate>`).
- **B. Direct dependency** of a nox crate needs a newer major/minor → bump it in the
  workspace / crate `Cargo.toml`, adapt the code, keep the API changes minimal.
- **C. Transitive, blocked** — the parent (usually GPUI or its sub-crates) caps the version, or
  the crate is unmaintained with no successor → candidate for an ignore. First check whether a
  newer *compatible* parent release exists (`cargo search <parent>` / `cargo info <parent>`) and
  try `cargo update -p <parent>`.
- **D. Not in the app** — Linux-only (`--target aarch64-apple-darwin` doesn't show it) or
  dev/test-only. Ignore with that reason.

A vulnerability in class C that *is* compiled into the app: stop and ask the user (options:
patch via `[patch.crates-io]`, drop the feature, accept risk).

## Step 3: Apply

- Run the updates from Step 2. After each, re-run `cargo-audit audit --no-fetch` to confirm the
  advisory is gone and no new one appeared.
- Add ignore entries for class C / D to `.cargo/audit.toml`, keeping the existing format:
  `"RUSTSEC-YYYY-NNNN", # crate: kind (why / via which parent)`. Update the header comment if a
  new category of reason appears.
- Remove stale ignore entries (and their header mentions).

## Step 4: Verify

```sh
cargo-audit audit --deny warnings
cargo build --workspace --locked
cargo fmt --all
cargo lint
cargo test --workspace            # or the tests of the crates whose deps changed, if slow
```

All must pass. If a dependency bump broke the build or tests and the fix isn't small, revert that
bump and treat the advisory as class C (or ask the user).

Also check `cargo build` prints no new warnings from the updated crates; if the future-incompat
notice in `.cargo/config.toml` is affected, run `cargo report future-incompatibilities` and update
that comment.

## Step 5: Report

Short summary for the user:

- table: advisory → crate → action (updated `a → b` / bumped in Cargo.toml / ignored: reason /
  ignore removed),
- anything left that needs their decision (e.g. only fixable by a GPUI upgrade),
- verification results (audit, build, lint, tests),
- files changed (`Cargo.lock`, `Cargo.toml`, `.cargo/audit.toml`, code).

If the user later commits this: dependency updates and ignore-list changes are one commit
("Update dependencies to fix cargo audit advisories" style); unrelated code fixes go separately.
