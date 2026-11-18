<script lang="ts">
  // PDF viewer: pages, thumbnails, find, zoom, page number, download and
  // print. Loaded lazily (pdf.js stays out of the startup bundle).
  import 'pdfjs-dist/web/pdf_viewer.css';
  import { onDestroy, onMount } from 'svelte';
  import type { PDFDocumentProxy, PDFDocumentLoadingTask } from 'pdfjs-dist';

  let { src, download, name }: { src: string; download: string; name: string } = $props();

  let container: HTMLDivElement;
  let viewerEl: HTMLDivElement;
  let doc = $state<PDFDocumentProxy | null>(null);
  let pages = $state(0);
  let page = $state(1);
  let pageInput = $state('1');
  let scaleLabel = $state('');
  let error = $state('');
  let query = $state('');
  let matches = $state<{ current: number; total: number } | null>(null);
  let notFound = $state(false);
  let showThumbs = $state(typeof window !== 'undefined' && window.innerWidth > 800);
  let printing = $state('');

  let task: PDFDocumentLoadingTask | null = null;
  let pageBox: HTMLInputElement;
  /** The page box's value when it got focus: leaving it unchanged goes nowhere. */
  let typedFrom = '';
  // pdf.js viewer objects (untyped here: the component module is loaded at runtime)
  let viewer: any = null;
  let bus: any = null;
  let destroyed = false;

  onMount(async () => {
    try {
      const { openPdf, viewerKit, pdfjs } = await import('./pdf');
      const v = await viewerKit();
      if (destroyed) return;

      bus = new v.EventBus();
      // In-document links navigate here; external ones see SafeLinkService.
      const links = new v.SafeLinkService({ eventBus: bus, externalLinkTarget: v.LinkTarget.BLANK, externalLinkRel: 'noopener noreferrer' });
      const find = new v.PDFFindController({ eventBus: bus, linkService: links });
      viewer = new v.PDFViewer({
        container,
        viewer: viewerEl,
        eventBus: bus,
        linkService: links,
        findController: find,
        // Links only: no form fields and no XFA. No scripting manager is
        // passed, which is what keeps document JavaScript from ever running.
        annotationMode: pdfjs.AnnotationMode.ENABLE,
        removePageBorders: false
      });
      links.setViewer(viewer);

      bus.on('pagesinit', () => (viewer.currentScaleValue = 'auto'));
      bus.on('pagechanging', (e: { pageNumber: number }) => {
        page = e.pageNumber;
        // Not while someone is typing a page number.
        if (document.activeElement !== pageBox) pageInput = String(e.pageNumber);
      });
      bus.on('scalechanging', (e: { scale: number }) => (scaleLabel = `${Math.round(e.scale * 100)}%`));
      bus.on('updatefindmatchescount', (e: { matchesCount: { current: number; total: number } }) => (matches = e.matchesCount));
      bus.on('updatefindcontrolstate', (e: { state: number; matchesCount: { current: number; total: number } }) => {
        notFound = e.state === v.FindState.NOT_FOUND;
        matches = e.matchesCount;
      });

      task = openPdf(src);
      task.onPassword = (supply: (p: string) => void, reason: number) => {
        const p = prompt(reason === pdfjs.PasswordResponses.INCORRECT_PASSWORD ? 'Wrong password. Try again:' : 'This PDF is password-protected. Password:');
        if (p === null) {
          error = 'This PDF is password-protected.';
          task?.destroy();
        } else supply(p);
      };
      const d = await task.promise;
      if (destroyed) return void task.destroy();
      doc = d;
      pages = d.numPages;
      viewer.setDocument(d);
      links.setDocument(d, null);
    } catch (e) {
      if (!destroyed && !error) error = `Could not open this PDF: ${(e as Error).message}`;
    }
  });

  onDestroy(() => {
    destroyed = true;
    thumbObserver?.disconnect();
    viewer?.cleanup?.();
    task?.destroy();
  });

  function search(again = false, previous = false) {
    if (!bus) return;
    bus.dispatch('find', {
      source: null,
      type: again ? 'again' : '',
      query,
      caseSensitive: false,
      entireWord: false,
      highlightAll: true,
      findPrevious: previous,
      matchDiacritics: false
    });
  }

  function goTo(n: number) {
    if (!viewer || !pages) return;
    const p = Math.min(Math.max(1, Math.round(n) || 1), pages);
    viewer.currentPageNumber = p;
    pageInput = String(p);
  }

  function zoom(factor: number) {
    if (!viewer) return;
    viewer.currentScale = Math.min(10, Math.max(0.1, viewer.currentScale * factor));
  }

  // Thumbnails: each one renders when it scrolls into the sidebar.
  let thumbObserver: IntersectionObserver | null = null;
  function thumb(canvas: HTMLCanvasElement, n: number) {
    thumbObserver ??= new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (!e.isIntersecting) continue;
          thumbObserver!.unobserve(e.target);
          void renderThumb(e.target as HTMLCanvasElement);
        }
      },
      { rootMargin: '200px' }
    );
    canvas.dataset.page = String(n);
    thumbObserver.observe(canvas);
    return { destroy: () => thumbObserver?.unobserve(canvas) };
  }
  async function renderThumb(canvas: HTMLCanvasElement) {
    if (!doc) return;
    try {
      const p = await doc.getPage(Number(canvas.dataset.page));
      const vp = p.getViewport({ scale: 1 });
      const scale = (120 * devicePixelRatio) / vp.width;
      const v2 = p.getViewport({ scale });
      canvas.width = v2.width;
      canvas.height = v2.height;
      await p.render({ canvas, viewport: v2 }).promise;
      p.cleanup();
    } catch {}
  }

  // Print: render every page to an image in a print-only container (the
  // browser can't print the canvases pdf.js shows, and the file itself is
  // served sandboxed, which browsers won't print from a frame). Pages are
  // JPEG at 150 dpi to keep memory in check; long documents ask first, and
  // the preparation can be cancelled.
  let cancelPrint = false;
  async function print() {
    if (!doc) return;
    if (printing) {
      cancelPrint = true;
      return;
    }
    const n = doc.numPages;
    if (n > 100 && !confirm(`Prepare all ${n} pages for printing? This can take a while; you can cancel it.`)) return;
    cancelPrint = false;
    const urls: string[] = [];
    const root = document.createElement('div');
    root.className = 'print-root';
    const html = document.documentElement;
    const cleanup = () => {
      html.classList.remove('printing-pdf');
      root.remove();
      urls.forEach((u) => URL.revokeObjectURL(u));
    };
    try {
      for (let i = 1; i <= n; i++) {
        if (cancelPrint || destroyed) return cleanup();
        printing = `Preparing ${i}/${n}… (cancel)`;
        const p = await doc.getPage(i);
        const vp = p.getViewport({ scale: 150 / 72 });
        const c = document.createElement('canvas');
        c.width = vp.width;
        c.height = vp.height;
        await p.render({ canvas: c, viewport: vp, intent: 'print' }).promise;
        p.cleanup();
        const blob = await new Promise<Blob | null>((r) => c.toBlob(r, 'image/jpeg', 0.92));
        c.width = c.height = 0;
        if (blob) urls.push(URL.createObjectURL(blob));
      }
      for (const u of urls) {
        const img = document.createElement('img');
        img.src = u;
        img.alt = '';
        root.append(img);
      }
      document.body.append(root);
      html.classList.add('printing-pdf');
      await Promise.all([...root.querySelectorAll('img')].map((i) => i.decode().catch(() => {})));
    } catch (e) {
      cleanup();
      throw e;
    } finally {
      printing = '';
    }
    // Some browsers (phones) return from print() before printing: clean up
    // only once the print dialog is done.
    addEventListener('afterprint', cleanup, { once: true });
    window.print();
  }

  function onKey(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key === 'f') {
      e.preventDefault();
      (document.querySelector('.pdf .find input') as HTMLInputElement | null)?.focus();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="pdf" aria-label={name}>
  <div class="bar">
    <button onclick={() => (showThumbs = !showThumbs)} aria-pressed={showThumbs} title="Thumbnails">☰</button>
    <label class="pageno">
      <span class="sr">Page</span>
      <input inputmode="numeric" aria-label="Page number" bind:value={pageInput} bind:this={pageBox}
        onkeydown={(e) => e.key === 'Enter' && goTo(Number(pageInput))}
        onfocus={() => (typedFrom = pageInput)}
        onblur={() => (pageInput !== typedFrom ? goTo(Number(pageInput)) : (pageInput = String(page)))} />
      <span>/ {pages || '…'}</span>
    </label>
    <button onclick={() => zoom(1 / 1.2)} aria-label="Zoom out">−</button>
    <span class="scale">{scaleLabel}</span>
    <button onclick={() => zoom(1.2)} aria-label="Zoom in">+</button>
    <button onclick={() => viewer && (viewer.currentScaleValue = 'page-width')} title="Fit width">↔</button>
    <form class="find" role="search" onsubmit={(e) => { e.preventDefault(); search(true); }}>
      <input type="search" placeholder="Find in document" aria-label="Find in document" bind:value={query}
        oninput={() => search()} class:nf={notFound && query} />
      {#if query && matches}<span class="count" aria-live="polite">{matches.total ? `${matches.current}/${matches.total}` : 'No matches'}</span>{/if}
      <button type="button" onclick={() => search(true, true)} aria-label="Previous match">↑</button>
      <button type="submit" aria-label="Next match">↓</button>
    </form>
    <a class="btn" href={download}>Download</a>
    <button onclick={print} disabled={!doc}>{printing || 'Print'}</button>
  </div>
  <div class="body">
    {#if showThumbs && doc}
      <nav class="thumbs" aria-label="Pages">
        {#each { length: pages } as _, i (i)}
          <button class:current={page === i + 1} onclick={() => goTo(i + 1)} aria-label="Page {i + 1}">
            <canvas use:thumb={i + 1}></canvas>
            <span>{i + 1}</span>
          </button>
        {/each}
      </nav>
    {/if}
    <div class="main">
      {#if error}<div class="error">{error} <a href={download}>Download it</a>.</div>{/if}
      <div class="scroller" bind:this={container}>
        <div class="pdfViewer" bind:this={viewerEl}></div>
      </div>
    </div>
  </div>
</div>


<style>
  .pdf { position: absolute; inset: 0; display: flex; flex-direction: column; background: #1f2937; }
  .bar { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; padding: 6px 10px; background: #111827; }
  .bar button, .bar .btn { background: rgb(255 255 255 / .1); color: inherit; border: 0; padding: 5px 10px; border-radius: 6px; }
  .bar input { background: #0b1120; color: inherit; border: 1px solid #374151; border-radius: 6px; padding: 4px 8px; }
  .pageno input { width: 3.5em; text-align: right; }
  .scale { min-width: 3.5em; text-align: center; font-variant-numeric: tabular-nums; }
  .find { display: flex; gap: 4px; align-items: center; margin-left: auto; }
  .find input { width: 14em; }
  .find input.nf { border-color: #b91c1c; }
  .count { font-size: 12px; color: #9ca3af; min-width: 5em; }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); }
  .body { flex: 1; min-height: 0; display: flex; }
  .thumbs { width: 150px; overflow-y: auto; padding: 8px; display: flex; flex-direction: column; gap: 10px; background: #111827; }
  .thumbs button { background: none; border: 2px solid transparent; border-radius: 4px; padding: 2px; color: #9ca3af; font-size: 12px; display: flex; flex-direction: column; align-items: center; gap: 2px; }
  .thumbs button.current { border-color: #60a5fa; color: #e5e7eb; }
  .thumbs canvas { width: 120px; min-height: 60px; background: #fff; }
  .main { flex: 1; min-width: 0; position: relative; }
  .scroller { position: absolute; inset: 0; overflow: auto; }
  .error { position: absolute; z-index: 2; inset: 20px auto auto 20px; color: #fca5a5; }
  /* Only while a PDF print is prepared (the class is set on <html>), so the
     app's own printing is unaffected after the viewer closes. */
  :global(.print-root) { display: none; }
  @media print {
    :global(html.printing-pdf body > *:not(.print-root)) { display: none !important; }
    :global(html.printing-pdf .print-root) { display: block; }
    :global(.print-root img) { display: block; width: 100%; break-after: page; }
  }
</style>
