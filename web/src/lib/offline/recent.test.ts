import 'fake-indexeddb/auto';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MemoryCacheStorage } from '../testing/caches';
import * as recent from './recent';
import { MB } from './lru';
import { tx } from './idb';
import { LAST_USER, META, USER_KEY } from './keys';

/** Who signed in on this device (as the app records it). */
const signIn = async (user: number) => {
  ls.set(LAST_USER, JSON.stringify({ id: user, username: 'u', is_admin: false }));
  await (await caches.open(META)).put(USER_KEY, new Response(String(user)));
};
/** A sign-out, as the app does it: the last user first, then the record. */
const signOut = async () => {
  ls.delete(LAST_USER);
  await caches.delete(META);
};

let store: MemoryCacheStorage;
const ls = new Map<string, string>();
let served: Record<string, { body: Uint8Array; headers?: Record<string, string>; status?: number }>;
const fetchMock = vi.fn(async (url: string) => {
  const f = served[url];
  if (!f) return new Response('nope', { status: 404 });
  return new Response(f.body as BodyInit, { status: f.status ?? 200, headers: { 'content-type': 'image/png', 'content-length': String(f.body.length), ...f.headers } });
});

beforeEach(async () => {
  store = new MemoryCacheStorage();
  vi.stubGlobal('caches', store);
  vi.stubGlobal('fetch', fetchMock);
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => ls.get(k) ?? null,
    setItem: (k: string, v: string) => void ls.set(k, v),
    removeItem: (k: string) => void ls.delete(k)
  });
  ls.clear();
  served = {};
  fetchMock.mockClear();
  await recent.clearAll();
  await signIn(1);
});
afterEach(() => vi.unstubAllGlobals());

const file = (n: number, fill = 1) => new Uint8Array(n).fill(fill);
const text = async (r: Response | undefined) => (r ? new Uint8Array(await r.arrayBuffer()) : null);

describe('recent files', () => {
  it('keeps an opened file under its version, per user, with the server headers', async () => {
    served['/c/a'] = { body: file(10), headers: { 'content-security-policy': "default-src 'none'", 'set-cookie': 'x=1' } };
    await recent.remember(1, 'a.png', '"v1"', '/c/a');
    const r = await recent.lookup(1, 'a.png');
    expect(await text(r)).toEqual(file(10));
    expect(r!.headers.get('content-security-policy')).toBe("default-src 'none'");
    expect(r!.headers.get('set-cookie')).toBeNull();
    // Another user sees nothing of it.
    expect(await recent.lookup(2, 'a.png')).toBeUndefined();
    expect((await store.keys()).filter((k) => k !== META)).toEqual([recent.filesCache(1)]);
  });

  it('opening the same version again only marks it used', async () => {
    served['/c/a'] = { body: file(10) };
    await recent.remember(1, 'a', '"v1"', '/c/a');
    const before = (await recent.list(1))[0].used;
    await new Promise((r) => setTimeout(r, 5));
    await recent.remember(1, 'a', '"v1"', '/c/a');
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect((await recent.list(1))[0].used).toBeGreaterThan(before);
  });

  it('a new version replaces the old one', async () => {
    served['/c/a'] = { body: file(10, 1) };
    await recent.remember(1, 'a', '"v1"', '/c/a');
    served['/c/a'] = { body: file(12, 2) };
    await recent.remember(1, 'a', '"v2"', '/c/a');
    expect(await text(await recent.lookup(1, 'a'))).toEqual(file(12, 2));
    const c = store.stores.get(recent.filesCache(1))!;
    expect([...c.entries.keys()]).toEqual([recent.fileKey(1, 'a', '"v2"')]);
  });

  it('skips files with no version, failed reads and files over 20 MB', async () => {
    await recent.remember(1, 'a', null, '/c/a');
    await recent.remember(1, 'gone', '"v"', '/c/gone');
    served['/c/big'] = { body: new Uint8Array(0), headers: { 'content-length': String(21 * MB) } };
    await recent.remember(1, 'big', '"v"', '/c/big');
    // (A size the server didn't declare is checked after reading.)
    served['/c/big2'] = { body: file(25), headers: { 'content-length': '' } };
    // (No stored settings; still signed in.)
    vi.stubGlobal('localStorage', { getItem: (k: string) => (k === LAST_USER ? (ls.get(k) ?? null) : null), setItem() {}, removeItem() {} });
    await recent.remember(1, 'big2', '"v"', '/c/big2');
    expect((await recent.list(1)).map((r) => r.path)).toEqual(['big2']);
  });

  it('checks the read size too, when no length was declared', async () => {
    recent.setCapMb(1);
    served['/c/x'] = { body: file(2 * MB), headers: { 'content-length': '' } };
    await recent.remember(1, 'x', '"v"', '/c/x');
    expect(await recent.list(1)).toEqual([]);
  });

  it('a response with no length or type is still kept (checked after reading)', async () => {
    vi.stubGlobal('fetch', async () => {
      const r = new Response(file(5));
      r.headers.delete('content-type');
      return r;
    });
    await recent.remember(1, 'a', '"v"', '/c/a');
    const kept = await recent.list(1);
    expect(kept[0]).toMatchObject({ size: 5, type: 'application/octet-stream' });
    // Declared too big with no body to cancel.
    vi.stubGlobal('fetch', async () => new Response(null, { headers: { 'content-length': String(30 * MB) } }));
    await recent.remember(1, 'b', '"v"', '/c/b');
    expect((await recent.list(1)).length).toBe(1);
  });

  it('evicts the least recently used past 50 files', async () => {
    for (let i = 0; i < 52; i++) {
      served[`/c/${i}`] = { body: file(1) };
      await recent.remember(1, `f${i}`, '"v"', `/c/${i}`);
    }
    const kept = (await recent.list(1)).map((r) => r.path);
    expect(kept.length).toBe(50);
    expect(kept).not.toContain('f0');
    expect(kept).not.toContain('f1');
    expect(await recent.lookup(1, 'f0')).toBeUndefined();
  });

  it('the total cap is a setting', async () => {
    expect(recent.capMb()).toBeNull();
    recent.setCapMb(1);
    expect(recent.capMb()).toBe('1');
    served['/c/a'] = { body: file(600 * 1024) };
    served['/c/b'] = { body: file(600 * 1024) };
    await recent.remember(1, 'a', '"v"', '/c/a');
    await recent.remember(1, 'b', '"v"', '/c/b');
    expect((await recent.list(1)).map((r) => r.path)).toEqual(['b']);
    recent.setCapMb(null);
    expect(recent.capMb()).toBeNull();
  });

  it('pins keep a file past eviction, and unpinning lets it go', async () => {
    recent.setCapMb(1);
    served['/c/p'] = { body: file(700 * 1024) };
    served['/c/a'] = { body: file(600 * 1024) };
    expect(await recent.pin(1, 'p', '"v"', '/c/p', true)).toBe(true);
    await recent.remember(1, 'a', '"v"', '/c/a'); // doesn't fit beside the pin
    expect((await recent.list(1)).map((r) => [r.path, r.pinned])).toEqual([['p', true]]);
    // Opening a pinned file again keeps the pin.
    await recent.remember(1, 'p', '"v"', '/c/p');
    expect((await recent.list(1))[0].pinned).toBe(true);
    expect(await recent.pin(1, 'p', '"v"', '/c/p', false)).toBe(true);
    expect((await recent.list(1))[0].pinned).toBe(false);
    // Unpinning what isn't kept is fine too.
    expect(await recent.pin(1, 'nothing', '"v"', '/c/x', false)).toBe(true);
  });

  it('pinning a kept version still has to fit beside the other pins', async () => {
    recent.setCapMb(1);
    served['/c/p'] = { body: file(700 * 1024) };
    served['/c/a'] = { body: file(500 * 1024) };
    expect(await recent.pin(1, 'p', '"v"', '/c/p', true)).toBe(true);
    recent.setCapMb(2);
    await recent.remember(1, 'a', '"v"', '/c/a'); // kept, unpinned
    recent.setCapMb(1);
    expect(await recent.pin(1, 'a', '"v"', '/c/a', true)).toBe('too-big');
  });

  it('without Cache Storage (plain http), sign-in and sign-out still clear the rest', async () => {
    await tx('jobs', 'readwrite', (s) => void s.put({ id: 'other', user: 9 }));
    vi.stubGlobal('caches', undefined);
    await recent.keepOnlyUser(1);
    expect(await tx('jobs', 'readonly', (s) => s.getAllKeys())).toEqual([]);
    await expect(recent.clearAll()).resolves.toBeUndefined();
  });

  it('pinning: an already kept version, a new version, and what cannot fit', async () => {
    served['/c/a'] = { body: file(10, 1) };
    await recent.remember(1, 'a', '"v1"', '/c/a');
    expect(await recent.pin(1, 'a', '"v1"', '/c/a', true)).toBe(true);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    served['/c/a'] = { body: file(11, 2) };
    expect(await recent.pin(1, 'a', '"v2"', '/c/a', true)).toBe(true);
    expect(await text(await recent.lookup(1, 'a'))).toEqual(file(11, 2));
    expect(await recent.pin(1, 'b', null, '/c/b', true)).toBe('no-version');
    expect(await recent.pin(1, 'b', '"v"', '/c/missing', true)).toBe('unreadable');
    // (A network failure too.)
    vi.stubGlobal('fetch', async () => { throw new TypeError('offline'); });
    expect(await recent.pin(1, 'b', '"v"', '/c/b', true)).toBe('unreadable');
    await recent.remember(1, 'b', '"v"', '/c/b'); // (quietly nothing)
    vi.stubGlobal('fetch', fetchMock);
    recent.setCapMb(1);
    served['/c/huge'] = { body: file(2 * MB) };
    expect(await recent.pin(1, 'huge', '"v"', '/c/huge', true)).toBe('too-big'); // over the cap
    // A new pin of an unkept file.
    served['/c/n'] = { body: file(3) };
    expect(await recent.pin(1, 'n', '"v"', '/c/n', true)).toBe(true);
  });

  it('a lookup with an index entry but no bytes is a miss', async () => {
    served['/c/a'] = { body: file(3) };
    await recent.remember(1, 'a', '"v"', '/c/a');
    await store.delete(recent.filesCache(1));
    expect(await recent.lookup(1, 'a')).toBeUndefined();
  });

  it('sign-out clears every user’s files and listings, and nothing else', async () => {
    served['/c/a'] = { body: file(3) };
    await recent.remember(1, 'a', '"v"', '/c/a');
    await signIn(2);
    await recent.remember(2, 'a', '"v"', '/c/a');
    await (await caches.open(`${recent.LISTS}1`)).put('/l', new Response('{}'));
    await (await caches.open('strata-shell-x')).put('/', new Response('shell'));
    await recent.clearAll();
    expect((await store.keys()).filter((k) => k !== META)).toEqual(['strata-shell-x']);
    expect(await recent.list(1)).toEqual([]);
  });

  it('a new user drops what other users left (files, listings, queued uploads)', async () => {
    served['/c/a'] = { body: file(3) };
    await recent.remember(1, 'a', '"v"', '/c/a');
    await signIn(2);
    await recent.remember(2, 'b', '"v"', '/c/a');
    await (await caches.open(`${recent.LISTS}1`)).put('/l', new Response('{}'));
    await (await caches.open(`${recent.LISTS}2`)).put('/l', new Response('{}'));
    await (await caches.open('strata-shell-x')).put('/', new Response('shell'));
    await tx('jobs', 'readwrite', (s) => {
      s.put({ id: 'j1', user: 1 });
      s.put({ id: 'j2', user: 2 });
    });
    await recent.keepOnlyUser(2);
    expect((await store.keys()).filter((k) => k !== META).sort()).toEqual([recent.filesCache(2), `${recent.LISTS}2`, 'strata-shell-x'].sort());
    expect(await recent.list(1)).toEqual([]);
    expect((await recent.list(2)).length).toBe(1);
    expect(await tx('jobs', 'readonly', (s) => s.getAllKeys())).toEqual(['j2']);
    // Nothing to drop: no change.
    await recent.keepOnlyUser(2);
    expect((await recent.list(2)).length).toBe(1);
  });

  it('settings storage that throws counts as no setting', () => {
    vi.stubGlobal('localStorage', {
      getItem() {
        throw new Error('denied');
      },
      setItem() {
        throw new Error('denied');
      },
      removeItem() {
        throw new Error('denied');
      }
    });
    expect(recent.capMb()).toBeNull();
    expect(() => recent.setCapMb(5)).not.toThrow();
    expect(() => recent.setCapMb(null)).not.toThrow();
  });

  it('never stores for a user who is not the one signed in here (a stale tab)', async () => {
    served['/c/a'] = { body: file(3) };
    await signIn(2);
    await recent.remember(1, 'a', '"v"', '/c/a');
    expect(await recent.pin(1, 'a', '"v"', '/c/a', true)).toBe('unreadable');
    expect(fetchMock).not.toHaveBeenCalled();
    await signOut();
    expect(await recent.signedInHere()).toBeNull();
  });

  it('a size known from the listing is checked before any download', async () => {
    served['/c/big'] = { body: file(3) };
    await recent.remember(1, 'big', '"v"', '/c/big', 30 * MB);
    expect(fetchMock).not.toHaveBeenCalled();
    expect(await recent.pin(1, 'big', '"v"', '/c/big', true, 600 * MB)).toBe('too-big');
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('an answer with no length is read only up to the limit', async () => {
    recent.setCapMb(1);
    let pulled = 0;
    const body = new ReadableStream<Uint8Array>({
      pull(c) {
        pulled++;
        c.enqueue(new Uint8Array(256 * 1024));
        if (pulled > 100) c.close();
      }
    });
    vi.stubGlobal('fetch', async () => new Response(body));
    await recent.remember(1, 'z', '"v"', '/c/z');
    expect(await recent.list(1)).toEqual([]);
    expect(pulled).toBeLessThan(10); // (stopped past 1 MB, not read to the end)
    // An empty answer (no body at all) is kept as an empty file.
    vi.stubGlobal('fetch', async () => new Response(null));
    await recent.remember(1, 'empty', '"v"', '/c/e');
    expect((await recent.list(1)).map((r) => [r.path, r.size])).toEqual([['empty', 0]]);
  });

  it('signed out while a file was being stored: nothing of it stays', async () => {
    vi.stubGlobal('fetch', async () => {
      await signOut(); // (in another tab, meanwhile)
      return new Response(file(4));
    });
    await recent.remember(1, 'a', '"v"', '/c/a');
    expect(await recent.list(1)).toEqual([]);
    expect(await recent.lookup(1, 'a')).toBeUndefined();
    await signIn(1);
    vi.stubGlobal('fetch', async () => {
      await signOut();
      return new Response(file(4));
    });
    expect(await recent.pin(1, 'b', '"v"', '/c/b', true)).toBe('unreadable');
    expect(await recent.list(1)).toEqual([]);
  });

  it('a newer version that can’t be kept drops the old one (unless pinned)', async () => {
    served['/c/a'] = { body: file(3) };
    await recent.remember(1, 'a', '"v1"', '/c/a');
    await recent.remember(1, 'a', '"v2"', '/c/a', 30 * MB);
    expect(await recent.lookup(1, 'a')).toBeUndefined();
    await recent.pin(1, 'p', '"v1"', '/c/a', true);
    await recent.remember(1, 'p', '"v2"', '/c/a', 30 * MB);
    expect(await recent.lookup(1, 'p')).toBeDefined();
  });
});

describe('who is signed in here', () => {
  it('a page goes by localStorage, so a lost Cache Storage record costs no pin (issue #29)', async () => {
    served['/c/a'] = { body: file(3) };
    await signIn(1);
    await caches.delete(META); // (what WebKit sometimes does to it)
    expect(await recent.signedInHere()).toBe(1);
    expect(await recent.pin(1, 'a', '"v"', '/c/a', true)).toBe(true);
  });

  it('the newest sign-in on the device wins, also over an older record', async () => {
    await signIn(1);
    ls.set(LAST_USER, JSON.stringify({ id: 2, username: 'b', is_admin: false }));
    expect(await recent.signedInHere()).toBe(2);
  });

  it('nothing readable there is no one', async () => {
    ls.set(LAST_USER, 'not json');
    expect(await recent.signedInHere()).toBeNull();
    ls.set(LAST_USER, JSON.stringify({ id: -1 }));
    expect(await recent.signedInHere()).toBeNull();
  });

  it('the service worker, without localStorage, reads the record', async () => {
    await signIn(3);
    vi.stubGlobal('localStorage', undefined);
    expect(await recent.signedInHere()).toBe(3);
    await caches.delete(META);
    expect(await recent.signedInHere()).toBeNull();
  });
});
