// The queue with no IndexedDB at all (site data blocked): it runs in memory.
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';

beforeAll(() => {
  vi.stubGlobal('indexedDB', {
    open() {
      throw new DOMException('blocked', 'SecurityError');
    }
  });
});
afterAll(() => vi.unstubAllGlobals());

describe('queue without IndexedDB', () => {
  it('queues, runs, cancels and forgets jobs in memory', async () => {
    const q = await import('./queue');
    const chunks = [{ offset: 0, size: 3, hash: 'aa' }];
    const puts: string[] = [];
    const fetch = (async (u: string, i: RequestInit = {}) => {
      if (u === '/api/uploads') return Response.json({ id: 's1', missing: [0] });
      if (i.method === 'PUT') puts.push(u);
      return Response.json({});
    }) as unknown as typeof globalThis.fetch;
    const deps = { fetch, chunk: async () => chunks, sleep: async () => {} };

    const a = q.newJob(1, 'a.txt', new Blob(['abc']));
    expect(await q.keepBytes(a.id, a.blob)).toBe('memory');
    await q.save(a);
    expect((await q.jobs(1)).map((j) => j.id)).toEqual([a.id]);
    expect(await q.jobs(2)).toEqual([]);
    expect(await q.hasBytes(a.id)).toBe(true);
    expect(await q.drain(1, deps)).toBe('done');
    expect(puts).toEqual(['/api/uploads/s1/chunks/aa']);
    expect(await q.load(a.id)).toBeUndefined(); // done: removed

    // A job whose bytes are gone (no page memory) fails, and says why.
    const b = q.newJob(1, 'b.txt', new Blob(['b']));
    await q.save(b);
    expect((await q.load(b.id))!).toMatchObject({ state: 'failed', error: q.GONE });
    // Not written back once removed (cancelled).
    expect(await q.exists(b.id)).toBe(true);
    expect(await q.hasBytes(b.id)).toBe(false);
    expect(await q.cancel(b.id, { fetch })).toBe(true);
    await q.save(b);
    await q.remove(b.id);
    expect(await q.exists(b.id)).toBe(false);
    expect(await q.update(b)).toBe(false);
  });
});
