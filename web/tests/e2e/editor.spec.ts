import { expect, test } from '@playwright/test';
import { enabled, newSession, openEditor, putFile, readFile, typeAtEnd } from './helpers';

test.skip(!enabled, 'set STRATA_E2E_URL / STRATA_E2E_USER / STRATA_E2E_PASSWORD');

test('two tabs: the second save gets the conflict dialog and both versions survive', async ({ browser }) => {
  const dir = `e2e-${Date.now()}`;
  const a = await newSession(browser);
  const b = await newSession(browser);
  await putFile(a.page, `${dir}/shared.md`, '# shared\n');

  await openEditor(a.page, dir, 'shared.md');
  await openEditor(b.page, dir, 'shared.md');

  await typeAtEnd(a.page, 'from A\n');
  await a.page.keyboard.press('ControlOrMeta+s');
  await expect(a.page.locator('.status')).toHaveText('Saved');

  await typeAtEnd(b.page, 'from B\n');
  await b.page.keyboard.press('ControlOrMeta+s');
  const dialog = b.page.getByRole('alertdialog', { name: 'Save conflict' });
  await expect(dialog).toBeVisible();

  // the diff view shows both
  await dialog.getByRole('button', { name: 'View differences' }).click();
  await expect(dialog.locator('.cm-mergeView')).toBeVisible();

  await dialog.getByRole('button', { name: 'Keep mine as a copy' }).click();
  await expect(dialog).toBeHidden();
  await expect(b.page.locator('.status')).toContainText('Your version was saved as');

  expect(await readFile(a.page, `${dir}/shared.md`)).toBe('# shared\nfrom A\n');
  const listing = await a.page.evaluate(async (d) => (await (await fetch(`/api/files/list?path=${encodeURIComponent(d)}`)).json()).entries.map((e: { name: string }) => e.name), dir);
  const copy = listing.find((n: string) => n.startsWith('shared (conflict '));
  expect(copy).toBeTruthy();
  expect(await readFile(a.page, `${dir}/${copy}`)).toBe('# shared\nfrom B\n');
  // B's editor now shows A's version
  await expect(b.page.locator('.cm-content')).toContainText('from A');

  await a.ctx.close();
  await b.ctx.close();
});

test('a crashed tab’s draft is offered back and restored', async ({ browser }) => {
  const dir = `e2e-${Date.now()}`;
  const s = await newSession(browser);
  await putFile(s.page, `${dir}/draft.txt`, 'line one\r\nline two\r\n');
  await openEditor(s.page, dir, 'draft.txt');
  await typeAtEnd(s.page, 'unsaved words');
  // wait for the 2-second autosave, then "crash" (close without saving)
  await s.page.waitForTimeout(2600);
  const storage = await s.ctx.storageState({ indexedDB: true });
  await s.ctx.close();

  const ctx = await browser.newContext({ storageState: storage });
  const page = await ctx.newPage();
  await openEditor(page, dir, 'draft.txt');
  await expect(page.getByText('Unsaved changes from')).toBeVisible();
  await page.getByRole('button', { name: 'Restore' }).click();
  await expect(page.locator('.cm-content')).toContainText('unsaved words');
  await page.keyboard.press('ControlOrMeta+s');
  await expect(page.locator('.status')).toHaveText('Saved');
  // CRLF endings preserved, the draft is cleared after saving
  expect(await readFile(page, `${dir}/draft.txt`)).toBe('line one\r\nline two\r\nunsaved words');
  await page.reload();
  await openEditor(page, dir, 'draft.txt');
  await expect(page.getByText('Unsaved changes from')).toBeHidden();
  await ctx.close();
});

test('markdown preview never runs script', async ({ browser }) => {
  const dir = `e2e-${Date.now()}`;
  const s = await newSession(browser);
  await putFile(s.page, `${dir}/x.md`, '# hi\n\n<script>window.__pwned=1</script>\n\n<img src=x onerror="window.__pwned=1">\n\n[a](javascript:window.__pwned=1)\n');
  await openEditor(s.page, dir, 'x.md');
  await expect(s.page.locator('.preview h1')).toHaveText('hi');
  // the javascript: link is not a link at all; nothing in the preview is active
  await expect(s.page.locator('.preview')).toContainText('[a](javascript:');
  await expect(s.page.locator('.preview a, .preview script, .preview img')).toHaveCount(0);
  expect(await s.page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
  await s.ctx.close();
});
