# syntax=docker/dockerfile:1

FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p triviabix \
 && cargo build --release --locked -p triviabix-questions --features cli \
 && cp target/release/triviabix target/release/triviabix-questions /usr/local/bin/

FROM debian:bookworm-slim
# CA certificates for HTTPS calls to the iTunes Search API (Name That Tune).
# The entrypoint uses setpriv (util-linux, already in the base image) to drop root.
# Python, Pillow and ffmpeg run the rendering jobs from the admin panel (gpu/vast.py and
# art/render_on_vast.py); the GPU work itself happens on a rented vast.ai machine.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    python3 python3-pil ffmpeg \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home /app triviabix \
 && mkdir -p /app /data && chown triviabix /data
WORKDIR /app
COPY --from=build /usr/local/bin/triviabix /usr/local/bin/triviabix-questions /usr/local/bin/
COPY web /app/web
# The rendering scripts, run from the admin panel's Rendering page.
COPY gpu /app/gpu
COPY art /app/art
# Question packs and host scripts, loaded into the database at startup.
COPY content /app/content
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
ENV PORT=8080 \
    STATIC_DIR=/app/web \
    CONTENT_DIR=/app/content \
    DATABASE_URL=sqlite:///data/triviabix.db \
    ASSETS_DIR=/data/assets \
    ART_DIR=/data/art \
    RENDER_TOOLS_DIR=/app \
    RUST_LOG=triviabix=info,tower_http=info \
    PUID=10001 \
    PGID=10001
VOLUME /data
EXPOSE 8080
# Starts as root only to fix /data ownership (see PUID/PGID), then runs as PUID.
ENTRYPOINT ["entrypoint.sh"]
CMD ["triviabix"]
