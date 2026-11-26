// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync } from 'svelte';

type Call = { method: string; url: string; body?: { paths?: string[]; permanent?: boolean; keep_going?: boolean } };
const calls: Call[] = [];
let handler: (c: Call) => unknown;
vi.mock('$lib/api', () => {
  class ApiError extends Error {
    constructor(
      public status: number,
      message: string
    ) {
      super(message);
    }
  }
  return {
    ApiError,
    api: vi.fn(async (method: string, url: string, body?: Call['body']) => {
      const c = { method, url, body };
      calls.push(c);
      return handler(c);
    }),
    contentUrl: (p: string) => `/c/${p}`,
    previewUrl: (p: string) => `/p/${p}`,
    thumbUrl: (p: string) => `/t/${p}`
  };
});
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));
vi.mock('$lib/mounts.svelte', () => ({ reloadMounts: vi.fn() }));
vi.mock('$lib/offline/app.svelte', () => ({ enqueue: vi.fn(async () => {}), userRecorded: async () => {} }));
vi.mock('$lib/offline/recent', () => ({ list: async () => [] }));
vi.mock('$lib/dialogs.svelte', () => ({ confirmDialog: vi.fn(async () => true), promptDialog: vi.fn(), alertDialog: vi.fn(), formDialog: vi.fn() }));
vi.mock('$lib/components/ui/Toaster.svelte', () => ({ default: () => {} }));
// (No server for the live-update socket.)
vi.stubGlobal(
  'WebSocket',
  class {
    close() {}
  }
);

const { harness } = await import('./harness.svelte');
const { BATCH } = await import('./bulk');
const { toasts } = await import('$lib/toast.svelte');
const { ApiError } = await import('$lib/api');

const file = (name: string) => ({ name, is_dir: false, size: 1, mtime: '2026-11-20T10:00:00Z', etag: 'e' });
let listing: Record<string, ReturnType<typeof file>[]>;
const lists = () => calls.filter((c) => c.url.startsWith('/api/files/list'));
const deletes = () => calls.filter((c) => c.url === '/api/files/delete');

const setup = (start = '/?p=A') => harness(start);

beforeEach(() => {
  calls.length = 0;
  toasts.list.splice(0);
  listing = { A: [file('a'), file('b'), file('c')], B: [file('x')] };
  handler = (c) => {
    if (c.url.startsWith('/api/files/list')) {
      const p = decodeURIComponent(c.url.split('path=')[1]);
      return { entries: listing[p] ?? [], mount: { id: 1, mode: 'hashed', connection_name: 'c', path: '', trash: true } };
    }
    if (c.url === '/api/files/delete') {
      const paths = c.body!.paths!;
      return { deleted: paths.length, trashed: paths.length, trash_ids: [], results: paths.map((p, i) => ({ path: p, trashed: true, trash_id: i + 1, error: null, status: null })) };
    }
    throw new Error(`unexpected ${c.url}`);
  };
});

describe('files controller', () => {
  it('never lets a stale listing replace a newer one', async () => {
    const { c, stop } = setup();
    await vi.waitFor(() => expect(c.listedPath).toBe('A'));
    let release!: () => void;
    const slow = new Promise<void>((r) => (release = r));
    const base = handler;
    handler = (call) => (call.url.includes('path=A') ? slow.then(() => base(call)) : base(call));
    const first = c.load('A');
    await c.load('B');
    expect(c.listedPath).toBe('B');
    release();
    await first;
    expect(c.listedPath).toBe('B');
    expect(c.entries.map((e) => e.name)).toEqual(['x']);
    stop();
  });

  it('clears the selection when the folder changes', async () => {
    const { c, go, stop } = setup();
    await vi.waitFor(() => expect(c.listedPath).toBe('A'));
    c.selection.toggle('a');
    flushSync();
    expect(c.selection.size).toBe(1);
    go('/?p=B');
    flushSync();
    expect(c.selection.size).toBe(0);
    await vi.waitFor(() => expect(c.listedPath).toBe('B'));
    expect(lists().map((l) => l.url)).toContain('/api/files/list?path=B');
    stop();
  });

  it('keeps what failed selected after a bulk delete, and offers Undo for the rest', async () => {
    const { c, stop } = setup();
    await vi.waitFor(() => expect(c.listedPath).toBe('A'));
    handler = ((base) => (call: Call) => {
      if (call.url !== '/api/files/delete') return base(call);
      return {
        results: call.body!.paths!.map((p, i) => (p === 'A/b' ? { path: p, trashed: false, trash_id: null, error: 'busy', status: 409 } : { path: p, trashed: true, trash_id: i + 1, error: null, status: null }))
      };
    })(handler);
    for (const n of ['a', 'b', 'c']) c.selection.toggle(n);
    flushSync();
    const failed = await c.removeItems(c.picked);
    expect(failed).toEqual(['b']);
    expect(deletes()).toHaveLength(1);
    expect(deletes()[0].body).toMatchObject({ paths: ['A/a', 'A/b', 'A/c'], keep_going: true, permanent: false });
    flushSync();
    expect([...c.selection.names]).toEqual(['b']);
    expect(toasts.list.at(-1)!.text).toBe('Couldn’t delete “b”: busy'.replace('’', "'"));
    expect(toasts.list.at(-1)!.action?.label).toBe('Undo');
    stop();
  });

  it('deletes in batches, and one by one when a batch fails as a whole', async () => {
    listing.A = Array.from({ length: BATCH + 2 }, (_, i) => file(`f${i}`));
    const { c, stop } = setup();
    await vi.waitFor(() => expect(c.entries).toHaveLength(BATCH + 2));
    let batches = 0;
    handler = ((base) => (call: Call) => {
      if (call.url === '/api/files/delete' && call.body!.paths!.length > 1 && batches++ === 0) throw new ApiError(500, 'internal error');
      if (call.url === '/api/files/delete' && call.body!.paths![0] === 'A/f3') throw new ApiError(404, 'not found');
      return base(call);
    })(handler);
    const failed = await c.removeItems(c.entries);
    // The first batch failed: one request per path; the second batch went whole.
    expect(deletes().map((d) => d.body!.paths!.length)).toEqual([BATCH, ...Array(BATCH).fill(1), 2]);
    expect(failed).toEqual(['f3']);
    expect(toasts.list.at(-1)!.text).toBe("Couldn't delete “f3”: not found");
    stop();
  });

  it('says how many went to the trash when some were deleted for good', async () => {
    const { c, stop } = setup();
    await vi.waitFor(() => expect(c.listedPath).toBe('A'));
    handler = ((base) => (call: Call) => {
      if (call.url !== '/api/files/delete') return base(call);
      // (b is in the trash, but its move is still being completed: no id.)
      return { results: call.body!.paths!.map((p) => ({ path: p, trashed: p !== 'A/c', trash_id: p === 'A/a' ? 1 : null, error: null, status: null })) };
    })(handler);
    await c.removeItems(c.entries, false);
    expect(toasts.list.at(-1)!.text).toBe('3 items deleted: 2 moved to Deleted files, 1 permanently.');
    stop();
  });
});
