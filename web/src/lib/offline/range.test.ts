import { describe, expect, it } from 'vitest';
import { ranged } from './range';

const file = () => new Response(new Uint8Array([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]), { headers: { 'content-type': 'video/mp4', 'content-security-policy': 'sandbox' } });
const bytes = async (r: Response) => [...new Uint8Array(await r.arrayBuffer())];

describe('ranged (offline copies)', () => {
  it('no Range: the stored response as it is', async () => {
    const r = file();
    expect(await ranged(r, null)).toBe(r);
  });
  it('bytes=a-b, a- and -n give 206 with Content-Range, keeping the headers', async () => {
    let r = await ranged(file(), 'bytes=2-4');
    expect([r.status, r.headers.get('content-range'), await bytes(r)]).toEqual([206, 'bytes 2-4/10', [2, 3, 4]]);
    expect(r.headers.get('content-security-policy')).toBe('sandbox');
    r = await ranged(file(), 'bytes=7-');
    expect([r.headers.get('content-range'), await bytes(r)]).toEqual(['bytes 7-9/10', [7, 8, 9]]);
    r = await ranged(file(), 'bytes=-3');
    expect(await bytes(r)).toEqual([7, 8, 9]);
    r = await ranged(file(), 'bytes=-30');
    expect(r.headers.get('content-range')).toBe('bytes 0-9/10');
    r = await ranged(file(), 'bytes=8-100');
    expect(r.headers.get('content-range')).toBe('bytes 8-9/10');
  });
  it('past the end: 416', async () => {
    for (const h of ['bytes=10-', 'bytes=-0']) {
      const r = await ranged(file(), h);
      expect([r.status, r.headers.get('content-range')]).toEqual([416, 'bytes */10']);
    }
    const empty = await ranged(new Response(new Uint8Array(0)), 'bytes=-5');
    expect(empty.status).toBe(416);
  });
  it('anything else: the whole file (200)', async () => {
    for (const h of ['bytes=0-1,4-5', 'items=0-1', 'bytes=-', 'bytes=5-2']) {
      const r = await ranged(file(), h);
      expect([r.status, (await bytes(r)).length]).toEqual([200, 10]);
    }
  });
});
