# Deploying Strata

Strata is one container (the Rust server, which also serves the web app) plus
PostgreSQL 14+ with the `pg_trgm` extension available.

## Build the image

```bash
docker build -t strata:local .          # on the target machine, or
docker buildx build --platform linux/amd64 -t strata:local .
```

The image runs as an unprivileged user (uid 10001), listens on 8080, keeps
storage under `/data` and server state under `/var/lib/strata` (back up both),
and has a `HEALTHCHECK` on `/healthz` (200 when the database answers).

Two volumes:

| Path               | What                                                       |
|--------------------|------------------------------------------------------------|
| `/data`            | storage base (`STRATA_LOCAL_BASE`) for local connections   |
| `/data/blobs`      | default connection: every user's private hashed root       |
| `/data/files`      | "Server files" connection, for raw mounts of plain files   |
| `/var/lib/strata`  | server state (`STRATA_DATA_DIR`): upload staging, thumbnails |

The state dir must not be inside the storage base (the server refuses to
start otherwise), so no connection can ever reach it. Host folders mounted at
either path must be writable by uid 10001 (`chown -R 10001 /srv/strata`).
Other folders can be bind mounted below `/data` and added as local
connections by an admin. `.strata-tmp` folders inside local connections hold
in-progress atomic writes; they are hidden and cannot be mounted.

A folder Strata cannot write to (bind mounted with `:ro`, or not writable by
uid 10001) is added as a **read-only** connection: it can be mounted in raw
mode to browse, preview, download and copy files out, while uploads, new
folders, renames, moves and deletes are refused. Hashed mode needs to write
blobs, so it is not offered for read-only connections. To make such a folder
writable, mount it without `:ro`, fix its ownership, and save the connection
again.

`STRATA_DEFAULT_STORE` and `STRATA_FILES_DIR` are used once, when the first
account is created; after that the connections live in the database.

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
