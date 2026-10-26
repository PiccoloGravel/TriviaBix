# syntax=docker/dockerfile:1

FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p triviabix \
 && cp target/release/triviabix /usr/local/bin/triviabix

FROM debian:bookworm-slim
# CA certificates for HTTPS calls to the iTunes Search API (Name That Tune).
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home /app triviabix \
 && mkdir -p /app /data && chown triviabix /data
WORKDIR /app
COPY --from=build /usr/local/bin/triviabix /usr/local/bin/triviabix
COPY web /app/web
# Question packs and host scripts, loaded into the database at startup.
COPY content /app/content
ENV PORT=8080 \
    STATIC_DIR=/app/web \
    CONTENT_DIR=/app/content \
    DATABASE_URL=sqlite:///data/triviabix.db \
    TTS_CACHE_DIR=/data/tts \
    RUST_LOG=triviabix=info,tower_http=info
USER triviabix
VOLUME /data
EXPOSE 8080
CMD ["triviabix"]
