#!/usr/bin/env bash
# Set the released version on the website (SPEC §15.1): the version pill and its release link in
# website/index.html and the version in the `cargo test` demo in website/script.js.
#
#   scripts/release/set_website_version.sh 1.2.0
#   scripts/release/set_website_version.sh v1.2.0   # a leading `v` is stripped
set -euo pipefail
cd "$(dirname "$0")/../.."

VERSION=${1:?version}
VERSION=${VERSION#v}
SEMVER='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'
[[ "$VERSION" =~ $SEMVER ]] || { echo "error: '$VERSION' is not a SemVer version" >&2; exit 1; }

V='v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?'
export VERSION

# The pill: <a class="version-pill" href="…/releases/tag/vX.Y.Z">vX.Y.Z</a>
perl -0pi -e '
  s{(class="version-pill" href="[^"]*/releases/tag/)'"$V"'(">)'"$V"'(</a>)}{$1v$ENV{VERSION}$2v$ENV{VERSION}$3}
    or die "error: no version pill in website/index.html\n";
' website/index.html

# The demo line: Compiling</b></span> nox_terminal vX.Y.Z
perl -0pi -e '
  s{(nox_terminal )'"$V"'}{$1v$ENV{VERSION}}
    or die "error: no nox_terminal version in website/script.js\n";
' website/script.js

echo "website version set to $VERSION"
