// The app's side of offline: who is signed in (for the service worker),
// the upload queue as the UI sees it, the "new version" prompt, and the
// sign-out that leaves nothing behind.

// (The queue, the cache and the chunker load on first use, not with the app.)
import { META, SYNC_TAG, USER_KEY } from './keys';
import { isActive, type Job } from './jobs';
import type { Deps } from './queue';
import { ApiError, type User } from '../api';

const Queue = () => import('./queue');
const Recent = () => import('./recent');

// ---------------------------------------------------------------- user

const LAST_USER = 'strata-last-user';

/** Someone is signed in: tell the service worker (it keys cached files by
 *  user), drop what other users left here, remember them for offline. */
export async function signedIn(u: User) {
  try {
    localStorage.setItem(LAST_USER, JSON.stringify({ id: u.id, username: u.username, is_admin: u.is_admin }));
  } catch {}
  // (Apart: with no Cache Storage, other users' queued uploads still go.)
  try {
    if (typeof caches !== 'undefined') await (await caches.open(META)).put(USER_KEY, new Response(String(u.id)));
  } catch {}
  try {
    await (await Recent()).keepOnlyUser(u.id);
  } catch {}
  dropOldUploadKeys();
}

/** The old uploader kept resume keys (named after file paths) in
 *  localStorage; nothing reads them now. */
function dropOldUploadKeys() {
  try {
    for (const k of Object.keys(localStorage)) if (k.startsWith('strata-upload:')) localStorage.removeItem(k);
  } catch {}
}

/** Did a request fail for want of the server (no answer, or a gateway
 *  answering for it), rather than with the server's own error? */
const unreachable = (e: unknown) => !(e instanceof ApiError) || e.status >= 502;

/** Offline, the user who was signed in last (until they sign out). */
export function lastUser(): User | null {
  try {
    const u = JSON.parse(localStorage.getItem(LAST_USER) ?? 'null');
    return u && Number.isInteger(u.id) ? u : null;
  } catch {
    return null;
  }
}

const PENDING_LOGOUT = 'strata-pending-logout';
/** Set at sign-out: other tabs see it change (a storage event) and leave. */
export const SIGNED_OUT = 'strata-signed-out';

/** Sign out: stop the upload queue (and wait for it), cancel queued uploads
 *  with their server sessions while still signed in, sign out, then delete
 *  every cache and the queue. Local data goes even when the server can't be
 *  reached; then the server session is ended on the next start (see
 *  `finishPendingLogout`). Resolves whether the server signed out. */
export async function signOut(user: number | null, logout: () => Promise<unknown>): Promise<boolean> {
  const [queue, recent] = await Promise.all([Queue(), Recent()]);
  queue.halt();
  await Promise.race([draining, new Promise((r) => setTimeout(r, 3000))]);
  if (user !== null) await queue.abandonSessions(user, fetch).catch(() => {});
  let ended = true;
  // (The server answering with an error still ended nothing it can't end.)
  await logout().catch((e) => (ended = !unreachable(e)));
  dropOldUploadKeys();
  try {
    if (ended) localStorage.removeItem(PENDING_LOGOUT);
    else localStorage.setItem(PENDING_LOGOUT, '1');
  } catch {}
  try {
    localStorage.removeItem(LAST_USER);
  } catch {}
  if (typeof caches !== 'undefined') await caches.delete(META).catch(() => false);
  await recent.clearAll().catch(() => {});
  uploads.jobs = [];
  // Every other open tab of the app leaves too (it would cache again).
  try {
    localStorage.setItem(SIGNED_OUT, String(Date.now()));
  } catch {}
  return ended;
}

/** A sign-out that couldn't reach the server: end that session now, before
 *  anything else. Resolves 'none' (nothing pending), 'done' (ended now: go
 *  to the sign-in page) or 'unreachable' (still pending). */
let finishing: Promise<'none' | 'done' | 'unreachable'> | null = null;
/** (One request at a time, shared by every caller: a late answer to a
 *  second logout would clear the cookie of a session started meanwhile.) */
export function finishPendingLogout(logout: () => Promise<unknown>): Promise<'none' | 'done' | 'unreachable'> {
  finishing ??= endPending(logout).finally(() => (finishing = null));
  return finishing;
}
async function endPending(logout: () => Promise<unknown>): Promise<'none' | 'done' | 'unreachable'> {
  try {
    if (!localStorage.getItem(PENDING_LOGOUT)) return 'none';
  } catch {
    return 'none';
  }
  try {
    await logout();
  } catch (e) {
    // Still unreachable: stay signed out here, try again next time.
    if (unreachable(e)) return 'unreachable';
  }
  try {
    localStorage.removeItem(PENDING_LOGOUT);
  } catch {}
  return 'done';
}

// ---------------------------------------------------------------- network

export const net = $state({ online: typeof navigator === 'undefined' ? true : navigator.onLine });
if (typeof window !== 'undefined') {
  window.addEventListener('online', () => (net.online = true));
  window.addEventListener('offline', () => (net.online = false));
}

// ---------------------------------------------------------------- uploads

export const uploads = $state<{ jobs: Job[] }>({ jobs: [] });

function changed(j: Job) {
  const i = uploads.jobs.findIndex((x) => x.id === j.id);
  if (i >= 0) uploads.jobs[i] = j;
  else uploads.jobs.push(j);
}

let queueUser: number | null = null;
let storing: Promise<unknown> = Promise.resolve();
let draining: Promise<unknown> | null = null;
let again = false;
let onDone: ((j: Job) => void) | null = null;

const deps = (): Deps => ({
  fetch: (u, i) => fetch(u, i),
  chunk: async (b, signal) => (await import('./chunks')).chunkFile(b, signal),
  sleep: (a) => new Promise((r) => setTimeout(r, 1000 * 2 ** a)),
  online: () => navigator.onLine,
  changed: (j) => {
    changed(j);
    // (Failed and cancelled ones end too: the folder shows what is there.)
    if (!isActive(j)) onDone?.(j);
  }
});

/** Work through the queue now (one run at a time; a request meanwhile runs
 *  it again afterwards). */
export function pump() {
  const user = queueUser;
  if (user === null) return;
  if (draining) {
    again = true;
    return;
  }
  draining = Queue()
    .then((queue) => queue.drain(user, deps()))
    .then((out) => {
      // Stopped by the network while the device says it is online (the
      // server was unreachable for a while): try again later by itself.
      if (out === 'offline' && navigator.onLine) {
        clearTimeout(retryTimer);
        retryTimer = setTimeout(pump, retryDelay);
        retryDelay = Math.min(retryDelay * 2, 5 * 60_000);
      } else retryDelay = 15_000;
    })
    .catch(() => {})
    .finally(() => {
      draining = null;
      if (again) {
        again = false;
        pump();
      }
    });
}
let retryTimer: ReturnType<typeof setTimeout> | undefined;
let retryDelay = 15_000;

/** Start the queue for a user: what was left (app open), and again
 *  whenever the network comes back. `done`: a job finished. */
export async function startQueue(user: number, done: (j: Job) => void) {
  queueUser = user;
  onDone = done;
  const loaded = await Queue()
    .then((queue) => queue.jobs(user))
    .catch(() => [] as Job[]);
  // (Merged: files added while this was loading stay listed.)
  const known = new Set(uploads.jobs.map((j) => j.id));
  uploads.jobs = [...uploads.jobs, ...loaded.filter((j) => !known.has(j.id))];
  pump();
}
if (typeof window !== 'undefined') window.addEventListener('online', () => pump());

/** Jobs finished or dropped elsewhere (another tab, the worker) leave this
 *  tab's list when it comes back to the foreground. */
async function refresh() {
  const user = queueUser;
  if (user === null) return;
  let stored: Job[];
  try {
    stored = await (await Queue()).jobs(user);
  } catch {
    return; // (can't read it now: leave the list as it is)
  }
  const byId = new Map(stored.map((j) => [j.id, j]));
  const wasActive = new Set(uploads.jobs.filter(isActive).map((j) => j.id));
  const gone = uploads.jobs.filter((j) => isActive(j) && !byId.has(j.id));
  // The stored state wins (another context may have moved it on or failed it).
  uploads.jobs = uploads.jobs.filter((j) => !gone.includes(j)).map((j) => {
    const s = byId.get(j.id);
    return s && isActive(j) ? { ...s, blob: j.blob } : j;
  });
  // (Only what changed refreshes the folder.)
  const ended = uploads.jobs.filter((j) => !isActive(j) && byId.has(j.id) && wasActive.has(j.id));
  for (const j of [...gone, ...ended]) onDone?.(j);
  // (Finished elsewhere: let go of their bytes and owner locks here.)
  const queue = await Queue();
  for (const j of gone) await queue.remove(j.id).catch(() => {});
}
if (typeof document !== 'undefined')
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') void refresh();
  });

/** Queue files for upload. */
export async function enqueue(user: number, items: { file: File; path: string }[]) {
  const queue = await Queue();
  const keeping: Promise<unknown>[] = [];
  for (const { file, path } of items) {
    const j = queue.newJob(user, path, file);
    // Shown and uploading at once; its bytes are stored for later (resume
    // after a reload) meanwhile. A job done before that finished has its
    // stored bytes dropped again.
    await queue.save(j);
    queue.holdBytes(j.id, file); // (only once it is queued: nothing held for nothing)
    changed(j);
    // (One file at a time: storing as buffers reads a file into memory.)
    storing = storing
      .then(() => queue.keepBytes(j.id, file))
      .then(async () => {
        if (!(await queue.exists(j.id))) await queue.remove(j.id);
      })
      .catch(() => {});
    keeping.push(storing);
  }
  pump();
  await Promise.all(keeping);
}

/** Background Sync (where the browser has it): asked for when this page
 *  goes away or offline with uploads left, so the worker finishes them
 *  without a tab. (While a tab is open, it runs the queue itself.) */
async function askForSync() {
  if (!uploads.jobs.some(isActive)) return;
  try {
    const reg = await navigator.serviceWorker?.ready;
    await (reg as ServiceWorkerRegistration & { sync?: { register(t: string): Promise<void> } })?.sync?.register(SYNC_TAG);
  } catch {}
}
if (typeof window !== 'undefined') {
  window.addEventListener('offline', () => void askForSync());
  window.addEventListener('pagehide', () => void askForSync());
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden') void askForSync();
  });
}

/** Resolves false when it is too late (the file is being saved). */
export async function cancelUpload(id: string): Promise<boolean> {
  if (!(await (await Queue()).cancel(id, { fetch: (u, i) => fetch(u, i), changed }))) return false;
  uploads.jobs = uploads.jobs.filter((j) => j.id !== id);
  return true;
}

/** Retry a failed upload. False if its bytes are gone (add it again). */
export async function retryUpload(id: string): Promise<boolean> {
  const queue = await Queue();
  const j = await queue.load(id);
  if (!j) return false;
  if (!(await queue.hasBytes(id))) return false;
  // (The old session goes: retrying starts a new one.)
  if (j.session) await fetch(`/api/uploads/${j.session}`, { method: 'DELETE', headers: { 'x-strata': '1' } }).catch(() => {});
  const n = queue.step(j, { t: 'retry' });
  // (Cancelled elsewhere meanwhile: not brought back.)
  if (!(await queue.update(n))) {
    uploads.jobs = uploads.jobs.filter((x) => x.id !== id);
    return true;
  }
  changed(n);
  pump();
  return true;
}

/** Done, failed and cancelled jobs leave the list (failed ones the queue too). */
export async function clearFinished() {
  const queue = await Queue();
  for (const j of uploads.jobs) {
    if (j.state !== 'failed') continue;
    // (Its server session and staged chunks go too.)
    if (j.session) await fetch(`/api/uploads/${j.session}`, { method: 'DELETE', headers: { 'x-strata': '1' } }).catch(() => {});
    await queue.remove(j.id);
  }
  uploads.jobs = uploads.jobs.filter((j) => isActive(j));
}

// ---------------------------------------------------------------- update

export const update = $state({ ready: false });
let waiting: ServiceWorker | null = null;

/** Watch for a new version of the app: it installs, waits, and the UI
 *  offers a reload (which hands over to it). */
export async function watchUpdates() {
  if (typeof navigator === 'undefined' || !navigator.serviceWorker) return;
  // (On a first visit the worker registers after the page loads: wait.)
  const reg = await navigator.serviceWorker.ready;
  const offer = (w: ServiceWorker | null) => {
    // (A first install has no controller: nothing to replace.)
    if (w && navigator.serviceWorker.controller) {
      waiting = w;
      update.ready = true;
    }
  };
  const follow = (w: ServiceWorker | null) =>
    w?.addEventListener('statechange', () => {
      if (w.state === 'installed') offer(w);
    });
  offer(reg.waiting);
  follow(reg.installing); // (one already installing when the app started)
  reg.addEventListener('updatefound', () => follow(reg.installing));
  // A new worker took over (from this tab or another). The tab that asked
  // reloads; any other tab (whose uploads must not be cut off) gets the
  // banner, and reloads when its user says so. The first worker claiming
  // a fresh page is no new version.
  let controlled = !!navigator.serviceWorker.controller;
  let reloading = false;
  navigator.serviceWorker.addEventListener('controllerchange', () => {
    if (reloading) return;
    if (asked) {
      reloading = true;
      location.reload();
    } else if (!controlled) {
      controlled = true;
    } else {
      waiting = null; // (already active: Reload just reloads)
      update.ready = true;
    }
  });
  // Look for a new version when the app comes back to the foreground.
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') void reg.update().catch(() => {});
  });
}

let asked = false;
export function applyUpdate() {
  asked = true;
  // (Activated by another tab meanwhile: nothing to hand over to.)
  if (!waiting || waiting.state === 'activated' || waiting.state === 'redundant') return location.reload();
  waiting.postMessage('skip-waiting');
}
