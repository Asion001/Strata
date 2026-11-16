# syntax=docker/dockerfile:1.7
# Strata: one image with the Rust server and the built web app.

FROM node:22-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM rust:1-bookworm AS server
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p strata-server && cp target/release/strata /strata

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates tini curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /var/lib/strata strata \
    && mkdir -p /data /var/lib/strata && chown strata /data /var/lib/strata
COPY --from=server /strata /usr/local/bin/strata
COPY --from=web /web/build /app/web
ENV STRATA_BIND=0.0.0.0:8080 \
    STRATA_DATA_DIR=/var/lib/strata \
    STRATA_LOCAL_BASE=/data \
    STRATA_WEB_DIR=/app/web \
    RUST_LOG=info,sqlx=warn,tower_http=warn
USER strata
# /data: storage (connections, default stores); /var/lib/strata: server state.
VOLUME /data /var/lib/strata
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["strata"]
