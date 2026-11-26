import 'fake-indexeddb/auto';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { MemoryCacheStorage } from '../testing/caches';
import { META, USER_KEY } from './keys';

const ls = new Map<string, string>();
let store: MemoryCacheStorage;

beforeEach(() => {
  vi.resetModules();
  store = new MemoryCacheStorage();
  vi.stubGlobal('caches', store);
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => ls.get(k) ?? null,
    setItem: (k: string, v: string) => void ls.set(k, v),
    removeItem: (k: string) => void ls.delete(k),
    key: () => null,
    length: 0
  });
  ls.clear();
});

const recorded = async () => {
  const r = await (await caches.open(META)).match(USER_KEY);
  return r ? Number(await r.text()) : null;
};
const user = { id: 1, username: 'a', is_admin: false };

describe('userRecorded', () => {
  it('writes the record again when it went missing for the signed-in user (issue #29)', async () => {
    const app = await import('./app.svelte');
    await app.signedIn(user);
    expect(await recorded()).toBe(1);
    await caches.delete(META); // (what WebKit sometimes shows after quick reloads)
    await app.userRecorded(1);
    expect(await recorded()).toBe(1);
  });

  it('never brings a user back after a sign-out', async () => {
    const app = await import('./app.svelte');
    await app.signedIn(user);
    ls.delete('strata-last-user'); // (signed out, here or in another tab)
    await caches.delete(META);
    await app.userRecorded(1);
    expect(await recorded()).toBe(null);
  });

  it('leaves another user who signed in since alone', async () => {
    const app = await import('./app.svelte');
    await app.signedIn(user);
    await app.signedIn({ id: 2, username: 'b', is_admin: false });
    await app.userRecorded(1);
    expect(await recorded()).toBe(2);
  });

  it('takes the record back when the sign-out lands while it is written', async () => {
    const app = await import('./app.svelte');
    await app.signedIn(user);
    await caches.delete(META);
    // The sign-out removes the last user just as the record is rewritten.
    const open = store.open.bind(store);
    vi.spyOn(store, 'open').mockImplementation(async (name: string) => {
      const c = await open(name);
      const put = c.put.bind(c);
      c.put = async (k, v) => {
        await put(k, v);
        ls.delete('strata-last-user');
      };
      return c;
    });
    await app.userRecorded(1);
    expect(await recorded()).toBe(null);
  });
});
