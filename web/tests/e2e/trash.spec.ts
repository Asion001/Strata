import { expect, test } from '@playwright/test';
import { enabled, newSession, putFile, readFile } from './helpers';

test.skip(!enabled, 'set STRATA_E2E_URL / STRATA_E2E_USER / STRATA_E2E_PASSWORD');

test('delete, then Undo brings the file back in place', async ({ browser }) => {
  const { ctx, page } = await newSession(browser);
  const dir = `undo-${Date.now()}`;
  await putFile(page, `${dir}/a.txt`, 'keep me');
  await putFile(page, `${dir}/b.txt`, 'other');
  await page.goto(`/?p=${encodeURIComponent(dir)}`);
  await page.getByRole('button', { name: 'Actions for a.txt', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('moves to Deleted files');
  await dialog.getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(page.getByRole('button', { name: 'a.txt', exact: true })).toBeHidden();

  // Undo restores exactly that file, even from another folder.
  const toast = page.getByRole('status').filter({ hasText: '“a.txt” moved to Deleted files.' });
  await page.getByRole('link', { name: 'Home', exact: true }).click();
  await expect(page).toHaveURL(/\?p=$/);
  await toast.getByRole('button', { name: 'Undo' }).click();
  await expect(page.getByText('“a.txt” restored.')).toBeVisible();
  expect(await readFile(page, `${dir}/a.txt`)).toBe('keep me');
  await page.goto(`/?p=${encodeURIComponent(dir)}`);
  await expect(page.getByRole('button', { name: 'a.txt', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'b.txt', exact: true })).toBeVisible();
  await ctx.close();
});

test('restore from Deleted files, delete permanently and empty', async ({ browser }) => {
  const { ctx, page } = await newSession(browser);
  const dir = `trash-${Date.now()}`;
  await putFile(page, `${dir}/c.txt`, 'restore me');
  await putFile(page, `${dir}/d.txt`, 'gone');
  await putFile(page, `${dir}/e.txt`, 'emptied');
  const del = async (name: string) => {
    await page.goto(`/?p=${encodeURIComponent(dir)}`);
    await page.getByRole('button', { name: `Actions for ${name}`, exact: true }).click();
    await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
    await page.getByRole('dialog').getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(page.getByRole('button', { name, exact: true })).toBeHidden();
  };
  await del('c.txt');
  await del('d.txt');
  await del('e.txt');

  await page.getByRole('link', { name: 'Deleted files' }).click();
  await expect(page).toHaveURL(/\/trash$/);
  await expect(page.getByTestId('retention')).toContainText('days');
  await page.getByRole('button', { name: 'Restore c.txt' }).first().click();
  await expect(page.getByText('“c.txt” restored.')).toBeVisible();
  expect(await readFile(page, `${dir}/c.txt`)).toBe('restore me');

  await page.getByRole('button', { name: 'Delete d.txt permanently' }).first().click();
  await page.getByRole('dialog').getByRole('button', { name: 'Delete permanently' }).click();
  await expect(page.getByText('“d.txt” deleted permanently.')).toBeVisible();

  await page.getByRole('button', { name: 'Empty' }).click();
  await expect(page.getByRole('dialog')).toContainText('will be deleted permanently');
  await page.getByRole('dialog').getByRole('button', { name: 'Empty' }).click();
  await expect(page.getByText(/Deleted files emptied/)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Restore e.txt' })).toHaveCount(0);
  await ctx.close();
});
