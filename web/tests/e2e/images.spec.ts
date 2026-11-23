import { deflateSync, crc32 } from 'node:zlib';
import { expect, test, type Page } from '@playwright/test';
import { enabled, newSession, openPreview, putBytes } from './helpers';

test.skip(!enabled, 'set STRATA_E2E_URL / STRATA_E2E_USER / STRATA_E2E_PASSWORD');

/** A solid-colour PNG. */
function png(w: number, h: number, [r, g, b]: [number, number, number]): Uint8Array {
  const chunk = (type: string, data: Buffer) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length);
    const td = Buffer.concat([Buffer.from(type, 'latin1'), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(td));
    return Buffer.concat([len, td, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr.set([8, 2, 0, 0, 0], 8); // 8-bit RGB
  const row = Buffer.alloc(1 + w * 3);
  for (let x = 0; x < w; x++) row.set([r, g, b], 1 + x * 3);
  const raw = Buffer.concat(Array.from({ length: h }, () => row));
  return new Uint8Array(
    Buffer.concat([
      Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
      chunk('IHDR', ihdr),
      chunk('IDAT', deflateSync(raw)),
      chunk('IEND', Buffer.alloc(0))
    ])
  );
}

const viewer = (page: Page) => page.getByRole('dialog', { name: /^Preview of / });
const scale = (page: Page) =>
  page.locator('[data-testid="viewer-image"]').evaluate((img) => {
    const m = getComputedStyle(img.parentElement!).transform;
    return m === 'none' ? 1 : new DOMMatrix(m).a;
  });

async function folder(page: Page) {
  const dir = `img-${Date.now()}`;
  await putBytes(page, `${dir}/a.png`, png(320, 200, [200, 30, 30]));
  await putBytes(page, `${dir}/b.png`, png(200, 320, [30, 200, 30]));
  await putBytes(page, `${dir}/c.png`, png(240, 240, [30, 30, 200]));
  return dir;
}

test('open, keyboard through the folder, close with Escape and with Back', async ({ browser }) => {
  const { ctx, page } = await newSession(browser);
  const dir = await folder(page);
  await openPreview(page, dir, 'a.png');
  await expect(viewer(page)).toHaveAccessibleName('Preview of a.png');
  await expect(page.getByRole('img', { name: 'a.png' })).toBeVisible();
  // Focus is inside the overlay.
  expect(await viewer(page).evaluate((d) => d.contains(document.activeElement))).toBe(true);

  await page.keyboard.press('ArrowRight');
  await expect(viewer(page)).toHaveAccessibleName('Preview of b.png');
  await page.keyboard.press('ArrowRight');
  await expect(viewer(page)).toHaveAccessibleName('Preview of c.png');
  await page.keyboard.press('ArrowLeft');
  await expect(viewer(page)).toHaveAccessibleName('Preview of b.png');

  // Tab never leaves the overlay.
  for (let i = 0; i < 15; i++) {
    await page.keyboard.press('Tab');
    expect(await viewer(page).evaluate((d) => d.contains(document.activeElement))).toBe(true);
  }

  await page.keyboard.press('Escape');
  await expect(viewer(page)).toBeHidden();
  await expect(page).toHaveURL(new RegExp(`p=${dir}`));

  // Back closes it too, and leaves you in the folder.
  await openPreview(page, dir, 'c.png');
  await expect(viewer(page)).toBeVisible();
  await page.goBack();
  await expect(viewer(page)).toBeHidden();
  await expect(page).toHaveURL(new RegExp(`p=${dir}`));
  await ctx.close();
});

test('swipe moves between images; double tap and pinch zoom', async ({ browser }) => {
  const { ctx, page } = await newSession(browser);
  const dir = await folder(page);
  await openPreview(page, dir, 'a.png');
  const stage = page.getByRole('group', { name: /^a\.png, zoom/ });
  const box = (await stage.boundingBox())!;
  const cy = box.y + box.height / 2;

  // A swipe left (pointer drag) goes to the next image.
  await page.mouse.move(box.x + box.width * 0.7, cy);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width * 0.3, cy, { steps: 8 });
  await page.mouse.up();
  await expect(viewer(page)).toHaveAccessibleName('Preview of b.png');

  // Double click zooms in, again zooms out.
  await expect(page.locator('[data-testid="viewer-image"]')).toBeVisible();
  await page.mouse.dblclick(box.x + box.width / 2, cy);
  await expect.poll(() => scale(page)).toBeGreaterThan(2);
  // Zoomed in, a drag pans instead of moving on.
  await page.mouse.move(box.x + box.width / 2, cy);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 - 150, cy, { steps: 8 });
  await page.mouse.up();
  await expect(viewer(page)).toHaveAccessibleName('Preview of b.png');
  await page.mouse.dblclick(box.x + box.width / 2, cy);
  await expect.poll(() => scale(page)).toBe(1);

  // Keys and buttons: +, 0, Zoom in.
  await page.keyboard.press('+');
  await expect.poll(() => scale(page)).toBeGreaterThan(1);
  await page.keyboard.press('0');
  await expect.poll(() => scale(page)).toBe(1);
  await page.getByRole('button', { name: 'Zoom in' }).click();
  await expect.poll(() => scale(page)).toBeGreaterThan(1);
  await page.keyboard.press('0');

  // A two-finger pinch (real touch events, through the DevTools protocol).
  const cdp = await ctx.newCDPSession(page);
  const cx = box.x + box.width / 2;
  const touch = (type: string, d: number) =>
    cdp.send('Input.dispatchTouchEvent', {
      type,
      touchPoints: type === 'touchEnd' ? [] : [
        { x: cx - d, y: cy, id: 1 },
        { x: cx + d, y: cy, id: 2 }
      ]
    });
  await touch('touchStart', 40);
  for (const d of [60, 90, 120, 150]) await touch('touchMove', d);
  await touch('touchEnd', 0);
  await expect.poll(() => scale(page)).toBeGreaterThan(2.5);

  await page.getByRole('button', { name: 'Close' }).click();
  await expect(viewer(page)).toBeHidden();
  await ctx.close();
});

test('the details panel shows size and dimensions, with download, share and delete', async ({ browser }) => {
  const { ctx, page } = await newSession(browser);
  const dir = await folder(page);
  await openPreview(page, dir, 'b.png');
  await page.getByRole('button', { name: 'Details' }).click();
  const info = page.getByRole('complementary', { name: 'Image details' });
  await expect(info).toContainText('200 × 320');
  await expect(info.getByRole('link', { name: 'Download' })).toBeVisible();
  await expect(info.getByRole('button', { name: 'Share…' })).toBeVisible();

  // Delete moves on to the next image.
  await info.getByRole('button', { name: 'Delete…' }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(viewer(page)).toHaveAccessibleName('Preview of c.png');
  await page.keyboard.press('Escape'); // the panel first
  await page.keyboard.press('Escape');
  await expect(viewer(page)).toBeHidden();
  await expect(page.getByRole('button', { name: 'b.png', exact: true })).toHaveCount(0);
  await ctx.close();
});

test('in a folder of 5,000 images the thumbnail shows within 300 ms (warm cache)', async ({ browser }) => {
  test.setTimeout(300_000);
  const { ctx, page } = await newSession(browser);
  const dir = `many-${Date.now()}`;
  const bytes = Buffer.from(png(64, 48, [120, 90, 200])).toString('base64');
  // Uploaded from the page, many at a time.
  const failed = await page.evaluate(
    async ([dir, b64]) => {
      const body = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      let next = 0, bad = 0;
      const worker = async () => {
        while (next < 5000) {
          const i = next++;
          const name = `img-${String(i).padStart(4, '0')}.png`;
          const r = await fetch(`/api/files/content?path=${encodeURIComponent(`${dir}/${name}`)}`, {
            method: 'PUT',
            headers: { 'x-strata': '1', 'if-none-match': '*' },
            body
          });
          if (!r.ok) bad++;
        }
      };
      await Promise.all(Array.from({ length: 24 }, worker));
      return bad;
    },
    [dir, bytes] as const
  );
  expect(failed).toBe(0);

  await page.goto(`/?p=${encodeURIComponent(dir)}`);
  const row = (n: string) => page.getByRole('row').filter({ hasText: n }).getByRole('button', { name: n, exact: true });
  await expect(row('img-0001.png')).toBeVisible();
  // Warm up: the viewer's code loaded once, and the list's thumbnails in.
  await row('img-0000.png').click();
  await expect(viewer(page)).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(viewer(page)).toBeHidden();
  await page.waitForFunction(() =>
    [...document.querySelectorAll<HTMLImageElement>('img[src*="/api/files/thumb"]')].slice(0, 5).every((i) => i.complete && i.naturalWidth > 0)
  );

  const ms = await page.evaluate(
    (name) =>
      new Promise<number>((resolve) => {
        const btn = [...document.querySelectorAll('button')].find((b) => b.textContent?.trim() === name)!;
        const t0 = performance.now();
        const done = () => resolve(performance.now() - t0);
        const check = () => {
          const img = document.querySelector<HTMLImageElement>('[data-testid="viewer-thumb"]');
          if (!img) return false;
          mo.disconnect();
          if (img.complete && img.naturalWidth > 0) done();
          else img.addEventListener('load', done, { once: true });
          return true;
        };
        const mo = new MutationObserver(check);
        mo.observe(document.body, { subtree: true, childList: true });
        btn.click();
      }),
    'img-0003.png'
  );
  console.log(`thumbnail shown after ${ms.toFixed(0)} ms`);
  expect(ms).toBeLessThan(300);
  await expect(viewer(page)).toHaveAccessibleName('Preview of img-0003.png');
  await ctx.close();
});
