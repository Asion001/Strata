// The app's side of offline: who is signed in (for the service worker),
// the upload queue as the UI sees it, the "new version" prompt, and the
// sign-out that leaves nothing behind.

// (The queue, the cache and the chunker load on first use, not with the app.)
import { META, SYNC_TAG, USER_KEY } from './keys';
import { isActive, type Job } from './jobs';
import type { Deps } from './queue';
import type { User } from '../api';

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
  try {
    await (await caches.open(META)).put(USER_KEY, new Response(String(u.id)));
    await (await Recent()).keepOnlyUser(u.id);
  } catch {}
}

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
  await logout().catch(() => (ended = false));
  try {
    if (ended) localStorage.removeItem(PENDING_LOGOUT);
    else localStorage.setItem(PENDING_LOGOUT, '1');
  } catch {}
  try {
    localStorage.removeItem(LAST_USER);
  } catch {}
  await caches.delete(META).catch(() => false);
  await recent.clearAll().catch(() => {});
  uploads.jobs = [];
  return ended;
}

/** Someone signed in: a pending sign-out is moot. */
export function clearPendingLogout() {
  try {
    localStorage.removeItem(PENDING_LOGOUT);
  } catch {}
}

/** A sign-out that couldn't reach the server: end that session now, before
 *  anything else. Resolves true when one was pending (the caller goes to the
 *  sign-in page). */
export async function finishPendingLogout(logout: () => Promise<unknown>): Promise<boolean> {
  try {
    if (!localStorage.getItem(PENDING_LOGOUT)) return false;
  } catch {
    return false;
  }
  try {
    await logout();
    localStorage.removeItem(PENDING_LOGOUT);
  } catch {}
  // (Still unreachable: stay signed out here, try again next time.)
  return true;
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
let draining: Promise<unknown> | null = null;
let again = false;
let onDone: ((j: Job) => void) | null = null;

const deps = (): Deps => ({
  fetch: (u, i) => fetch(u, i),
  chunk: async (b) => (await import('./chunks')).chunkFile(b),
  sleep: (a) => new Promise((r) => setTimeout(r, 1000 * 2 ** a)),
  online: () => navigator.onLine,
  changed: (j) => {
    changed(j);
    if (j.state === 'done') onDone?.(j);
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
        retryTimer = setTimeout(pump, (retryDelay = Math.min(retryDelay * 2, 5 * 60_000)));
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

/** Queue files for upload. */
export async function enqueue(user: number, items: { file: File; path: string }[]) {
  const queue = await Queue();
  const keeping: Promise<unknown>[] = [];
  for (const { file, path } of items) {
    const j = queue.newJob(user, path, file);
    // Shown and uploading at once; its bytes are stored for later (resume
    // after a reload) meanwhile. A job done before that finished has its
    // stored bytes dropped again.
    queue.holdBytes(j.id, file);
    await queue.save(j);
    changed(j);
    keeping.push(
      queue
        .keepBytes(j.id, file)
        .then(async () => {
          if (!(await queue.load(j.id))) await queue.remove(j.id);
        })
        .catch(() => {})
    );
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

export async function cancelUpload(id: string) {
  await (await Queue()).cancel(id, { fetch: (u, i) => fetch(u, i), changed });
  uploads.jobs = uploads.jobs.filter((j) => j.id !== id);
}

/** Retry a failed upload. False if its bytes are gone (add it again). */
export async function retryUpload(id: string): Promise<boolean> {
  const queue = await Queue();
  const j = await queue.load(id);
  if (!j) return false;
  if (!(await queue.hasBytes(id))) return false;
  const n = queue.step(j, { t: 'retry' });
  await queue.save(n);
  changed(n);
  pump();
  return true;
}

/** Done, failed and cancelled jobs leave the list (failed ones the queue too). */
export async function clearFinished() {
  const queue = await Queue();
  for (const j of uploads.jobs) if (j.state === 'failed') await queue.remove(j.id);
  uploads.jobs = uploads.jobs.filter((j) => isActive(j));
}

// ---------------------------------------------------------------- update

export const update = $state({ ready: false });
let waiting: ServiceWorker | null = null;

/** Watch for a new version of the app: it installs, waits, and the UI
 *  offers a reload (which hands over to it). */
export async function watchUpdates() {
  if (typeof navigator === 'undefined' || !navigator.serviceWorker) return;
  const reg = await navigator.serviceWorker.getRegistration();
  if (!reg) return;
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
  // A new worker took over (from this tab or another): reload, so no tab
  // keeps running the old app against the new worker and assets.
  // (Not for the first worker claiming a fresh page, unless asked to.)
  const hadController = !!navigator.serviceWorker.controller;
  let reloading = false;
  navigator.serviceWorker.addEventListener('controllerchange', () => {
    if (reloading || !(hadController || asked)) return;
    reloading = true;
    location.reload();
  });
  // Look for a new version when the app comes back to the foreground.
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') void reg.update().catch(() => {});
  });
}

let asked = false;
export function applyUpdate() {
  asked = true;
  if (!waiting) return location.reload();
  waiting.postMessage('skip-waiting');
}
