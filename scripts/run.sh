#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo run -p nox --bin nox-app
