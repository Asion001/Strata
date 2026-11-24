// Sign-out halts the queue for the rest of the page's life (it reloads to
// the sign-in page next): its own module instance here.
import 'fake-indexeddb/auto';
import { describe, expect, it } from 'vitest';
import * as q from './queue';

describe('halt', () => {
  it('stops what runs and runs nothing more', async () => {
    const add = async (name: string) => {
      const j = q.newJob(1, name, new Blob(['abc']));
      await q.keepBytes(j.id, j.blob);
      await q.save(j);
    };
    await add('a.txt');
    await add('b.txt');
    let entered!: () => void;
    const inside = new Promise<void>((r) => (entered = r));
    const calls: string[] = [];
    const fetch = (async (u: string, i: RequestInit = {}) => {
      calls.push(u);
      entered();
      // (Until aborted.)
      await new Promise((_, reject) => i.signal?.addEventListener('abort', () => reject(Object.assign(new Error('aborted'), { name: 'AbortError' }))));
      return Response.json({});
    }) as unknown as typeof globalThis.fetch;
    const deps = { fetch, chunk: async () => [{ offset: 0, size: 3, hash: 'aa' }], sleep: async () => {} };
    const drained = q.drain(1, deps);
    await inside;
    q.halt();
    expect(await drained).toBe('done');
    expect(calls).toEqual(['/api/uploads']); // (b.txt never started)
    expect(await q.drain(1, deps)).toBe('done');
    expect(calls).toHaveLength(1);
  });
});
