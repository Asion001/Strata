// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { canOpen, clean, crumbs, recent, remember, whyNot } from './folder-picker';

beforeEach(() => localStorage.clear());

describe('folder picker rules', () => {
  it('cleans paths', () => {
    expect(clean(' /a//b/ ')).toBe('a/b');
    expect(clean('/')).toBe('');
  });

  it('refuses the source folder, moved folders and what is inside them', () => {
    const rules = { from: 'docs', exclude: ['docs/old', 'x'] };
    expect(whyNot('docs/', rules)).toBe('The items are already here.');
    expect(whyNot('docs/old', rules)).toBe('This is one of the folders being moved.');
    expect(whyNot('/docs/old/2024', rules)).toBe('This is inside a folder being moved.');
    expect(whyNot('docs/older', rules)).toBeNull();
    expect(whyNot('', rules)).toBeNull();
    expect(whyNot('docs', {})).toBeNull();
    // Moving out of Home: Home itself is the source.
    expect(whyNot('', { from: '' })).toBe('The items are already here.');
  });

  it('opens anything but a moved folder', () => {
    const rules = { from: 'docs', exclude: ['docs/old'] };
    expect(canOpen('docs', rules)).toBe(true);
    expect(canOpen('docs/old', rules)).toBe(false);
    expect(canOpen('docs/old/a', rules)).toBe(false);
    expect(canOpen('docs/older', {})).toBe(true);
  });

  it('keeps the last five picks, newest first, without repeats', () => {
    expect(recent()).toEqual([]);
    for (const p of ['a', 'b', 'c', 'd', 'e', 'f', '/b/']) remember(p);
    expect(recent()).toEqual(['b', 'f', 'e', 'd', 'c']);
  });

  it('survives storage that is blocked or holds junk', () => {
    localStorage.setItem('strata-recent-folders', '{"not":"a list"}');
    expect(recent()).toEqual([]);
    localStorage.setItem('strata-recent-folders', '["a", 3, null]');
    expect(recent()).toEqual(['a']);
    localStorage.setItem('strata-recent-folders', 'not json');
    expect(recent()).toEqual([]);
    const set = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('quota');
    });
    expect(() => remember('a')).not.toThrow();
    set.mockRestore();
  });

  it('builds crumbs from Home', () => {
    expect(crumbs('')).toEqual([{ name: 'Home', path: '' }]);
    expect(crumbs('a/b')).toEqual([
      { name: 'Home', path: '' },
      { name: 'a', path: 'a' },
      { name: 'b', path: 'a/b' }
    ]);
  });
});
