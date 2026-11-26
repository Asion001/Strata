import { beforeEach, describe, expect, it, vi } from 'vitest';

const calls: { method: string; url: string }[] = [];
let handler: (method: string, url: string) => unknown;
vi.mock('./api', () => ({
  api: vi.fn(async (method: string, url: string) => {
    calls.push({ method, url });
    return handler(method, url);
  })
}));
const goto = vi.fn();
vi.mock('$app/navigation', () => ({ goto: (u: string) => goto(u) }));
vi.mock('$lib/components/ui/Toaster.svelte', () => ({ default: () => {} }));

const trash = await import('./trash.svelte');
const { toasts, runToastAction } = await import('./toast.svelte');

const item = (id: number, original_path: string) => ({
  id,
  name: original_path.split('/').pop()!,
  original_path,
  is_dir: false,
  size: 1,
  deleted_at: '2026-11-20T10:00:00Z',
  purge_after: '2026-12-20T10:00:00Z'
});

beforeEach(() => {
  calls.length = 0;
  toasts.list.splice(0);
  goto.mockClear();
  trash.trashInfo.days = null;
});

describe('trash info', () => {
  it('learns the retention, and keeps it when the server is unreachable', async () => {
    handler = () => ({ retention_days: 30, items: [], more: false });
    await trash.loadTrashInfo();
    expect(trash.trashInfo.days).toBe(30);
    handler = () => {
      throw new Error('offline');
    };
    await trash.loadTrashInfo();
    expect(trash.trashInfo.days).toBe(30);
  });

  it('lists every page, newest first', async () => {
    handler = (_m, url) =>
      url.includes('before=5')
        ? { retention_days: 7, items: [item(4, 'a/d')], more: false }
        : { retention_days: 7, items: [item(9, 'a/b'), item(5, 'a/c')], more: true };
    const r = await trash.listTrash();
    expect(r.items.map((i) => i.id)).toEqual([9, 5, 4]);
    expect(r.days).toBe(7);
    expect(calls.map((c) => c.url)).toEqual(['/api/trash?limit=1000', '/api/trash?limit=1000&before=5']);
    expect(trash.trashInfo.days).toBe(7);
  });

  it('stops on an empty page even if the server says there is more', async () => {
    handler = () => ({ retention_days: 7, items: [], more: true });
    expect((await trash.listTrash()).items).toEqual([]);
  });
});

describe('restoring', () => {
  it('restores each item; one failure does not stop the rest', async () => {
    handler = (_m, url) => {
      if (url.includes('/2/')) throw new Error('not mounted any more');
      return { path: url.includes('/3/') ? 'd/x (restored).txt' : 'd/a.txt' };
    };
    const r = await trash.restoreItems([
      { id: 1, path: 'd/a.txt' },
      { id: 2, path: 'd/b.txt' },
      { id: 3, path: 'd/x.txt' }
    ]);
    expect(r.restored).toEqual([
      { id: 1, from: 'd/a.txt', path: 'd/a.txt' },
      { id: 3, from: 'd/x.txt', path: 'd/x (restored).txt' }
    ]);
    expect(r.failed).toEqual([{ id: 2, from: 'd/b.txt', error: 'not mounted any more' }]);
  });

  it('says what failed and what came back under a new name, with a way to its folder', () => {
    trash.reportRestore({
      restored: [{ id: 3, from: 'd/x.txt', path: 'd/x (restored).txt' }],
      failed: [{ id: 2, from: 'd/b.txt', error: 'gone' }]
    });
    expect(toasts.list.map((t) => t.text)).toEqual([
      "Couldn't restore “b.txt”: gone",
      'Restored as “x (restored).txt”, since “x.txt” is taken.'
    ]);
    runToastAction(toasts.list[1].id);
    expect(goto).toHaveBeenCalledWith('/?p=d');
  });

  it('sums up many failures and many renames', () => {
    trash.reportRestore({
      restored: [
        { id: 1, from: 'a', path: 'a (restored)' },
        { id: 2, from: 'b', path: 'b (restored)' }
      ],
      failed: [
        { id: 3, from: 'c', error: 'e1' },
        { id: 4, from: 'd', error: 'e2' }
      ]
    });
    expect(toasts.list.map((t) => t.text)).toEqual([
      "Couldn't restore 2 items. e1",
      '2 items were restored under new names, since theirs are taken.'
    ]);
  });

  it('says it all came back', () => {
    trash.reportRestore({ restored: [{ id: 1, from: 'a.txt', path: 'a.txt' }], failed: [] });
    trash.reportRestore({
      restored: [
        { id: 1, from: 'a', path: 'a' },
        { id: 2, from: 'b', path: 'b' }
      ],
      failed: []
    });
    expect(toasts.list.map((t) => [t.text, !!t.action])).toEqual([
      ['“a.txt” restored.', true],
      ['2 items restored.', false]
    ]);
    trash.reportRestore({ restored: [], failed: [] });
    expect(toasts.list).toHaveLength(2);
  });

  it('Undo restores exactly the items it was made for', async () => {
    handler = (_m, url) => ({ path: url.includes('/7/') ? 'x/a.txt' : 'x/b.txt' });
    const after = vi.fn();
    const a = trash.undoAction(
      [
        { id: 7, path: 'x/a.txt' },
        { id: 8, path: 'x/b.txt' }
      ],
      after
    );
    expect(a.label).toBe('Undo');
    a.run();
    await vi.waitFor(() => expect(after).toHaveBeenCalled());
    expect(calls.map((c) => c.url)).toEqual(['/api/trash/7/restore', '/api/trash/8/restore']);
    expect(toasts.list.map((t) => t.text)).toEqual(['2 items restored.']);
    // (Without a follow-up too.)
    trash.undoAction([{ id: 7, path: 'x/a.txt' }]).run();
    await vi.waitFor(() => expect(toasts.list).toHaveLength(2));
  });
});
