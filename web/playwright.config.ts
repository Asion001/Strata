import { defineConfig, devices } from '@playwright/test';

// End-to-end tests against a running Strata (see tests/e2e/README.md):
//   STRATA_E2E_URL=http://localhost:8080 STRATA_E2E_USER=admin STRATA_E2E_PASSWORD=... npx playwright test
export default defineConfig({
  testDir: 'tests/e2e',
  timeout: 60_000,
  fullyParallel: false,
  use: {
    baseURL: process.env.STRATA_E2E_URL ?? 'http://localhost:8080',
    trace: 'off'
  },
  projects: [
    // PW_CHANNEL=chrome uses an installed Google Chrome instead of the bundled build.
    { name: 'chromium', use: { ...devices['Desktop Chrome'], channel: process.env.PW_CHANNEL } },
    { name: 'firefox', use: { ...devices['Desktop Firefox'] } },
    { name: 'webkit', use: { ...devices['Desktop Safari'] } }
  ]
});
