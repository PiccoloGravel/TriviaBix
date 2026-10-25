# syntax=docker/dockerfile:1

FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p triviabix \
 && cp target/release/triviabix /usr/local/bin/triviabix

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --home /app triviabix \
 && mkdir -p /app /data && chown triviabix /data
WORKDIR /app
COPY --from=build /usr/local/bin/triviabix /usr/local/bin/triviabix
COPY web /app/web
ENV PORT=8080 \
    STATIC_DIR=/app/web \
    DATABASE_URL=sqlite:///data/triviabix.db \
    RUST_LOG=triviabix=info,tower_http=info
USER triviabix
VOLUME /data
EXPOSE 8080
CMD ["triviabix"]
