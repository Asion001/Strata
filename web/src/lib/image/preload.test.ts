// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { Preloader } from './preload';

describe('Preloader', () => {
  it('holds only the neighbours and releases the rest', () => {
    const p = new Preloader();
    p.keep(['/a', '/b']);
    expect(p.urls.sort()).toEqual(['/a', '/b']);
    p.keep(['/b', '/c', null]);
    expect(p.urls.sort()).toEqual(['/b', '/c']);
    p.clear();
    expect(p.urls).toEqual([]);
  });

  it('cancels a released image by clearing its src', () => {
    const created: HTMLImageElement[] = [];
    const Real = globalThis.Image;
    globalThis.Image = class extends Real {
      constructor() {
        super();
        created.push(this);
      }
    } as typeof Image;
    try {
      const p = new Preloader();
      p.keep(['/x']);
      expect(created[0].getAttribute('src')).toBe('/x');
      p.keep([]);
      expect(created[0].hasAttribute('src')).toBe(false);
      // Keeping the same URL again doesn't start a second load.
      p.keep(['/y']);
      p.keep(['/y']);
      expect(created.length).toBe(2);
    } finally {
      globalThis.Image = Real;
    }
  });
});
