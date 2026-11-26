// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, tick, unmount } from 'svelte';

type Call = { method: string; url: string; body?: { path?: string } };
const calls: Call[] = [];
let handler: (c: Call) => unknown;
vi.mock('$lib/api', () => ({
  api: vi.fn(async (method: string, url: string, body?: Call['body']) => {
    const c = { method, url, body };
    calls.push(c);
    return handler(c);
  }),
  enc: encodeURIComponent
}));

import FolderPicker from './FolderPicker.svelte';
import { recent } from './folder-picker';

const dir = (name: string, extra: Partial<{ mount_id: number; offline: boolean }> = {}) => ({ name, is_dir: true, size: 0, mtime: '2026-11-20T10:00:00Z', etag: null, mount_id: null, offline: false, ...extra });
const file = (name: string) => ({ ...dir(name), is_dir: false });
let tree: Record<string, ReturnType<typeof dir>[]>;
const mountInfo = (offline = false) => ({ id: 1, mode: 'raw', connection_name: 'c', connection_kind: 'local', path: '', trash: true, offline });

let target: HTMLElement;
let comp: ReturnType<typeof mount> | null = null;
let answer: string | null | undefined;

beforeEach(() => {
  calls.length = 0;
  localStorage.clear();
  answer = undefined;
  tree = {
    '': [dir('docs'), file('a.txt'), dir('Photos 10'), dir('Photos 9'), dir('nas', { mount_id: 2, offline: true })],
    docs: [dir('old'), dir('new')],
    'docs/new': [],
    'docs/old': []
  };
  handler = (c) => {
    if (c.method === 'GET') {
      const p = decodeURIComponent(c.url.split('path=')[1]);
      if (!(p in tree)) throw new Error('Not found');
      return { entries: tree[p], mount: mountInfo(), path: p, user: 1 };
    }
    if (c.url === '/api/files/mkdir') {
      tree[c.body!.path!] = [];
      return {};
    }
    throw new Error(`unexpected ${c.url}`);
  };
  target = document.body.appendChild(document.createElement('div'));
});
afterEach(() => {
  if (comp) unmount(comp);
  comp = null;
  target.remove();
});

async function open(opts: { start: string; from?: string; exclude?: string[] }) {
  comp = mount(FolderPicker, { target, props: { opts: { title: 'Move “x”', confirm: 'Move here', ...opts }, done: (v: string | null) => (answer = v), titleId: 't' } });
  await settle();
}
const settle = async () => {
  for (let i = 0; i < 5; i++) await tick();
  flushSync();
};
const rows = () => [...target.querySelectorAll<HTMLButtonElement>('[data-row]')];
const names = () => rows().map((r) => r.querySelector('span span')!.textContent);
const button = (text: string) => [...target.querySelectorAll('button')].find((b) => b.textContent!.trim() === text)!;
const status = () => target.querySelector('[aria-live]')!.textContent!.trim();
const row = (name: string) => rows().find((r) => r.textContent!.includes(name))!;
const key = (el: Element, k: string, init: KeyboardEventInit = {}) => el.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true, ...init }));

describe('FolderPicker', () => {
  it('lists folders only, in natural order, and says why one can’t be picked', async () => {
    await open({ start: '', from: '' });
    expect(names()).toEqual(['docs', 'nas', 'Photos 9', 'Photos 10']);
    // The offline mount can't be opened.
    expect(row('nas').getAttribute('aria-disabled')).toBe('true');
    expect(row('nas').textContent).toContain('This storage is offline.');
    row('nas').click();
    await settle();
    expect(calls.filter((c) => c.method === 'GET')).toHaveLength(1);
    // The items are here already: no Move here.
    expect(status()).toBe('The items are already here.');
    expect(button('Move here').disabled).toBe(true);

    row('docs').click();
    await settle();
    expect(names()).toEqual(['new', 'old']);
    expect(button('Move here').disabled).toBe(false);
    button('Move here').click();
    expect(answer).toBe('docs');
    expect(recent()).toEqual(['docs']);
  });

  it('never opens a folder being moved, nor picks inside it', async () => {
    await open({ start: 'docs', from: 'docs', exclude: ['docs/old'] });
    expect(row('old').getAttribute('aria-disabled')).toBe('true');
    expect(row('old').textContent).toContain('This is one of the folders being moved.');
    row('old').click();
    await settle();
    expect(names()).toEqual(['new', 'old']);
    // The breadcrumb goes back up.
    button('Home').click();
    await settle();
    expect(names()).toContain('docs');
    expect(button('Move here').disabled).toBe(false);
  });

  it('shows only the newest listing when folders are clicked fast', async () => {
    await open({ start: '' });
    let release!: () => void;
    const slow = new Promise<void>((r) => (release = r));
    const base = handler;
    handler = (c) => (c.url.endsWith('path=docs') ? slow.then(() => base(c)) : base(c));
    tree['Photos 9'] = [dir('2024')];
    row('docs').click();
    // (The rows are still Home's: this is Home's Photos 9, not docs/Photos 9.)
    row('Photos 9').click();
    await settle();
    expect(names()).toEqual(['2024']);
    release();
    await settle();
    expect(names()).toEqual(['2024']);
    expect(target.querySelector('[aria-current]')!.textContent).toBe('Photos 9');
  });

  it('creates a folder inside the picker and opens it', async () => {
    await open({ start: 'docs', from: 'docs' });
    button('New folder').click();
    flushSync();
    const field = target.querySelector<HTMLInputElement>('input[aria-label="New folder name"]')!;
    // ArrowDown stays in the field (it isn't the filter).
    field.focus();
    key(field, 'ArrowDown');
    expect(document.activeElement).toBe(field);
    // Escape closes the field only.
    key(field, 'Escape');
    flushSync();
    expect(target.querySelector('input[aria-label="New folder name"]')).toBeNull();
    expect(answer).toBeUndefined();

    button('New folder').click();
    flushSync();
    const again = target.querySelector<HTMLInputElement>('input[aria-label="New folder name"]')!;
    again.value = ' 2026 ';
    again.dispatchEvent(new Event('input', { bubbles: true }));
    flushSync();
    again.form!.requestSubmit();
    await settle();
    expect(calls.find((c) => c.method === 'POST')!.body).toEqual({ path: 'docs/2026' });
    expect(target.querySelector('[aria-current]')!.textContent).toBe('2026');
    expect(button('Move here').disabled).toBe(false);
  });

  it('opens a new folder where it was made, even after moving on', async () => {
    await open({ start: 'docs' });
    let release!: () => void;
    const slow = new Promise<void>((r) => (release = r));
    const base = handler;
    handler = (c) => (c.method === 'POST' ? slow.then(() => base(c)) : base(c));
    button('New folder').click();
    flushSync();
    const field = target.querySelector<HTMLInputElement>('input[aria-label="New folder name"]')!;
    field.value = 'x';
    field.dispatchEvent(new Event('input', { bubbles: true }));
    flushSync();
    field.form!.requestSubmit();
    button('Home').click();
    await settle();
    release();
    await settle();
    expect(calls.find((c) => c.method === 'POST')!.body).toEqual({ path: 'docs/x' });
    expect([...target.querySelectorAll('nav button')].map((b) => b.textContent)).toEqual(['Home', 'docs', 'x']);
  });

  it('shows why a new folder failed, and keeps the field', async () => {
    await open({ start: '' });
    handler = ((base) => (c: Call) => {
      if (c.method === 'POST') throw new Error('Already exists');
      return base(c);
    })(handler);
    button('New folder').click();
    flushSync();
    const field = target.querySelector<HTMLInputElement>('input[aria-label="New folder name"]')!;
    field.value = 'docs';
    field.dispatchEvent(new Event('input', { bubbles: true }));
    flushSync();
    field.form!.requestSubmit();
    await settle();
    expect(target.querySelector('[role="alert"]')!.textContent).toBe('Already exists');
    expect(target.querySelector('input[aria-label="New folder name"]')).not.toBeNull();
  });

  it('moves with the keyboard: arrows, Enter, Backspace and Ctrl+Enter', async () => {
    await open({ start: '', from: '' });
    const filter = target.querySelector<HTMLInputElement>('input[aria-label="Filter folders"]')!;
    key(filter, 'ArrowDown');
    expect(document.activeElement).toBe(rows()[0]);
    key(rows()[0], 'ArrowDown');
    expect(document.activeElement).toBe(rows()[1]);
    key(rows()[1], 'End');
    expect(document.activeElement).toBe(rows()[3]);
    key(rows()[3], 'Home');
    key(rows()[0], 'ArrowUp');
    expect(document.activeElement).toBe(filter);
    // Ctrl+Enter here does nothing: the items are already here.
    key(filter, 'Enter', { ctrlKey: true });
    expect(answer).toBeUndefined();

    rows()[0].focus();
    rows()[0].click();
    await settle();
    // Focus follows into the opened folder.
    expect(document.activeElement).toBe(rows()[0]);
    key(rows()[0], 'Backspace');
    await settle();
    expect(names()).toContain('docs');
    row('docs').click();
    await settle();
    key(rows()[0], 'Enter', { metaKey: true });
    expect(answer).toBe('docs');
  });

  it('filters the folders here', async () => {
    await open({ start: '' });
    const filter = target.querySelector<HTMLInputElement>('input[aria-label="Filter folders"]')!;
    filter.value = 'PHO';
    filter.dispatchEvent(new Event('input', { bubbles: true }));
    flushSync();
    expect(names()).toEqual(['Photos 9', 'Photos 10']);
    filter.value = 'zzz';
    filter.dispatchEvent(new Event('input', { bubbles: true }));
    flushSync();
    expect(target.textContent).toContain('No folder matches.');
  });

  it('offers recent folders that are allowed here', async () => {
    localStorage.setItem('strata-recent-folders', JSON.stringify(['docs/old', 'docs/new', '']));
    await open({ start: 'docs', exclude: ['docs/old'] });
    const chips = [...target.querySelectorAll('[aria-label="Recent folders"] button')];
    expect(chips.map((c) => c.textContent!.trim())).toEqual(['new', 'Home']);
    (chips[0] as HTMLButtonElement).click();
    await settle();
    expect(target.querySelector('[aria-current]')!.textContent).toBe('new');
    expect(target.textContent).toContain('No folders here.');
  });

  it('can’t pick a folder that didn’t open, or offline storage', async () => {
    await open({ start: 'gone' });
    expect(target.querySelector('[role="alert"]')!.textContent).toBe("Couldn't open this folder: Not found");
    expect(button('Move here').disabled).toBe(true);
    expect(button('New folder').disabled).toBe(true);
    expect(status()).toBe('Pick another folder.');

    handler = ((base) => (c: Call) => ({ ...(base(c) as object), mount: mountInfo(true) }))(handler);
    button('Home').click();
    await settle();
    expect(status()).toBe('This storage is offline.');
    expect(button('Move here').disabled).toBe(true);
    button('Cancel').click();
    expect(answer).toBeNull();
  });
});
