End-to-end tests run against a live server with a fresh test account:

```bash
STRATA_E2E_URL=http://localhost:8080 STRATA_E2E_USER=admin STRATA_E2E_PASSWORD=... \
  npx playwright test --project=chromium
```

They are skipped when `STRATA_E2E_PASSWORD` is not set. CI wiring comes with
the CI/CD task.
