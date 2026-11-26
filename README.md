# Strata

[![CI](https://github.com/Asion001/Strata/actions/workflows/ci.yml/badge.svg)](https://github.com/Asion001/Strata/actions/workflows/ci.yml)
[![Privacy](https://github.com/Asion001/Strata/actions/workflows/privacy.yml/badge.svg)](https://github.com/Asion001/Strata/actions/workflows/privacy.yml)
[![License: AGPL-3.0-only](https://img.shields.io/badge/license-AGPL--3.0--only-blue.svg)](LICENSE)
![Status: alpha](https://img.shields.io/badge/status-alpha-orange.svg)

**A fast, self-hosted file cloud.** Strata lets you plug in the storage you
already have (local disks, S3 buckets, FTP servers) and mount any of it
anywhere in your own file tree. Each mount is stored either as plain files or
as deduplicated, content-addressed blobs, and the two kinds nest freely. It is
written in Rust with a SvelteKit web app that installs as a PWA.

> **Status: alpha.** Data formats and APIs may still change. Keep backups.

## Features

* **Mount anything anywhere.** Local folders, S3 buckets and FTP servers can be
  mounted at any path, nested to any depth.
* **Raw or hashed storage per mount.** Raw mounts keep plain files and pick up
  outside changes. Hashed mounts store deduplicated BLAKE3 blobs with FastCDC
  chunking.
* **Fast, resumable uploads** that skip content you already stored, plus ranged
  downloads and media streaming.
* **Viewers and editor:** images, video and audio, PDFs with thumbnails and
  search, and a code and text editor whose saves never silently overwrite
  someone else's change.
* **Sharing:** share links with password and expiry.
* **Live updates:** a per-mount change journal pushed to clients over websockets.
* **Name search**, thumbnails and an installable PWA.

## Architecture

```
 Browser (SvelteKit PWA)
        |  JSON API + websocket
        v
 strata-server (Rust, axum) ---- PostgreSQL (tree, journal, users, shares)
        |
  mount resolution: /Photos -> S3 (hashed), /Work -> local (raw), ...
        |
  Apache OpenDAL ---- local disk | S3 | FTP
```

See [docs/DESIGN.md](docs/DESIGN.md) for how it works and
[docs/DEPLOY.md](docs/DEPLOY.md) for deployment.

## Quick start (Docker)

```bash
cp deploy/strata.env.example deploy/strata.env   # fill in the secrets
cd deploy && docker compose -f compose.example.yml up -d
```

Open `http://your-server:8080` and sign in with `STRATA_ADMIN_USER` /
`STRATA_ADMIN_PASSWORD`. Put a TLS-terminating reverse proxy in front of it
for anything beyond a LAN (set `STRATA_COOKIE_SECURE=false` only for plain HTTP).

## Configuration

Set these in `deploy/strata.env` (or the environment):

| Variable | Default | Purpose |
| --- | --- | --- |
| `DATABASE_URL` | required | PostgreSQL connection string |
| `STRATA_SECRET_KEY` | required | Long random string that encrypts connection credentials. Never change it after setup. |
| `STRATA_ADMIN_USER` / `STRATA_ADMIN_PASSWORD` | unset | First admin, created only when the database has no users |
| `STRATA_COOKIE_SECURE` | `true` | Set to `false` only when serving over plain HTTP |
| `STRATA_BIND` | `0.0.0.0:8080` | Listen address |
| `STRATA_DATA_DIR` | `./data` | Server state |
| `STRATA_LOCAL_BASE` | `<data dir>/local` | Root for "local folder" connections |
| `STRATA_DEFAULT_STORE` | `blobs` | Hashed store for private user roots, below the local base |
| `STRATA_FILES_DIR` | `files` | The "Server files" connection, below the local base |
| `STRATA_WEB_DIR` | `./web/build` | Built web app to serve |
| `STRATA_SESSION_DAYS` | `30` | Sign-in session lifetime |
| `STRATA_GC_GRACE_HOURS` | `24` | How long unreferenced blobs wait before garbage collection |
| `STRATA_RAW_TTL_SECS` | `30` | Seconds before a raw mount is rescanned on access |
| `STRATA_RAW_BACKGROUND_RESCAN` | `true` | Answer a stale raw folder from the cache and rescan it in the background (`false`: the request waits for the rescan) |
| `STRATA_TRASH_DAYS` | `30` | Days deleted items stay in the trash; `0` makes new deletes permanent (items already in the trash keep their date) |

The container image sets its own defaults for the paths; see
`deploy/strata.env.example`.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full setup, all test commands
and the pull request process. The short version:

```bash
docker run -d --name strata-pg -e POSTGRES_USER=strata -e POSTGRES_PASSWORD=strata -p 5432:5432 postgres:18-alpine
export STRATA_TEST_DATABASE_URL=postgres://strata:strata@localhost:5432/strata
cargo test --workspace               # S3 tests also run when STRATA_TEST_S3_ENDPOINT is set
cd web && npm ci && npm run build && npm test
DATABASE_URL=postgres://strata:strata@localhost:5432/strata STRATA_SECRET_KEY=dev \
  STRATA_ADMIN_USER=admin STRATA_ADMIN_PASSWORD=change-me-please STRATA_COOKIE_SECURE=false \
  cargo run -p strata-server
```

### Keeping private data out

`scripts/check-private.sh` blocks commits whose changes, message or author match
patterns you keep outside git: a gitignored `.private-patterns` file in the clone or
`~/.config/strata/private-patterns` (one case-insensitive regex per line). Enable the
hooks once per clone with `git config core.hooksPath .githooks`; the pre-commit hook
also runs [gitleaks](https://github.com/gitleaks/gitleaks) when it is installed.
In CI the `Privacy` workflow runs the same check against the `PRIVATE_PATTERNS`
repository secret, reporting matches by location only, plus gitleaks for secrets.

## Contributing and security

Contributions are welcome; please read [CONTRIBUTING.md](CONTRIBUTING.md) and the
[Code of Conduct](CODE_OF_CONDUCT.md). Report vulnerabilities privately as
described in [SECURITY.md](SECURITY.md).

## License

AGPL-3.0-only. See [LICENSE](LICENSE).
