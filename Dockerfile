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
    && useradd --system --uid 10001 --home /data strata \
    && mkdir -p /data && chown strata /data
COPY --from=server /strata /usr/local/bin/strata
COPY --from=web /web/build /app/web
ENV STRATA_BIND=0.0.0.0:8080 \
    STRATA_DATA_DIR=/data \
    STRATA_WEB_DIR=/app/web \
    RUST_LOG=info,sqlx=warn,tower_http=warn
USER strata
VOLUME /data
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s CMD curl -fsS http://127.0.0.1:8080/api/health || exit 1
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["strata"]
