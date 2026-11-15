# Strata

A fast, self-hosted file cloud. Rust server (axum, PostgreSQL, Apache OpenDAL),
SvelteKit web app that installs as a PWA.

* Mount local folders, S3 buckets and FTP servers anywhere in your tree, nested
  to any depth, in **raw** mode (plain files, external changes picked up) or
  **hashed** mode (deduplicated, content-addressed BLAKE3 blobs, FastCDC chunks).
* Resumable chunked uploads that skip content you already stored, ranged
  downloads and media streaming, thumbnails, name search, share links with
  password and expiry, a per-volume change journal with websocket push.

See [docs/DESIGN.md](docs/DESIGN.md) for how it works.

## Quick start (Docker)

```bash
cp deploy/strata.env.example deploy/strata.env   # fill in the secrets
cd deploy && docker compose -f compose.example.yml up -d
```

Open `http://your-server:8080` and sign in with `STRATA_ADMIN_USER` /
`STRATA_ADMIN_PASSWORD`. Put a TLS-terminating reverse proxy in front of it
for anything beyond a LAN (set `STRATA_COOKIE_SECURE=false` only for plain HTTP).

## Development

```bash
docker run -d --name strata-pg -e POSTGRES_USER=strata -e POSTGRES_PASSWORD=strata -p 5432:5432 postgres:18-alpine
export STRATA_TEST_DATABASE_URL=postgres://strata:strata@localhost:5432/strata
cargo test --workspace               # S3 tests also run when STRATA_TEST_S3_ENDPOINT is set
cd web && npm ci && npm run build && npm test
DATABASE_URL=postgres://strata:strata@localhost:5432/strata STRATA_SECRET_KEY=dev \
  STRATA_ADMIN_USER=admin STRATA_ADMIN_PASSWORD=change-me-please STRATA_COOKIE_SECURE=false \
  cargo run -p strata-server
```

`scripts/check-private.sh` blocks commits containing patterns listed in a local,
gitignored `.private-patterns` file (install it as a pre-commit hook).

## License

AGPL-3.0-only. See [LICENSE](LICENSE).
