import { afterEach, describe, expect, it } from 'vitest';
import type { Entry } from '$lib/api';
import { Selection, loadPref, nextSort, savePref, sortEntries } from './listing';

const e = (name: string, is_dir = false, size = 0, mtime = '2026-01-01T00:00:00Z'): Entry => ({
  name, is_dir, size, mtime, etag: null, mount_id: null
});

describe('sortEntries', () => {
  const list = [e('b10.txt', false, 5, '2026-01-03T00:00:00Z'), e('B2.txt', false, 9, '2026-01-01T00:00:00Z'), e('zdir', true), e('adir', true), e('a.txt', false, 5, '2026-01-02T00:00:00Z')];
  it('puts folders first and compares names naturally, ignoring case', () => {
    expect(sortEntries(list, { key: 'name', desc: false }).map((x) => x.name)).toEqual(['adir', 'zdir', 'a.txt', 'B2.txt', 'b10.txt']);
    expect(sortEntries(list, { key: 'name', desc: true }).map((x) => x.name)).toEqual(['zdir', 'adir', 'b10.txt', 'B2.txt', 'a.txt']);
  });
  it('sorts by size and date with name as the tie-break', () => {
    expect(sortEntries(list, { key: 'size', desc: false }).map((x) => x.name)).toEqual(['adir', 'zdir', 'a.txt', 'b10.txt', 'B2.txt']);
    expect(sortEntries(list, { key: 'mtime', desc: true }).map((x) => x.name)).toEqual(['adir', 'zdir', 'b10.txt', 'a.txt', 'B2.txt']);
    expect(sortEntries(list, { key: 'size', desc: true }).map((x) => x.name)).toEqual(['adir', 'zdir', 'B2.txt', 'a.txt', 'b10.txt']);
  });
  it('does not change the input', () => {
    const copy = [...list];
    sortEntries(list, { key: 'size', desc: true });
    expect(list).toEqual(copy);
  });
  it('falls back to name for unparseable dates', () => {
    const bad = [e('b', false, 0, 'x'), e('a', false, 0, 'y')];
    expect(sortEntries(bad, { key: 'mtime', desc: false }).map((x) => x.name)).toEqual(['a', 'b']);
  });
});

describe('nextSort', () => {
  it('flips the same column and starts others in their natural order', () => {
    expect(nextSort({ key: 'name', desc: false }, 'name')).toEqual({ key: 'name', desc: true });
    expect(nextSort({ key: 'name', desc: false }, 'size')).toEqual({ key: 'size', desc: false });
    expect(nextSort({ key: 'name', desc: false }, 'mtime')).toEqual({ key: 'mtime', desc: true });
  });
});

describe('Selection', () => {
  const order = ['a', 'b', 'c', 'd', 'e'];
  it('toggles and ranges from the anchor', () => {
    const s = new Selection();
    s.toggle('b');
    s.range('d', order);
    expect([...s.names].sort()).toEqual(['b', 'c', 'd']);
    s.range('a', order); // anchor stays b
    expect([...s.names].sort()).toEqual(['a', 'b', 'c', 'd']);
    s.toggle('b'); // anchor b, now off
    s.range('c', order);
    expect([...s.names].sort()).toEqual(['a', 'd']);
    expect(s.size).toBe(2);
    expect(s.has('a')).toBe(true);
  });
  it('ranges without an anchor act as a toggle', () => {
    const s = new Selection();
    s.range('c', order);
    expect([...s.names]).toEqual(['c']);
    s.anchor = 'gone';
    s.range('a', order);
    expect([...s.names].sort()).toEqual(['a', 'c']);
  });
  it('selects all, then clears', () => {
    const s = new Selection();
    s.toggle('a');
    s.toggleAll(order);
    expect(s.size).toBe(5);
    s.toggleAll(order);
    expect(s.size).toBe(0);
    expect(s.anchor).toBeNull();
    s.toggleAll([]);
    expect(s.size).toBe(0);
  });
  it('prunes names that left the list', () => {
    const s = new Selection();
    s.toggle('a');
    s.toggle('z');
    s.prune(order);
    expect([...s.names]).toEqual(['a']);
    expect(s.anchor).toBeNull();
    s.toggle('b');
    s.prune(order);
    expect(s.anchor).toBe('b');
  });
});

describe('prefs', () => {
  const real = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  afterEach(() => {
    if (real) Object.defineProperty(globalThis, 'localStorage', real);
    else delete (globalThis as { localStorage?: Storage }).localStorage;
  });
  it('reads allowed values and falls back otherwise', () => {
    const m = new Map<string, string>();
    Object.defineProperty(globalThis, 'localStorage', {
      configurable: true,
      value: { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v) }
    });
    savePref('t-view', 'grid');
    expect(loadPref('t-view', ['list', 'grid'] as const, 'list')).toBe('grid');
    savePref('t-view', 'junk');
    expect(loadPref('t-view', ['list', 'grid'] as const, 'list')).toBe('list');
  });
  it('survives blocked storage', () => {
    Object.defineProperty(globalThis, 'localStorage', { configurable: true, get: () => { throw new Error('blocked'); } });
    expect(loadPref('x', ['a'] as const, 'a')).toBe('a');
    expect(() => savePref('x', 'a')).not.toThrow();
  });
});
