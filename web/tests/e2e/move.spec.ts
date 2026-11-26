import { expect, test } from '@playwright/test';
import { enabled, newSession, putFile, readFile } from './helpers';

test.skip(!enabled, 'set STRATA_E2E_URL / STRATA_E2E_USER / STRATA_E2E_PASSWORD');

test('move a file into a nested folder with the picker, no typing', async ({ browser }) => {
  const { ctx, page } = await newSession(browser);
  const dir = `pick-${Date.now()}`;
  await putFile(page, `${dir}/a.txt`, 'moved');
  await putFile(page, `${dir}/to/deep/keep.txt`, 'x');
  await page.goto(`/?p=${encodeURIComponent(dir)}`);
  await page.getByRole('button', { name: 'Actions for a.txt', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Move…', exact: true }).click();

  const picker = page.getByRole('dialog');
  await expect(picker).toContainText('The items are already here.');
  await expect(picker.getByRole('button', { name: 'Move here' })).toBeDisabled();
  await picker.getByRole('button', { name: 'to', exact: true }).click();
  await picker.getByRole('button', { name: 'deep', exact: true }).click();
  await expect(picker.getByRole('navigation', { name: 'Folder path' })).toContainText('deep');
  await picker.getByRole('button', { name: 'Move here' }).click();

  await expect(page.getByRole('button', { name: 'a.txt', exact: true })).toBeHidden();
  expect(await readFile(page, `${dir}/to/deep/a.txt`)).toBe('moved');
  await ctx.close();
});

test('copy a selection into a folder made in the picker', async ({ browser }) => {
  const { ctx, page } = await newSession(browser);
  const dir = `pickcopy-${Date.now()}`;
  await putFile(page, `${dir}/b.txt`, 'one');
  await putFile(page, `${dir}/c.txt`, 'two');
  await putFile(page, `${dir}/sub/d.txt`, 'three');
  await page.goto(`/?p=${encodeURIComponent(dir)}`);
  for (const n of ['b.txt', 'c.txt', 'sub']) await page.getByRole('checkbox', { name: `Select ${n}`, exact: true }).check();
  await page.getByRole('button', { name: 'Copy', exact: true }).click();

  const picker = page.getByRole('dialog');
  // A folder being copied can't be the target.
  await expect(picker.getByRole('button', { name: /^sub/ })).toHaveAttribute('aria-disabled', 'true');
  await picker.getByRole('button', { name: 'New folder' }).click();
  await picker.getByLabel('New folder name').fill('copies');
  await picker.getByLabel('New folder name').press('Enter');
  await expect(picker.locator('[aria-current]')).toHaveText('copies');
  await picker.getByRole('button', { name: 'Copy here' }).click();
  await expect(picker).toBeHidden();

  await expect(page.getByText('3 items copied.')).toBeVisible();
  expect(await readFile(page, `${dir}/copies/b.txt`)).toBe('one');
  expect(await readFile(page, `${dir}/copies/c.txt`)).toBe('two');
  expect(await readFile(page, `${dir}/copies/sub/d.txt`)).toBe('three');
  // The originals stay.
  expect(await readFile(page, `${dir}/b.txt`)).toBe('one');
  await ctx.close();
});
