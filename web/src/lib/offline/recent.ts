// The recent-files cache: the bytes of files opened lately (and of pinned
// ones) in Cache Storage, one cache per user, with an index in IndexedDB.
// Used by the app (remember, pin) and the service worker (serve offline).
// Nothing else ever stores authenticated file bytes.

import { byUser, tx, wipe } from './idb';
import { admits, evictions, limits, type Limits } from './lru';
import { NETWORK_ONLY } from './keys';

export interface Recent {
  /** `${user}|${path}` */
  key: string;
  user: number;
  path: string;
  etag: string;
  size: number;
  type: string;
  used: number;
  pinned: boolean;
}

export const FILES = 'strata-files-';
export const LISTS = 'strata-lists-';
const CAP_KEY = 'strata-offline-cap-mb';

export const filesCache = (user: number) => `${FILES}${user}`;
/** The cache key of one version of a file (never a real URL). */
export const fileKey = (user: number, path: string, etag: string) =>
  `/__offline/${user}?path=${encodeURIComponent(path)}&v=${encodeURIComponent(etag)}`;

/** The size cap in MB (a setting), or null for the default. */
export function capMb(): string | null {
  try {
    return localStorage.getItem(CAP_KEY);
  } catch {
    return null;
  }
}
export function setCapMb(mb: number | null) {
  try {
    if (mb === null) localStorage.removeItem(CAP_KEY);
    else localStorage.setItem(CAP_KEY, String(mb));
  } catch {}
}

export const list = (user: number) => byUser<Recent>('recent', user);
const get = (user: number, path: string) => tx<Recent | undefined>('recent', 'readonly', (s) => s.get(`${user}|${path}`));
const put = (r: Recent) => tx('recent', 'readwrite', (s) => void s.put(r));

async function drop(r: Recent) {
  await (await caches.open(filesCache(r.user))).delete(fileKey(r.user, r.path, r.etag));
  await tx('recent', 'readwrite', (s) => void s.delete(r.key));
}

/** Apply the limits: drop what no longer fits, least recently used first. */
export async function trim(user: number, l: Limits = limits(capMb())) {
  const all = await list(user);
  const gone = new Set(evictions(all, l));
  for (const r of all) if (gone.has(r.key)) await drop(r);
}

/** Store a file's bytes (fetched from `url`) under its version. */
async function store(user: number, path: string, etag: string, url: string, pinned: boolean, l: Limits): Promise<Recent | 'unreadable' | 'too-big'> {
  const others = (await list(user)).filter((r) => r.pinned && r.path !== path).reduce((a, r) => a + r.size, 0);
  const r = await fetch(url, { credentials: 'same-origin', headers: { [NETWORK_ONLY]: '1' } });
  if (!r.ok) return 'unreadable';
  const declared = Number(r.headers.get('content-length') ?? NaN);
  // Too big: don't even read it.
  if (Number.isFinite(declared) && !admits(declared, pinned, others, l)) {
    await r.body?.cancel();
    return 'too-big';
  }
  const blob = await r.blob();
  if (!admits(blob.size, pinned, others, l)) return 'too-big';
  const type = r.headers.get('content-type') ?? 'application/octet-stream';
  // The server's own headers are kept (its CSP, nosniff and disposition
  // keep a stored SVG or HTML file inert, offline too).
  const headers = new Headers(r.headers);
  headers.delete('set-cookie');
  headers.set('content-length', String(blob.size));
  const cache = await caches.open(filesCache(user));
  await cache.put(fileKey(user, path, etag), new Response(blob, { headers }));
  return { key: `${user}|${path}`, user, path, etag, size: blob.size, type, used: Date.now(), pinned };
}

/** A file was opened: keep its bytes (if they fit), or just note the use. */
export async function remember(user: number, path: string, etag: string | null, url: string): Promise<void> {
  if (!etag) return; // (no version to key it by)
  const l = limits(capMb());
  const old = await get(user, path);
  if (old?.etag === etag) {
    await put({ ...old, used: Date.now() });
    return;
  }
  const r = await store(user, path, etag, url, old?.pinned ?? false, l).catch(() => 'unreadable' as const);
  if (typeof r === 'string') return;
  if (old) await (await caches.open(filesCache(user))).delete(fileKey(user, path, old.etag));
  await put(r);
  await trim(user, l);
}

export type PinResult = true | 'no-version' | 'unreadable' | 'too-big';

/** "Keep offline" on or off. Resolves true, or why it couldn't. */
export async function pin(user: number, path: string, etag: string | null, url: string, on: boolean): Promise<PinResult> {
  const l = limits(capMb());
  const old = await get(user, path);
  if (!on) {
    if (old) {
      await put({ ...old, pinned: false });
      await trim(user, l);
    }
    return true;
  }
  if (!etag) return 'no-version';
  if (old?.etag === etag) {
    // (Already kept: it must still fit beside the other pins.)
    const others = (await list(user)).filter((r) => r.pinned && r.path !== path).reduce((a, r) => a + r.size, 0);
    if (!admits(old.size, true, others, l)) return 'too-big';
    await put({ ...old, pinned: true, used: Date.now() });
    return true;
  }
  const r = await store(user, path, etag, url, true, l).catch(() => 'unreadable' as const);
  if (typeof r === 'string') return r;
  if (old) await (await caches.open(filesCache(user))).delete(fileKey(user, path, old.etag));
  await put(r);
  await trim(user, l);
  return true;
}

/** The stored bytes of a file (its latest kept version). */
export async function lookup(user: number, path: string): Promise<Response | undefined> {
  const r = await get(user, path);
  if (!r) return undefined;
  return (await (await caches.open(filesCache(user))).match(fileKey(user, path, r.etag))) ?? undefined;
}

/** Sign-out: every user's cached files and listings, and the whole offline
 *  database (the upload queue too). */
export async function clearAll(): Promise<void> {
  // (No Cache Storage at all on a plain-http origin: nothing was cached.)
  if (typeof caches !== 'undefined') for (const k of await caches.keys()) if (k.startsWith(FILES) || k.startsWith(LISTS)) await caches.delete(k);
  await wipe();
}

/** Someone signed in: drop what other users left on this device. */
export async function keepOnlyUser(user: number): Promise<void> {
  for (const k of await caches.keys()) {
    const owner = k.startsWith(FILES) ? k.slice(FILES.length) : k.startsWith(LISTS) ? k.slice(LISTS.length) : null;
    if (owner !== null && owner !== String(user)) await caches.delete(k);
  }
  // (One transaction per store: nothing can change the rows in between.)
  for (const store of ['recent', 'jobs'] as const) {
    const gone: IDBValidKey[] = [];
    await tx(store, 'readwrite', (s) => {
      const c = s.openCursor();
      c.onsuccess = () => {
        const cur = c.result;
        if (!cur) return;
        if ((cur.value as { user: number }).user !== user) {
          gone.push(cur.primaryKey);
          cur.delete();
        }
        cur.continue();
      };
    });
    if (store === 'jobs') await tx('blobs', 'readwrite', (s) => gone.forEach((k) => s.delete(k)));
  }
}
