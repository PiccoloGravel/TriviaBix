#!/bin/sh
# Prepares /data and drops root before starting the server.
#
# PUID/PGID pick who owns the files in /data (Unraid uses 99:100, nobody:users).
# If CONTENT_DIR points inside /data, the built-in question packs are copied there so you can
# add your own packs next to them. The built-in "starter" folders are refreshed on every start,
# so put your own packs in another folder, e.g. packs/mine.
set -eu

PUID="${PUID:-10001}"
PGID="${PGID:-10001}"

seed_content() {
    dir="$1"
    mkdir -p "$dir/packs" "$dir/scripts"
    [ -e "$dir/categories.toml" ] || cp /app/content/categories.toml "$dir/"
    cp /app/content/README.md "$dir/"
    for sub in packs scripts; do
        for src in /app/content/$sub/*/; do
            name=$(basename "$src")
            rm -rf "$dir/$sub/$name"
            cp -r "$src" "$dir/$sub/$name"
        done
    done
}

if [ "$(id -u)" = "0" ]; then
    mkdir -p /data
    case "${CONTENT_DIR:-}" in
        /data/*) seed_content "$CONTENT_DIR" && chown -R "$PUID:$PGID" "$CONTENT_DIR" ;;
    esac
    if [ "$(stat -c %u:%g /data)" != "$PUID:$PGID" ]; then
        chown -R "$PUID:$PGID" /data
    fi
    exec setpriv --reuid="$PUID" --regid="$PGID" --clear-groups -- "$@"
fi

case "${CONTENT_DIR:-}" in
    /data/*) seed_content "$CONTENT_DIR" ;;
esac
exec "$@"
