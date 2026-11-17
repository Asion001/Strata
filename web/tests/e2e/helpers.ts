import { expect, type Browser, type BrowserContext, type Page } from '@playwright/test';

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

/** Create or overwrite a text file through the API (as the logged-in page). */
export async function putFile(page: Page, path: string, text: string) {
  const r = await page.evaluate(
    async ([p, t]) => {
      const url = `/api/files/content?path=${encodeURIComponent(p)}`;
      const head = await fetch(url, { cache: 'no-store' });
      const pre: Record<string, string> = head.ok ? { 'if-match': head.headers.get('etag')! } : { 'if-none-match': '*' };
      const r = await fetch(url, { method: 'PUT', headers: { 'x-strata': '1', ...pre }, body: t });
      return r.status;
    },
    [path, text]
  );
  expect(r).toBe(200);
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
