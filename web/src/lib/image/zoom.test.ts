import { describe, expect, it } from 'vitest';
import { clamp, fit, toggle, zoomAt, MAX } from './zoom';

describe('zoom', () => {
  it('keeps the point under the finger fixed', () => {
    const v = zoomAt(fit, 2, 100, 50);
    // The point (100, 50) was at 100,50; after: x + s*p_image = 100.
    // Image point under it at scale 1 was (100, 50); now at v.x + 2*100.
    expect(v.x + 2 * 100).toBe(100);
    expect(v.y + 2 * 50).toBe(50);
    const w = zoomAt(v, 4, 100, 50);
    expect(w.x + 4 * 100).toBe(100);
  });

  it('never pans past the edges or zooms out past fit', () => {
    expect(clamp({ s: 2, x: 999, y: -999 }, 400, 300)).toEqual({ s: 2, x: 200, y: -150 });
    expect(clamp({ s: 0.5, x: 10, y: 10 }, 400, 300)).toEqual({ s: 1, x: 0, y: 0 });
    expect(clamp({ s: 50, x: 0, y: 0 }, 400, 300).s).toBe(MAX);
  });

  it('double tap zooms in on the point, then back to fit', () => {
    const v = toggle(fit, 40, -20);
    expect(v.s).toBeGreaterThan(1);
    expect(v.x).toBeCloseTo(40 - v.s * 40);
    expect(toggle(v, 0, 0)).toEqual(fit);
  });
});
