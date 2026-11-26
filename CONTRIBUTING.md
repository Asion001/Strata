# Contributing to Strata

Thanks for helping! Strata is alpha software, so expect things to move. For
anything bigger than a small fix, open a [discussion](https://github.com/Asion001/Strata/discussions)
or an issue first so we can agree on the approach.

## Development setup

You need:

- **Rust**: install [rustup](https://rustup.rs). `rust-toolchain.toml` picks the
  toolchain and components (rustfmt, clippy) automatically.
- **Node.js 22**: the version is in `.nvmrc` (`nvm use` or `fnm use`).
- **Docker**: for PostgreSQL in development and for the integration tests.

```bash
# PostgreSQL for development and tests
docker run -d --name strata-pg -e POSTGRES_USER=strata -e POSTGRES_PASSWORD=strata \
  -p 5432:5432 postgres:18-alpine
export STRATA_TEST_DATABASE_URL=postgres://strata:strata@localhost:5432/strata

# Web app
cd web && npm ci && cd ..

# Run the server (serves the built web app)
(cd web && npm run build)
DATABASE_URL=$STRATA_TEST_DATABASE_URL STRATA_SECRET_KEY=dev \
  STRATA_ADMIN_USER=admin STRATA_ADMIN_PASSWORD=change-me-please STRATA_COOKIE_SECURE=false \
  cargo run -p strata-server
```

For UI work, `cd web && npm run dev` runs Vite with hot reload.

## Tests

| What                       | Command                                                       |
| -------------------------- | ------------------------------------------------------------- |
| Rust format and lints      | `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` |
| Dependency policy          | `cargo deny check` (licenses, advisories, sources; see `deny.toml`) |
| Rust unit and integration  | `cargo test --workspace` (needs `STRATA_TEST_DATABASE_URL`)   |
| S3 backend tests           | set `STRATA_TEST_S3_ENDPOINT` (for example a local MinIO), then `cargo test --workspace` |
| Web type check             | `cd web && npm run check`                                     |
| Web unit tests (Vitest)    | `cd web && npm test`                                          |
| End-to-end (Playwright)    | `cd web && npm run e2e` (see `web/tests/e2e/README.md`)       |
| Startup bundle size        | `cd web && npm run build && npm run size`                     |
| Property tests             | part of `cargo test`; `PROPTEST_CASES` sets the case count (CI 256, nightly 10,000) |
| API contract (Schemathesis)| `scripts/api-contract.sh` (Docker)                            |
| Fuzzing (cargo-fuzz)       | `cd fuzz && cargo +nightly fuzz run -O <target>` (see `fuzz/README.md`) |

Benchmarks are planned; see the roadmap in `docs/DESIGN.md`.

### Coverage

Code that can lose data (the hashed store, garbage collection, mount
resolution, moves, and save and write paths) is held to 100% test coverage.
Everything else aims for at least 90%, and coverage should never drop.

## The API document

The HTTP API is described by an OpenAPI document generated from the code
(`crates/server/src/openapi.rs`, served at `/api/v1/openapi.json`). A copy is
committed at `api/openapi.json`, and the web app's API types are generated
from it. When you change a route or what it sends or takes:

```bash
UPDATE_OPENAPI=1 cargo test -p strata-server openapi   # rewrite api/openapi.json
cd web && npm run api:types                            # regenerate the TypeScript types
```

CI fails when either is out of date, when a route is missing from the
document, or when an operation documents no 4xx answer.
`scripts/api-contract.sh` runs Schemathesis against a server in Docker
(Postgres and MinIO): no 5xx and only documented answers, for every
operation.

## Pull requests

- One focused change per PR. Small PRs get reviewed faster.
- Fill in the PR template, link the issue, and add tests for new behaviour.
- CI must be green. PRs are squash-merged, so the PR title becomes the commit
  on `main`.

### Commit messages

Use [Conventional Commits](https://www.conventionalcommits.org):

```
feat(storage): detect btrfs snapshot support
fix(web): keep the selection after a rename
docs: explain hashed mounts
```

Types: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`,
`chore`. Releases and the changelog are generated from these.

### Developer Certificate of Origin

Every commit must be signed off under the
[Developer Certificate of Origin](https://developercertificate.org): you
certify that you wrote the change or have the right to submit it under the
project's license (AGPL-3.0-only). Sign off with `git commit -s`, which adds:

```
Signed-off-by: Your Name <you@example.com>
```

Use the GitHub noreply address if you don't want your email in the history. A
CI check rejects PRs with unsigned commits. To fix a branch, run
`git rebase --signoff main` and force-push.

### Keep private data out

Never commit real host names, IP addresses, personal names or emails, or
credentials. Use placeholders such as `strata.example.com` and `your-server`.
See "Keeping private data out" in the README for the optional pre-commit hook.

## Code of Conduct

Everyone taking part is expected to follow our [Code of Conduct](CODE_OF_CONDUCT.md).
Report security issues privately as described in [SECURITY.md](SECURITY.md).
