<script lang="ts">
  // One image in the viewer: its thumbnail at once (the list already loaded
  // it), the full image over it when that has loaded, both filling the same
  // box so nothing moves. Pinch, wheel and double-tap zoom; drag to pan when
  // zoomed; a horizontal swipe at fit size moves to the next or previous.
  import { clamp, fit, toggle, zoomAt, type View } from './zoom';

  let {
    src,
    thumb = null,
    name,
    view = $bindable(fit),
    onswipe,
    onsize
  }: {
    /** The full image (or, for a format the browser can't show, a large thumbnail). */
    src: string;
    /** The small thumbnail shown until `src` has loaded. */
    thumb?: string | null;
    name: string;
    view?: View;
    onswipe?: (dir: 1 | -1) => void;
    /** The image's natural size, once known. */
    onsize?: (w: number, h: number) => void;
  } = $props();

  let stage: HTMLDivElement;
  let loaded = $state(false);
  let failed = $state(false);
  let natural = $state<{ w: number; h: number } | null>(null);
  // Animate only discrete changes (a double tap, the buttons), not gestures.
  let gesturing = $state(false);

  /** The image's size on screen at scale 1 (contained in the stage). */
  function shown() {
    const r = stage.getBoundingClientRect();
    if (!natural) return { w: r.width, h: r.height };
    // (object-fit: contain, so a small image is scaled up to fit too.)
    const k = Math.min((r.width - 16) / natural.w, (r.height - 16) / natural.h);
    return { w: natural.w * k, h: natural.h * k };
  }
  function set(v: View) {
    const { w, h } = shown();
    view = clamp(v, w, h);
  }
  /** A point from the stage's centre. */
  function rel(x: number, y: number) {
    const r = stage.getBoundingClientRect();
    return { px: x - (r.left + r.width / 2), py: y - (r.top + r.height / 2) };
  }

  const pointers = new Map<number, { x: number; y: number }>();
  let pinch: { d: number; v: View; px: number; py: number } | null = null;
  let down: { x: number; y: number; t: number } | null = null;
  let lastTap: { x: number; y: number; t: number } | null = null;

  const dist = () => {
    const [a, b] = [...pointers.values()];
    return Math.hypot(a.x - b.x, a.y - b.y);
  };
  const mid = () => {
    const [a, b] = [...pointers.values()];
    return rel((a.x + b.x) / 2, (a.y + b.y) / 2);
  };

  function pointerdown(e: PointerEvent) {
    if (e.button !== 0) return;
    stage.setPointerCapture?.(e.pointerId);
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    gesturing = true;
    if (pointers.size === 1) down = { x: e.clientX, y: e.clientY, t: e.timeStamp };
    if (pointers.size === 2) {
      down = null; // a pinch is never a swipe or a tap
      pinch = { d: dist(), v: view, ...mid() };
    }
  }
  function pointermove(e: PointerEvent) {
    const p = pointers.get(e.pointerId);
    if (!p) return;
    const dx = e.clientX - p.x, dy = e.clientY - p.y;
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (pinch && pointers.size === 2) {
      const s = pinch.v.s * (dist() / Math.max(1, pinch.d));
      const m = mid();
      // Zoom about where the pinch started, and follow the fingers' midpoint.
      const z = zoomAt(pinch.v, s, pinch.px, pinch.py);
      set({ s: z.s, x: z.x + m.px - pinch.px, y: z.y + m.py - pinch.py });
    } else if (pointers.size === 1 && view.s > 1) {
      set({ ...view, x: view.x + dx, y: view.y + dy });
    }
  }
  function pointerup(e: PointerEvent) {
    if (!pointers.delete(e.pointerId)) return;
    if (pointers.size < 2) pinch = null;
    if (pointers.size === 0) gesturing = false;
    const d = down;
    if (!d || pointers.size > 0) return;
    down = null;
    const dx = e.clientX - d.x, dy = e.clientY - d.y;
    // A swipe: only at fit size (zoomed, a drag pans).
    if (view.s <= 1.01 && Math.abs(dx) > 60 && Math.abs(dx) > 1.5 * Math.abs(dy)) {
      onswipe?.(dx < 0 ? 1 : -1);
      return;
    }
    // A double tap (touch and pen; a mouse has dblclick).
    if (e.pointerType !== 'mouse' && Math.hypot(dx, dy) < 10) {
      const t = { x: e.clientX, y: e.clientY, t: e.timeStamp };
      if (lastTap && t.t - lastTap.t < 300 && Math.hypot(t.x - lastTap.x, t.y - lastTap.y) < 30) {
        const { px, py } = rel(t.x, t.y);
        set(toggle(view, px, py));
        lastTap = null;
      } else lastTap = t;
    }
  }
  function dblclick(e: MouseEvent) {
    const { px, py } = rel(e.clientX, e.clientY);
    set(toggle(view, px, py));
  }
  // Wheel (and trackpad pinch, which arrives as a wheel with ctrlKey).
  // Not passive: the page itself must not scroll or zoom.
  function wheel(node: HTMLElement) {
    const on = (e: WheelEvent) => {
      e.preventDefault();
      const { px, py } = rel(e.clientX, e.clientY);
      const k = Math.exp(-e.deltaY * (e.ctrlKey ? 0.02 : 0.002));
      set(zoomAt(view, view.s * k, px, py));
    };
    node.addEventListener('wheel', on, { passive: false });
    return { destroy: () => node.removeEventListener('wheel', on) };
  }

  function full(e: Event) {
    const img = e.currentTarget as HTMLImageElement;
    natural = { w: img.naturalWidth, h: img.naturalHeight };
    onsize?.(natural.w, natural.h);
    loaded = true;
  }
</script>

<div class="stage" bind:this={stage} use:wheel role="group" aria-label="{name}, zoom {Math.round(view.s * 100)}%"
  onpointerdown={pointerdown} onpointermove={pointermove} onpointerup={pointerup} onpointercancel={pointerup}
  ondblclick={dblclick}>
  <div class="frame" class:still={!gesturing}
    style="transform: translate({view.x}px, {view.y}px) scale({view.s})">
    {#if thumb && !loaded}
      <!-- Decorative: the full image below carries the name. -->
      <img class="thumb" src={thumb} alt="" draggable="false" data-testid="viewer-thumb" />
    {/if}
    {#if !failed}
      <img class="full" class:shown={loaded} {src} alt={name} draggable="false" decoding="async"
        data-testid="viewer-image" onload={full} onerror={() => (failed = true)} />
    {/if}
  </div>
  {#if failed && !thumb}
    <div class="failed" role="status">This image couldn't be shown.</div>
  {/if}
</div>

<style>
  .stage { position: relative; flex: 1; min-height: 0; overflow: hidden; touch-action: none; user-select: none; cursor: zoom-in; }
  .frame { position: absolute; inset: 8px; transform-origin: center; }
  .frame.still { transition: transform .18s ease-out; }
  img { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: contain; -webkit-user-drag: none; }
  /* The thumbnail has the image's proportions, so both fill the same box:
     blurry for a moment, then replaced in place. */
  .full { opacity: 0; transition: opacity .15s; }
  .full.shown { opacity: 1; }
  .failed { position: absolute; inset: 0; display: grid; place-items: center; color: #94a3b8; }
  @media (prefers-reduced-motion: reduce) {
    .frame.still, .full { transition: none; }
  }
</style>
