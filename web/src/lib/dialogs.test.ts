import { describe, expect, it, vi } from 'vitest';

vi.mock('$lib/components/ui/DialogHost.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/ui/Toaster.svelte', () => ({ default: () => {} }));

const { dialogs, confirmDialog, promptDialog, formDialog, alertDialog } = await import('./dialogs.svelte');
const { toasts, toast, toastError, dismissToast } = await import('./toast.svelte');

describe('dialogs', () => {
  it('queues requests and shows them in order', async () => {
    const a = confirmDialog({ title: 'A' });
    const b = promptDialog({ title: 'B', label: 'Name', value: 'x' });
    expect(dialogs.queue.map((r) => r.opts.title)).toEqual(['A', 'B']);
    const first = dialogs.queue[0];
    if (first.kind !== 'confirm') throw new Error('expected confirm');
    first.done(true);
    first.done(false); // settles once
    expect(await a).toBe(true);
    const second = dialogs.queue[0];
    if (second.kind !== 'form') throw new Error('expected form');
    expect(second.opts.fields[0]).toMatchObject({ value: 'x', required: true });
    second.done({ v: 'renamed' });
    expect(await b).toBe('renamed');
    expect(dialogs.queue).toHaveLength(0);
  });

  it('returns null for a cancelled prompt and resolves alerts', async () => {
    const p = promptDialog({ title: 'P', label: 'L', required: false });
    const r = dialogs.queue[0];
    if (r.kind !== 'form') throw new Error('expected form');
    expect(r.opts.fields[0].required).toBe(false);
    r.done(null);
    expect(await p).toBeNull();

    const f = formDialog({ title: 'F', fields: [{ name: 'a', label: 'A' }] });
    const fr = dialogs.queue[0];
    if (fr.kind !== 'form') throw new Error('expected form');
    fr.done({ a: '1' });
    expect(await f).toEqual({ a: '1' });

    const al = alertDialog({ title: 'Done', copy: 'https://example.com/s/x' });
    const ar = dialogs.queue[0];
    if (ar.kind !== 'alert') throw new Error('expected alert');
    ar.done();
    await expect(al).resolves.toBeUndefined();
  });
  it('withdraws a dialog when its signal aborts', async () => {
    const ac = new AbortController();
    const c = confirmDialog({ title: 'C', signal: ac.signal });
    const p = promptDialog({ title: 'P', label: 'L', signal: ac.signal });
    ac.abort();
    expect(await c).toBe(false);
    expect(await p).toBeNull();
    expect(dialogs.queue).toHaveLength(0);
    expect(await confirmDialog({ title: 'late', signal: ac.signal })).toBe(false);
  });
});

describe('toasts', () => {
  it('keeps the newest four and dismisses by id', () => {
    toasts.list.splice(0);
    const ids = [1, 2, 3, 4, 5].map((n) => toast(`t${n}`, 'info', 0));
    expect(toasts.list.map((t) => t.text)).toEqual(['t2', 't3', 't4', 't5']);
    dismissToast(ids[2]);
    expect(toasts.list.map((t) => t.text)).toEqual(['t2', 't4', 't5']);
    dismissToast(999);
    expect(toasts.list).toHaveLength(3);
  });

  it('expires toasts and words errors', () => {
    vi.useFakeTimers();
    toasts.list.splice(0);
    toastError(new Error('boom'));
    toastError('plain');
    toastError(42);
    expect(toasts.list.map((t) => [t.kind, t.text])).toEqual([
      ['error', 'boom'],
      ['error', 'plain'],
      ['error', 'Something went wrong.']
    ]);
    toast('hi');
    vi.advanceTimersByTime(4000);
    expect(toasts.list.map((t) => t.text)).not.toContain('hi');
    vi.advanceTimersByTime(4000);
    expect(toasts.list).toHaveLength(0);
    vi.useRealTimers();
  });
});
