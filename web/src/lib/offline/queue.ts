// The upload queue: files waiting to upload, kept in IndexedDB (the bytes
// too), so an upload survives going offline, a reload or a closed tab. It
// resumes on app open, on the `online` event and, where the browser has it,
// from Background Sync in the service worker.
//
// Each job remembers its server upload session and the chunks the server
// confirmed, written after every chunk: a resumed upload asks the server
// what is still missing and sends only that, never a finished chunk again.

import { byUser, tx } from './idb';
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

/** The queue's state machine: the job after `e`. A transition that doesn't
 *  apply leaves the job as it was (same object). */
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
      const skipped = j.chunks.filter((c) => !missing.has(c.hash) && !j.sent.includes(c.hash)).reduce((a, c) => a + c.size, 0);
      return { ...j, session: e.id, sent: have, skipped: j.session === e.id ? j.skipped : skipped };
    }
    case 'sent':
      if (j.state !== 'uploading' || j.sent.includes(e.hash) || !j.chunks?.some((c) => c.hash === e.hash)) return j;
      return { ...j, sent: [...j.sent, e.hash] };
    case 'commit': {
      if (j.state !== 'uploading' || !j.chunks || !j.session) return j;
      const all = j.chunks.every((c) => j.sent.includes(c.hash));
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
      return { ...j, state: 'queued', error: undefined };
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

type Stored = Blob | { parts: ArrayBuffer[]; type: string };

/** Keep a job's bytes. Resolves how: 'blob', 'buffers' or 'memory'. */
export async function keepBytes(id: string, blob: Blob): Promise<'blob' | 'buffers' | 'memory'> {
  try {
    await tx('blobs', 'readwrite', (s) => void s.put(blob, id));
    return 'blob';
  } catch {
    if (blob.size <= BUFFER_MAX) {
      try {
        const parts: ArrayBuffer[] = [];
        for (let o = 0; o < blob.size; o += PART) parts.push(await blob.slice(o, o + PART).arrayBuffer());
        await tx('blobs', 'readwrite', (s) => void s.put({ parts, type: blob.type }, id));
        return 'buffers';
      } catch {
        // (fall through)
      }
    }
    inMemory.set(id, blob);
    return 'memory';
  }
}

async function bytesOf(id: string): Promise<Blob | null> {
  const m = inMemory.get(id);
  if (m) return m;
  const s = await tx<Stored | undefined>('blobs', 'readonly', (st) => st.get(id));
  if (!s) return null;
  return s instanceof Blob ? s : new Blob(s.parts, { type: s.type });
}

/** With its bytes; a job whose bytes are gone (memory, after a reload) fails. */
async function withBytes(j: Job): Promise<Job> {
  const blob = await bytesOf(j.id);
  if (blob) return { ...j, blob };
  return isActive(j) ? { ...j, blob: new Blob([]), state: 'failed', error: 'The file is no longer available here; add it again.' } : { ...j, blob: new Blob([]) };
}

export async function jobs(user: number): Promise<Job[]> {
  const all = await byUser<Job>('jobs', user);
  return Promise.all(all.sort((a, b) => a.created - b.created).map(withBytes));
}
/** Save a job's state (never its bytes: see `keepBytes`). */
export const save = (j: Job) => tx('jobs', 'readwrite', (s) => void s.put({ ...j, blob: undefined }));
export async function remove(id: string) {
  inMemory.delete(id);
  await tx('jobs', 'readwrite', (s) => void s.delete(id));
  await tx('blobs', 'readwrite', (s) => void s.delete(id));
}
export async function load(id: string): Promise<Job | undefined> {
  const j = await tx<Job | undefined>('jobs', 'readonly', (s) => s.get(id));
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
  chunk: (blob: Blob) => Promise<Chunk[]>;
  /** Waits before a retry (attempt 0, 1, ...). */
  sleep: (attempt: number) => Promise<void>;
  /** Called whenever a job changes (for the UI). */
  changed?: (j: Job) => void;
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

async function call(d: Deps, url: string, init: RequestInit = {}): Promise<Response> {
  for (let attempt = 0; ; attempt++) {
    let r: Response;
    try {
      r = await d.fetch(url, { credentials: 'same-origin', ...init, headers: { ...H, ...(init.headers ?? {}) } });
    } catch (e) {
      if ((e as Error)?.name === 'AbortError') throw e;
      throw new Offline('offline');
    }
    if (r.ok || r.status < 500 || attempt >= RETRIES) {
      if (r.ok) return r;
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
  let j = job;
  const live = () => {
    if (signal?.aborted) throw new DOMException('cancelled', 'AbortError');
  };
  const go = async (e: Event) => {
    // (Cancelled: `cancel` has dropped the job; never write it back.)
    live();
    const n = step(j, e);
    if (n !== j) {
      j = n;
      await save(j);
      d.changed?.(j);
    }
  };
  try {
    await go({ t: 'start' });
    if (j.state === 'hashing') {
      const chunks = await d.chunk(j.blob);
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
        const r = await call(d, '/api/uploads', {
          ...json({
            path: j.path,
            size: j.size,
            mtime: new Date(j.lastModified).toISOString(),
            chunks: j.chunks!.map((c) => ({ hash: c.hash, size: c.size }))
          }),
          signal
        });
        const b = await r.json();
        missing = b.missing;
        await go({ t: 'session', id: b.id, missing: b.missing });
      } else {
        await go({ t: 'session', id: j.session, missing: missing! });
      }
      // Send what the server is missing, each distinct chunk once.
      const todo = j.chunks!.filter((c, i, all) => !j.sent.includes(c.hash) && all.findIndex((x) => x.hash === c.hash) === i);
      for (const c of todo) {
        live();
        try {
          await call(d, `/api/uploads/${j.session}/chunks/${c.hash}`, {
            method: 'PUT',
            body: j.blob.slice(c.offset, c.offset + c.size),
            signal
          });
        } catch (e) {
          // The session expired meanwhile: start a new one.
          if (e instanceof HttpError && e.status === 404) {
            await go({ t: 'gone' });
            break;
          }
          throw e;
        }
        await go({ t: 'sent', hash: c.hash });
      }
      await go({ t: 'commit' });
    }
    if (j.state === 'committing') {
      live();
      try {
        await call(d, `/api/uploads/${j.session}/commit`, { ...json({}), signal });
        await go({ t: 'committed' });
      } catch (e) {
        if (!(e instanceof HttpError && e.status === 404)) throw e;
        if (restarts >= 2) throw e; // (never loop on a server that keeps losing it)
        await go({ t: 'gone' });
        return run(j, d, signal, restarts + 1);
      }
    }
    return j;
  } catch (e) {
    if (e instanceof Offline || (e as Error)?.name === 'AbortError') throw e;
    await go({ t: 'fail', error: e instanceof Error ? e.message : String(e) });
    return j;
  }
}

/** Run every active job of `user`, oldest first, one at a time (in one
 *  context at a time: a tab and the service worker never run the same
 *  queue together). Stops quietly when offline. */
export async function drain(user: number, d: Deps): Promise<'done' | 'offline'> {
  const work = async (): Promise<'done' | 'offline'> => {
    for (const j of await jobs(user)) {
      if (!isActive(j)) continue;
      const cur = await load(j.id); // (cancelled meanwhile?)
      if (!cur || !isActive(cur)) continue;
      const ctl = new AbortController();
      running.set(j.id, ctl);
      try {
        const out = await run(cur, d, ctl.signal);
        if (out.state === 'done') await remove(out.id);
      } catch (e) {
        if (e instanceof Offline) return 'offline';
        // Aborted: cancelled; `cancel` cleans up.
      } finally {
        running.delete(j.id);
      }
    }
    return 'done';
  };
  const locks = (globalThis.navigator as Navigator | undefined)?.locks;
  return locks ? locks.request('strata-uploads', work) : work();
}

const running = new Map<string, AbortController>();

/** Cancel a job: stop it if it is running, drop it and its server session. */
export async function cancel(id: string, d: Pick<Deps, 'fetch' | 'changed'>): Promise<void> {
  running.get(id)?.abort();
  const j = await load(id);
  if (!j) return;
  const c = step(j, { t: 'cancel' });
  await remove(id);
  d.changed?.(c);
  if (j.session) {
    await d
      .fetch(`/api/uploads/${j.session}`, { method: 'DELETE', headers: H, credentials: 'same-origin' })
      .catch(() => undefined);
  }
}

/** Sign-out: drop the server sessions of a user's jobs (best effort, while
 *  still signed in). The local queue goes with the offline database. */
export async function abandonSessions(user: number, f: typeof fetch): Promise<void> {
  for (const [, ctl] of running) ctl.abort();
  for (const j of await jobs(user)) {
    if (j.session) await f(`/api/uploads/${j.session}`, { method: 'DELETE', headers: H, credentials: 'same-origin' }).catch(() => undefined);
  }
}
