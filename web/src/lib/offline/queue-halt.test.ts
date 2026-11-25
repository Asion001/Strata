// Sign-out halts the queue for the rest of the page's life (it reloads to
// the sign-in page next): its own module instance here.
import 'fake-indexeddb/auto';
import { describe, expect, it, vi } from 'vitest';
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
    // Nothing more is stored after a sign-out.
    await expect(q.keepBytes('late', new Blob(['x']))).rejects.toMatchObject({ name: 'AbortError' });
  });
});

describe('halt while bytes are being stored', () => {
  it('what was being written as the sign-out began goes again', async () => {
    vi.resetModules(); // (a queue that wasn't halted yet)
    const fresh = await import('./queue');
    const { tx } = await import('./idb');
    const realPut = IDBObjectStore.prototype.put;
    let once = true;
    // Halt right as the put happens (the sign-out in the middle of it).
    const spy = vi.spyOn(IDBObjectStore.prototype, 'put').mockImplementation(function (this: IDBObjectStore, v: unknown, k?: IDBValidKey) {
      if (once && k === 'mid') {
        once = false;
        fresh.halt();
      }
      return realPut.call(this, v, k);
    });
    await expect(fresh.keepBytes('mid', new Blob(['x']))).rejects.toMatchObject({ name: 'AbortError' });
    expect(await tx('blobs', 'readonly', (s) => s.count('mid'))).toBe(0);
    spy.mockRestore();
  });
});

describe('halt while bytes are being stored as buffers', () => {
  it('those go again too', async () => {
    vi.resetModules();
    const fresh = await import('./queue');
    const { tx } = await import('./idb');
    const realPut = IDBObjectStore.prototype.put;
    const spy = vi.spyOn(IDBObjectStore.prototype, 'put').mockImplementation(function (this: IDBObjectStore, v: unknown, k?: IDBValidKey) {
      if (v instanceof Blob) throw new DOMException('no blobs here', 'UnknownError');
      if (k === 'buf') fresh.halt();
      return realPut.call(this, v, k);
    });
    await expect(fresh.keepBytes('buf', new Blob(['x']))).rejects.toMatchObject({ name: 'AbortError' });
    expect(await tx('blobs', 'readonly', (s) => s.count('buf'))).toBe(0);
    spy.mockRestore();
  });
});

describe('halt while only a memory mark is being written', () => {
  it('is reported as signed out', async () => {
    vi.resetModules();
    const fresh = await import('./queue');
    const realPut = IDBObjectStore.prototype.put;
    const spy = vi.spyOn(IDBObjectStore.prototype, 'put').mockImplementation(function (this: IDBObjectStore, v: unknown, k?: IDBValidKey) {
      if (v instanceof Blob || (v && typeof v === 'object' && 'parts' in v)) throw new DOMException('no room', 'QuotaExceededError');
      if (k === 'mem') fresh.halt();
      return realPut.call(this, v, k);
    });
    try {
      await expect(fresh.keepBytes('mem', new Blob(['x']))).rejects.toMatchObject({ name: 'AbortError' });
    } finally {
      spy.mockRestore();
    }
  });
});
