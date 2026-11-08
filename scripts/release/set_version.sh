#!/usr/bin/env bash
# Set the workspace version (SPEC §15.1) in Cargo.toml and the workspace members' entries in
# Cargo.lock, without running cargo (no toolchain or registry access needed).
#
#   scripts/release/set_version.sh 1.2.0
#   scripts/release/set_version.sh v1.2.0-beta.1   # a leading `v` is stripped
set -euo pipefail
cd "$(dirname "$0")/../.."

VERSION=${1:?version}
VERSION=${VERSION#v}
SEMVER='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'
[[ "$VERSION" =~ $SEMVER ]] || { echo "error: '$VERSION' is not a SemVer version" >&2; exit 1; }

# Cargo.toml: the first `version = …` line inside [workspace.package].
awk -v v="$VERSION" '
  /^\[/ { in_pkg = ($0 == "[workspace.package]") }
  in_pkg && !done && /^version[[:space:]]*=/ { print "version = \"" v "\""; done = 1; next }
  { print }
  END { if (!done) { print "error: no version in [workspace.package]" > "/dev/stderr"; exit 1 } }
' Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml

# Cargo.lock: workspace members are the `[[package]]` entries without a `source` line. Each entry
# is buffered until its end so the version line can be rewritten once that's known.
awk -v v="$VERSION" '
  function flush() {
    for (i = 1; i <= n; i++) {
      if (is_pkg && !has_source && buf[i] ~ /^version = /) buf[i] = "version = \"" v "\""
      print buf[i]
    }
    n = 0; has_source = 0; is_pkg = 0
  }
  /^\[\[package\]\]$/ || /^$/ { flush() }
  /^\[\[package\]\]$/ { is_pkg = 1 }
  { buf[++n] = $0; if ($0 ~ /^source = /) has_source = 1 }
  END { flush() }
' Cargo.lock > Cargo.lock.tmp && mv Cargo.lock.tmp Cargo.lock

echo "version set to $VERSION"
