<script lang="ts">
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { api, contentUrl, previewUrl, thumbUrl, type Entry } from '$lib/api';
  import { bytes, when, join, parent, icon, isImage, isTextName } from '$lib/format';
  import { runUpload, type UploadTask } from '$lib/upload';
  import VirtualList from '$lib/VirtualList.svelte';
  import Editor from '$lib/editor/Editor.svelte';
  import { staleApp } from '$lib/lazy';
  import { session } from '$lib/session.svelte';

  let path = $derived(page.url.searchParams.get('p') ?? '');
  let entries = $state<Entry[]>([]);
  let mount = $state<{ id: number; mode: string; connection_name: string; path: string } | null>(null);
  let editing = $state<string | null>(null);
  let loading = $state(false);
  let error = $state('');
  let query = $state('');
  let hits = $state<{ path: string; name: string; is_dir: boolean; size: number; mtime: string }[] | null>(null);
  let uploads = $state<UploadTask[]>([]);
  let menu = $state<Entry | null>(null);
  let viewerIndex = $state(-1);
  // The preview viewer (and, through it, the PDF viewer) loads on first use.
  let Viewer = $state<typeof import('$lib/Viewer.svelte').default>();
  $effect(() => {
    if (viewerIndex >= 0 && !Viewer) import('$lib/Viewer.svelte').then((m) => (Viewer = m.default), staleApp);
  });
  let dragging = $state(false);
  let fileInput: HTMLInputElement;
  let folderInput: HTMLInputElement;
  let seq = 0;

  async function load(p = path) {
    const my = ++seq;
    loading = true;
    error = '';
    try {
      const r = await api('GET', `/api/files/list?path=${encodeURIComponent(p)}`);
      if (my !== seq) return;
      entries = r.entries;
      mount = r.mount;
    } catch (e) {
      if (my === seq) { error = (e as Error).message; entries = []; }
    } finally {
      if (my === seq) loading = false;
    }
  }

  $effect(() => { load(path); });

  // Live updates: the server pushes the journal cursor; refresh on change.
  $effect(() => {
    let ws: WebSocket | null = null;
    let timer: ReturnType<typeof setTimeout>;
    let closed = false;
    const connect = () => {
      ws = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/ws`);
      let first = true;
      ws.onmessage = () => {
        if (first) { first = false; return; }
        clearTimeout(timer);
        timer = setTimeout(() => load(), 300);
      };
      ws.onclose = () => { if (!closed) setTimeout(connect, 3000); };
    };
    connect();
    return () => { closed = true; ws?.close(); };
  });

  const crumbs = $derived(path ? path.split('/').map((n, i, a) => ({ name: n, path: a.slice(0, i + 1).join('/') })) : []);
  const viewable = $derived(entries.filter((e) => !e.is_dir));

  function open(e: Entry) {
    if (e.is_dir) goto(`/?p=${encodeURIComponent(join(path, e.name))}`);
    else viewerIndex = viewable.indexOf(e);
  }

  async function act(fn: () => Promise<unknown>) {
    try { await fn(); } catch (e) { alert((e as Error).message); }
    menu = null;
    await load();
  }

  const newFolder = () => {
    const name = prompt('New folder name');
    if (name) act(() => api('POST', '/api/files/mkdir', { path: join(path, name) }));
  };
  const rename = (e: Entry) => {
    const name = prompt('Rename to', e.name);
    if (name && name !== e.name) act(() => api('POST', '/api/files/move', { from: join(path, e.name), to: join(path, name) }));
  };
  const moveTo = (e: Entry, copy = false) => {
    const to = prompt(`${copy ? 'Copy' : 'Move'} to (full path)`, join(path, e.name));
    if (to && to !== join(path, e.name)) act(() => api('POST', copy ? '/api/files/copy' : '/api/files/move', { from: join(path, e.name), to }));
  };
  const remove = (e: Entry) => {
    if (confirm(`Delete "${e.name}"?`)) act(() => api('POST', '/api/files/delete', { paths: [join(path, e.name)] }));
  };
  const share = async (e: Entry) => {
    const password = prompt('Password for the link (leave empty for none)') ?? '';
    const days = prompt('Expires after how many days? (empty = never)') ?? '';
    try {
      const s = await api('POST', '/api/shares', {
        path: join(path, e.name),
        password: password || null,
        expires_at: days ? new Date(Date.now() + Number(days) * 86400000).toISOString() : null
      });
      const url = `${location.origin}/s/${s.token}`;
      try { await navigator.clipboard.writeText(url); } catch {}
      prompt('Share link (copied):', url);
    } catch (err) { alert((err as Error).message); }
    menu = null;
  };

  let uid = 0;
  function enqueue(files: Iterable<File>, base = path) {
    for (const f of files) {
      const rel = (f as File & { webkitRelativePath?: string }).webkitRelativePath || f.name;
      uploads.push({ id: ++uid, file: f, path: join(base, rel), status: 'queued', progress: 0, sent: 0, skipped: 0 });
    }
    pump();
  }
  let active = 0;
  function pump() {
    while (active < 2) {
      const t = uploads.find((u) => u.status === 'queued');
      if (!t) return;
      active++;
      t.status = 'hashing';
      runUpload(t, () => (uploads = uploads)).finally(() => {
        active--;
        if (parent(t.path) === path) load();
        pump();
      });
    }
  }

  function onDrop(e: DragEvent) {
    e.preventDefault();
    dragging = false;
    if (e.dataTransfer?.files.length) enqueue(e.dataTransfer.files);
  }

  let searchTimer: ReturnType<typeof setTimeout>;
  $effect(() => {
    const q = query.trim();
    clearTimeout(searchTimer);
    if (!q) { hits = null; return; }
    searchTimer = setTimeout(async () => {
      try { hits = (await api('GET', `/api/search?q=${encodeURIComponent(q)}`)).hits; } catch { hits = []; }
    }, 200);
  });
</script>

<svelte:window ondragover={(e) => { e.preventDefault(); dragging = true; }} ondragleave={(e) => { if (!e.relatedTarget) dragging = false; }} ondrop={onDrop} />

<div class="browser">
  <div class="toolbar">
    <div class="crumbs">
      <a href="/?p=">Home</a>
      {#each crumbs as c}<span>/</span><a href="/?p={encodeURIComponent(c.path)}">{c.name}</a>{/each}
      {#if mount && mount.path !== ''}<span class="badge" title="Mounted from {mount.connection_name}">{mount.mode} · {mount.connection_name}</span>{/if}
    </div>
    <div class="row actions">
      <input class="search" type="search" placeholder="Search by name" bind:value={query} />
      <button onclick={newFolder}>New folder</button>
      <button class="primary" onclick={() => fileInput.click()}>Upload</button>
      <button onclick={() => folderInput.click()} title="Upload a folder">Folder…</button>
    </div>
    <input bind:this={fileInput} type="file" multiple hidden onchange={(e) => { enqueue(e.currentTarget.files!); e.currentTarget.value = ''; }} />
    <input bind:this={folderInput} type="file" webkitdirectory hidden onchange={(e) => { enqueue(e.currentTarget.files!); e.currentTarget.value = ''; }} />
  </div>

  {#if hits}
    <div class="results">
      <div class="muted pad">{hits.length} results for “{query}”</div>
      {#each hits as h}
        <a class="entry" href={h.is_dir ? `/?p=${encodeURIComponent(h.path)}` : `/?p=${encodeURIComponent(parent(h.path))}`} onclick={() => (query = '')}>
          <span class="ico">{icon(h)}</span>
          <span class="name">{h.name}<small class="muted">{h.path}</small></span>
          <span class="meta">{h.is_dir ? '' : bytes(h.size)}</span>
        </a>
      {/each}
    </div>
  {:else if error}
    <div class="pad error">{error}</div>
  {:else}
    <div class="list">
      {#if !loading && entries.length === 0}
        <div class="empty muted">This folder is empty. Drop files here or press Upload.</div>
      {/if}
      <VirtualList items={entries} rowHeight={56}>
        {#snippet row(e: Entry)}
          <div class="entry" role="button" tabindex="0" onclick={() => open(e)} onkeydown={(k) => k.key === 'Enter' && open(e)}>
            <span class="ico">
              {#if !e.is_dir && isImage(e.name)}
                <img loading="lazy" src={thumbUrl(join(path, e.name), e.etag)} alt="" />
              {:else}{icon(e)}{/if}
            </span>
            <span class="name">{e.name}{#if e.mount_id}<small class="muted">mount</small>{/if}</span>
            <span class="meta">{e.is_dir ? '' : bytes(e.size)}</span>
            <span class="meta date">{when(e.mtime)}</span>
            <button class="more" aria-label="Actions" onclick={(ev) => { ev.stopPropagation(); menu = e; }}>⋯</button>
          </div>
        {/snippet}
      </VirtualList>
    </div>
  {/if}

  {#if uploads.length}
    <div class="uploads card">
      <div class="row"><b class="grow">Uploads</b><button onclick={() => (uploads = uploads.filter((u) => u.status !== 'done' && u.status !== 'error'))}>Clear</button></div>
      {#each uploads.slice(-6) as u (u.id)}
        <div class="up">
          <div class="row"><span class="grow trunc">{u.path}</span>
            <span class="muted">{u.status === 'error' ? '' : u.status === 'done' ? (u.skipped ? `done, ${bytes(u.skipped)} deduped` : 'done') : u.status}</span></div>
          {#if u.status === 'error'}<div class="error">{u.error}</div>{/if}
          <progress max="1" value={u.progress}></progress>
        </div>
      {/each}
    </div>
  {/if}

  {#if dragging}<div class="drop">Drop to upload to {path || 'Home'}</div>{/if}
</div>

{#if menu}
  {@const e = menu}
  <div class="sheet-bg" role="presentation" onclick={() => (menu = null)}>
    <div class="sheet card" role="menu" tabindex="-1" onclick={(ev) => ev.stopPropagation()} onkeydown={() => {}}>
      <b class="trunc">{e.name}</b>
      {#if !e.is_dir}<a class="item" href={contentUrl(join(path, e.name), true)}>Download</a>{/if}
      {#if !e.mount_id}
        {#if !e.is_dir && isTextName(e.name)}
          <button class="item" onclick={() => { editing = join(path, e.name); menu = null; }}>Edit</button>
        {/if}
        <button class="item" onclick={() => rename(e)}>Rename</button>
        <button class="item" onclick={() => moveTo(e)}>Move…</button>
      {/if}
      <button class="item" onclick={() => moveTo(e, true)}>Copy…</button>
      <button class="item" onclick={() => share(e)}>Share link…</button>
      {#if e.mount_id}
        <a class="item" href="/mounts">Manage mount…</a>
      {:else}
        <button class="item danger" onclick={() => remove(e)}>Delete</button>
      {/if}
    </div>
  </div>
{/if}

{#if viewerIndex >= 0 && Viewer}
  <Viewer
    items={viewable.map((v) => ({ name: v.name, size: v.size, src: previewUrl(join(path, v.name)), download: contentUrl(join(path, v.name), true) }))}
    bind:index={viewerIndex}
    onedit={(i) => { editing = join(path, viewable[i].name); viewerIndex = -1; }}
    onclose={() => (viewerIndex = -1)} />
{/if}

{#if editing && session.user}
  {#key editing}
    <Editor path={editing} userId={session.user.id} mountId={mount?.id ?? null} onclose={() => { editing = null; load(); }} />
  {/key}
{/if}

<style>
  .browser { display: flex; flex-direction: column; height: 100%; position: relative; }
  .toolbar { display: flex; flex-wrap: wrap; gap: 8px 16px; align-items: center; justify-content: space-between; padding: 10px 16px; border-bottom: 1px solid var(--border); }
  .crumbs { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; min-width: 0; }
  .crumbs a { color: var(--text); font-weight: 500; }
  .crumbs span { color: var(--muted); }
  .actions { flex-wrap: nowrap; }
  .search { width: 200px; }
  .list { flex: 1; min-height: 0; position: relative; }
  .results { flex: 1; overflow: auto; }
  .pad { padding: 16px; }
  .empty { position: absolute; inset: 40px 0 auto; text-align: center; }
  .entry { display: flex; align-items: center; gap: 12px; height: 56px; padding: 0 16px; border-bottom: 1px solid var(--border); cursor: pointer; color: var(--text); }
  .entry:hover { background: var(--hover); }
  .ico { width: 36px; height: 36px; display: grid; place-items: center; font-size: 22px; flex: none; }
  .ico img { width: 36px; height: 36px; object-fit: cover; border-radius: 6px; }
  .name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; display: flex; flex-direction: column; }
  .name small { font-size: .78em; }
  .meta { color: var(--muted); font-size: .88em; width: 80px; text-align: right; flex: none; }
  .more { border: 0; background: none; font-size: 20px; padding: 4px 10px; color: var(--muted); }
  .uploads { position: fixed; right: 16px; bottom: 16px; width: min(380px, calc(100vw - 32px)); max-height: 50vh; overflow: auto; display: grid; gap: 8px; z-index: 20; box-shadow: 0 10px 30px rgb(0 0 0 / .2); }
  .up progress { width: 100%; height: 6px; }
  .trunc { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
  .drop { position: fixed; inset: 0; background: rgb(14 165 233 / .15); border: 3px dashed var(--accent); display: grid; place-items: center; font-size: 1.3em; z-index: 40; pointer-events: none; }
  .sheet-bg { position: fixed; inset: 0; background: rgb(0 0 0 / .35); z-index: 30; display: grid; place-items: center; }
  .sheet { display: grid; gap: 2px; width: min(320px, calc(100vw - 32px)); padding: 12px; }
  .sheet b { padding: 6px 10px 10px; }
  .item { text-align: left; border: 0; background: none; padding: 10px; border-radius: 8px; color: var(--text); display: block; }
  .item:hover { background: var(--hover); }
  .item.danger { color: var(--danger); }
  @media (max-width: 700px) {
    .date { display: none; }
    .actions { width: 100%; }
    .search { flex: 1; width: auto; }
    .sheet-bg { align-items: end; }
    .sheet { width: 100%; border-radius: 16px 16px 0 0; padding-bottom: max(12px, env(safe-area-inset-bottom)); }
  }
</style>
