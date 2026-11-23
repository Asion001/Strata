End-to-end tests run against a live server with a fresh test account:

```bash
STRATA_E2E_URL=http://localhost:8080 STRATA_E2E_USER=admin STRATA_E2E_PASSWORD=... \
  npx playwright test --project=chromium
```

They are skipped when `STRATA_E2E_PASSWORD` is not set, except in CI, where
that is an error. CI's `e2e` job builds the server and the web app from the
commit, starts them against a Postgres service container and runs the
Chromium project.
