import { randomBytes } from 'node:crypto';
import { expect, test, type BrowserContext, type Page } from '@playwright/test';
import { enabled, newSession, putBytes, putFile } from './helpers';

test.skip(!enabled, 'set STRATA_E2E_URL / STRATA_E2E_USER / STRATA_E2E_PASSWORD');

/** A 2×2 PNG. */
const PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAFklEQVR4nGP8z8DAwMDAxMDAwMDAAAANHQEDK+mmyAAAAABJRU5ErkJggg==',
  'base64'
);

/** Wait until the service worker controls the page. */
async function controlled(page: Page) {
  await page.waitForFunction(() => !!navigator.serviceWorker?.controller, null, { timeout: 15_000 }).catch(async () => {
    // First visit: the worker claims the page once active; a reload makes sure.
    await page.reload();
    await page.waitForFunction(() => !!navigator.serviceWorker?.controller, null, { timeout: 15_000 });
  });
}

async function offlineState(page: Page) {
  return page.evaluate(async () => {
    const names = await caches.keys();
    const dbs = (await indexedDB.databases?.()) ?? [];
    return { caches: names, dbs: dbs.map((d) => d.name) };
  });
}

async function setOffline(ctx: BrowserContext, page: Page, off: boolean) {
  await ctx.setOffline(off);
  await page.waitForFunction((o) => navigator.onLine === !o, off);
}

test('offline: the app opens, the last-seen folder lists with a banner, pinned files open', async ({ browser }) => {
  const { ctx, page } = await newSession(browser);
  page.on('console', (m) => { if (m.text().startsWith('DBG')) console.log(m.text()); });
  const dir = `off-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  await putBytes(page, `${dir}/pic.png`, new Uint8Array(PNG));
  await putFile(page, `${dir}/notes.txt`, 'hello offline');
  await page.goto(`/?p=${encodeURIComponent(dir)}`);
  await controlled(page);
  await page.reload(); // (this listing now goes through the worker)
  await expect(page.getByRole('button', { name: 'pic.png', exact: true })).toBeVisible();

  // Keep the picture offline.
  await page.getByRole('button', { name: 'Actions for pic.png', exact: true }).click();
  await page.getByRole('menuitemcheckbox', { name: 'Keep offline' }).click();
  await expect(page.getByText('“pic.png” is kept offline.')).toBeVisible();

  await setOffline(ctx, page, true);
  await page.reload();
  await expect(page.getByTestId('offline-banner')).toContainText('Offline, as of');
  await expect(page.getByRole('button', { name: 'notes.txt', exact: true })).toBeVisible();

  // The pinned file opens from the cache.
  await page.getByRole('row').filter({ hasText: 'pic.png' }).getByRole('button', { name: 'pic.png', exact: true }).click();
  const img = page.getByTestId('viewer-image');
  await expect(img).toBeVisible();
  await expect.poll(() => img.evaluate((i: HTMLImageElement) => i.complete && i.naturalWidth)).toBe(2);
  await page.keyboard.press('Escape');

  await setOffline(ctx, page, false);
  await ctx.close();
});

test('an interrupted large upload resumes without re-sending finished chunks', async ({ browser }) => {
  test.setTimeout(180_000);
  const { ctx, page } = await newSession(browser);
  const dir = `up-${Date.now()}`;
  await putFile(page, `${dir}/.keep`, '');

  // What the app sends, and which chunks the server confirmed to it (its
  // own fetch calls, slowed down so the network can be cut halfway).
  const sent: string[] = [];
  const confirmed: string[] = [];
  await page.exposeFunction('__chunk', (h: string, ok: boolean) => void (ok ? confirmed : sent).push(h));
  await ctx.addInitScript(() => {
    const f = window.fetch;
    const tell = (window as unknown as { __chunk: (h: string, ok: boolean) => void }).__chunk;
    window.fetch = async (u, i) => {
      const m = /\/chunks\/([0-9a-f]+)$/.exec(String(u));
      if (!m) return f(u, i);
      tell(m[1], false);
      await new Promise((r) => setTimeout(r, 300));
      const r = await f(u, i);
      if (r.ok) tell(m[1], true);
      return r;
    };
  });
  await page.goto(`/?p=${encodeURIComponent(dir)}`);
  await controlled(page);

  const data = randomBytes(48 * 1024 * 1024);
  await page.locator('input[type=file][multiple]').setInputFiles({ name: 'big.bin', mimeType: 'application/octet-stream', buffer: data });
  await expect.poll(() => confirmed.length, { timeout: 60_000 }).toBeGreaterThanOrEqual(3);
  await setOffline(ctx, page, true);
  await expect(page.getByTestId('upload')).toContainText('big.bin');

  // Reopen the app while offline: the queued upload is still there.
  await page.reload();
  await expect(page.getByTestId('upload')).toContainText('big.bin');
  const finished = new Set(confirmed);
  const mark = sent.length;

  // Back online: it finishes.
  await setOffline(ctx, page, false);
  await expect(page.getByRole('button', { name: 'big.bin', exact: true })).toBeVisible({ timeout: 90_000 });
  const resumed = sent.slice(mark);
  expect(resumed.length).toBeGreaterThan(0);
  // No chunk the server had confirmed went up again, and none twice.
  expect(resumed.filter((h) => finished.has(h))).toEqual([]);
  expect(new Set(resumed).size).toBe(resumed.length);
  // And the file is whole.
  const size = await page.evaluate(async (p) => (await fetch(`/api/files/content?path=${encodeURIComponent(p)}`, { method: 'HEAD' })).headers.get('content-length'), `${dir}/big.bin`);
  expect(Number(size)).toBe(data.length);
  await ctx.close();
});

test('sign-out removes cached files, listings and queued uploads', async ({ browser }) => {
  const { ctx, page } = await newSession(browser);
  page.on('console', (m) => { if (m.text().startsWith('DBG')) console.log('OUT', m.text()); });
  const dir = `out-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  await putBytes(page, `${dir}/pic.png`, new Uint8Array(PNG));
  await page.goto(`/?p=${encodeURIComponent(dir)}`);
  await controlled(page);
  await page.reload();
  await expect(page.getByRole('button', { name: 'pic.png', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Actions for pic.png', exact: true }).click();
  await page.getByRole('menuitemcheckbox', { name: 'Keep offline' }).click();
  // (Any answer about it, so a failure shows the reason given.)
  const said = page.getByText(/“pic\.png”.*offline/).first();
  await expect(said).toBeVisible();
  expect(await said.textContent()).toBe('“pic.png” is kept offline.');

  // An upload queued while offline.
  await setOffline(ctx, page, true);
  await page.locator('input[type=file][multiple]').setInputFiles({ name: 'queued.txt', mimeType: 'text/plain', buffer: Buffer.from('later') });
  await expect(page.getByTestId('upload')).toContainText('queued.txt');
  await setOffline(ctx, page, false);
  // (Let it go back to waiting if it already started: the point is the queue.)

  let st = await offlineState(page);
  expect(st.caches.some((c) => c.startsWith('strata-files-'))).toBe(true);
  expect(st.caches.some((c) => c.startsWith('strata-lists-'))).toBe(true);

  await page.goto('/settings');
  await page.getByRole('button', { name: /sign out/i }).click();
  await expect(page).toHaveURL(/\/login/);
  st = await offlineState(page);
  expect(st.caches.filter((c) => /^strata-(files|lists)-|^strata-meta$/.test(c))).toEqual([]);
  expect(st.dbs).not.toContain('strata-offline');
  expect(st.dbs).not.toContain('strata-editor');
  await ctx.close();
});

test('installable: Chrome finds no installability errors', async ({ browser, browserName }) => {
  test.skip(browserName !== 'chromium', 'a Chromium check');
  const { ctx, page } = await newSession(browser);
  await page.goto('/');
  await controlled(page);
  const cdp = await ctx.newCDPSession(page);
  const { installabilityErrors } = await cdp.send('Page.getInstallabilityErrors');
  // (Test contexts are incognito, where Chrome never offers to install.)
  expect(installabilityErrors.filter((e) => e.errorId !== 'in-incognito')).toEqual([]);
  const { url, errors } = await cdp.send('Page.getAppManifest');
  expect(url).toMatch(/manifest\.webmanifest$/);
  expect(errors).toEqual([]);
  await ctx.close();
});

test('signing out offline ends the server session on the next start', async ({ browser }) => {
  const { ctx, page } = await newSession(browser);
  await page.goto('/settings');
  await controlled(page);
  await setOffline(ctx, page, true);
  await page.getByRole('button', { name: /sign out/i }).click();
  await expect(page.getByRole('dialog')).toContainText("The server couldn't be reached");
  await page.getByRole('dialog').getByRole('button', { name: 'OK' }).click();
  await expect(page).toHaveURL(/\/login/);
  // Online again: opening the app ends the old session before anything.
  await setOffline(ctx, page, false);
  await page.goto('/');
  await expect(page).toHaveURL(/\/login/);
  const me = await page.evaluate(async () => (await fetch('/api/auth/me')).status);
  expect(me).toBe(401);
  await ctx.close();
});
