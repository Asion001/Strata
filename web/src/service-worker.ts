/// <reference types="@sveltejs/kit" />
/// <reference no-default-lib="true"/>
/// <reference lib="esnext" />
/// <reference lib="webworker" />
// Strata offline:
// - the app shell and the hashed build assets are precached, so the app
//   opens offline and instantly from the home screen;
// - folder listings are network-first, with the last answer kept per user
//   and per folder, served (marked) when the network is down;
// - file bytes are only ever served from the recent-files cache (see
//   lib/offline/recent.ts), and only when the network is down;
// - queued uploads resume from Background Sync where the browser has it;
// - share links (/s/..., /api/public/...) are never touched.
// A new version installs and waits; the app offers a reload, which tells
// it to take over (see watchUpdates in lib/offline/app.svelte.ts).
import { build, files, version } from '$service-worker';
import { LISTS, lookup } from '$lib/offline/recent';
import { drain } from '$lib/offline/queue';
import { chunkFile } from '$lib/offline/chunks';
import { USER_KEY, META, OFFLINE_AT, SYNC_TAG } from '$lib/offline/keys';
import { ranged } from '$lib/offline/range';

const sw = self as unknown as ServiceWorkerGlobalScope;
const SHELL = `strata-shell-${version}`;
// The PDF viewer's large files (worker, stylesheet, CMaps, fonts, WASM) are
// fetched only when a PDF is opened, not precached with the shell.
const lazyPdf = (f: string) => f.startsWith('/pdfjs/') || /\/(pdf\.worker|PdfViewer)[.-]/.test(f);
// (Launch screens are read by iOS from the network at install, not by us.)
const ASSETS = [...build, ...files.filter((f) => !f.endsWith('.gz') && !f.startsWith('/splash/'))].filter((f) => !lazyPdf(f)).concat('/');
const ASSET_SET = new Set(ASSETS);

sw.addEventListener('install', (event) => {
  // No skipWaiting here: a new version waits until the app says so.
  event.waitUntil(caches.open(SHELL).then((c) => c.addAll(ASSETS)));
});

sw.addEventListener('message', (event) => {
  if (event.data === 'skip-waiting') void sw.skipWaiting();
});

sw.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      // Only older shells: users' offline files and listings stay.
      .then((keys) => Promise.all(keys.filter((k) => k.startsWith('strata-shell-') && k !== SHELL).map((k) => caches.delete(k))))
      .then(() => sw.clients.claim())
  );
});

/** Who the app says is signed in (written by the app; gone on sign-out). */
async function currentUser(): Promise<number | null> {
  const r = await (await caches.open(META)).match(USER_KEY);
  const n = r ? Number(await r.text()) : NaN;
  return Number.isInteger(n) && n > 0 ? n : null;
}

/** A listing: the network, else the last answer for this user and folder. */
async function listing(req: Request, user: string): Promise<Response> {
  const cache = await caches.open(`${LISTS}${user}`);
  const key = new URL(req.url).pathname + new URL(req.url).search;
  try {
    const r = await fetch(req);
    if (r.ok) {
      const body = await r.clone().arrayBuffer();
      await cache.put(key, new Response(body, { headers: { 'content-type': 'application/json', [OFFLINE_AT]: new Date().toISOString() } }));
    } else if (r.status === 401 || r.status === 403) {
      // Not this user's (any more): nothing of it stays.
      await cache.delete(key);
    }
    return r;
  } catch (e) {
    const hit = await cache.match(key);
    if (hit) return hit; // (carries when it was saved: the app shows a banner)
    throw e;
  }
}

/** File bytes: the network; offline, the recent-files cache. */
async function content(req: Request): Promise<Response> {
  try {
    return await fetch(req);
  } catch (e) {
    const user = await currentUser();
    const path = new URL(req.url).searchParams.get('path');
    if (user === null || path === null) throw e;
    const hit = await lookup(user, path);
    if (!hit) throw e;
    return ranged(hit, req.headers.get('range'));
  }
}

sw.addEventListener('fetch', (event) => {
  const req = event.request;
  if (req.method !== 'GET') return;
  const url = new URL(req.url);
  if (url.origin !== sw.location.origin) return;
  // Share links: left entirely to the network.
  if (url.pathname.startsWith('/s/') || url.pathname.startsWith('/api/public/')) return;

  if (url.pathname === '/api/files/list') {
    const user = req.headers.get('x-strata-user');
    if (user && /^\d+$/.test(user)) event.respondWith(listing(req, user));
    return;
  }
  if (url.pathname === '/api/files/content' || url.pathname === '/api/files/image') {
    event.respondWith(content(req));
    return;
  }
  if (url.pathname.startsWith('/api/')) return;

  // Navigations: network first, fall back to the cached shell.
  if (req.mode === 'navigate') {
    event.respondWith(fetch(req).catch(async () => (await caches.match('/', { cacheName: SHELL })) ?? Response.error()));
    return;
  }
  // Static assets: cache first.
  if (ASSET_SET.has(url.pathname)) {
    event.respondWith(caches.match(url.pathname, { cacheName: SHELL }).then((r) => r ?? fetch(req)));
  }
});

// Background Sync (not in Safari): finish queued uploads with no tab open.
interface SyncEvent extends ExtendableEvent {
  tag: string;
}
sw.addEventListener('sync', ((event: SyncEvent) => {
  if (event.tag !== SYNC_TAG) return;
  event.waitUntil(
    (async () => {
      // An open tab runs the queue itself (and shows its progress): fail
      // this attempt, so the browser tries again later, maybe tab-less.
      if ((await sw.clients.matchAll({ type: 'window' })).length) throw new Error('a tab is running the queue');
      // The signed-in user, from the server (the queue only runs as them).
      const r = await fetch('/api/auth/me', { headers: { 'x-strata': '1' } });
      if (!r.ok) return;
      const me = (await r.json()) as { id: number };
      const out = await drain(me.id, { fetch: (u, i) => fetch(u, i), chunk: (b) => chunkFile(b), sleep: (a) => new Promise((res) => setTimeout(res, 1000 * 2 ** a)) });
      // Still offline: fail, so the browser tries again later.
      if (out === 'offline') throw new Error('offline');
    })()
  );
}) as EventListener);
