import 'fake-indexeddb/auto';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { db, DB, tx, wipe } from './idb';

afterEach(() => vi.restoreAllMocks());

describe('offline database', () => {
  it('lets go when another context upgrades or deletes it', async () => {
    const d = await db();
    await tx('jobs', 'readwrite', (s) => void s.put({ id: 'a', user: 1 }));
    // Another tab signs out: its delete must not be blocked by us.
    await new Promise<void>((resolve, reject) => {
      const req = indexedDB.deleteDatabase(DB);
      req.onsuccess = () => resolve();
      req.onerror = () => reject(req.error);
      req.onblocked = () => reject(new Error('blocked'));
    });
    // A fresh connection after that (the old one closed itself).
    expect(await db()).not.toBe(d);
    await wipe();
  });

  it('a failed open is reported, and tried again next time', async () => {
    await wipe();
    const real = indexedDB.open.bind(indexedDB);
    const spy = vi.spyOn(indexedDB, 'open').mockImplementationOnce(() => {
      const req = {} as IDBOpenDBRequest;
      queueMicrotask(() => {
        Object.defineProperty(req, 'error', { value: new Error('denied') });
        req.onerror?.(new Event('error'));
      });
      return req;
    });
    await expect(db()).rejects.toThrow('denied');
    // (Signing out right after a failed open still works.)
    spy.mockImplementationOnce(() => {
      const req = {} as IDBOpenDBRequest;
      queueMicrotask(() => {
        Object.defineProperty(req, 'error', { value: new Error('denied') });
        req.onerror?.(new Event('error'));
      });
      return req;
    });
    const pending = db().catch(() => null);
    await wipe();
    await pending;
    spy.mockImplementation(real);
    await expect(db()).resolves.toBeTruthy();
    await wipe();
  });

  it('a failed transaction rejects', async () => {
    await expect(
      tx('jobs', 'readwrite', (s) => {
        s.add({ id: 'dup', user: 1 });
        s.add({ id: 'dup', user: 1 }); // a constraint error aborts it
      })
    ).rejects.toMatchObject({ name: 'ConstraintError' });
    await expect(tx('jobs', 'readwrite', (s) => s.transaction.abort())).rejects.toThrow('transaction aborted');
    await wipe();
  });
});
