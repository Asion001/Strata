import { expect, test } from '@playwright/test';
import { enabled, makePdf, newSession, openPreview, putFile } from './helpers';

test.skip(!enabled, 'set STRATA_E2E_URL / STRATA_E2E_USER / STRATA_E2E_PASSWORD');

test('a PDF opens, finds a word and jumps to a page', async ({ browser }) => {
  const dir = `e2e-${Date.now()}`;
  const s = await newSession(browser);
  const errors: string[] = [];
  s.page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
  s.page.on('pageerror', (e) => errors.push(e.message));
  const texts = Array.from({ length: 12 }, (_, i) => `Page ${i + 1} text`);
  texts[8] = 'Page 9 has the zebra';
  await putFile(s.page, `${dir}/doc.pdf`, makePdf(texts));

  await openPreview(s.page, dir, 'doc.pdf');
  const pdf = s.page.locator('.pdf');
  await expect(pdf.getByText('/ 12')).toBeVisible();
  // the first page's text layer is rendered
  await expect(pdf.locator('.page[data-page-number="1"] .textLayer')).toContainText('Page 1 text');
  // thumbnails, one per page
  if (await pdf.locator('.thumbs').isHidden()) await pdf.getByTitle('Thumbnails').click();
  await expect(pdf.locator('.thumbs button')).toHaveCount(12);

  // find in document: jumps to page 9 and highlights the match
  await pdf.getByLabel('Find in document').fill('zebra');
  await expect(pdf.locator('.count')).toHaveText('1/1');
  await expect(pdf.getByLabel('Page number')).toHaveValue('9');
  await expect(pdf.locator('.page[data-page-number="9"] .textLayer .highlight')).toBeVisible();

  // jump to a page by number
  await pdf.getByLabel('Page number').fill('4');
  await pdf.getByLabel('Page number').press('Enter');
  await expect(pdf.locator('.page[data-page-number="4"] .textLayer')).toContainText('Page 4 text');
  await expect(pdf.getByLabel('Page number')).toHaveValue('4');

  // no CSP violations or script errors along the way
  expect(errors.filter((e) => /Content Security Policy|Refused to/i.test(e))).toEqual([]);
  await s.ctx.close();
});
