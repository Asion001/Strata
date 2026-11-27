import { expect, test } from '@playwright/test';
import { enabled, newSession, putFile, readFile } from './helpers';

test.skip(!enabled, 'set STRATA_E2E_URL / STRATA_E2E_USER / STRATA_E2E_PASSWORD');

test('an upload onto a taken name asks first: keep both, then replace', async ({ browser }) => {
  const { ctx, page } = await newSession(browser);
  const dir = `clash-${Date.now()}`;
  await putFile(page, `${dir}/notes.txt`, 'old');
  await page.goto(`/?p=${encodeURIComponent(dir)}`);
  await expect(page.getByRole('button', { name: 'notes.txt', exact: true })).toBeVisible();

  const send = (text: string) => page.locator('input[type=file][multiple]').setInputFiles({ name: 'notes.txt', mimeType: 'text/plain', buffer: Buffer.from(text) });
  await send('second');
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('A file named “notes.txt” is already there');
  await expect(dialog.getByTestId('conflict-sides')).toContainText('3 B');
  // Nothing was replaced while it asks.
  expect(await readFile(page, `${dir}/notes.txt`)).toBe('old');
  await dialog.getByRole('button', { name: 'Keep both' }).click();
  await expect(page.getByRole('button', { name: 'notes (2).txt', exact: true })).toBeVisible();
  expect(await readFile(page, `${dir}/notes (2).txt`)).toBe('second');

  await send('third');
  await dialog.getByRole('button', { name: 'Replace' }).click();
  await expect.poll(() => readFile(page, `${dir}/notes.txt`)).toBe('third');
  await ctx.close();
});

test('an upload left undecided waits in the panel', async ({ browser }) => {
  const { ctx, page } = await newSession(browser);
  const dir = `clash-wait-${Date.now()}`;
  await putFile(page, `${dir}/a.txt`, 'old');
  await page.goto(`/?p=${encodeURIComponent(dir)}`);
  await page.locator('input[type=file][multiple]').setInputFiles({ name: 'a.txt', mimeType: 'text/plain', buffer: Buffer.from('new') });
  await page.getByRole('dialog').press('Escape');
  const waiting = page.getByTestId('upload-decide');
  await expect(waiting).toContainText('already there');
  await waiting.getByRole('button', { name: 'Replace' }).click();
  await expect.poll(() => readFile(page, `${dir}/a.txt`)).toBe('new');
  await ctx.close();
});

test('moving several onto taken names asks once with "apply to all"', async ({ browser }) => {
  const { ctx, page } = await newSession(browser);
  const dir = `clash-move-${Date.now()}`;
  for (const n of ['1', '2', '3']) {
    await putFile(page, `${dir}/${n}.txt`, `new ${n}`);
    await putFile(page, `${dir}/to/${n}.txt`, `old ${n}`);
  }
  await page.goto(`/?p=${encodeURIComponent(dir)}`);
  for (const n of ['1', '2', '3']) await page.getByRole('checkbox', { name: `Select ${n}.txt`, exact: true }).check();
  await page.getByRole('button', { name: 'Move', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('button', { name: 'to', exact: true }).click();
  await dialog.getByRole('button', { name: 'Move here' }).click();

  await expect(dialog).toContainText('A file named “1.txt” is already there');
  await dialog.getByLabel('Apply to the other 2 conflicts').check();
  await dialog.getByRole('button', { name: 'Replace' }).click();
  await expect(page.getByText('3 items moved.')).toBeVisible();
  for (const n of ['1', '2', '3']) expect(await readFile(page, `${dir}/to/${n}.txt`)).toBe(`new ${n}`);
  await ctx.close();
});

test('a folder onto a folder merges; Duplicate keeps both', async ({ browser }) => {
  const { ctx, page } = await newSession(browser);
  const dir = `clash-merge-${Date.now()}`;
  await putFile(page, `${dir}/d/new.txt`, 'n');
  await putFile(page, `${dir}/to/d/old.txt`, 'o');
  await page.goto(`/?p=${encodeURIComponent(dir)}`);
  await page.getByRole('button', { name: 'Actions for d', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Move…', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('button', { name: 'to', exact: true }).click();
  await dialog.getByRole('button', { name: 'Move here' }).click();
  await dialog.getByRole('button', { name: 'Merge' }).click();
  await expect(page.getByRole('button', { name: 'd', exact: true })).toBeHidden();
  expect(await readFile(page, `${dir}/to/d/new.txt`)).toBe('n');
  expect(await readFile(page, `${dir}/to/d/old.txt`)).toBe('o');

  await page.goto(`/?p=${encodeURIComponent(`${dir}/to/d`)}`);
  await page.getByRole('button', { name: 'Actions for new.txt', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Duplicate', exact: true }).click();
  await expect(page.getByText('Duplicated as “new (2).txt”.')).toBeVisible();
  expect(await readFile(page, `${dir}/to/d/new (2).txt`)).toBe('n');
  await ctx.close();
});
