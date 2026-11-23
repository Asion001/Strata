import { expect, type Browser, type BrowserContext, type Page } from '@playwright/test';
import { blake3 } from 'hash-wasm';

export const enabled = !!process.env.STRATA_E2E_PASSWORD;
// In CI the tests must run: a missing server password would only skip them.
if (process.env.CI && !enabled) throw new Error('STRATA_E2E_PASSWORD is not set in CI');

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
  await page.getByRole('button', { name: `Actions for ${name}`, exact: true }).click();
  await page.getByRole('menuitem', { name: 'Edit', exact: true }).click();
  await expect(page.locator('.cm-content')).toBeVisible();
}

export async function typeAtEnd(page: Page, text: string) {
  await page.locator('.cm-content').click();
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.type(text);
}

/** A small, valid PDF with one line of Helvetica text per page (ASCII only,
 *  so it can be sent as a string). */
export function makePdf(pageTexts: string[]): string {
  const objs: string[] = [];
  const n = pageTexts.length;
  // 1: catalog, 2: pages, 3: font, then per page: page + content stream
  objs.push('<< /Type /Catalog /Pages 2 0 R >>');
  const kids = pageTexts.map((_, i) => `${4 + i * 2} 0 R`).join(' ');
  objs.push(`<< /Type /Pages /Kids [${kids}] /Count ${n} >>`);
  objs.push('<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>');
  pageTexts.forEach((t, i) => {
    const stream = `BT /F1 24 Tf 72 700 Td (${t.replace(/[()\\]/g, '\\$&')}) Tj ET`;
    objs.push(`<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents ${5 + i * 2} 0 R >>`);
    objs.push(`<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`);
  });
  let out = '%PDF-1.4\n';
  const offsets: number[] = [];
  objs.forEach((o, i) => {
    offsets.push(out.length);
    out += `${i + 1} 0 obj\n${o}\nendobj\n`;
  });
  const xref = out.length;
  out += `xref\n0 ${objs.length + 1}\n0000000000 65535 f \n`;
  for (const o of offsets) out += `${String(o).padStart(10, '0')} 00000 n \n`;
  out += `trailer\n<< /Size ${objs.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return out;
}

/** Open a file in the preview viewer from the folder listing. */
export async function openPreview(page: Page, dir: string, name: string) {
  await page.goto(`/?p=${encodeURIComponent(dir)}`);
  // The row's name button opens the file (the row itself also selects).
  await page.getByRole('row').filter({ hasText: name }).getByRole('button', { name, exact: true }).click();
}
