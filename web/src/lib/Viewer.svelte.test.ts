// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

// Shallow routing, as SvelteKit does it: a history entry with page state.
vi.mock('$app/state', async () => {
  const { nav } = await import('./testing/nav.svelte');
  return {
    navigating: { to: null },
    page: {
      get state() {
        return nav.state;
      }
    }
  };
});
vi.mock('$app/navigation', async () => {
  const { nav } = await import('./testing/nav.svelte');
  return {
    pushState: (_url: string, state: Record<string, unknown>) => {
      nav.pushes++;
      nav.state = state;
    }
  };
});

import Viewer from './Viewer.svelte';
import { nav } from './testing/nav.svelte';

type Item = { name: string; size: number; src: string; download: string; thumb?: string; poster?: string; mtime: string };
const img = (name: string): Item => ({
  name,
  size: 1000,
  src: `/full/${name}`,
  download: `/dl/${name}`,
  thumb: `/thumb/${name}`,
  mtime: '2026-01-02T03:04:05Z'
});

let target: HTMLElement;
let comp: ReturnType<typeof mount> | null = null;

function open(items: Item[], index = 0, extra: Record<string, unknown> = {}) {
  const props = $state({ items, index, onclose: vi.fn(), ...extra });
  comp = mount(Viewer, { target, props });
  flushSync();
  return props;
}
const press = (key: string, opts: KeyboardEventInit = {}) => {
  (document.activeElement ?? document.body).dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, ...opts }));
  flushSync();
};

// (jsdom has no ResizeObserver; the viewer only uses it to re-fit.)
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

beforeEach(() => {
  nav.state = {};
  nav.pushes = 0;
  target = document.createElement('div');
  document.body.append(target);
  vi.spyOn(history, 'back').mockImplementation(() => {
    nav.state = {};
  });
});
afterEach(() => {
  if (comp) unmount(comp);
  comp = null;
  target.remove();
  vi.restoreAllMocks();
});

describe('Viewer', () => {
  it('is a labelled modal that takes focus and pushes a history entry', () => {
    const before = document.createElement('button');
    document.body.append(before);
    before.focus();
    open([img('a.jpg'), img('b.png')]);
    const dialog = target.querySelector('[role="dialog"]') as HTMLElement;
    expect(dialog.getAttribute('aria-modal')).toBe('true');
    expect(dialog.getAttribute('aria-label')).toBe('Preview of a.jpg');
    expect(dialog.contains(document.activeElement)).toBe(true);
    expect(typeof nav.state.viewer).toBe('number');
    // Focus goes back where it was on close.
    unmount(comp!);
    comp = null;
    expect(document.activeElement).toBe(before);
    before.remove();
  });

  it('shows the thumbnail at once, then the full image over it', () => {
    open([img('a.jpg')]);
    const thumb = target.querySelector('[data-testid="viewer-thumb"]') as HTMLImageElement;
    const full = target.querySelector('[data-testid="viewer-image"]') as HTMLImageElement;
    expect(thumb.getAttribute('src')).toBe('/thumb/a.jpg');
    expect(thumb.alt).toBe(''); // decorative; the full image carries the name
    expect(full.getAttribute('src')).toBe('/full/a.jpg');
    expect(full.alt).toBe('a.jpg');
    full.dispatchEvent(new Event('load'));
    flushSync();
    expect(target.querySelector('[data-testid="viewer-thumb"]')).toBeNull();
  });

  it('arrow keys step through the folder, in order', () => {
    const p = open([img('a.jpg'), img('b.png'), img('c.gif')], 1);
    press('ArrowRight');
    expect(p.index).toBe(2);
    press('ArrowRight'); // the last one: stays
    expect(p.index).toBe(2);
    press('ArrowLeft');
    press('ArrowLeft');
    expect(p.index).toBe(0);
  });

  it('preloads only the neighbours, and lets go of the rest', () => {
    const made: HTMLImageElement[] = [];
    const Real = globalThis.Image;
    globalThis.Image = class extends Real {
      constructor() {
        super();
        made.push(this);
      }
    } as typeof Image;
    try {
      open([img('a.jpg'), img('b.jpg'), img('c.jpg'), img('d.jpg')], 1);
      const live = () => made.filter((i) => i.hasAttribute('src')).map((i) => i.getAttribute('src')).sort();
      expect(live()).toEqual(['/full/a.jpg', '/full/c.jpg']);
      press('ArrowRight');
      expect(live()).toEqual(['/full/b.jpg', '/full/d.jpg']);
      unmount(comp!);
      comp = null;
      expect(live()).toEqual([]);
    } finally {
      globalThis.Image = Real;
    }
  });

  it('Escape closes through history (so Back does the same)', () => {
    const p = open([img('a.jpg')]);
    press('Escape');
    expect(history.back).toHaveBeenCalled();
    flushSync();
    expect(p.onclose).toHaveBeenCalledTimes(1);
  });

  it('a held Escape goes back only once', () => {
    vi.spyOn(history, 'back').mockImplementation(() => {}); // popstate comes later
    open([img('a.jpg')]);
    press('Escape');
    press('Escape');
    expect(history.back).toHaveBeenCalledTimes(1);
  });

  it('Escape first closes the details panel', () => {
    const p = open([img('a.jpg')], 0, { onshare: vi.fn(), ondelete: vi.fn() });
    const btn = target.querySelector('button[aria-label="Details"]') as HTMLButtonElement;
    expect(btn.getAttribute('aria-expanded')).toBe('false');
    btn.click();
    flushSync();
    expect(btn.getAttribute('aria-expanded')).toBe('true');
    const panel = target.querySelector('#viewer-info') as HTMLElement;
    expect(panel.getAttribute('aria-label')).toBe('Image details');
    expect(panel.textContent).toContain('1000 B');
    expect(panel.textContent).toContain('Share');
    expect(panel.textContent).toContain('Delete');
    press('Escape');
    expect(target.querySelector('#viewer-info')).toBeNull();
    expect(p.onclose).not.toHaveBeenCalled();
  });

  it('keeps Tab inside the overlay', () => {
    open([img('a.jpg'), img('b.jpg')]);
    const dialog = target.querySelector('[role="dialog"]') as HTMLElement;
    const all = [...dialog.querySelectorAll<HTMLElement>('a[href], button:not([disabled])')];
    all[all.length - 1].focus();
    press('Tab');
    expect(document.activeElement).toBe(all[0]);
    press('Tab', { shiftKey: true });
    expect(document.activeElement).toBe(all[all.length - 1]);
    // Focus that fell to <body> (its button went away) comes back in.
    (document.activeElement as HTMLElement).blur();
    expect(document.activeElement).toBe(document.body);
    press('Tab');
    expect(dialog.contains(document.activeElement)).toBe(true);
  });

  it('labels every icon-only button', () => {
    open([img('a.jpg'), img('b.jpg'), img('c.jpg')], 1);
    for (const b of target.querySelectorAll('button')) {
      expect(b.getAttribute('aria-label') || b.textContent?.trim(), b.outerHTML).toBeTruthy();
    }
  });

  it('a format the browser cannot show gets its thumbnail, or a download link', () => {
    open([{ ...img('scan.tiff'), poster: '/big/scan.tiff' }, { ...img('photo.heic'), thumb: undefined }]);
    expect(target.querySelector('[data-testid="viewer-image"]')?.getAttribute('src')).toBe('/big/scan.tiff');
    expect(target.textContent).toContain("can't be shown in full");
    press('ArrowRight');
    expect(target.querySelector('[data-testid="viewer-image"]')).toBeNull();
    expect(target.querySelector('a[href="/dl/photo.heic"]')).not.toBeNull();
  });
});
