// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount, tick } from 'svelte';

const calls: string[] = [];
let handler: (method: string, url: string) => unknown;
vi.mock('$lib/api', () => ({
  api: vi.fn(async (method: string, url: string) => {
    calls.push(`${method} ${url}`);
    return handler(method, url);
  })
}));
let answer = true;
const asked: { title: string; body?: string }[] = [];
vi.mock('$lib/dialogs.svelte', () => ({
  confirmDialog: vi.fn(async (o: { title: string; body?: string }) => {
    asked.push(o);
    return answer;
  })
}));
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));
vi.mock('$lib/components/ui/Toaster.svelte', () => ({ default: () => {} }));

// (jsdom has no ResizeObserver, which bind:clientHeight uses.)
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

import TrashView from './TrashView.svelte';
import { toasts } from '$lib/toast.svelte';

const item = (id: number, original_path: string, is_dir = false) => ({
  id,
  name: original_path.split('/').pop()!,
  original_path,
  is_dir,
  size: 2048,
  deleted_at: '2026-11-20T10:00:00Z',
  purge_after: '2026-12-20T10:00:00Z'
});

let target: HTMLElement;
let comp: ReturnType<typeof mount> | null = null;
let trash: ReturnType<typeof item>[];
let days = 30;

beforeEach(() => {
  target = document.body.appendChild(document.createElement('div'));
  calls.length = 0;
  asked.length = 0;
  answer = true;
  toasts.list.splice(0);
  days = 30;
  trash = [item(3, 'Docs/report.pdf'), item(2, 'Pics', true), item(1, 'notes.txt')];
  handler = (method, url) => {
    if (method === 'GET') return { retention_days: days, items: trash, more: false };
    const id = Number(url.match(/\/api\/trash\/(\d+)/)?.[1]);
    if (url.endsWith('/restore')) {
      trash = trash.filter((i) => i.id !== id);
      return { path: id === 1 ? 'notes (restored).txt' : 'Docs/report.pdf' };
    }
    if (method === 'DELETE') {
      trash = trash.filter((i) => i.id !== id);
      return {};
    }
    if (url === '/api/trash/empty') {
      const n = trash.length;
      trash = [];
      return { purged: n, failed: 0 };
    }
    throw new Error(`unexpected ${method} ${url}`);
  };
});
afterEach(() => {
  if (comp) unmount(comp);
  comp = null;
  target.remove();
});

async function show() {
  comp = mount(TrashView, { target });
  flushSync();
  await vi.waitFor(() => expect(target.textContent).not.toContain('Loading…'));
  flushSync();
}
const button = (name: string) => {
  const b = [...target.querySelectorAll('button')].find((x) => (x.getAttribute('aria-label') ?? x.textContent?.trim()) === name);
  if (!b) throw new Error(`no button ${name}`);
  return b as HTMLButtonElement;
};
const box = (name: string) => target.querySelector(`input[aria-label="${name}"]`) as HTMLInputElement;
/** Does something, then waits for the page to load the trash again. */
const act = async (fn: () => void) => {
  const n = calls.length;
  fn();
  await vi.waitFor(() => {
    expect(calls.length).toBeGreaterThan(n + 1);
    expect(calls.at(-1)).toMatch(/^GET \/api\/trash/);
  });
  await tick();
  flushSync();
};

describe('Deleted files page', () => {
  it('lists items with where they came from, and says how long they stay', async () => {
    await show();
    expect(target.querySelector('[data-testid=retention]')!.textContent).toBe('Items are removed for good after 30 days.');
    const rows = target.querySelectorAll('[role=row]');
    expect(rows).toHaveLength(4);
    expect(rows[1].textContent).toContain('report.pdf');
    expect(rows[1].textContent).toContain('Docs');
    expect(rows[3].textContent).toContain('Home');
  });

  it('says when it is empty, and when the trash is off', async () => {
    trash = [];
    await show();
    expect(target.querySelector('[data-testid=trash-empty]')!.textContent).toContain('for 30 days');
    unmount(comp!);
    days = 0;
    comp = null;
    await show();
    expect(target.querySelector('[data-testid=trash-off]')).toBeTruthy();
  });

  it('shows a load error', async () => {
    handler = () => {
      throw new Error('server down');
    };
    comp = mount(TrashView, { target });
    await vi.waitFor(() => expect(target.querySelector('[role=alert]')?.textContent).toBe('server down'));
  });

  it('restores one item and says when it came back under a new name', async () => {
    await show();
    await act(() => button('Restore notes.txt').click());
    expect(calls).toContain('POST /api/trash/1/restore');
    expect(toasts.list.map((t) => t.text)).toEqual(['Restored as “notes (restored).txt”, since “notes.txt” is taken.']);
    expect(target.querySelectorAll('[role=row]')).toHaveLength(3);
  });

  it('restores and deletes a selection, keeping the selection in step', async () => {
    await show();
    box('Select all').click();
    flushSync();
    expect(target.querySelector('[aria-label="Selected items"]')!.textContent).toContain('3 selected');
    box('Select all').click();
    flushSync();
    expect(target.querySelector('[aria-label="Selected items"]')).toBeNull();
    box('Select report.pdf').click();
    flushSync();
    await act(() => button('Restore').click());
    expect(calls).toContain('POST /api/trash/3/restore');
    expect(target.querySelector('[aria-label="Selected items"]')).toBeNull();

    box('Select Pics').click();
    box('Select notes.txt').click();
    flushSync();
    await act(() => button('Delete permanently').click());
    expect(asked.at(-1)!.title).toBe('Delete 2 items permanently?');
    expect(calls).toEqual(expect.arrayContaining(['DELETE /api/trash/2', 'DELETE /api/trash/1']));
    expect(toasts.list.at(-1)!.text).toBe('2 items deleted permanently.');
    expect(target.querySelector('[data-testid=trash-empty]')).toBeTruthy();
  });

  it('asks before deleting for good, and reports a failure', async () => {
    await show();
    answer = false;
    button('Delete notes.txt permanently').click();
    await vi.waitFor(() => expect(asked).toHaveLength(1));
    expect(asked[0].title).toBe('Delete “notes.txt” permanently?');
    expect(calls.some((c) => c.startsWith('DELETE'))).toBe(false);
    answer = true;
    const ok = handler;
    handler = (m, u) => {
      if (m === 'DELETE') throw new Error('storage offline');
      return ok(m, u);
    };
    await act(() => button('Delete notes.txt permanently').click());
    expect(toasts.list.at(-1)!.text).toBe("Couldn't delete 1 item permanently: storage offline");
    handler = ok;
    await act(() => button('Delete notes.txt permanently').click());
    expect(toasts.list.at(-1)!.text).toBe('“notes.txt” deleted permanently.');
  });

  it('empties everything after a confirm that states the count', async () => {
    await show();
    answer = false;
    button('Empty').click();
    await vi.waitFor(() => expect(asked).toHaveLength(1));
    expect(asked[0].body).toContain('3 items will be deleted permanently');
    expect(calls).not.toContain('POST /api/trash/empty');
    answer = true;
    await act(() => button('Empty').click());
    expect(calls).toContain('POST /api/trash/empty');
    expect(toasts.list.at(-1)!.text).toBe('Deleted files emptied (3 items).');

    // Some that can't be removed now stay, and the answer says so.
    trash = [item(5, 'a.txt')];
    const ok = handler;
    handler = (m, u) => (u === '/api/trash/empty' ? { purged: 0, failed: 1 } : ok(m, u));
    unmount(comp!);
    comp = null;
    await show();
    await act(() => button('Empty').click());
    expect(toasts.list.at(-1)!.text).toBe("0 items deleted permanently; 1 item couldn't be (their storage is offline).");
    handler = (m, u) => {
      if (u === '/api/trash/empty') throw new Error('nope');
      return ok(m, u);
    };
    await act(() => button('Empty').click());
    expect(toasts.list.at(-1)!.text).toBe('nope');
  });
});
