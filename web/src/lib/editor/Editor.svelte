<script lang="ts">
  // Text/code editor with conflict-safe saves:
  // - loads the file with its ETag, saves with If-Match (412 -> conflict dialog);
  // - autosaves unsaved drafts to IndexedDB every 2 s, offers them back on reopen;
  // - keeps the file's line endings and trailing newline;
  // - Ctrl/Cmd+S saves; leaving with unsaved changes asks first.
  import { onDestroy, onMount } from 'svelte';
  import { beforeNavigate } from '$app/navigation';
  import { contentUrl } from '$lib/api';
  import { join, parent } from '$lib/format';
  import { MAX_EDIT_BYTES, MAX_VIEW_BYTES, conflictName, decodeUtf8, fromEditor, looksBinary, toEditor, type Eol } from './textio';
  import {
    clearAfterSave,
    clearDraft,
    markOfferSeen,
    offerSeen,
    clearTabDraft,
    draftKey,
    expireDrafts,
    isSignedIn,
    listDrafts,
    saveTabDraft,
    type Draft
  } from './drafts';
  import { createBLAKE3 } from 'hash-wasm';

  let {
    path,
    userId,
    mountId,
    onclose
  }: { path: string; userId: number; mountId: number | null; onclose: () => void } = $props();

  // One editor instance per file: the parent re-creates it ({#key}) when
  // the path changes, so these never go stale.
  const name = $derived(path.slice(path.lastIndexOf('/') + 1));
  const isMarkdown = $derived(/\.(md|markdown)$/i.test(name));
  const key = $derived(draftKey(userId, mountId, path));

  let host: HTMLDivElement;
  let diffHost = $state<HTMLDivElement>();
  let editor: Awaited<ReturnType<typeof import('./cm').createEditor>> | null = null;
  let mergeView: { destroy(): void } | null = null;

  let loading = $state(true);
  let error = $state('');
  let notice = $state('');
  let readOnly = $state(false);
  let dirty = $state(false);
  let saving = $state(false);
  let status = $state('');
  let etag = $state<string | null>(null);
  /** BLAKE3 of the bytes the current edit is based on (sent with saves). */
  let baseHash = $state<string | null>(null);

  // One BLAKE3 instance (WASM) per editor, reused for every hash.
  let hasher: Awaited<ReturnType<typeof createBLAKE3>> | null = null;
  async function blake3Hex(bytes: Uint8Array): Promise<string> {
    hasher ??= await createBLAKE3();
    hasher.init();
    hasher.update(bytes);
    return hasher.digest('hex');
  }
  let eol: Eol = '\n';
  let savedDoc = '';
  let draftOffer = $state<Draft | null>(null);
  let conflict = $state<{ theirsEtag: string | null; showDiff: boolean } | null>(null);
  let preview = $state('');
  let showPreview = $state(false);
  let previewTimer: ReturnType<typeof setTimeout>;
  let draftTimer: ReturnType<typeof setInterval>;
  let lastDraft = '';

  let destroyed = false;
  /** This editor instance: drafts record which tab wrote them. */
  const tab = Math.random().toString(36).slice(2);

  async function readCapped(r: Response, cap: number): Promise<Uint8Array> {
    const reader = r.body!.getReader();
    const parts: Uint8Array[] = [];
    let total = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      total += value.length;
      if (total > cap) {
        await reader.cancel();
        throw new Error('This file is too large to open in the editor. Download it instead.');
      }
      parts.push(value);
    }
    const out = new Uint8Array(total);
    let off = 0;
    for (const p of parts) {
      out.set(p, off);
      off += p.length;
    }
    return out;
  }

  async function fetchText(): Promise<{ text: string; etag: string | null; hash: string | null; tooBig: boolean; utf8: boolean }> {
    const r = await fetch(contentUrl(path), { cache: 'no-store' });
    if (!r.ok) throw new Error(r.status === 404 ? 'The file no longer exists.' : `Could not open the file (HTTP ${r.status}).`);
    // Never pull a huge file into the tab: stop at the size limit, whatever
    // Content-Length says (the file may grow meanwhile).
    if (Number(r.headers.get('content-length') ?? 0) > MAX_VIEW_BYTES) {
      await r.body?.cancel();
      throw new Error('This file is too large to open in the editor. Download it instead.');
    }
    const bytes = await readCapped(r, MAX_VIEW_BYTES);
    if (looksBinary(bytes)) throw new Error('This looks like a binary file, so it cannot be opened as text.');
    const strict = decodeUtf8(bytes);
    const tooBig = bytes.length > MAX_EDIT_BYTES;
    return {
      text: strict ?? new TextDecoder('utf-8').decode(bytes),
      etag: r.headers.get('etag'),
      // Only editable files need their base hash (sent with saves).
      hash: tooBig || strict === null ? null : await blake3Hex(bytes),
      tooBig,
      utf8: strict !== null
    };
  }

  function renderPreview(doc: string) {
    if (!isMarkdown) return;
    clearTimeout(previewTimer);
    // Bigger documents re-render less often.
    previewTimer = setTimeout(async () => {
      const { renderMarkdown } = await import('./markdown');
      preview = renderMarkdown(doc);
    }, doc.length > 200_000 ? 800 : 150);
  }

  /** Editing rules for what was loaded: big or non-UTF-8 files are read-only. */
  function applyRules(f: { tooBig: boolean; utf8: boolean }) {
    readOnly = f.tooBig || !f.utf8;
    notice = f.tooBig
      ? 'This file is larger than 5 MB, so it opens read-only.'
      : !f.utf8
        ? 'This file is not UTF-8 text, so it opens read-only.'
        : '';
    editor?.setReadOnly(readOnly);
  }

  onMount(async () => {
    showPreview = isMarkdown;
    expireDrafts().catch(() => {});
    try {
      const f = await fetchText();
      if (destroyed) return;
      etag = f.etag;
      baseHash = f.hash;
      const t = toEditor(f.text);
      eol = t.eol;
      savedDoc = t.doc;
      applyRules(f);
      const { createEditor } = await import('./cm');
      if (destroyed) return;
      const created = await createEditor(host, {
        doc: t.doc,
        normalizePaste: () => eol !== 'keep',
        name,
        readOnly,
        onChange: (length, getDoc) => {
          // Cheap for big files: different length means changed; only when
          // the length matches is the text compared (undo back to saved).
          dirty = length !== savedDoc.length || (length < 1_000_000 ? getDoc() !== savedDoc : true);
          if (isMarkdown) renderPreview(getDoc());
        }
      });
      if (destroyed) {
        created.destroy();
        return;
      }
      editor = created;
      renderPreview(t.doc);
      if (!readOnly) {
        // The newest unsaved work any tab left for this file.
        const all = await listDrafts(key).catch(() => [] as Draft[]);
        if (destroyed) return;
        draftOffer = all.find((d) => !offerSeen(d) && d.text !== t.doc) ?? null;
        draftTimer = setInterval(autosave, 2000);
      }
    } catch (e) {
      error = (e as Error).message;
    } finally {
      loading = false;
    }
  });

  onDestroy(() => {
    destroyed = true;
    clearInterval(draftTimer);
    clearTimeout(previewTimer);
    editor?.destroy();
    mergeView?.destroy();
  });

  async function autosave() {
    if (!editor || !dirty) return;
    // Only while this user is still the one signed in here (any tab may
    // have signed out); checked again after writing, since a sign-out can
    // land in between.
    if (!isSignedIn(userId)) return;
    const text = editor.getDoc();
    if (text === lastDraft) return;
    lastDraft = text;
    // This tab's own entry: other tabs' drafts (and a recovery offer still
    // on screen) are never overwritten.
    await saveTabDraft(key, tab, { baseEtag: etag, baseHash, text, savedAt: Date.now() }).catch(() => {});
    if (!isSignedIn(userId)) await clearTabDraft(key, tab).catch(() => {});
  }

  async function restoreDraft() {
    const d = draftOffer;
    if (!d || !editor) return;
    if (dirty && !confirm('Replace what you typed since opening with the recovered draft?')) return;
    // Keep the draft's base: if the file changed since, saving asks how to resolve.
    etag = d.baseEtag;
    baseHash = d.baseHash ?? null;
    editor.setDoc(d.text);
    draftOffer = null;
    lastDraft = '';
    // Our own copy carries on from here. The old entry is left alone (it may
    // be another tab's live work); once our copy is stored, it just isn't
    // offered again here. If storing fails, both stay recoverable.
    if (!isSignedIn(userId)) return;
    try {
      await saveTabDraft(key, tab, { baseEtag: etag, baseHash, text: d.text, savedAt: Date.now() });
      lastDraft = d.text;
      markOfferSeen(d);
    } catch {}
  }

  function discardDraft() {
    const d = draftOffer;
    draftOffer = null;
    // Not deleted: it may be the live work of another open tab. It isn't
    // offered again unless that tab saves newer text into it.
    if (d) markOfferSeen(d);
  }

  async function put(target: string, body: Uint8Array<ArrayBuffer>, precondition: Record<string, string>) {
    return fetch(`/api/files/content?path=${encodeURIComponent(target)}`, {
      method: 'PUT',
      headers: { 'x-strata': '1', 'content-type': 'text/plain; charset=utf-8', ...precondition },
      body
    });
  }

  async function save() {
    if (!editor || readOnly || saving) return;
    saving = true;
    status = 'Saving…';
    const doc = editor.getDoc();
    const bytes = new TextEncoder().encode(fromEditor(doc, eol));
    const pre: Record<string, string> = etag ? { 'if-match': etag } : { 'if-none-match': '*' };
    if (etag && baseHash) pre['x-strata-base-hash'] = baseHash;
    try {
      // The hash of what we're about to save becomes the next base: compute
      // it first, so a hashing problem can't leave a saved file half-tracked.
      const nextHash = await blake3Hex(bytes);
      const r = await put(path, bytes, pre);
      const body = await r.json().catch(() => ({}));
      if (r.status === 412) {
        conflict = { theirsEtag: body.etag ?? null, showDiff: false };
        status = '';
        return;
      }
      if (r.status === 428 && etag) {
        // The server can't verify what this edit was based on (e.g. a draft
        // restored without its base hash): resolve it like a conflict.
        const cur = await fetch(contentUrl(path), { method: 'HEAD', cache: 'no-store' });
        conflict = { theirsEtag: cur.ok ? cur.headers.get('etag') ?? 'unknown' : null, showDiff: false };
        status = '';
        return;
      }
      if (!r.ok) throw new Error(body.error ?? `HTTP ${r.status}`);
      etag = r.headers.get('etag') ?? body.etag;
      baseHash = nextHash;
      savedDoc = doc;
      dirty = editor.getDoc() !== savedDoc;
      lastDraft = '';
      await clearAfterSave(key, tab, doc).catch(() => {});
      status = 'Saved';
      setTimeout(() => status === 'Saved' && (status = ''), 2000);
    } catch (e) {
      status = `Save failed: ${(e as Error).message}`;
    } finally {
      saving = false;
    }
  }

  async function keepMine() {
    if (!editor) return;
    const body = new TextEncoder().encode(fromEditor(editor.getDoc(), eol));
    // A copy next to the original; add seconds if that name is taken too.
    for (const seconds of [false, true]) {
      const target = join(parent(path), conflictName(name, new Date(), seconds));
      const r = await put(target, body, { 'if-none-match': '*' });
      if (r.ok) {
        const msg = `Your version was saved as “${target.slice(target.lastIndexOf('/') + 1)}”.`;
        if (conflict?.theirsEtag) {
          await reloadTheirs(msg);
        } else {
          // The original is gone: the copy holds the text now. Close the
          // editor; the folder shows the copy.
          closeConflict();
          dirty = false;
          await clearTabDraft(key, tab).catch(() => {});
          alert(msg);
          onclose();
        }
        return;
      }
      if (r.status !== 412) {
        status = `Could not save a copy (HTTP ${r.status})`;
        return;
      }
    }
    status = 'Could not save a copy: the name is taken.';
  }

  /** The file was deleted meanwhile: save mine under its name again. */
  async function recreate() {
    etag = null; // save() then uses If-None-Match: *
    baseHash = null;
    closeConflict();
    await save();
  }

  async function reloadTheirs(message = '') {
    try {
      const f = await fetchText();
      const t = toEditor(f.text);
      etag = f.etag;
      baseHash = f.hash;
      eol = t.eol;
      savedDoc = t.doc;
      editor?.setDoc(t.doc);
      applyRules(f);
      dirty = false;
      lastDraft = '';
      await clearTabDraft(key, tab).catch(() => {});
      closeConflict();
      status = message;
    } catch (e) {
      status = (e as Error).message;
    }
  }

  async function showDiff() {
    if (!editor || !conflict) return;
    conflict.showDiff = true;
    try {
      const theirs = toEditor((await fetchText()).text).doc;
      await Promise.resolve();
      if (diffHost) {
        const { createMergeView } = await import('./cm');
        mergeView?.destroy();
        mergeView = await createMergeView(diffHost, editor.getDoc(), theirs, name);
      }
    } catch (e) {
      status = (e as Error).message;
    }
  }

  function closeConflict() {
    mergeView?.destroy();
    mergeView = null;
    conflict = null;
  }

  async function close() {
    if (dirty && !confirm('Close with unsaved changes? A draft stays in this browser for up to 30 days, until you sign out, or until someone else signs in here.')) return;
    await autosave(); // the last seconds of typing too
    onclose();
  }

  beforeNavigate((nav) => {
    if (dirty && !confirm('You have unsaved changes. Leave anyway?')) {
      nav.cancel();
      return;
    }
    void autosave();
  });

  // Ctrl/Cmd+S anywhere while the editor is open (not only in the text area).
  function onKey(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 's') {
      e.preventDefault();
      void save();
    }
  }

  function beforeUnload(e: BeforeUnloadEvent) {
    if (dirty) {
      e.preventDefault();
      e.returnValue = '';
    }
  }
</script>

<svelte:window onbeforeunload={beforeUnload} onpagehide={() => void autosave()} onkeydown={onKey} />

<div class="editor" role="dialog" aria-modal="true" aria-label="Edit {name}" tabindex="-1">
  <header>
    <span class="name">{name}{dirty ? ' •' : ''}</span>
    <span class="status" aria-live="polite">{status}</span>
    {#if isMarkdown}
      <button onclick={() => (showPreview = !showPreview)}>{showPreview ? 'Hide preview' : 'Preview'}</button>
    {/if}
    {#if !readOnly && !error}
      <button class="primary" onclick={save} disabled={saving || !dirty} title="Ctrl/Cmd+S">Save</button>
    {/if}
    <button onclick={close} aria-label="Close">✕</button>
  </header>

  {#if notice}<div class="banner">{notice}</div>{/if}
  {#if draftOffer}
    <div class="banner">
      Unsaved changes from {new Date(draftOffer.savedAt).toLocaleString()} were found{draftOffer.baseEtag !== etag || (draftOffer.baseHash ?? null) !== baseHash ? ' (the file has changed since)' : ''}.
      <button onclick={restoreDraft}>Restore</button>
      <button onclick={discardDraft}>Discard</button>
    </div>
  {/if}

  {#if error}
    <div class="error pad">{error}</div>
  {/if}
  <div class="body" class:split={showPreview} class:hidden={!!error}>
    <div class="cm" bind:this={host}>{#if loading}<div class="pad muted">Loading…</div>{/if}</div>
    {#if showPreview}
      <!-- Sanitised by DOMPurify (see markdown.ts); raw HTML is disabled. -->
      <article class="preview">{@html preview}</article>
    {/if}
  </div>
</div>

{#if conflict}
  <div class="conflict-bg">
    <div class="conflict card stack" class:wide={conflict.showDiff} role="alertdialog" aria-label="Save conflict">
      {#if conflict.theirsEtag}
        <b>Someone else changed “{name}” while you were editing.</b>
      {:else}
        <b>“{name}” was deleted or moved while you were editing.</b>
      {/if}
      <p class="muted">Nothing has been overwritten. Choose what to do with your changes:</p>
      {#if conflict.showDiff}
        <div class="labels"><span>Yours</span><span>Theirs (saved)</span></div>
        <div class="diff" bind:this={diffHost}></div>
      {/if}
      <div class="row">
        {#if conflict.theirsEtag}
          <button class="primary" onclick={keepMine}>Keep mine as a copy</button>
          <button onclick={() => reloadTheirs()}>Reload theirs</button>
          {#if !conflict.showDiff}<button onclick={showDiff}>View differences</button>{/if}
        {:else}
          <button class="primary" onclick={recreate}>Save it again</button>
          <button onclick={keepMine}>Save as a copy</button>
        {/if}
        <span class="grow"></span>
        <button onclick={closeConflict}>Back to editing</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .editor { position: fixed; inset: 0; z-index: 45; background: var(--bg); display: flex; flex-direction: column; }
  header { display: flex; gap: 8px; align-items: center; padding: 8px 12px; padding-top: max(8px, env(safe-area-inset-top)); border-bottom: 1px solid var(--border); background: var(--panel); }
  .name { font-weight: 600; flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .status { color: var(--muted); font-size: .9em; }
  .banner { padding: 8px 12px; background: var(--hover); border-bottom: 1px solid var(--border); display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  .pad { padding: 16px; }
  .body { flex: 1; min-height: 0; display: grid; grid-template-columns: 1fr; }
  .body.split { grid-template-columns: 1fr 1fr; }
  .body.hidden { display: none; }
  .cm { min-height: 0; overflow: auto; background: var(--panel); }
  .cm :global(.cm-editor) { height: 100%; font-size: 14px; }
  .cm :global(.cm-scroller) { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
  .preview { overflow: auto; padding: 16px 24px; border-left: 1px solid var(--border); line-height: 1.6; }
  .preview :global(pre) { background: var(--hover); padding: 10px; border-radius: 8px; overflow: auto; }
  .preview :global(img) { max-width: 100%; }
  .conflict-bg { position: fixed; inset: 0; z-index: 60; background: rgb(0 0 0 / .45); display: grid; place-items: center; padding: 16px; }
  .conflict { width: min(560px, 100%); max-height: 90dvh; overflow: auto; }
  .conflict.wide { width: min(1200px, 100%); }
  .conflict p { margin: 0; }
  .labels { display: grid; grid-template-columns: 1fr 1fr; color: var(--muted); font-size: .9em; }
  .diff { max-height: 60dvh; overflow: auto; border: 1px solid var(--border); border-radius: 8px; }
  @media (max-width: 800px) {
    .body.split { grid-template-columns: 1fr; grid-template-rows: 1fr 1fr; }
    .preview { border-left: 0; border-top: 1px solid var(--border); }
  }
</style>
