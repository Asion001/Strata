<script lang="ts">
  // Full-screen preview: image gallery (arrows, swipe, pinch and double-tap
  // zoom), native video/audio, PDF, text. A modal: focus stays inside, and
  // it is a history entry, so Back closes it.
  import { onDestroy, onMount } from 'svelte';
  import { pushState } from '$app/navigation';
  import { navigating, page } from '$app/state';
  import { isAudio, isBrowserImage, isImage, isPdf, isTextName, isVideo, bytes } from './format';
  import { trapFocus } from './focus';
  import { Preloader } from './image/preload';
  import { fit, MAX, zoomAt, type View } from './image/zoom';
  import ImageView from './image/ImageView.svelte';
  import { staleApp } from './lazy';
  import { cn } from './utils';
  import { session } from './session.svelte';
  import Button, { buttonClass } from './components/ui/button/button.svelte';
  import XIcon from '@lucide/svelte/icons/x';
  import DownloadIcon from '@lucide/svelte/icons/download';
  import PencilIcon from '@lucide/svelte/icons/pencil';
  import ChevronLeftIcon from '@lucide/svelte/icons/chevron-left';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import InfoIcon from '@lucide/svelte/icons/info';
  import ZoomInIcon from '@lucide/svelte/icons/zoom-in';
  import ZoomOutIcon from '@lucide/svelte/icons/zoom-out';
  import ShareIcon from '@lucide/svelte/icons/share-2';
  import TrashIcon from '@lucide/svelte/icons/trash-2';

  // Light-on-dark buttons for the viewer's black backdrop.
  const dark = 'text-white/90 hover:bg-white/15 hover:text-white focus-visible:ring-white/40';

  interface Item {
    name: string;
    size: number;
    src: string;
    download: string;
    /** The small thumbnail the list already loaded (shown at once). */
    thumb?: string;
    /** A large thumbnail, for an image format the browser can't decode. */
    poster?: string;
    mtime?: string;
  }
  let {
    items,
    index = $bindable(),
    onclose,
    onedit,
    onshare,
    ondelete
  }: {
    items: Item[];
    index: number;
    onclose: () => void;
    onedit?: (index: number) => void;
    onshare?: (index: number) => void;
    /** Asks, deletes, and drops the item from `items` once it is gone. */
    ondelete?: (index: number) => void;
  } = $props();

  let item = $derived(items[index]);
  let text = $state<string | null>(null);
  let view = $state<View>(fit);
  let dims = $state<{ w: number; h: number } | null>(null);
  let info = $state(false);
  let root: HTMLDivElement;

  /** How an image shows: itself, its large thumbnail, or not at all. */
  const imageSrc = (it: Item) =>
    isBrowserImage(it.name) ? it.src : isImage(it.name) && it.poster ? it.poster : null;
  let shownImage = $derived(item ? imageSrc(item) : null);
  let anImage = $derived(item ? isBrowserImage(item.name) || isImage(item.name) : false);

  // Back closes the viewer: it is a (shallow) history entry of its own.
  let pushed = false;
  onMount(() => {
    pushState('', { ...page.state, viewer: true });
    pushed = true;
  });
  $effect(() => {
    if (pushed && !page.state.viewer) {
      pushed = false;
      onclose();
    }
  });
  function close() {
    if (pushed && page.state.viewer) history.back(); // the effect above closes
    else onclose();
  }
  onDestroy(() => {
    // Closed another way (edit, the last file deleted): drop our entry, but
    // not when leaving for another page (that back would undo it).
    if (pushed && page.state.viewer && !navigating.to) history.back();
    pushed = false;
    preload.clear();
  });

  // The full images either side are fetched ahead; any other is let go.
  const preload = new Preloader();
  $effect(() => {
    preload.keep([index - 1, index + 1].map((j) => (items[j] ? imageSrc(items[j]) : null)));
  });

  const isText = (n: string, size: number) => size < 2_000_000 && isTextName(n);

  // The PDF viewer component, loaded on first use.
  let Pdf = $state<typeof import('./pdf/PdfViewer.svelte').default>();
  $effect(() => {
    if (!Pdf && isPdf(item.name))
      import('./pdf/PdfViewer.svelte').then((m) => (Pdf = m.default), staleApp);
  });

  // The media player, likewise (it stays out of the startup bundle).
  let Player = $state<typeof import('./media/MediaPlayer.svelte').default>();
  $effect(() => {
    if (!Player && (isVideo(item.name) || isAudio(item.name)))
      import('./media/MediaPlayer.svelte').then((m) => (Player = m.default), staleApp);
  });

  $effect(() => {
    text = null;
    view = fit;
    dims = null;
    const it = item;
    if (it && !anImage && isText(it.name, it.size)) {
      fetch(it.src).then(decodeText).then((t) => { if (item === it) text = t; }).catch(() => {});
    }
  });

  // Decode with the charset the server detected; text without one is UTF-8
  // if valid, else a legacy 8-bit encoding (Windows-1251 covers Cyrillic).
  async function decodeText(r: Response): Promise<string> {
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    const buf = await r.arrayBuffer();
    const charset = /charset=([\w-]+)/i.exec(r.headers.get('content-type') ?? '')?.[1];
    // Trust the server's charset (an odd invalid byte further in shows as
    // U+FFFD); only when it gave none, find out ourselves.
    if (charset) return new TextDecoder(charset).decode(buf);
    try {
      return new TextDecoder('utf-8', { fatal: true }).decode(buf);
    } catch {
      // Not UTF-8: guess the legacy code page from the bytes. Cyrillic text
      // is mostly high bytes; Western text is mostly ASCII letters with a
      // few accented ones.
      // Code-mostly files with Cyrillic comments are mostly ASCII, so a
      // Cyrillic browser language also tips it to 1251 when letters appear.
      const bytes = new Uint8Array(buf);
      let cyr = 0, ascii = 0;
      for (const b of bytes) {
        if (b >= 0xc0 || (b >= 0xa5 && b <= 0xbf)) cyr++;
        else if ((b | 0x20) >= 0x61 && (b | 0x20) <= 0x7a) ascii++;
      }
      const cyrLocale = /^(uk|ru|be|bg|sr|mk|kk)\b/i.test(navigator.language);
      const cp1251 = cyr > ascii || (cyrLocale && cyr > 0);
      return new TextDecoder(cp1251 ? 'windows-1251' : 'windows-1252').decode(buf);
    }
  }

  function step(d: number) {
    const n = index + d;
    if (n >= 0 && n < items.length) index = n;
  }
  /** The zoom buttons and keys: about the centre of the stage. */
  function zoomBy(k: number) {
    const s = Math.min(MAX, view.s * k);
    view = s <= 1.01 ? fit : zoomAt(view, s, 0, 0);
  }
  function key(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    // Keys in a dialog over the viewer (delete, share) are that dialog's.
    if (t && t !== document.body && !root.contains(t)) return;
    if (e.key === 'Escape') {
      e.preventDefault();
      if (info) info = false;
      else close();
      return;
    }
    // (Nor are arrow keys on a focused player: they seek.)
    if (t?.closest('input, textarea, select, [contenteditable], video, audio')) return;
    // In a PDF the arrow keys scroll the pages.
    if (isPdf(item.name)) return;
    if (e.key === 'ArrowRight') step(1);
    if (e.key === 'ArrowLeft') step(-1);
    // (With Ctrl/Cmd these zoom the page itself.)
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (shownImage && (e.key === '+' || e.key === '=')) zoomBy(1.5);
    if (shownImage && e.key === '-') zoomBy(1 / 1.5);
    if (shownImage && e.key === '0') view = fit;
  }
  let touchX = 0;
</script>

<svelte:window onkeydown={key} />

<div class="viewer" role="dialog" aria-modal="true" aria-label="Preview of {item.name}" tabindex="-1"
  bind:this={root} use:trapFocus
  ontouchstart={(e) => (touchX = e.touches[0].clientX)}
  ontouchend={(e) => {
    // Images swipe in their own view (which knows about zoom); a PDF scrolls.
    const t = e.target as HTMLElement | null;
    if (anImage || isPdf(item.name) || t?.closest('video, audio, header, aside')) return;
    const dx = e.changedTouches[0].clientX - touchX;
    if (Math.abs(dx) > 60) step(dx < 0 ? 1 : -1);
  }}>
  <header>
    <div class="min-w-0 flex-1">
      <h2 class="truncate text-sm font-semibold">{item.name}</h2>
      <div class="text-xs text-white/60">{bytes(item.size)} · {index + 1} of {items.length}</div>
    </div>
    {#if shownImage}
      <Button variant="ghost" size="icon" class={dark} onclick={() => zoomBy(1 / 1.5)} disabled={view.s <= 1.01} aria-label="Zoom out"><ZoomOutIcon /></Button>
      <Button variant="ghost" size="icon" class={dark} onclick={() => zoomBy(1.5)} disabled={view.s >= MAX} aria-label="Zoom in"><ZoomInIcon /></Button>
    {/if}
    {#if anImage}
      <Button variant="ghost" size="icon" class={cn(dark, info && 'bg-white/15')} onclick={() => (info = !info)}
        aria-label="Details" aria-expanded={info} aria-controls="viewer-info"><InfoIcon /></Button>
    {/if}
    {#if onedit && isTextName(item.name)}
      <Button variant="ghost" size="sm" class={dark} onclick={() => onedit(index)}><PencilIcon /><span class="max-sm:sr-only">Edit</span></Button>
    {/if}
    <a data-slot="button" class={cn(buttonClass('ghost', 'sm'), dark)} href={item.download}><DownloadIcon /><span class="max-sm:sr-only">Download</span></a>
    <Button variant="ghost" size="icon" class={dark} onclick={close} aria-label="Close"><XIcon /></Button>
  </header>
  <div class="body">
    <div class="stage" class:image={anImage}>
      {#if anImage}
        {#if shownImage}
          {#key item.src}
            <!-- SVG is shown only through <img> (the image route), which never runs scripts. -->
            <ImageView src={shownImage} thumb={item.thumb ?? null} name={item.name} bind:view
              onswipe={step} onsize={(w, h) => { if (isBrowserImage(item.name)) dims = { w, h }; }} />
          {/key}
          {#if !isBrowserImage(item.name)}
            <p class="note">A preview: this format can't be shown in full here. <a href={item.download}>Download it</a>.</p>
          {/if}
        {:else}
          <div class="none">This image format can't be shown here. <a href={item.download}>Download it</a>.</div>
        {/if}
      {:else if isPdf(item.name)}
        <!-- pdf.js loads only now, in its own chunks. -->
        {#if Pdf}{#key item.src}<Pdf src={item.src} download={item.download} name={item.name} />{/key}{/if}
      {:else if isVideo(item.name) || isAudio(item.name)}
        <!-- Keyed by file: each one gets a fresh player and its own position. -->
        {#if Player}{#key item.src}
          <Player kind={isVideo(item.name) ? 'video' : 'audio'} src={item.src} name={item.name}
            owner={session.user?.id ?? null}
            file={session.user && !item.src.startsWith('/api/public/') ? `${session.user.id}|${item.src}` : null} />
        {/key}{/if}
      {:else if text !== null}
        <pre>{text}</pre>
      {:else}
        <div class="none">No preview for this file. <a href={item.download}>Download it</a>.</div>
      {/if}
      {#if index > 0}<Button variant="ghost" size="icon" class={cn(dark, 'nav prev size-12 rounded-full bg-black/40')} onclick={() => step(-1)} aria-label="Previous"><ChevronLeftIcon class="size-7" /></Button>{/if}
      {#if index < items.length - 1}<Button variant="ghost" size="icon" class={cn(dark, 'nav next size-12 rounded-full bg-black/40')} onclick={() => step(1)} aria-label="Next"><ChevronRightIcon class="size-7" /></Button>{/if}
    </div>
    {#if info && anImage}
      <aside id="viewer-info" aria-label="Image details">
        <dl>
          <dt>Name</dt><dd class="break-all">{item.name}</dd>
          <dt>Size</dt><dd>{bytes(item.size)}</dd>
          <dt>Dimensions</dt><dd>{dims ? `${dims.w} × ${dims.h}` : '—'}</dd>
          {#if item.mtime}<dt>Modified</dt><dd>{new Date(item.mtime).toLocaleString()}</dd>{/if}
        </dl>
        <div class="actions">
          <a data-slot="button" class={cn(buttonClass('ghost', 'sm'), dark)} href={item.download}><DownloadIcon />Download</a>
          {#if onshare}<Button variant="ghost" size="sm" class={dark} onclick={() => onshare(index)}><ShareIcon />Share…</Button>{/if}
          {#if ondelete}<Button variant="ghost" size="sm" class={cn(dark, 'text-red-300 hover:text-red-200')} onclick={() => ondelete(index)}><TrashIcon />Delete…</Button>{/if}
        </div>
      </aside>
    {/if}
  </div>
</div>

<style>
  .viewer { position: fixed; inset: 0; background: #0a0a0a; z-index: 50; display: flex; flex-direction: column; color: #e5e7eb; }
  .viewer:focus { outline: none; }
  header { display: flex; gap: 4px; align-items: center; padding: 8px 8px 8px 16px; padding-top: max(8px, env(safe-area-inset-top)); }
  .body { flex: 1; min-height: 0; display: flex; }
  .stage { flex: 1; min-width: 0; display: grid; place-items: center; overflow: auto; padding: 8px; position: relative; }
  .stage.image { display: flex; flex-direction: column; place-items: stretch; padding: 0; overflow: hidden; }
  pre { background: #0b1120; color: #e5e7eb; padding: 16px; border-radius: 8px; width: min(1000px, 100%); height: 100%; overflow: auto; margin: 0; white-space: pre-wrap; word-break: break-word; }
  .none, .note { color: #94a3b8; }
  .stage.image .none { margin: auto; }
  .note { text-align: center; font-size: .8rem; padding: 4px 8px 8px; }
  .none a, .note a { text-decoration: underline; }
  aside { width: 18rem; flex-shrink: 0; border-left: 1px solid rgb(255 255 255 / .1); padding: 16px; overflow: auto; font-size: .875rem; }
  dl { display: grid; grid-template-columns: auto 1fr; gap: 6px 12px; margin: 0 0 16px; }
  dt { color: #94a3b8; }
  dd { margin: 0; }
  .actions { display: flex; flex-direction: column; align-items: flex-start; gap: 4px; }
  @media (width < 48rem) {
    .body { flex-direction: column; }
    aside { width: auto; border-left: 0; border-top: 1px solid rgb(255 255 255 / .1); max-height: 45%; padding-bottom: max(16px, env(safe-area-inset-bottom)); }
  }
  .viewer :global(.nav) { position: absolute; top: 50%; transform: translateY(-50%); z-index: 1; }
  .viewer :global(.prev) { left: 12px; } .viewer :global(.next) { right: 12px; }
  @media (hover: none) { .viewer :global(.nav) { display: none; } }
</style>
