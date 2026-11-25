import 'fake-indexeddb/auto';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as q from './queue';
import { tx, wipe } from './idb';

const chunks: q.Chunk[] = [
  { offset: 0, size: 4, hash: 'aa' },
  { offset: 4, size: 4, hash: 'bb' },
  { offset: 8, size: 4, hash: 'aa' }, // a repeat of the first
  { offset: 12, size: 2, hash: 'cc' }
];
const blob = () => new Blob([new Uint8Array(14)]);

/** A fake upload server: sessions, what each has, every request seen. */
class Server {
  sessions = new Map<string, Set<string>>();
  have = new Set<string>(); // chunks the user stored before
  puts: string[] = [];
  committed: string[] = [];
  log: string[] = [];
  n = 0;
  /** Throw a network error on the request matching this, once. */
  dropOnce: ((url: string, method: string) => boolean) | null = null;
  /** Answer with this status for requests matching. */
  fail: ((url: string, method: string) => number | null) | null = null;
  fetch = vi.fn(async (input: RequestInfo | URL, init: RequestInit = {}) => {
    const url = String(input);
    const method = init.method ?? 'GET';
    this.log.push(`${method} ${url}`);
    if (init.signal?.aborted) throw new DOMException('aborted', 'AbortError');
    if (this.dropOnce?.(url, method)) {
      this.dropOnce = null;
      throw new TypeError('Failed to fetch');
    }
    const st = this.fail?.(url, method);
    if (st) return Response.json({ error: `status ${st}` }, { status: st });
    const missing = (id: string) =>
      chunks.map((c, i) => [c, i] as const).filter(([c]) => !this.sessions.get(id)!.has(c.hash) && !this.have.has(c.hash)).map(([, i]) => i);
    let m: RegExpMatchArray | null;
    if (url === '/api/uploads' && method === 'POST') {
      const id = `s${++this.n}`;
      this.sessions.set(id, new Set());
      return Response.json({ id, missing: missing(id) });
    }
    if ((m = url.match(/^\/api\/uploads\/(\w+)$/)) && method === 'GET') {
      if (!this.sessions.has(m[1])) return Response.json({ error: 'gone' }, { status: 404 });
      return Response.json({ id: m[1], missing: missing(m[1]) });
    }
    if ((m = url.match(/^\/api\/uploads\/(\w+)$/)) && method === 'DELETE') {
      this.sessions.delete(m[1]);
      return Response.json({});
    }
    if ((m = url.match(/^\/api\/uploads\/(\w+)\/chunks\/(\w+)$/)) && method === 'PUT') {
      if (!this.sessions.has(m[1])) return Response.json({ error: 'gone' }, { status: 404 });
      this.sessions.get(m[1])!.add(m[2]);
      this.puts.push(m[2]);
      return Response.json({});
    }
    if ((m = url.match(/^\/api\/uploads\/(\w+)\/commit$/)) && method === 'POST') {
      if (!this.sessions.has(m[1])) return Response.json({ error: 'gone' }, { status: 404 });
      if (missing(m[1]).length) return Response.json({ error: 'missing' }, { status: 409 });
      this.sessions.delete(m[1]);
      this.committed.push(m[1]);
      return Response.json({});
    }
    return Response.json({ error: 'no route' }, { status: 400 });
  });
}

let srv: Server;
let changes: q.Job[];
const deps = (over: Partial<q.Deps> = {}): q.Deps => ({
  fetch: srv.fetch as unknown as typeof fetch,
  chunk: async () => chunks,
  sleep: async () => {},
  changed: (j) => changes.push(j),
  ...over
});
const job = (over: Partial<q.Job> = {}) => ({ ...q.newJob(1, 'dir/f.bin', Object.assign(blob(), { lastModified: 5 }), 1000), ...over });

const add = async (j: q.Job) => {
  await q.keepBytes(j.id, j.blob);
  await q.save(j);
};
const fresh = async (over: Partial<q.Job> = {}) => {
  const j = job(over);
  await add(j);
  return j;
};

beforeEach(async () => {
  await wipe();
  srv = new Server();
  changes = [];
});
afterEach(() => vi.unstubAllGlobals());

describe('state machine', () => {
  const base = job();
  it('walks queued → hashing → uploading → committing → done', () => {
    let j = q.step(base, { t: 'start' });
    expect(j.state).toBe('hashing');
    j = q.step(j, { t: 'hashed', chunks });
    expect(j.state).toBe('uploading');
    j = q.step(j, { t: 'session', id: 's1', missing: [0, 1, 3] });
    expect(j).toMatchObject({ session: 's1', sent: [], skipped: 0 });
    for (const h of ['aa', 'bb']) j = q.step(j, { t: 'sent', hash: h });
    expect(q.step(j, { t: 'commit' })).toBe(j); // cc not sent yet
    j = q.step(j, { t: 'sent', hash: 'cc' });
    j = q.step(j, { t: 'commit' });
    expect(j.state).toBe('committing');
    j = q.step(j, { t: 'committed' });
    expect(j.state).toBe('done');
    expect(q.progress(j)).toBe(1);
  });

  it('a job with its chunks starts straight at uploading', () => {
    expect(q.step(job({ chunks }), { t: 'start' }).state).toBe('uploading');
  });

  it('counts what the server already had as skipped, once per session', () => {
    let j = q.step(job({ state: 'uploading', chunks }), { t: 'session', id: 's1', missing: [3] });
    expect(j.sent.sort()).toEqual(['aa', 'bb']);
    expect(j.skipped).toBe(12);
    // The same session again (resume): skipped unchanged.
    j = q.step(j, { t: 'session', id: 's1', missing: [3] });
    expect(j.skipped).toBe(12);
  });

  it('never records a chunk twice, nor one that is not in the file', () => {
    const j = q.step(job({ state: 'uploading', chunks }), { t: 'sent', hash: 'aa' });
    expect(q.step(j, { t: 'sent', hash: 'aa' })).toBe(j);
    expect(q.step(j, { t: 'sent', hash: 'zz' })).toBe(j);
  });

  it('a lost session starts over (chunks kept), from uploading or committing', () => {
    const j = job({ state: 'committing', chunks, session: 's1', sent: ['aa', 'bb', 'cc'], skipped: 3 });
    expect(q.step(j, { t: 'gone' })).toMatchObject({ state: 'uploading', session: null, sent: [], skipped: 0, chunks });
    expect(q.step(job(), { t: 'gone' }).state).toBe('queued');
  });

  it('fail, retry and cancel', () => {
    const f = q.step(job({ state: 'uploading' }), { t: 'fail', error: 'boom' });
    expect(f).toMatchObject({ state: 'failed', error: 'boom' });
    expect(q.step(f, { t: 'fail', error: 'again' })).toBe(f);
    const r = q.step(f, { t: 'retry' });
    expect(r).toMatchObject({ state: 'queued', error: undefined, session: null, sent: [], skipped: 0 });
    expect(q.step(r, { t: 'retry' })).toBe(r);
    expect(q.step(f, { t: 'cancel' }).state).toBe('cancelled');
    const done = job({ state: 'done' });
    expect(q.step(done, { t: 'cancel' })).toBe(done);
  });

  it('ignores events that do not apply', () => {
    const u = job({ state: 'uploading' });
    for (const e of [{ t: 'start' }, { t: 'hashed', chunks }, { t: 'committed' }] as q.Event[]) expect(q.step(u, e)).toBe(u);
    expect(q.step(u, { t: 'session', id: 's', missing: [] })).toBe(u); // no chunks yet
    expect(q.step(job({ state: 'uploading', chunks }), { t: 'commit' }).state).toBe('uploading'); // no session
    expect(q.step(job({ state: 'uploading', chunks: null, session: 's' }), { t: 'commit' }).state).toBe('uploading');
  });

  it('progress and done bytes', () => {
    expect(q.progress(job())).toBe(0);
    expect(q.doneBytes(job())).toBe(0);
    const j = job({ state: 'uploading', chunks, sent: ['aa'] });
    expect(q.doneBytes(j)).toBe(8); // both copies of aa
    expect(q.progress(j)).toBeCloseTo(8 / 14);
  });

  it('new jobs get unique ids and the file’s time', () => {
    const a = q.newJob(1, 'p', new Blob(['x']));
    const b = q.newJob(1, 'p', new Blob(['x']));
    expect(a.id).not.toBe(b.id);
    expect(a.lastModified).toBe(a.created);
  });
});

describe('runner', () => {
  it('uploads each distinct missing chunk once, then commits, saving as it goes', async () => {
    const j = job();
    await add(j);
    const out = await q.run(j, deps());
    expect(out.state).toBe('done');
    expect(srv.puts).toEqual(['aa', 'bb', 'cc']);
    expect(srv.committed).toEqual(['s1']);
    expect((await q.load(j.id))!.state).toBe('done');
    expect(changes.map((c) => c.state)).toContain('hashing');
  });

  it('chunks the server already has are not sent', async () => {
    srv.have.add('bb');
    const out = await q.run(await fresh(), deps());
    expect(srv.puts).toEqual(['aa', 'cc']);
    expect(out.skipped).toBe(4);
  });

  it('interrupted mid-upload, it resumes without re-sending finished chunks', async () => {
    const j = job();
    await add(j);
    srv.dropOnce = (u, m) => m === 'PUT' && u.endsWith('/bb');
    await expect(q.run(j, deps())).rejects.toBeInstanceOf(q.Offline);
    const saved = (await q.load(j.id))!;
    expect(saved).toMatchObject({ state: 'uploading', session: 's1' });
    expect(saved.sent).toContain('aa');
    expect(saved.sent).not.toContain('bb');
    // Later (another tab, the worker): from what was saved.
    const out = await q.run(saved, deps());
    expect(out.state).toBe('done');
    expect([...srv.puts].sort()).toEqual(['aa', 'bb', 'cc']); // each once
    expect(srv.log.filter((l) => l.startsWith('POST /api/uploads') && !l.includes('commit'))).toHaveLength(1);
  });

  it('the server forgetting the session (before, during, or at commit) starts a new one', async () => {
    // Expired while away: GET says 404.
    const a = await q.run(await fresh({ state: 'uploading', chunks, session: 'old', sent: ['aa'] }), deps());
    expect(a.state).toBe('done');
    expect(srv.puts).toEqual(['aa', 'bb', 'cc']);
    // Expired mid-upload: a chunk PUT says 404.
    srv = new Server();
    let once = true;
    srv.fail = (u, m) => (m === 'PUT' && u.endsWith('/bb') && once ? ((once = false), 404) : null);
    expect((await q.run(await fresh(), deps())).state).toBe('done');
    // Committed by an attempt whose answer was lost: commit says 404; the
    // same bytes are committed again in a new session.
    srv = new Server();
    let lost = true;
    srv.fail = (u, m) => (u.endsWith('/commit') && m === 'POST' && lost ? ((lost = false), 404) : null);
    expect((await q.run(await fresh(), deps())).state).toBe('done');
    expect(srv.committed).toEqual(['s2']);
  });

  it('a server that keeps losing the commit fails the job instead of looping', async () => {
    srv.fail = (u, m) => (u.endsWith('/commit') && m === 'POST' ? 404 : null);
    const out = await q.run(await fresh(), deps());
    expect(out.state).toBe('failed');
    expect(srv.log.filter((l) => l.endsWith('/commit'))).toHaveLength(3);
  });

  it('retries server errors with backoff, then fails', async () => {
    const sleep = vi.fn(async () => {});
    let n = 0;
    srv.fail = (u, m) => (m === 'PUT' && n++ < 2 ? 503 : null);
    expect((await q.run(await fresh(), deps({ sleep }))).state).toBe('done');
    expect(sleep).toHaveBeenCalledTimes(2);
    srv = new Server();
    srv.fail = (u, m) => (m === 'PUT' ? 500 : null);
    const f = await q.run(await fresh(), deps({ sleep }));
    expect(f).toMatchObject({ state: 'failed', error: 'status 500' });
  });

  it('a refused request fails the job with the server’s message', async () => {
    srv.fail = (u, m) => (u === '/api/uploads' && m === 'POST' ? 403 : null);
    expect(await q.run(await fresh(), deps())).toMatchObject({ state: 'failed', error: 'status 403' });
    // A refusal with no readable body still says something.
    srv = new Server();
    const f = (async () => new Response('<html>', { status: 400, statusText: 'Bad' })) as unknown as typeof fetch;
    expect((await q.run(await fresh(), deps({ fetch: f }))).error).toBe('HTTP 400');
  });

  it('a finished or failed job runs no further', async () => {
    const f = job({ state: 'failed', error: 'x' });
    expect(await q.run(f, deps())).toBe(f);
    expect(srv.log).toEqual([]);
  });

  it('a hashing failure fails the job', async () => {
    const out = await q.run(await fresh(), deps({ chunk: async () => { throw 'unreadable'; } }));
    expect(out).toMatchObject({ state: 'failed', error: 'unreadable' });
  });

  it('goes offline at the session check, and at commit', async () => {
    srv.dropOnce = (u, m) => m === 'GET';
    await expect(q.run(await fresh({ state: 'uploading', chunks, session: 's9' }), deps())).rejects.toBeInstanceOf(q.Offline);
    srv.dropOnce = (u) => u.endsWith('/commit');
    await expect(q.run(await fresh(), deps())).rejects.toBeInstanceOf(q.Offline);
  });

  it('an aborted run neither saves nor fails the job', async () => {
    const j = job();
    await add(j);
    const ctl = new AbortController();
    ctl.abort();
    await expect(q.run(j, deps(), ctl.signal)).rejects.toMatchObject({ name: 'AbortError' });
    expect((await q.load(j.id))!.state).toBe('queued');
    // Aborted mid-request: the fetch rejects with AbortError.
    const c2 = new AbortController();
    const f = (async (_u: string, i: RequestInit) => {
      c2.abort();
      throw Object.assign(new Error('aborted'), { name: 'AbortError' });
    }) as unknown as typeof fetch;
    await expect(q.run(await fresh({ chunks }), deps({ fetch: f }), c2.signal)).rejects.toMatchObject({ name: 'AbortError' });
  });
});

describe('queue', () => {
  it('drains the user’s active jobs in order, removing finished ones', async () => {
    const a = job({ id: 'a', created: 1 });
    const b = job({ id: 'b', created: 2 });
    const other = { ...job({ id: 'c', created: 0 }), user: 2 };
    const failed = job({ id: 'f', created: 3, state: 'failed' });
    for (const j of [b, a, other, failed]) await add(j);
    expect(await q.drain(1, deps())).toBe('done');
    expect((await q.jobs(1)).map((j) => j.id)).toEqual(['f']);
    expect((await q.jobs(2)).map((j) => j.id)).toEqual(['c']);
    expect(srv.committed).toEqual(['s1', 's2']);
  });

  it('stops when offline and picks up later', async () => {
    await add(job({ id: 'a' }));
    srv.dropOnce = (u, m) => m === 'POST';
    expect(await q.drain(1, deps())).toBe('offline');
    expect((await q.load('a'))!.state).toBe('uploading');
    expect(await q.drain(1, deps())).toBe('done');
    expect(await q.jobs(1)).toEqual([]);
  });

  it('a job that fails stays in the queue (for retry)', async () => {
    await add(job({ id: 'a' }));
    srv.fail = (u, m) => (u === '/api/uploads' && m === 'POST' ? 403 : null);
    expect(await q.drain(1, deps())).toBe('done');
    expect((await q.load('a'))!.state).toBe('failed');
  });

  it('skips a job cancelled while the queue was running', async () => {
    await add(job({ id: 'a', created: 1 }));
    await add(job({ id: 'b', created: 2 }));
    const d = deps({
      changed: (j) => {
        if (j.id === 'a' && j.state === 'done') void q.remove('b');
      }
    });
    expect(await q.drain(1, d)).toBe('done');
    expect(srv.committed).toHaveLength(1);
  });

  it('runs under a lock where the browser has Web Locks', async () => {
    const request = vi.fn(async (_n: string, opts: { signal?: AbortSignal }, fn: () => Promise<unknown>) => {
      if (opts.signal?.aborted) throw new DOMException('aborted', 'AbortError');
      return fn();
    });
    vi.stubGlobal('navigator', { locks: { request } });
    await add(job({ id: 'a' }));
    expect(await q.drain(1, deps())).toBe('done');
    expect(request).toHaveBeenCalledWith('strata-uploads', {}, expect.any(Function));
    // A stop ends the wait for the lock too.
    await add(job({ id: 'b' }));
    const stop = new AbortController();
    stop.abort();
    expect(await q.drain(1, deps(), stop.signal)).toBe('done');
    expect(await q.exists('b')).toBe(true);
    // Any other lock failure is reported.
    request.mockImplementationOnce(async () => {
      throw new Error('lock broke');
    });
    await expect(q.drain(1, deps())).rejects.toThrow('lock broke');
  });

  it('cancel stops a running job, drops it and its server session', async () => {
    await add(job({ id: 'a' }));
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const slow = (async (u: RequestInfo | URL, i: RequestInit = {}) => {
      if (i.method === 'PUT') {
        await gate;
        if (i.signal?.aborted) throw Object.assign(new Error('aborted'), { name: 'AbortError' });
      }
      return srv.fetch(u, i);
    }) as unknown as typeof fetch;
    const drained = q.drain(1, deps({ fetch: slow }));
    await vi.waitFor(() => expect(srv.sessions.size).toBe(1));
    const seen: q.Job[] = [];
    await q.cancel('a', { fetch: srv.fetch as unknown as typeof fetch, changed: (j) => seen.push(j) });
    release();
    expect(await drained).toBe('done');
    expect(await q.load('a')).toBeUndefined();
    expect(srv.sessions.size).toBe(0);
    expect(seen.at(-1)!.state).toBe('cancelled');
    expect(srv.committed).toEqual([]);
  });

  it('cancel of a missing job, or one with no session, is quiet', async () => {
    await q.cancel('nope', { fetch: srv.fetch as unknown as typeof fetch });
    await add(job({ id: 'x' }));
    await q.cancel('x', { fetch: srv.fetch as unknown as typeof fetch });
    expect(srv.log).toEqual([]);
    // A server that can't be reached doesn't stop a cancel.
    await add(job({ id: 'y', session: 's1' }));
    const down = (async () => { throw new TypeError('offline'); }) as unknown as typeof fetch;
    await q.cancel('y', { fetch: down });
    expect(await q.load('y')).toBeUndefined();
  });

  it('sign-out stops a running upload', async () => {
    await add(job({ id: 'a' }));
    let entered!: () => void;
    const inPut = new Promise<void>((r) => (entered = r));
    const slow = (async (u: RequestInfo | URL, i: RequestInit = {}) => {
      if (i.method === 'PUT') {
        entered();
        await new Promise((r) => setTimeout(r, 20));
        if (i.signal?.aborted) throw Object.assign(new Error('aborted'), { name: 'AbortError' });
      }
      return srv.fetch(u, i);
    }) as unknown as typeof fetch;
    const drained = q.drain(1, deps({ fetch: slow }));
    await inPut;
    await q.abandonSessions(1, srv.fetch as unknown as typeof fetch);
    await drained;
    expect(srv.committed).toEqual([]);
    expect(srv.sessions.size).toBe(0);
  });

  it('sign-out abandons the user’s server sessions', async () => {
    srv.sessions.set('s1', new Set());
    await add(job({ id: 'a', session: 's1' }));
    await add(job({ id: 'b' }));
    await q.abandonSessions(1, srv.fetch as unknown as typeof fetch);
    expect(srv.log).toEqual(['DELETE /api/uploads/s1']);
    expect(await q.jobs(1)).toEqual([]); // (the jobs go first)
    await add(job({ id: 'c', session: 's2' }));
    const down = (async () => { throw new TypeError('offline'); }) as unknown as typeof fetch;
    await expect(q.abandonSessions(1, down)).resolves.toBeUndefined();
  });
});

describe('stored bytes', () => {
  const refuseBlobs = () => {
    const put = IDBObjectStore.prototype.put;
    return vi.spyOn(IDBObjectStore.prototype, 'put').mockImplementation(function (this: IDBObjectStore, v: unknown, k?: IDBValidKey) {
      if (v instanceof Blob) throw new DOMException('Error preparing Blob/File data', 'UnknownError');
      return put.call(this, v, k);
    });
  };

  it('kept as a Blob where the browser can', async () => {
    const j = job();
    expect(await q.keepBytes(j.id, new Blob(['abc'], { type: 'text/plain' }))).toBe('blob');
    await q.save(j);
    const back = (await q.load(j.id))!;
    expect(await back.blob.text()).toBe('abc');
  });

  it('as buffers where Blobs can’t be stored (private modes)', async () => {
    const spy = refuseBlobs();
    const j = job();
    expect(await q.keepBytes(j.id, new Blob(['hello'], { type: 'text/plain' }))).toBe('buffers');
    await q.save(j);
    const back = (await q.load(j.id))!;
    expect([await back.blob.text(), back.blob.type]).toEqual(['hello', 'text/plain']);
    spy.mockRestore();
  });

  it('only in memory when too big for buffers, or when nothing can be stored', async () => {
    const spy = refuseBlobs();
    const big = job({ id: 'big' });
    const huge = new (class extends Blob {
      get size() {
        return q.BUFFER_MAX + 1;
      }
    })([]);
    expect(await q.keepBytes(big.id, huge)).toBe('memory');
    await q.save(big);
    expect((await q.load('big'))!.blob).toBe(huge);
    spy.mockImplementation(() => {
      throw new DOMException('quota', 'QuotaExceededError');
    });
    expect(await q.keepBytes('x', new Blob(['a']))).toBe('memory');
    spy.mockRestore();
    // Removing a job forgets its bytes too.
    await q.remove('big');
    expect(await q.load('big')).toBeUndefined();
  });

  it('a job with no bytes here waits for a while, then counts as gone (no locks to ask)', async () => {
    const fresh1 = q.newJob(1, 'p', new Blob(['x']));
    await q.save(fresh1);
    expect(q.waiting((await q.load(fresh1.id))!)).toBe(true);
    const old = job({ id: 'lost', created: Date.now() - q.PENDING_GRACE - 1 });
    await q.save(old);
    expect((await q.load('lost'))!).toMatchObject({ state: 'failed', error: q.GONE });
    // A finished one is just listed.
    await q.save(job({ id: 'old', state: 'failed', error: 'x' }));
    expect((await q.load('old'))!.error).toBe('x');
  });

  it('exists: whether a job is still queued', async () => {
    const j = await fresh();
    expect(await q.exists(j.id)).toBe(true);
    await q.remove(j.id);
    expect(await q.exists(j.id)).toBe(false);
  });

  it('bytes held in memory for this page, and whether they are there', async () => {
    q.holdBytes('m', new Blob(['mem']));
    expect(await q.hasBytes('m')).toBe(true);
    // (None stored, and no page holding them.)
    expect(await q.hasBytes('none')).toBe(false);
    await q.remove('m');
    expect(await q.hasBytes('m')).toBe(false);
    // Held by another open page (its owner lock): there.
    vi.stubGlobal('navigator', { locks: { request: vi.fn(), query: async () => ({ held: [{ name: 'strata-upload-owner-far' }] }) } });
    expect(await q.hasBytes('far')).toBe(true);
  });
});

describe('cancel from another tab, and sign-out', () => {
  it('a job removed elsewhere is never written back; its run stops', async () => {
    const j = await fresh({ id: 'x' });
    expect(await q.update(j)).toBe(true);
    let first = true;
    const f = (async (u: RequestInfo | URL, i: RequestInit = {}) => {
      if (i.method === 'PUT' && first) {
        first = false;
        await q.remove('x'); // (the other tab's cancel)
      }
      return srv.fetch(u, i);
    }) as unknown as typeof fetch;
    await expect(q.run((await q.load('x'))!, deps({ fetch: f }))).rejects.toMatchObject({ name: 'AbortError' });
    expect(await q.load('x')).toBeUndefined();
    expect(await q.update(j)).toBe(false);
  });

  it('a chunk confirmed after the session was lost is not recorded', async () => {
    const order: string[] = [];
    const f = (async (u: RequestInfo | URL, i: RequestInit = {}) => {
      const url = String(u);
      if (i.method === 'PUT' && url.endsWith('/aa') && srv.n === 1) {
        await new Promise((r) => setTimeout(r, 20)); // finishes after bb's 404
        order.push('aa');
      }
      if (i.method === 'PUT' && url.endsWith('/bb') && srv.n === 1) {
        order.push('bb-404');
        return Response.json({ error: 'gone' }, { status: 404 });
      }
      return srv.fetch(u, i);
    }) as unknown as typeof fetch;
    const out = await q.run(await fresh(), deps({ fetch: f }));
    expect(order).toEqual(['bb-404', 'aa']);
    expect(out.state).toBe('done');
    expect(srv.committed).toEqual(['s2']);
  });

  it('bytes in another page’s memory: the job waits while that page owns it', async () => {
    const held = new Set<string>();
    const request = vi.fn((name: string, fn: () => Promise<void>) => {
      held.add(name);
      return fn().then(() => void held.delete(name));
    });
    vi.stubGlobal('navigator', { locks: { request, query: async () => ({ held: [...held].map((name) => ({ name })) }) } });
    const old = Date.now() - q.OWNERLESS_GRACE - 1;
    const j = job({ id: 'elsewhere', created: old });
    await q.save(j);
    await tx('blobs', 'readwrite', (st) => void st.put({ memory: true }, 'elsewhere'));
    // Another page holds it (its owner lock).
    held.add('strata-upload-owner-elsewhere');
    expect(q.waiting((await q.load('elsewhere'))!)).toBe(true);
    // No owner, but just queued: waits.
    held.clear();
    await q.save({ ...j, id: 'young', created: Date.now() });
    await tx('blobs', 'readwrite', (st) => void st.put({ memory: true }, 'young'));
    expect(q.waiting((await q.load('young'))!)).toBe(true);
    // No owner for long: gone.
    expect((await q.load('elsewhere'))!).toMatchObject({ state: 'failed', error: q.GONE });
    // This page owning one: a lock, released when the job goes.
    q.holdBytes('mine', new Blob(['m']));
    q.holdBytes('mine', new Blob(['m'])); // (once)
    await vi.waitFor(() => expect(held.has('strata-upload-owner-mine')).toBe(true));
    expect(request).toHaveBeenCalledTimes(1);
    await q.remove('mine');
    await vi.waitFor(() => expect(held.has('strata-upload-owner-mine')).toBe(false));
    // Locks without query: can't tell, so the long grace applies.
    vi.stubGlobal('navigator', { locks: { request } });
    await q.save({ ...j, id: 'nq', created: old });
    await tx('blobs', 'readwrite', (st) => void st.put({ memory: true }, 'nq'));
    expect(q.waiting((await q.load('nq'))!)).toBe(true);
  });

  it('an unreachable server behind a proxy (502, 530…) is offline, not a failure', async () => {
    const f = (async (u: RequestInfo | URL, i: RequestInit = {}) =>
      i.method === 'PUT' ? new Response(new Uint8Array([1]), { status: 502 }) : srv.fetch(u, i)) as unknown as typeof fetch; // (no type at all)
    await expect(q.run(await fresh(), deps({ fetch: f }))).rejects.toBeInstanceOf(q.Offline);
  });

  it('a new job whose bytes are still being stored waits (and the queue skips it)', async () => {
    const j = q.newJob(1, 'p', new Blob(['x']));
    await q.save(j);
    const back = (await q.load(j.id))!;
    expect(back.state).toBe('queued');
    expect(q.waiting(back)).toBe(true);
    expect(await q.drain(1, deps())).toBe('done');
    expect(srv.log).toEqual([]);
    expect((await q.load(j.id))!.state).toBe('queued');
  });
});

describe('network and file errors', () => {
  it('online, a failed request is retried (a blip); still failing, it is offline', async () => {
    srv.dropOnce = (u, m) => m === 'PUT';
    expect((await q.run(await fresh(), deps({ online: () => true }))).state).toBe('done');
    const down = (async () => {
      throw new TypeError('Failed to fetch');
    }) as unknown as typeof fetch;
    await expect(q.run(await fresh(), deps({ fetch: down, online: () => true }))).rejects.toBeInstanceOf(q.Offline);
  });

  it('a file that can no longer be read fails its upload (not "offline")', async () => {
    for (const thrown of [new DOMException('changed', 'NotReadableError'), 'odd']) {
      const bad = new (class extends Blob {
        slice() {
          return { arrayBuffer: () => Promise.reject(thrown) } as unknown as Blob;
        }
      })([new Uint8Array(14)]);
      const j = q.newJob(1, 'dir/f.bin', bad);
      await add(j);
      q.holdBytes(j.id, bad); // (this page's copy, preferred to the stored one)
      const out = await q.run((await q.load(j.id))!, deps());
      expect(out.state).toBe('failed');
      expect(out.error).toMatch(thrown === 'odd' ? /\(error\)/ : /NotReadableError/);
    }
  });

  it('chunks confirmed just before going offline are saved', async () => {
    const j = await fresh();
    srv.dropOnce = (u, m) => m === 'PUT' && u.endsWith('/cc');
    // (One chunk at a time here: aa and bb confirmed, then cc fails.)
    const f = (async (u: RequestInfo | URL, i: RequestInit = {}) => {
      if (i.method === 'PUT' && String(u).endsWith('/cc')) await new Promise((r) => setTimeout(r, 10));
      return srv.fetch(u, i);
    }) as unknown as typeof fetch;
    await expect(q.run(j, deps({ fetch: f }))).rejects.toBeInstanceOf(q.Offline);
    expect((await q.load(j.id))!.sent.sort()).toEqual(['aa', 'bb']);
  });

  it('a job cancelled right as the network went: nothing is written back', async () => {
    const j = await fresh();
    const f = (async (u: RequestInfo | URL, i: RequestInit = {}) => {
      if (i.method === 'PUT' && String(u).endsWith('/cc')) {
        await new Promise((r) => setTimeout(r, 10));
        await q.remove(j.id);
        throw new TypeError('Failed to fetch');
      }
      return srv.fetch(u, i);
    }) as unknown as typeof fetch;
    await expect(q.run(j, deps({ fetch: f }))).rejects.toBeInstanceOf(q.Offline);
    expect(await q.load(j.id)).toBeUndefined();
  });

  it('the queue reports a job it found gone (for the panel)', async () => {
    await q.save(job({ id: 'gone', created: Date.now() - q.PENDING_GRACE - 1 }));
    expect(await q.drain(1, deps())).toBe('done');
    expect(changes.map((c) => [c.id, c.state])).toEqual([['gone', 'failed']]);
  });

  it('the server’s own 503 (storage offline) fails that job; a sign-out waits', async () => {
    srv.fail = (u, m) => (u === '/api/uploads' && m === 'POST' ? 503 : null);
    expect(await q.run(await fresh(), deps())).toMatchObject({ state: 'failed', error: 'status 503' });
    srv = new Server();
    srv.fail = (u, m) => (m === 'PUT' ? 401 : null);
    await expect(q.run(await fresh(), deps())).rejects.toBeInstanceOf(q.Offline);
  });

  it('a gateway 503 (HTML, not the server) is offline', async () => {
    const f = (async (u: RequestInfo | URL, i: RequestInit = {}) =>
      i.method === 'PUT' ? new Response('<html>down</html>', { status: 503, headers: { 'content-type': 'text/html' } }) : srv.fetch(u, i)) as unknown as typeof fetch;
    await expect(q.run(await fresh(), deps({ fetch: f }))).rejects.toBeInstanceOf(q.Offline);
  });

  it('chunks that keep getting 404 fail the job after a few new sessions', async () => {
    srv.fail = (u, m) => (m === 'PUT' ? 404 : null);
    const out = await q.run(await fresh(), deps());
    expect(out.state).toBe('failed');
    expect(srv.log.filter((l) => l === 'POST /api/uploads')).toHaveLength(3);
  });

  it('cancelling while hashing stops the hashing', async () => {
    const ctl = new AbortController();
    const chunk = vi.fn(async (_b: Blob, signal?: AbortSignal) => {
      ctl.abort();
      if (signal?.aborted) throw new DOMException('cancelled', 'AbortError');
      return chunks;
    });
    await expect(q.run(await fresh(), deps({ chunk }), ctl.signal)).rejects.toMatchObject({ name: 'AbortError' });
    expect(chunk.mock.calls[0][1]).toBe(ctl.signal);
  });

  it('too late to cancel while the file is being saved (here or elsewhere)', async () => {
    const j = await fresh({ state: 'committing', chunks, session: 's1' });
    expect(await q.cancel(j.id, { fetch: srv.fetch as unknown as typeof fetch })).toBe(false);
    expect(await q.exists(j.id)).toBe(true);
    expect(srv.log).toEqual([]);
  });

  it('a job dropped before its session exists never opens one', async () => {
    const j = job({ state: 'uploading', chunks }); // (never saved: dropped)
    await expect(q.run(j, deps())).rejects.toMatchObject({ name: 'AbortError' });
    expect(srv.log).toEqual([]);
  });

  it('a job removed before its owner lock came lets the lock go at once', async () => {
    let grant!: () => Promise<void>;
    const request = vi.fn((_n: string, fn: () => Promise<void>) => {
      grant = fn;
      return new Promise<void>(() => {});
    });
    vi.stubGlobal('navigator', { locks: { request, query: async () => ({ held: [] }) } });
    q.holdBytes('early', new Blob(['x']));
    await q.remove('early');
    await expect(grant()).resolves.toBeUndefined(); // (released, not kept)
  });

  it('a job with only its commit left needs no bytes', async () => {
    await q.save(job({ id: 'c', state: 'committing', chunks, session: 's1', sent: ['aa', 'bb', 'cc'] }));
    const back = (await q.load('c'))!;
    expect([back.state, q.waiting(back)]).toEqual(['committing', false]);
  });

  it('a job resumed at its commit restarts with its bytes when the session is gone', async () => {
    const j = await fresh({ state: 'committing', chunks, session: 'old', sent: ['aa', 'bb', 'cc'] });
    const loaded = (await q.load(j.id))!; // (without its bytes: only the commit was left)
    const out = await q.run(loaded, deps());
    expect(out.state).toBe('done');
    expect([...srv.puts].sort()).toEqual(['aa', 'bb', 'cc']); // real chunks, re-read
  });

  it('a commit restart for a job dropped meanwhile stops', async () => {
    const j = await fresh({ state: 'committing', chunks, session: 'old', sent: ['aa', 'bb', 'cc'] });
    const f = (async (u: RequestInfo | URL, i: RequestInit = {}) => {
      if (String(u).endsWith('/commit')) {
        await tx('jobs', 'readwrite', (s) => void s.delete(j.id)); // (gone, as the 404 comes)
        return Response.json({ error: 'gone' }, { status: 404 });
      }
      return srv.fetch(u, i);
    }) as unknown as typeof fetch;
    await expect(q.run(j, deps({ fetch: f }))).rejects.toMatchObject({ name: 'AbortError' });
  });

  it('a job with no bytes here is never run', async () => {
    const j = q.newJob(1, 'p', new Blob(['x']));
    await q.save(j);
    const waitingJob = (await q.load(j.id))!;
    expect(await q.run(waitingJob, deps())).toBe(waitingJob);
    expect(srv.log).toEqual([]);
  });

  it('a stopped run of the queue (another user signed in) aborts and starts nothing more', async () => {
    await add(job({ id: 'a', created: 1 }));
    await add(job({ id: 'b', created: 2 }));
    const stop = new AbortController();
    const f = (async (u: RequestInfo | URL, i: RequestInit = {}) => {
      if (i.method === 'PUT') {
        stop.abort();
        throw Object.assign(new Error('aborted'), { name: 'AbortError' });
      }
      return srv.fetch(u, i);
    }) as unknown as typeof fetch;
    expect(await q.drain(1, deps({ fetch: f }), stop.signal)).toBe('done');
    expect(srv.log.filter((l) => l === 'POST /api/uploads')).toHaveLength(1); // (b never started)
  });

  it('runs a user’s jobs only while the server says it is them', async () => {
    await add(job({ id: 'a' }));
    expect(await q.drain(1, deps({ whoami: async () => 2 }))).toBe('offline');
    expect(srv.log).toEqual([]);
    expect(await q.drain(1, deps({ whoami: async () => 1 }))).toBe('done');
    expect(srv.committed).toHaveLength(1);
  });

  it('a stop while a job loads still aborts it', async () => {
    await add(job({ id: 'a' }));
    const stop = new AbortController();
    const out = q.drain(1, deps({ whoami: async () => (stop.abort(), 1) }), stop.signal);
    expect(await out).toBe('done');
    expect(srv.log.filter((l) => l.startsWith('PUT'))).toEqual([]);
  });

  it('a commit restart whose bytes are gone is reported failed', async () => {
    const j = await fresh({ state: 'committing', chunks, session: 'old', sent: ['aa', 'bb', 'cc'], created: Date.now() - q.PENDING_GRACE - 1 });
    await tx('blobs', 'readwrite', (s) => void s.delete(j.id));
    const out = await q.run((await q.load(j.id))!, deps());
    expect(out.state).toBe('failed');
    expect(changes.at(-1)!.state).toBe('failed');
  });
});

