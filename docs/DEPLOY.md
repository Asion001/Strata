# Deploying Strata

Strata is one container (the Rust server, which also serves the web app) plus
PostgreSQL 14+ with the `pg_trgm` extension available.

## Build the image

```bash
docker build -t strata:local .          # on the target machine, or
docker buildx build --platform linux/amd64 -t strata:local .
```

The image runs as an unprivileged user (uid 10001), listens on 8080, keeps all
data under `/data`, and has a `HEALTHCHECK` on `/healthz` (200 when the database
answers).

Data layout inside `/data`:

| Path           | What                                                        |
|----------------|-------------------------------------------------------------|
| `/data/blobs`  | default connection: every user's private hashed root        |
| `/data/files`  | "Server files" connection, for raw mounts of plain files    |
| `/data/staging`| chunks of uploads into raw volumes (cleaned after 2 days)   |
| `/data/thumbs` | thumbnail cache                                             |

Mount a volume (or a host folder) at `/data`. Other local folders can be bind
mounted below `/data` and added as local connections by an admin.

## Database

```sql
CREATE ROLE strata LOGIN PASSWORD 'generated-password';
CREATE DATABASE strata OWNER strata;
\c strata
CREATE EXTENSION IF NOT EXISTS pg_trgm;
```

Migrations run automatically at start-up.

## Configuration

See `deploy/strata.env.example`. Keep the env file out of version control
(mode 600). `STRATA_SECRET_KEY` encrypts connection credentials: losing or
changing it makes stored credentials unreadable. The admin account is created
only when the database has no users.

## Behind Traefik

Put the container on Traefik's Docker network and add labels like:

```yaml
labels:
  traefik.enable: "true"
  traefik.docker.network: proxy
  traefik.http.services.strata.loadbalancer.server.port: "8080"
  traefik.http.routers.strata.rule: Host(`strata.example.com`)
  traefik.http.routers.strata.entrypoints: websecure
  traefik.http.routers.strata.tls.certresolver: letsencrypt
```

Websockets (`/api/ws`) work through Traefik without extra settings. Uploads use
8 MiB chunks, so request-body limits of ~16 MiB or more are enough (Cloudflare's
free plan allows 100 MB). Don't add a buffering middleware with a lower limit.

## Smoke test

```bash
STRATA_USER=admin STRATA_PASSWORD=... STRATA_BIN="docker run --rm -v /tmp:/tmp strata:local strata" \
  scripts/smoke.sh https://strata.example.com
```

It checks `/healthz`, logs in, creates a folder, uploads 20 MB with the chunked
protocol, downloads and compares the BLAKE3 hash, tests a range request and
deletes the folder.
