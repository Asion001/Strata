<script lang="ts">
  // Full-screen preview: image gallery (arrows, swipe, zoom), native video/audio, text.
  import { isAudio, isImage, isPdf, isSvg, isTextName, isVideo, bytes } from './format';

  interface Item { name: string; size: number; src: string; download: string }
  let {
    items,
    index = $bindable(),
    onclose,
    onedit
  }: { items: Item[]; index: number; onclose: () => void; onedit?: (index: number) => void } = $props();

  let item = $derived(items[index]);
  let text = $state<string | null>(null);
  let zoom = $state(1);

  const isText = (n: string, size: number) => size < 2_000_000 && isTextName(n);

  // The PDF viewer component, loaded on first use.
  let Pdf = $state<typeof import('./pdf/PdfViewer.svelte').default>();
  $effect(() => {
    if (!Pdf && isPdf(item.name)) import('./pdf/PdfViewer.svelte').then((m) => (Pdf = m.default));
  });

  $effect(() => {
    text = null;
    zoom = 1;
    const it = item;
    if (it && isText(it.name, it.size)) {
      fetch(it.src).then(decodeText).then((t) => { if (item === it) text = t; }).catch(() => {});
    }
    // preload neighbouring images
    for (const j of [index - 1, index + 1]) {
      const n = items[j];
      if (n && isImage(n.name)) new Image().src = n.src;
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
  function key(e: KeyboardEvent) {
    // Typing in a field (e.g. the PDF viewer's find box) isn't navigation.
    const t = e.target as HTMLElement | null;
    if (t?.closest('input, textarea, select, [contenteditable]')) return;
    if (e.key === 'Escape') onclose();
    if (e.key === 'ArrowRight') step(1);
    if (e.key === 'ArrowLeft') step(-1);
  }
  let touchX = 0;
</script>

<svelte:window onkeydown={key} />

<div class="viewer" role="dialog" aria-modal="true" tabindex="-1"
  ontouchstart={(e) => (touchX = e.touches[0].clientX)}
  ontouchend={(e) => { if (zoom === 1 && !isPdf(item.name)) { const dx = e.changedTouches[0].clientX - touchX; if (Math.abs(dx) > 60) step(dx < 0 ? 1 : -1); } }}>
  <header>
    <span class="name">{item.name}</span>
    <span class="muted">{bytes(item.size)} · {index + 1}/{items.length}</span>
    {#if onedit && isTextName(item.name)}<button onclick={() => onedit(index)}>Edit</button>{/if}
    <a class="btn" href={item.download}>Download</a>
    <button onclick={onclose} aria-label="Close">✕</button>
  </header>
  <div class="stage">
    {#if isImage(item.name) || isSvg(item.name)}
      <!-- SVG is shown only through <img>, which never runs scripts. -->
      <img src={item.src} alt={item.name} style="transform: scale({zoom})"
        ondblclick={() => (zoom = zoom === 1 ? 2.5 : 1)} />
    {:else if isPdf(item.name)}
      <!-- pdf.js loads only now, in its own chunks. -->
      {#if Pdf}<Pdf src={item.src} download={item.download} name={item.name} />{/if}
    {:else if isVideo(item.name)}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video src={item.src} controls autoplay playsinline></video>
    {:else if isAudio(item.name)}
      <audio src={item.src} controls autoplay></audio>
    {:else if text !== null}
      <pre>{text}</pre>
    {:else}
      <div class="none">No preview for this file. <a href={item.download}>Download it</a>.</div>
    {/if}
  </div>
  {#if index > 0}<button class="nav prev" onclick={() => step(-1)} aria-label="Previous">‹</button>{/if}
  {#if index < items.length - 1}<button class="nav next" onclick={() => step(1)} aria-label="Next">›</button>{/if}
</div>

<style>
  .viewer { position: fixed; inset: 0; background: rgb(0 0 0 / .92); z-index: 50; display: flex; flex-direction: column; color: #e5e7eb; }
  header { display: flex; gap: 12px; align-items: center; padding: 10px 14px; padding-top: max(10px, env(safe-area-inset-top)); }
  .name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 600; }
  header button, .btn { background: rgb(255 255 255 / .1); color: inherit; border: 0; padding: 6px 12px; border-radius: 8px; }
  .stage { flex: 1; min-height: 0; display: grid; place-items: center; overflow: auto; padding: 8px; position: relative; }
  img, video { max-width: 100%; max-height: 100%; object-fit: contain; transition: transform .15s; touch-action: pinch-zoom; }
  pre { background: #0b1120; color: #e5e7eb; padding: 16px; border-radius: 8px; width: min(1000px, 100%); height: 100%; overflow: auto; margin: 0; white-space: pre-wrap; word-break: break-word; }
  .none { color: #94a3b8; }
  .nav { position: absolute; top: 50%; transform: translateY(-50%); font-size: 36px; background: rgb(255 255 255 / .08); border: 0; color: #fff; width: 48px; height: 72px; }
  .prev { left: 8px; } .next { right: 8px; }
  @media (hover: none) { .nav { display: none; } }
</style>
