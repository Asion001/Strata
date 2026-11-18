import { expect, type Browser, type BrowserContext, type Page } from '@playwright/test';
import { blake3 } from 'hash-wasm';

export const enabled = !!process.env.STRATA_E2E_PASSWORD;

export async function login(page: Page) {
  await page.goto('/login');
  await page.getByLabel('Username').fill(process.env.STRATA_E2E_USER ?? 'admin');
  await page.getByLabel('Password').fill(process.env.STRATA_E2E_PASSWORD!);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page).toHaveURL(/\/(\?.*)?$/);
}

export async function newSession(browser: Browser): Promise<{ ctx: BrowserContext; page: Page }> {
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  await login(page);
  return { ctx, page };
}

/** Create or overwrite a text file through the API (as the logged-in page),
 *  the way the editor does: If-Match plus the hash of the current bytes. */
export async function putFile(page: Page, path: string, text: string) {
  const url = `/api/files/content?path=${encodeURIComponent(path)}`;
  const cur = await page.evaluate(async (u) => {
    const r = await fetch(u, { cache: 'no-store' });
    return r.ok ? { etag: r.headers.get('etag')!, bytes: Array.from(new Uint8Array(await r.arrayBuffer())) } : null;
  }, url);
  const pre: Record<string, string> = cur
    ? { 'if-match': cur.etag, 'x-strata-base-hash': await blake3(new Uint8Array(cur.bytes)) }
    : { 'if-none-match': '*' };
  const status = await page.evaluate(
    async ([u, t, p]) => (await fetch(u, { method: 'PUT', headers: { 'x-strata': '1', ...p }, body: t })).status,
    [url, text, pre] as const
  );
  expect(status).toBe(200);
}

export async function readFile(page: Page, path: string): Promise<string> {
  return page.evaluate(async (p) => (await fetch(`/api/files/content?path=${encodeURIComponent(p)}`, { cache: 'no-store' })).text(), path);
}

/** Open a file in the editor from the folder listing. */
export async function openEditor(page: Page, dir: string, name: string) {
  await page.goto(`/?p=${encodeURIComponent(dir)}`);
  const row = page.locator('.entry', { hasText: name });
  await row.getByRole('button', { name: 'Actions' }).click();
  await page.getByRole('button', { name: 'Edit' }).click();
  await expect(page.locator('.cm-content')).toBeVisible();
}

export async function typeAtEnd(page: Page, text: string) {
  await page.locator('.cm-content').click();
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.type(text);
}
