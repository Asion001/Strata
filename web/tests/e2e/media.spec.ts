import { readFileSync } from 'node:fs';
import { expect, test, type Page } from '@playwright/test';
import { enabled, newSession, openPreview, putBytes, rawLocalMount } from './helpers';

test.skip(!enabled, 'set STRATA_E2E_URL / STRATA_E2E_USER / STRATA_E2E_PASSWORD');

const clip = new Uint8Array(readFileSync(new URL('./fixtures/clip.mp4', import.meta.url)));

/** The open player: wait for metadata, seek, and check it plays from there. */
async function playAndSeek(page: Page) {
  const video = page.locator('video');
  await expect(video).toBeVisible();
  // Only metadata up front (the rest streams in ranges as it plays).
  await expect(video).toHaveAttribute('preload', 'metadata');
  const at = await video.evaluate(async (v: HTMLVideoElement) => {
    if (v.readyState < 1) await new Promise((r) => v.addEventListener('loadedmetadata', r, { once: true }));
    v.muted = true;
    v.currentTime = 12;
    await new Promise((r) => v.addEventListener('seeked', r, { once: true }));
    const start = v.currentTime;
    // Not awaiting play(): some WebKit builds settle it late. Wait until
    // playback has moved on from the seek instead.
    void v.play().catch(() => {});
    await new Promise<void>((r) => {
      const tick = () => {
        if (v.currentTime > start + 0.5) {
          v.removeEventListener('timeupdate', tick);
          r();
        }
      };
      v.addEventListener('timeupdate', tick);
    });
    v.pause();
    return { duration: v.duration, start, now: v.currentTime };
  });
  expect(at.duration).toBeGreaterThan(29);
  expect(at.start).toBeGreaterThanOrEqual(11.5);
  expect(at.now).toBeGreaterThan(at.start); // it played on from the seek
  expect(at.now).toBeLessThan(at.start + 10); // ... and didn't jump to the end
  return at.now;
}

for (const mode of ['hashed', 'raw'] as const) {
  test(`an MP4 plays, seeks and resumes (${mode} mount)`, async ({ browser }) => {
    const s = await newSession(browser);
    const dir = mode === 'raw' ? `raw-${Date.now()}` : `e2e-${Date.now()}`;
    if (mode === 'raw') await rawLocalMount(s.page, dir);
    await putBytes(s.page, `${dir}/clip.mp4`, clip);

    await openPreview(s.page, dir, 'clip.mp4');
    const stoppedAt = await playAndSeek(s.page);

    // Reopened: playback resumes where it stopped.
    await s.page.keyboard.press('Escape');
    await openPreview(s.page, dir, 'clip.mp4');
    const resumed = await s.page.locator('video').evaluate(async (v: HTMLVideoElement) => {
      if (v.readyState < 1) await new Promise((r) => v.addEventListener('loadedmetadata', r, { once: true }));
      await new Promise((r) => setTimeout(r, 100));
      return v.currentTime;
    });
    expect(Math.abs(resumed - stoppedAt)).toBeLessThan(1);
    await s.ctx.close();
  });
}
