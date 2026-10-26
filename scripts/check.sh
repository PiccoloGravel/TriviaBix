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

if [ "${1:-}" = "--docker" ]; then
    echo "== docker image"
    docker build -t triviabix:check .
fi
echo "all good"
