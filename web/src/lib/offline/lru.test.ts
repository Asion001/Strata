import { describe, expect, it } from 'vitest';
import { admits, DEFAULTS, evictions, limits, MB, type Held } from './lru';

const h = (key: string, size: number, used: number, pinned = false): Held => ({ key, size, used, pinned });

describe('admits', () => {
  it('takes unpinned files up to 20 MB (and the cap)', () => {
    expect(admits(20 * MB, false, 0, DEFAULTS)).toBe(true);
    expect(admits(20 * MB + 1, false, 0, DEFAULTS)).toBe(false);
    expect(admits(5 * MB, false, 0, { ...DEFAULTS, maxBytes: 4 * MB })).toBe(false);
  });
  it('takes pinned files of any size while the pinned total fits the cap', () => {
    expect(admits(300 * MB, true, 0, DEFAULTS)).toBe(true);
    expect(admits(300 * MB, true, 250 * MB, DEFAULTS)).toBe(false);
  });
  it('refuses nonsense sizes', () => {
    expect(admits(-1, false, 0, DEFAULTS)).toBe(false);
    expect(admits(NaN, true, 0, DEFAULTS)).toBe(false);
    expect(admits(Infinity, true, 0, DEFAULTS)).toBe(false);
  });
});

describe('evictions', () => {
  it('keeps the 50 most recently opened', () => {
    const all = Array.from({ length: 53 }, (_, i) => h(`k${i}`, 1, i));
    expect(evictions(all, DEFAULTS)).toEqual(['k0', 'k1', 'k2']);
  });
  it('drops least recently used first until the total fits', () => {
    const l = { ...DEFAULTS, maxBytes: 10 };
    expect(evictions([h('new', 4, 3), h('old', 4, 1), h('mid', 4, 2)], l)).toEqual(['old']);
  });
  it('never drops pinned files, even over the cap', () => {
    const l = { ...DEFAULTS, maxBytes: 10, maxEntries: 0 };
    expect(evictions([h('p', 20, 1, true), h('a', 1, 2)], l)).toEqual(['a']);
    expect(evictions([h('p', 20, 1, true)], l)).toEqual([]);
  });
  it('drops an unpinned file too big to keep (it was pinned before)', () => {
    expect(evictions([h('big', 30 * MB, 5), h('small', 1, 1)], DEFAULTS)).toEqual(['big']);
  });
  it('pinned files count toward the total, not the 50', () => {
    const all = [...Array.from({ length: 50 }, (_, i) => h(`k${i}`, 1, i)), h('p', 1, 0, true)];
    expect(evictions(all, DEFAULTS)).toEqual([]);
  });
});

describe('limits', () => {
  it('reads the total cap in MB from settings', () => {
    expect(limits('100').maxBytes).toBe(100 * MB);
    expect(limits('100.7').maxBytes).toBe(100 * MB);
    for (const bad of [null, undefined, '', '0', '-5', 'abc', 'Infinity']) expect(limits(bad)).toEqual(DEFAULTS);
  });
});
