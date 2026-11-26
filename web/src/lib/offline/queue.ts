// The upload queue: files waiting to upload, kept in IndexedDB (the bytes
// too), so an upload survives going offline, a reload or a closed tab. It
// resumes on app open, on the `online` event and, where the browser has it,
// from Background Sync in the service worker.
//
// Each job remembers its server upload session and the chunks the server
// confirmed, written after every chunk: a resumed upload asks the server
// what is still missing and sends only that, never a finished chunk again.

import { byUser, db, tx } from './idb';
import { isActive, type Chunk, type Job, type State } from './jobs';

export * from './jobs';

export type Event =
  | { t: 'start' }
  | { t: 'hashed'; chunks: Chunk[] }
  | { t: 'session'; id: string; missing: number[] }
  | { t: 'sent'; hash: string }
  | { t: 'commit' }
  | { t: 'committed' }
  /** The server forgot the session (expired, or committed by an attempt
   *  whose answer was lost): start a new one. */
  | { t: 'gone' }
  | { t: 'fail'; error: string }
  | { t: 'retry' }
  | { t: 'cancel' };

// Membership sets for a job's arrays, kept beside them (a file can have
// tens of thousands of chunks: no scanning the arrays for every event).
const sets = new WeakMap<object, Set<string>>();
function setOf(arr: string[]): Set<string>;
function setOf(arr: Chunk[]): Set<string>;
function setOf(arr: (string | Chunk)[]): Set<string> {
  let s = sets.get(arr);
  if (!s) sets.set(arr, (s = new Set(arr.map((x) => (typeof x === 'string' ? x : x.hash)))));
  return s;
}

/** The queue's state machine: the job after `e`. A transition that doesn't
 *  apply leaves the job as it was (same object). A confirmed chunk is
 *  appended to `sent` in place (the old job is not used again). */
export function step(j: Job, e: Event): Job {
  switch (e.t) {
    case 'start':
      if (j.state !== 'queued') return j;
      return { ...j, state: j.chunks ? 'uploading' : 'hashing', error: undefined };
    case 'hashed':
      if (j.state !== 'hashing') return j;
      return { ...j, state: 'uploading', chunks: e.chunks };
    case 'session': {
      if (j.state !== 'uploading' || !j.chunks) return j;
      // What the server has: every chunk it doesn't list as missing.
      const missing = new Set(e.missing.map((i) => j.chunks![i]?.hash));
      const have = [...new Set(j.chunks.map((c) => c.hash).filter((h) => !missing.has(h)))];
      const sent = setOf(j.sent);
      const skipped = j.chunks.filter((c) => !missing.has(c.hash) && !sent.has(c.hash)).reduce((a, c) => a + c.size, 0);
      return { ...j, session: e.id, sent: have, skipped: j.session === e.id ? j.skipped : skipped };
    }
    case 'sent':
      if (j.state !== 'uploading' || !j.chunks || setOf(j.sent).has(e.hash) || !setOf(j.chunks).has(e.hash)) return j;
      setOf(j.sent).add(e.hash);
      j.sent.push(e.hash);
      return { ...j };
    case 'commit': {
      if (j.state !== 'uploading' || !j.chunks || !j.session) return j;
      const sent = setOf(j.sent);
      const all = j.chunks.every((c) => sent.has(c.hash));
      return all ? { ...j, state: 'committing' } : j;
    }
    case 'committed':
      if (j.state !== 'committing') return j;
      return { ...j, state: 'done' };
    case 'gone':
      if (j.state !== 'uploading' && j.state !== 'committing') return j;
      // Start over with a new session (the chunks stay hashed). Committing
      // the same bytes again is harmless; guessing "it went through" isn't.
      return { ...j, state: 'uploading', session: null, sent: [], skipped: 0 };
    case 'fail':
      if (!isActive(j)) return j;
      return { ...j, state: 'failed', error: e.error };
    case 'retry':
      if (j.state !== 'failed') return j;
      // A new session (the old one may be what failed: a changed folder).
      return { ...j, state: 'queued', error: undefined, session: null, sent: [], skipped: 0 };
    case 'cancel':
      if (!isActive(j) && j.state !== 'failed') return j;
      return { ...j, state: 'cancelled' };
  }
}

// ---------------------------------------------------------------- storage

// A job's bytes are stored once, apart from its state (saved after every
// chunk). Where the browser can't keep a Blob in IndexedDB (private modes),
// they are kept as plain buffers if not too big, else only in memory: such
// an upload resumes while the tab is open, not after a reload.

/** Up to this size, bytes that can't be stored as a Blob are stored as buffers. */
export const BUFFER_MAX = 256 * 1024 * 1024;
const PART = 16 * 1024 * 1024;
const inMemory = new Map<string, Blob>();

/** A job's bytes in the store: a Blob, buffers, or a mark that they were
 *  kept in memory only (by the page that queued it). */
type Stored = Blob | { parts: ArrayBuffer[]; type: string } | { memory: true };


// Where IndexedDB can't be used at all (site data blocked), the queue
// lives in this page's memory: uploads still run, they just can't outlive it.
let memJobs: Map<string, Job> | null = null;
let probing: Promise<boolean> | null = null;
async function memory(): Promise<Map<string, Job> | null> {
  probing ??= db().then(
    () => false,
    () => true
  );
  if (await probing) memJobs ??= new Map();
  return memJobs;
}

/** Keep a job's bytes. Resolves how: 'blob', 'buffers' or 'memory'. */
export async function keepBytes(id: string, blob: Blob): Promise<'blob' | 'buffers' | 'memory'> {
  // Signed out: nothing more is written here, and what was being written
  // as the sign-out began goes again.
  const signedOut = () => new DOMException('signed out', 'AbortError');
  const put = async (v: Stored) => {
    await tx('blobs', 'readwrite', (s) => void s.put(v, id));
    if (halted) {
      await tx('blobs', 'readwrite', (s) => void s.delete(id));
      throw signedOut();
    }
  };
  if (halted) throw signedOut();
  if (await memory()) {
    holdBytes(id, blob);
    return 'memory';
  }
  try {
    await put(blob);
    return 'blob';
  } catch (e) {
    if (halted) throw e;
    if (blob.size <= BUFFER_MAX) {
      try {
        const parts: ArrayBuffer[] = [];
        for (let o = 0; o < blob.size; o += PART) parts.push(await blob.slice(o, o + PART).arrayBuffer());
        await put({ parts, type: blob.type });
        return 'buffers';
      } catch (e2) {
        if (halted) throw e2;
      }
    }
    holdBytes(id, blob);
    await put({ memory: true }).catch((e3) => {
      if (halted) throw e3;
    });
    return 'memory';
  }
}

/** The bytes; 'pending' while they are still being stored (by the page
 *  that queued the job), null when they are gone. */
async function bytesOf(id: string): Promise<Blob | 'pending' | null> {
  const m = inMemory.get(id);
  if (m) return m;
  if (await memory()) return null;
  const s = await tx<Stored | undefined>('blobs', 'readonly', (st) => st.get(id));
  if (!s) return 'pending';
  if (s instanceof Blob) return s;
  if ('parts' in s) return new Blob(s.parts, { type: s.type });
  // In some page's memory: maybe another tab uploading it now. It waits
  // like a pending one, and is given up after the same grace.
  return 'pending';
}

export const GONE = 'The file is no longer available here; add it again.';

const NONE = new Blob([]);
/** A job whose bytes are still being stored elsewhere: not runnable yet. */
export const waiting = (j: Job) => j.blob === NONE && j.state !== 'committing';

/** With its bytes; a job whose bytes are gone (kept in memory only, and the
 *  page was reloaded) is failed for good, and saved so. */
async function withBytes(j: Job, now = Date.now()): Promise<Job> {
  // (No reading for finished jobs, nor for one with only its commit left:
  // the server has every chunk.)
  if (!isActive(j) || j.state === 'committing') return { ...j, blob: NONE };
  const blob = await bytesOf(j.id);
  if (blob instanceof Blob) return { ...j, blob };
  // Still being stored, or held in a tab's memory: it waits while that tab
  // is open (it holds the job's owner lock). With no owner left, or where
  // there are no locks after a long grace, the bytes are gone.
  if (blob === 'pending') {
    const alive = await ownerAlive(j.id);
    if (alive || (alive === null && now - j.created < PENDING_GRACE) || (alive === false && now - j.created < OWNERLESS_GRACE)) {
      return { ...j, blob: NONE };
    }
  }
  const lost: Job = { ...j, blob: new Blob([]), state: 'failed', error: GONE };
  await update(lost);
  return lost;
}

/** Are a job's bytes here, or held by a page that is still open? */
export async function hasBytes(id: string): Promise<boolean> {
  const b = await bytesOf(id);
  if (b instanceof Blob) return true;
  return b === 'pending' && (await ownerAlive(id)) === true;
}

/** Without Web Locks, how long bytes may stay unseen before they count as
 *  gone; with them, how long a job may have no owner (just queued). */
export const PENDING_GRACE = 60 * 60 * 1000;
export const OWNERLESS_GRACE = 2 * 60 * 1000;
const owners = new Map<string, () => void>();
const lockName = (id: string) => `strata-upload-owner-${id}`;
const locks = () => (globalThis.navigator as Navigator | undefined)?.locks;

/** Is some page holding this job's bytes in memory? null: can't tell. */
async function ownerAlive(id: string): Promise<boolean | null> {
  const l = locks();
  if (!l?.query) return null;
  const { held = [] } = await l.query();
  return held.some((h) => h.name === lockName(id));
}

/** Keep the bytes in memory for this page (it can upload them at once,
 *  while `keepBytes` stores them for later), and own the job meanwhile. */
const asking = new Set<string>();
export function holdBytes(id: string, blob: Blob) {
  inMemory.set(id, blob);
  const l = locks();
  // (Once per job: a second request would never be released.)
  if (l && !asking.has(id)) {
    asking.add(id);
    void l.request(lockName(id), () =>
      new Promise<void>((release) => {
        if (inMemory.has(id)) owners.set(id, release);
        else release(); // (removed before the lock came)
      })
    );
  }
}

/** A user's jobs, oldest first: their state only (their bytes are read by
 *  `load`, for the one about to run). */
export async function jobs(user: number): Promise<Job[]> {
  const m = await memory();
  const all = m ? [...m.values()].filter((j) => j.user === user) : await byUser<Job>('jobs', user);
  return all.sort((a, b) => a.created - b.created).map((j) => ({ ...j, blob: NONE }));
}
/** Save a job's state (never its bytes: see `keepBytes`). */
export async function save(j: Job) {
  const m = await memory();
  if (m) m.set(j.id, j);
  else await tx('jobs', 'readwrite', (s) => void s.put({ ...j, blob: undefined }));
}

/** Save a job's state only if it still exists: one cancelled or dropped
 *  (from any context) is never written back. Resolves whether it was. */
export async function update(j: Job): Promise<boolean> {
  const m = await memory();
  if (m) {
    if (!m.has(j.id)) return false;
    m.set(j.id, j);
    return true;
  }
  let there = false;
  await tx('jobs', 'readwrite', (s) => {
    const g = s.get(j.id);
    g.onsuccess = () => {
      there = g.result !== undefined;
      if (there) s.put({ ...j, blob: undefined });
    };
  });
  return there;
}
export async function remove(id: string) {
  inMemory.delete(id);
  owners.get(id)?.();
  owners.delete(id);
  asking.delete(id);
  const m = await memory();
  if (m) return void m.delete(id);
  await tx('jobs', 'readwrite', (s) => void s.delete(id));
  await tx('blobs', 'readwrite', (s) => void s.delete(id));
}
/** Is the job still queued (without reading its bytes)? */
export async function exists(id: string): Promise<boolean> {
  const m = await memory();
  if (m) return m.has(id);
  return (await tx<number>('jobs', 'readonly', (s) => s.count(id))) > 0;
}

export async function load(id: string): Promise<Job | undefined> {
  const m = await memory();
  const j = m ? m.get(id) : await tx<Job | undefined>('jobs', 'readonly', (s) => s.get(id));
  return j && withBytes(j);
}

export function newJob(user: number, path: string, file: Blob & { lastModified?: number }, now = Date.now()): Job {
  return {
    id: `${now.toString(36)}-${Math.random().toString(36).slice(2, 10)}`,
    user,
    path,
    blob: file,
    size: file.size,
    lastModified: file.lastModified ?? now,
    state: 'queued',
    session: null,
    chunks: null,
    sent: [],
    skipped: 0,
    created: now
  };
}

// ---------------------------------------------------------------- running

/** What the runner needs from the outside (the network, the chunker). */
export interface Deps {
  fetch: typeof fetch;
  chunk: (blob: Blob, signal?: AbortSignal) => Promise<Chunk[]>;
  /** Waits before a retry (attempt 0, 1, ...). */
  sleep: (attempt: number) => Promise<void>;
  /** Called whenever a job changes (for the UI). */
  changed?: (j: Job) => void;
  /** Whether the device is online (a failed request is then retried). */
  online?: () => boolean;
  /** Who the server says is signed in (null: can't tell). The queue only
   *  runs a user's jobs while it is them: the cookie may belong to someone
   *  else by now (signed in in another tab). */
  whoami?: () => Promise<number | null>;
}

/** The network went away: stop, keep the job as it is, resume later. */
export class Offline extends Error {}

class HttpError extends Error {
  constructor(public status: number, message: string) {
    super(message);
  }
}

const H = { 'x-strata': '1' };
const RETRIES = 4;
/** Gateway answers that mean the server itself can't be reached. */
export const UNREACHABLE = new Set([502, 503, 504, 520, 521, 522, 523, 524, 525, 526, 530]);
/** Chunks in flight at once. */
const PARALLEL = 3;
/** Confirmed chunks are saved at most this often (ms). */
export const SAVE_EVERY = 1000;

async function call(d: Deps, url: string, init: RequestInit = {}): Promise<Response> {
  for (let attempt = 0; ; attempt++) {
    let r: Response;
    try {
      r = await d.fetch(url, { credentials: 'same-origin', ...init, headers: { ...H, ...(init.headers ?? {}) } });
    } catch (e) {
      if ((e as Error)?.name === 'AbortError') throw e;
      // A blip (a server restart, a dropped connection) while the device
      // is online is retried; offline, the queue waits for the network.
      if (!d.online?.() || attempt >= RETRIES) throw new Offline('offline');
      await d.sleep(attempt);
      continue;
    }
    if (r.ok || r.status < 500 || attempt >= RETRIES) {
      if (r.ok) return r;
      // A proxy answering for an unreachable server (restarting, a tunnel
      // down): that is "offline" too; the job waits instead of failing.
      // (The server's own 503, "this storage is offline", is JSON: that is a
      // real answer, about this job only.)
      const own = (r.headers.get('content-type') ?? '').includes('application/json');
      if (UNREACHABLE.has(r.status) && !own) throw new Offline(`HTTP ${r.status}`);
      // Signed out meanwhile (the session expired): wait for a sign-in.
      if (r.status === 401) throw new Offline('signed out');
      const msg = await r
        .json()
        .then((b) => (b as { error?: string }).error)
        .catch(() => undefined);
      throw new HttpError(r.status, msg ?? `HTTP ${r.status}`);
    }
    await d.sleep(attempt);
  }
}

const json = (body: unknown): RequestInit => ({
  method: 'POST',
  headers: { 'content-type': 'application/json' },
  body: JSON.stringify(body)
});

/** Run one job as far as it goes. Throws `Offline` when the network fails
 *  (the job stays resumable); any other failure marks it failed. */
export async function run(job: Job, d: Deps, signal?: AbortSignal, restarts = 0): Promise<Job> {
  // (No bytes here to send: never run, it would send empty chunks.)
  if (waiting(job)) return job;
  let j = job;
  const live = () => {
    if (signal?.aborted) throw new DOMException('cancelled', 'AbortError');
  };
  // Saved after every change, except confirmed chunks: those at most every
  // SAVE_EVERY (a job of thousands of chunks would otherwise be rewritten
  // thousands of times). Nothing is lost by that: a resumed upload asks the
  // server what it is missing.
  let lastSave = 0;
  let dirty = false;
  const persist = async () => {
    dirty = false;
    lastSave = Date.now();
    // Cancelled meanwhile (here or in another tab): stop, write nothing.
    if (!(await update(j))) throw new DOMException('cancelled', 'AbortError');
    d.changed?.(j);
  };
  const go = async (e: Event) => {
    live();
    const n = step(j, e);
    if (n === j) return;
    j = n;
    if (e.t === 'sent' && Date.now() - lastSave < SAVE_EVERY) dirty = true;
    else await persist();
  };
  try {
    await go({ t: 'start' });
    if (j.state === 'hashing') {
      const chunks = await d.chunk(j.blob, signal);
      live();
      await go({ t: 'hashed', chunks });
    }
    while (j.state === 'uploading') {
      live();
      // The session: the one we have (what is it missing?) or a new one.
      let missing: number[] | null = null;
      if (j.session) {
        try {
          missing = (await (await call(d, `/api/uploads/${j.session}`, { signal })).json()).missing;
        } catch (e) {
          if (!(e instanceof HttpError && e.status === 404)) throw e;
          await go({ t: 'gone' });
        }
      }
      if (!j.session) {
        // (Dropped meanwhile, e.g. by a sign-out elsewhere: no new session.)
        if (!(await exists(j.id))) throw new DOMException('cancelled', 'AbortError');
        // A new session is opened as whoever the cookie belongs to: only
        // while that is still this job's user.
        if (d.whoami && (await d.whoami()) !== j.user) throw new Offline('signed in as someone else');
        const r = await call(d, '/api/uploads', {
          ...json({
            path: j.path,
            size: j.size,
            mtime: new Date(j.lastModified).toISOString(),
            chunks: j.chunks!.map((c) => ({ hash: c.hash, size: c.size })),
            // An upload replaces a file of that name (it goes to the trash
            // where there is one). (Asking first comes with the conflict
            // dialog.)
            on_conflict: 'replace'
          }),
          signal
        });
        const b = await r.json();
        missing = b.missing;
        await go({ t: 'session', id: b.id, missing: b.missing });
      } else {
        await go({ t: 'session', id: j.session, missing: missing! });
      }
      // Send what the server is missing, each distinct chunk once, a few
      // at a time; each is recorded as soon as the server confirms it.
      const have = new Set(j.sent);
      const todo = j.chunks!.filter((c) => !have.has(c.hash) && (have.add(c.hash), true));
      const session = j.session;
      let next = 0;
      let lost = false;
      let failed = false;
      const worker = async () => {
        try {
          while (next < todo.length && !lost && !failed) {
            const c = todo[next++];
            live();
            // Read first: a file that can't be read (changed or gone on
            // disk) fails this upload, rather than passing for the network.
            let body: ArrayBuffer;
            try {
              body = await j.blob.slice(c.offset, c.offset + c.size).arrayBuffer();
            } catch (e) {
              throw new Error(`The file can't be read any more (${(e as Error)?.name ?? 'error'}); add it again.`);
            }
            try {
              await call(d, `/api/uploads/${session}/chunks/${c.hash}`, { method: 'PUT', body, signal });
            } catch (e) {
              // The session expired meanwhile: start a new one.
              if (e instanceof HttpError && e.status === 404) {
                lost = true;
                return;
              }
              throw e;
            }
            if (!lost) await go({ t: 'sent', hash: c.hash });
          }
        } catch (e) {
          failed = true; // (the others stop taking chunks)
          throw e;
        }
      };
      // Every worker has stopped before this run ends (none keeps sending
      // behind the next run's back); the first failure is then reported.
      const done = await Promise.allSettled(Array.from({ length: PARALLEL }, worker));
      const bad = done.find((r): r is PromiseRejectedResult => r.status === 'rejected');
      if (bad) throw bad.reason;
      if (lost) {
        // (A server that keeps losing sessions fails the job: no endless loop.)
        if (restarts >= 2) throw new Error('The server keeps losing this upload; try again later.');
        restarts++;
        await go({ t: 'gone' });
      }
      else await go({ t: 'commit' });
    }
    if (j.state === 'committing') {
      live();
      try {
        await call(d, `/api/uploads/${j.session}/commit`, { ...json({ on_conflict: 'replace' }), signal });
        await go({ t: 'committed' });
      } catch (e) {
        if (!(e instanceof HttpError && e.status === 404)) throw e;
        if (restarts >= 2) throw e; // (never loop on a server that keeps losing it)
        await go({ t: 'gone' });
        // A new session needs the chunks again: with the bytes, read anew
        // (a job resumed at its commit was loaded without them).
        const again = await load(j.id);
        if (!again) throw new DOMException('cancelled', 'AbortError'); // (cancelled just now)
        // (Its bytes gone meanwhile: it is failed now; say so.)
        if (again.state === 'failed') d.changed?.(again);
        return run(again, d, signal, restarts + 1);
      }
    }
    return j;
  } catch (e) {
    if (e instanceof Offline) {
      if (dirty) await persist().catch(() => {}); // (what was confirmed so far)
      throw e;
    }
    if ((e as Error)?.name === 'AbortError') throw e;
    await go({ t: 'fail', error: e instanceof Error ? e.message : String(e) });
    return j;
  }
}

/** Run every active job of `user`, oldest first, one at a time (in one
 *  context at a time: a tab and the service worker never run the same
 *  queue together). Stops quietly when offline. */
/** `stop`: this run of the queue ends (another user signed in here); what
 *  is in flight is aborted and nothing more starts. */
export async function drain(user: number, d: Deps, stop?: AbortSignal): Promise<'done' | 'offline'> {
  const work = async (): Promise<'done' | 'offline'> => {
    for (const j of await jobs(user)) {
      if (halted || stop?.aborted) break;
      if (!isActive(j)) continue;
      const cur = await load(j.id); // (cancelled meanwhile?)
      if (cur && cur.state === 'failed' && isActive(j)) d.changed?.(cur); // (its bytes are gone)
      if (!cur || !isActive(cur) || waiting(cur)) continue;
      if (d.whoami && (await d.whoami()) !== user) return stop?.aborted ? 'done' : 'offline'; // (not theirs now: wait)
      const ctl = new AbortController();
      const abort = () => ctl.abort();
      stop?.addEventListener('abort', abort);
      if (stop?.aborted) ctl.abort(); // (stopped while this job was loading)
      running.set(j.id, ctl);
      try {
        const out = await run(cur, d, ctl.signal);
        if (out.state === 'done') await remove(out.id);
      } catch (e) {
        if (e instanceof Offline) return 'offline';
        // Aborted: cancelled; `cancel` cleans up.
      } finally {
        running.delete(j.id);
        stop?.removeEventListener('abort', abort);
      }
    }
    return 'done';
  };
  const locks = (globalThis.navigator as Navigator | undefined)?.locks;
  if (!locks) return work();
  // (A stop also ends the wait for the lock.)
  return locks.request('strata-uploads', stop ? { signal: stop } : {}, work).catch((e) => {
    if ((e as Error)?.name === 'AbortError') return 'done' as const;
    throw e;
  });
}

const running = new Map<string, AbortController>();
let halted = false;

/** Sign-out: stop this page's queue for good (it reloads to the sign-in
 *  page next) and abort what is in flight. */
export function halt() {
  halted = true;
  for (const [, ctl] of running) ctl.abort();
}

/** Cancel a job: stop it if it is running, drop it and its server session. */
/** Resolves false when it is too late: the file is being saved (here or
 *  in another context), so cancelling could only pretend. */
export async function cancel(id: string, d: Pick<Deps, 'fetch' | 'changed'>): Promise<boolean> {
  // Checked and removed in one transaction: a runner (any context) writes
  // 'committing' before it sends the commit, and writes nothing once the
  // job is gone, so the two can't both go ahead.
  const m = await memory();
  let j: Job | undefined;
  let late = false;
  if (m) {
    j = m.get(id);
    late = j?.state === 'committing';
    if (!late) m.delete(id);
  } else {
    await tx('jobs', 'readwrite', (s) => {
      const g = s.get(id);
      g.onsuccess = () => {
        j = g.result;
        late = j?.state === 'committing';
        if (j && !late) s.delete(id);
      };
    });
  }
  if (late) return false;
  running.get(id)?.abort();
  await remove(id); // (its bytes and owner lock)
  if (!j) return true;
  d.changed?.(step(j, { t: 'cancel' }));
  if (j.session) {
    await d
      .fetch(`/api/uploads/${j.session}`, { method: 'DELETE', headers: H, credentials: 'same-origin' })
      .catch(() => undefined);
  }
  return true;
}

/** Sign-out: drop the server sessions of a user's jobs (best effort, while
 *  still signed in). The local queue goes with the offline database. */
export async function abandonSessions(user: number, f: typeof fetch): Promise<void> {
  for (const [, ctl] of running) ctl.abort();
  // The jobs go first: a runner in another context then stops at its next
  // save, and never opens a new session for them.
  const all = await jobs(user);
  for (const j of all) await remove(j.id);
  for (const j of all) {
    if (j.session) await f(`/api/uploads/${j.session}`, { method: 'DELETE', headers: H, credentials: 'same-origin' }).catch(() => undefined);
  }
}
