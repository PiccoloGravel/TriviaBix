#!/bin/sh
# What to run before pushing: formatting, the same builds the Dockerfile does (the line writer
# only builds with --features cli, so a plain workspace build misses it), and every test.
# Usage: scripts/check.sh            (add --docker to also build the image)
set -eu
cd "$(dirname "$0")/.."

echo "== format"
cargo fmt --all --check

echo "== build, as the Dockerfile does"
cargo build --locked -p triviabix
cargo build --locked -p triviabix-questions --features cli

echo "== tests"
cargo test --locked --workspace
cargo test --locked -p triviabix-questions --features cli

echo "== web scripts"
for f in web/*.js; do node --check "$f"; done
# The TV page also runs in older TV browsers (Samsung's on 2019-2020 sets is Chromium 63-69),
# so its scripts stay clear of ?? and ?. and catch without a binding.
if grep -nE ' \?\? |[A-Za-z_)\]]\?\.[A-Za-z_\[(]|catch \{' web/common.js web/bix.js web/cutscenes.js web/sfx.js web/host.js; then
    echo "newer syntax in a TV script (above); spell it out for older TV browsers"
    exit 1
fi

if [ "${1:-}" = "--docker" ]; then
    echo "== docker image"
    docker build -t triviabix:check .
fi
echo "all good"
