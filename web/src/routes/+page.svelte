<script lang="ts">
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { untrack } from 'svelte';
  import { api, contentUrl, previewUrl, thumbUrl, type Entry } from '$lib/api';
  import { bytes, when, join, parent, isImage, isTextName } from '$lib/format';
  import { cn } from '$lib/utils';
  import FileIcon from '$lib/components/files/FileIcon.svelte';
  import { Selection, loadPref, nextSort, savePref, sortEntries, type Sort, type SortKey } from '$lib/components/files/listing';
  import { buttonClass } from '$lib/components/ui/button/button.svelte';
  import Button from '$lib/components/ui/button/button.svelte';
  import ArrowUpIcon from '@lucide/svelte/icons/arrow-up';
  import ArrowDownIcon from '@lucide/svelte/icons/arrow-down';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import LayoutGridIcon from '@lucide/svelte/icons/layout-grid';
  import ListIcon from '@lucide/svelte/icons/list';
  import UploadIcon from '@lucide/svelte/icons/upload';
  import FolderUpIcon from '@lucide/svelte/icons/folder-up';
  import FolderPlusIcon from '@lucide/svelte/icons/folder-plus';
  import FolderInputIcon from '@lucide/svelte/icons/folder-input';
  import CopyIcon from '@lucide/svelte/icons/copy';
  import TrashIcon from '@lucide/svelte/icons/trash';
  import SearchIcon from '@lucide/svelte/icons/search';
  import XIcon from '@lucide/svelte/icons/x';
  import InfoIcon from '@lucide/svelte/icons/info';
  import { fileHref } from '$lib/components/shell/nav';
  import { runUpload, type UploadTask } from '$lib/upload';
  import { reloadMounts } from '$lib/mounts.svelte';
  import VirtualList from '$lib/VirtualList.svelte';
  import Editor from '$lib/editor/Editor.svelte';
  import { staleApp } from '$lib/lazy';
  import { session } from '$lib/session.svelte';
  import { alertDialog, confirmDialog, formDialog, promptDialog } from '$lib/dialogs.svelte';
  import { toast, toastError } from '$lib/toast.svelte';

  let path = $derived(page.url.searchParams.get('p') ?? '');
  let entries = $state<Entry[]>([]);
  // The folder `entries` belong to. It lags `path` until a new listing
  // arrives, and every action on an entry joins with it, never with `path`.
  let listedPath = $state('');
  let mount = $state<{ id: number; mode: string; connection_name: string; path: string; offline?: boolean } | null>(null);
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
  // The details panel, by entry name in the listed folder; loads on first use.
  let details = $state<string | null>(null);
  let Details = $state<typeof import('$lib/components/files/DetailsPanel.svelte').default>();
  $effect(() => {
    if (details !== null && !Details) import('$lib/components/files/DetailsPanel.svelte').then((m) => (Details = m.default), staleApp);
  });
  // ?f=name opens a file's details once its folder lists (links from ⌘K).
  let appliedF = '';
  let dragging = $state(false);
  let fileInput: HTMLInputElement;
  let folderInput: HTMLInputElement;
  let seq = 0;

  // Sorting, view mode and selection. Preferences stay in this browser.
  const SORT_KEYS = ['name', 'size', 'mtime'] as const;
  let sort = $state<Sort>({
    key: loadPref<SortKey>('strata-sort-key', SORT_KEYS, 'name'),
    desc: loadPref('strata-sort-desc', ['0', '1'] as const, '0') === '1'
  });
  let view = $state(loadPref('strata-view', ['list', 'grid'] as const, 'list'));
  function setSort(s: Sort) {
    sort = { ...s };
    savePref('strata-sort-key', sort.key);
    savePref('strata-sort-desc', sort.desc ? '1' : '0');
  }
  const sortBy = (key: SortKey) => setSort(nextSort(sort, key));
  function setView(v: 'list' | 'grid') {
    view = v;
    savePref('strata-view', v);
  }
  const sorted = $derived(sortEntries(entries, sort));
  const order = $derived(sorted.map((e) => e.name));
  const selection = new Selection();
  // A new folder starts with nothing selected, both when asked for and when
  // its listing replaces the old one (a pick made on the old rows must not
  // carry over to a same-named file). A refresh keeps what's still there.
  $effect(() => {
    path;
    listedPath;
    selection.clear();
  });
  $effect(() => {
    const o = order;
    untrack(() => selection.prune(o));
  });
  // While a bulk action runs, the selection bar and Delete key wait.
  let busy = $state(false);
  const byName = $derived(new Map(entries.map((e) => [e.name, e])));
  // From the selection, not a scan of the folder: cheap in big folders.
  const picked = $derived([...selection.names].flatMap((n) => byName.get(n) ?? []));
  const pickedMounts = $derived(picked.filter((e) => e.mount_id).length);
  const detailsEntry = $derived(details === null ? undefined : byName.get(details));
  // Gone after a refresh (deleted, renamed elsewhere): close.
  $effect(() => {
    if (details !== null && !loading && !detailsEntry) details = null;
  });
  const showDetails = (e: Entry) => {
    menu = null;
    details = e.name;
  };

  // Grid: as many columns as fit, rendered as virtual rows.
  let gridWidth = $state(0);
  const cols = $derived(Math.max(2, Math.floor((gridWidth - 16) / 172)));
  const gridRows = $derived(Array.from({ length: Math.ceil(sorted.length / cols) }, (_, i) => sorted.slice(i * cols, i * cols + cols)));

  /** Row/card clicks: modifier keys select, and while anything is selected
   *  a plain click selects too (no keyboard modifiers on phones). Enter on
   *  a name (a click with detail 0) always opens. */
  function activate(e: Entry, ev: MouseEvent) {
    if (ev.shiftKey) selection.range(e.name, order);
    else if (ev.ctrlKey || ev.metaKey || (selection.size && ev.detail !== 0)) selection.toggle(e.name);
    else open(e);
  }
  function check(e: Entry, ev: MouseEvent) {
    ev.stopPropagation();
    const box = ev.currentTarget as HTMLInputElement;
    if (ev.shiftKey) selection.range(e.name, order);
    else selection.toggle(e.name);
    // The box flipped itself; the range may say otherwise.
    box.checked = selection.has(e.name);
  }

  // Phones have no column headers to click, so one menu holds every order.
  const SORT_CHOICES: [string, Sort][] = [
    ['Name, A to Z', { key: 'name', desc: false }],
    ['Name, Z to A', { key: 'name', desc: true }],
    ['Newest first', { key: 'mtime', desc: true }],
    ['Oldest first', { key: 'mtime', desc: false }],
    ['Largest first', { key: 'size', desc: true }],
    ['Smallest first', { key: 'size', desc: false }]
  ];
  const pickSort = (i: number) => setSort(SORT_CHOICES[i][1]);

  function onKeys(ev: KeyboardEvent) {
    // Focus stays on the ⋯ button, outside the menu, so Escape lands here.
    if (menu && ev.key === 'Escape') return void (menu = null);
    if (viewerIndex >= 0 || editing || menu || hits) return;
    if (ev.key === 'Escape' && !selection.size && details !== null) return void (details = null);
    const t = ev.target as HTMLElement | null;
    if (t?.closest('input:not([type=checkbox]), textarea, select, [contenteditable]')) return;
    if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === 'a') {
      ev.preventDefault();
      selection.toggleAll(order);
    } else if (ev.key === 'Escape' && selection.size) selection.clear();
    // Delete, or Cmd+Backspace on a Mac (its delete key is Backspace).
    else if ((ev.key === 'Delete' || (ev.key === 'Backspace' && ev.metaKey)) && selection.size && !busy) {
      ev.preventDefault();
      void removePicked();
    }
  }

  async function load(p = path) {
    const my = ++seq;
    loading = true;
    error = '';
    try {
      const r = await api('GET', `/api/files/list?path=${encodeURIComponent(p)}`);
      if (my !== seq) return;
      entries = r.entries;
      if (listedPath !== p) details = null;
      listedPath = p;
      const f = page.url.searchParams.get('f');
      if (f && appliedF !== page.url.href && (page.url.searchParams.get('p') ?? '') === p) {
        appliedF = page.url.href;
        if (r.entries.some((e: Entry) => e.name === f)) details = f;
      }
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
        // Renames and moves can shift mount paths too, so the sidebar refreshes.
        // A bulk action refreshes once at its end.
        timer = setTimeout(() => { if (!busy) { load(); reloadMounts(); } }, 300);
      };
      ws.onclose = () => { if (!closed) setTimeout(connect, 3000); };
    };
    connect();
    return () => { closed = true; ws?.close(); };
  });

  const crumbs = $derived(path ? path.split('/').map((n, i, a) => ({ name: n, path: a.slice(0, i + 1).join('/') })) : []);
  const viewable = $derived(sorted.filter((e) => !e.is_dir));
  let viewerItems = $state<{ name: string; size: number; src: string; download: string; path: string }[]>([]);

  function open(e: Entry) {
    if (e.is_dir) goto(`/?p=${encodeURIComponent(join(listedPath, e.name))}`);
    else {
      // A snapshot: a refresh (an upload landing, another device) must not
      // shift which file the open viewer shows.
      viewerItems = viewable.map((v) => ({ name: v.name, size: v.size, src: previewUrl(join(listedPath, v.name)), download: contentUrl(join(listedPath, v.name), true), path: join(listedPath, v.name) }));
      viewerIndex = viewable.indexOf(e);
    }
  }

  async function act(fn: () => Promise<unknown>) {
    try { await fn(); } catch (e) { toastError(e); }
    menu = null;
    await load();
    reloadMounts();
  }

  // Each action fixes its target before asking: the folder may change
  // (Back, a sidebar click) while the dialog is open.
  const newFolder = async () => {
    const dir = path;
    const name = await promptDialog({ title: 'New folder', label: 'Name', confirm: 'Create' });
    if (name) act(() => api('POST', '/api/files/mkdir', { path: join(dir, name) }));
  };
  const rename = async (e: Entry) => {
    menu = null;
    const dir = listedPath;
    const name = await promptDialog({ title: `Rename “${e.name}”`, label: 'New name', value: e.name, confirm: 'Rename' });
    if (name && name !== e.name) act(() => api('POST', '/api/files/move', { from: join(dir, e.name), to: join(dir, name) }));
  };
  const moveTo = async (e: Entry, copy = false) => {
    menu = null;
    const from = join(listedPath, e.name);
    const to = await promptDialog({
      title: `${copy ? 'Copy' : 'Move'} “${e.name}”`,
      label: 'To (full path)',
      value: from,
      confirm: copy ? 'Copy' : 'Move'
    });
    if (to && to !== from) act(() => api('POST', copy ? '/api/files/copy' : '/api/files/move', { from, to }));
  };
  const remove = (e: Entry) => {
    menu = null;
    void removeItems([e], false);
  };
  const share = async (e: Entry) => {
    menu = null;
    const target = join(listedPath, e.name);
    const f = await formDialog({
      title: `Share “${e.name}”`,
      body: 'Anyone with the link can open it.',
      confirm: 'Create link',
      fields: [
        { name: 'password', label: 'Password', type: 'password', hint: 'Optional. Leave empty for none.' },
        { name: 'days', label: 'Expires after (days)', type: 'number', min: 1, hint: 'Optional. Leave empty to never expire.' }
      ]
    });
    if (!f) return;
    try {
      const s = await api('POST', '/api/shares', {
        path: target,
        password: f.password || null,
        // The field allows only whole days from 1 up.
        expires_at: f.days ? new Date(Date.now() + Number(f.days) * 86400000).toISOString() : null
      });
      const url = `${location.origin}/s/${s.token}`;
      let copied = false;
      try { await navigator.clipboard.writeText(url); copied = true; } catch {}
      await alertDialog({ title: 'Share link', body: copied ? 'Copied to the clipboard.' : undefined, copy: url, ok: 'Done' });
      return s;
    } catch (err) { toastError(err); }
  };

  const names = (items: Entry[]) => items.length === 1 ? `“${items[0].name}”` : `${items.length} items`;

  /** Runs `fn` for each item in turn; one failure doesn't stop the rest.
   *  Says what failed, then refreshes. */
  async function each(items: Entry[], verb: string, fn: (e: Entry) => Promise<unknown>, fromSelection = true) {
    const dir = listedPath;
    const failed: string[] = [];
    let lastError = '';
    busy = true;
    try {
      for (const e of items) {
        try { await fn(e); } catch (err) { failed.push(e.name); lastError = err instanceof Error ? err.message : String(err); }
      }
    } finally {
      busy = false;
    }
    if (failed.length === 1) toast(`Couldn't ${verb} “${failed[0]}”: ${lastError}`, 'error');
    else if (failed.length) toast(`Couldn't ${verb} ${failed.length} items (${failed.slice(0, 3).join(', ')}${failed.length > 3 ? ', …' : ''}). ${lastError}`, 'error');
    else toast(`${names(items)} ${verb === 'delete' ? 'deleted' : verb === 'move' ? 'moved' : 'copied'}.`, 'success');
    await load();
    reloadMounts();
    // What failed stays selected for a retry, if the user is still here.
    // (A row-menu action leaves the selection alone.)
    if (fromSelection && listedPath === dir) {
      selection.clear();
      for (const n of failed) if (byName.has(n)) selection.toggle(n);
    }
  }

  /** Mounts are moved and removed on the Mounts page, never as files. */
  function withoutMounts(items: Entry[], verb: string) {
    const kept = items.filter((e) => !e.mount_id);
    const skipped = items.length - kept.length;
    if (skipped) toast(`${skipped === 1 ? 'A mount is' : `${skipped} mounts are`} skipped: ${verb} ${skipped === 1 ? 'it' : 'them'} on the Mounts page.`, 'info', 6000);
    return kept;
  }

  const removePicked = () => removeItems(picked);

  const waitForBusy = () => toast('Wait for the current action to finish.', 'info');

  async function removeItems(all: Entry[], fromSelection = true) {
    if (busy) return waitForBusy();
    const items = withoutMounts(all, 'remove');
    if (!items.length) return;
    const dir = listedPath;
    const ok = await confirmDialog({
      title: `Delete ${names(items)}?`,
      body: items.length === 1
        ? (items[0].is_dir ? 'The folder and everything in it will be deleted.' : 'The file will be deleted.')
        : `${items.slice(0, 5).map((e) => e.name).join(', ')}${items.length > 5 ? `, and ${items.length - 5} more` : ''}. Folders are deleted with everything in them.`,
      confirm: 'Delete',
      danger: true
    });
    if (!ok) return;
    if (busy) return waitForBusy();
    await each(items, 'delete', (e) => api('POST', '/api/files/delete', { paths: [join(dir, e.name)] }), fromSelection);
  }

  async function transferPicked(copy: boolean) {
    if (busy) return waitForBusy();
    const items = copy ? picked : withoutMounts(picked, 'move');
    if (!items.length) return;
    const dir = listedPath;
    const dest = await promptDialog({
      title: `${copy ? 'Copy' : 'Move'} ${names(items)}`,
      label: 'To folder (full path, empty for Home)',
      value: dir,
      required: false,
      confirm: copy ? 'Copy' : 'Move'
    });
    if (dest === null) return;
    // Outer spaces and slashes are never meant: ' docs/ ' is docs.
    const to = dest.trim().replace(/^\/+|\/+$/g, '');
    // Same folder: a move does nothing and a copy would collide with itself.
    if (to === dir) return void toast('Pick a different folder.', 'info');
    if (busy) return waitForBusy();
    await each(items, copy ? 'copy' : 'move', (e) =>
      api('POST', copy ? '/api/files/copy' : '/api/files/move', { from: join(dir, e.name), to: join(to, e.name) })
    );
  }

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

<svelte:window
  onkeydown={onKeys}
  ondragover={(e) => { e.preventDefault(); dragging = true; }}
  ondragleave={(e) => { if (!e.relatedTarget) dragging = false; }}
  ondrop={onDrop} />

{#snippet sortHead(key: SortKey, label: string, cls: string)}
  <div role="columnheader" class={cls} aria-sort={sort.key === key ? (sort.desc ? 'descending' : 'ascending') : 'none'}>
    <button type="button" class="inline-flex items-center gap-1 rounded-sm hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring" onclick={() => sortBy(key)}>
      {label}
      {#if sort.key === key}
        {#if sort.desc}<ArrowDownIcon class="size-3.5" />{:else}<ArrowUpIcon class="size-3.5" />{/if}
      {/if}
    </button>
  </div>
{/snippet}

{#snippet selectAll()}
  <input type="checkbox" class="size-4 cursor-pointer accent-primary" aria-label="Select all"
            checked={sorted.length > 0 && selection.size === sorted.length}
            indeterminate={selection.size > 0 && selection.size < sorted.length}
            disabled={!sorted.length}
            onclick={(ev) => {
              selection.toggleAll(order);
              ev.currentTarget.checked = selection.size === sorted.length;
              ev.currentTarget.indeterminate = false;
            }} />
{/snippet}

{#snippet sortMenu(extra = '')}
  <select aria-label="Sort by" class={cn('h-7 rounded-md border border-input bg-transparent px-1.5 text-xs text-foreground', extra)}
    value={SORT_CHOICES.findIndex(([, c]) => c.key === sort.key && c.desc === sort.desc)}
    onchange={(ev) => pickSort(Number(ev.currentTarget.value))}>
    {#each SORT_CHOICES as [label], i (label)}<option value={i}>{label}</option>{/each}
  </select>
{/snippet}

{#snippet checkbox(e: Entry, extra = '')}
  <input
    type="checkbox"
    class={cn('size-4 shrink-0 cursor-pointer accent-primary', extra)}
    checked={selection.has(e.name)}
    aria-label="Select {e.name}"
    onclick={(ev) => check(e, ev)} />
{/snippet}

{#snippet moreButton(e: Entry, extra = '')}
  <button
    type="button"
    class={cn(buttonClass('ghost', 'icon'), 'size-8 text-muted-foreground', extra)}
    aria-label="Actions for {e.name}"
    onclick={(ev) => { ev.stopPropagation(); menu = e; }}>
    <EllipsisIcon />
  </button>
{/snippet}

<div class="flex h-full">
<div class="relative flex min-w-0 flex-1 flex-col text-[15px]">
  <header class="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 border-b border-border px-4 py-2.5">
    <nav aria-label="Folder path" class="flex min-w-0 flex-wrap items-center gap-1 text-sm">
      <a href="/?p=" class="rounded-md px-1.5 py-1 font-medium text-foreground hover:bg-accent">Home</a>
      {#each crumbs as c, i (c.path)}
        <ChevronRightIcon class="size-4 text-muted-foreground" aria-hidden="true" />
        <a href="/?p={encodeURIComponent(c.path)}" class="truncate rounded-md px-1.5 py-1 font-medium text-foreground hover:bg-accent" aria-current={i === crumbs.length - 1 ? 'page' : undefined}>{c.name}</a>
      {/each}
      {#if mount && mount.path !== ''}
        <span class="ml-1 rounded-full border border-border px-2 py-0.5 text-xs text-muted-foreground" title="Mounted from {mount.connection_name}">{mount.mode} · {mount.connection_name}{mount.offline ? ' · offline' : ''}</span>
      {/if}
    </nav>
    <div class="flex w-full items-center gap-2 sm:w-auto">
      <label class="relative min-w-0 flex-1 sm:w-52 sm:flex-none">
        <span class="sr-only">Search by name</span>
        <SearchIcon class="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
        <input type="search" placeholder="Search by name" bind:value={query}
          class="h-9 w-full rounded-md border border-input bg-transparent pr-3 pl-8 text-sm outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50" />
      </label>
      <div class="flex rounded-md border border-border p-0.5" role="group" aria-label="View">
        <button type="button" class={cn(buttonClass('ghost', 'icon'), 'size-7', view === 'list' && 'bg-accent text-foreground')} aria-pressed={view === 'list'} aria-label="List view" onclick={() => setView('list')}><ListIcon /></button>
        <button type="button" class={cn(buttonClass('ghost', 'icon'), 'size-7', view === 'grid' && 'bg-accent text-foreground')} aria-pressed={view === 'grid'} aria-label="Grid view" onclick={() => setView('grid')}><LayoutGridIcon /></button>
      </div>
      <Button variant="outline" size="icon" aria-label="New folder" title="New folder" onclick={newFolder}><FolderPlusIcon /></Button>
      <Button variant="outline" size="icon" aria-label="Upload a folder" title="Upload a folder" onclick={() => folderInput.click()}><FolderUpIcon /></Button>
      <Button onclick={() => fileInput.click()}><UploadIcon /><span class="max-sm:sr-only">Upload</span></Button>
    </div>
    <input bind:this={fileInput} type="file" multiple hidden onchange={(e) => { enqueue(e.currentTarget.files!); e.currentTarget.value = ''; }} />
    <input bind:this={folderInput} type="file" webkitdirectory hidden onchange={(e) => { enqueue(e.currentTarget.files!); e.currentTarget.value = ''; }} />
  </header>
  {#if mount?.offline}
    <p class="border-b border-border px-4 py-2 text-sm text-muted-foreground" role="status">This storage is offline. Its files can't be opened or changed until it's back.</p>
  {/if}

  {#if selection.size && !hits}
    <div class="flex items-center gap-2 border-b border-border bg-accent/60 px-4 py-1.5 text-sm" role="toolbar" aria-label="Selected items">
      <Button variant="ghost" size="icon" class="size-8" aria-label="Clear selection" onclick={() => selection.clear()}><XIcon /></Button>
      <span class="mr-auto font-medium" aria-live="polite">{selection.size} selected</span>
      {#if picked.length === 1}
        <Button variant="ghost" size="sm" onclick={() => showDetails(picked[0])}><InfoIcon /><span class="max-sm:sr-only">Details</span></Button>
      {/if}
      <Button variant="ghost" size="sm" onclick={() => transferPicked(false)} disabled={busy || pickedMounts === picked.length} title={pickedMounts ? 'Mounts are moved on the Mounts page' : undefined}><FolderInputIcon /><span class="max-sm:sr-only">Move</span></Button>
      <Button variant="ghost" size="sm" onclick={() => transferPicked(true)} disabled={busy}><CopyIcon /><span class="max-sm:sr-only">Copy</span></Button>
      <Button variant="ghost" size="sm" class="text-destructive hover:text-destructive" onclick={removePicked} disabled={busy || pickedMounts === picked.length} title={pickedMounts ? 'Mounts are removed on the Mounts page' : undefined}><TrashIcon /><span class="max-sm:sr-only">Delete</span></Button>
    </div>
  {/if}

  {#if hits}
    <div class="flex-1 overflow-auto">
      <p class="px-4 py-3 text-sm text-muted-foreground">{hits.length} results for “{query}”</p>
      {#each hits as h (h.path)}
        <a class="flex h-14 items-center gap-3 border-b border-border px-4 text-foreground hover:bg-accent"
          href={h.is_dir ? `/?p=${encodeURIComponent(h.path)}` : fileHref(h.path)} onclick={() => (query = '')}>
          <FileIcon entry={h} class="size-6" />
          <span class="flex min-w-0 flex-1 flex-col"><span class="truncate">{h.name}</span><small class="truncate text-xs text-muted-foreground">{h.path}</small></span>
          <span class="text-sm text-muted-foreground">{h.is_dir ? '' : bytes(h.size)}</span>
        </a>
      {/each}
    </div>
  {:else if error}
    <p class="p-4 text-destructive">{error}</p>
  {:else if view === 'list'}
    <div class="flex min-h-0 flex-1 flex-col" role="table" aria-label="Files" aria-rowcount={sorted.length + 1}>
      <div role="row" aria-rowindex={1} class="flex h-9 shrink-0 items-center gap-3 border-b border-border px-4 text-xs font-medium text-muted-foreground">
        <div role="columnheader" class="flex w-4 justify-center">{@render selectAll()}</div>
        <div class="w-7" aria-hidden="true"></div>
        {@render sortHead('name', 'Name', 'flex-1')}
        {@render sortHead('size', 'Size', 'hidden w-20 justify-end text-right sm:flex')}
        {@render sortHead('mtime', 'Modified', 'hidden w-28 justify-end text-right md:flex')}
        {@render sortMenu('sm:hidden')}
        <div class="w-8 max-sm:hidden" aria-hidden="true"></div>
      </div>
      <div class="relative min-h-0 flex-1">
        {#if !loading && entries.length === 0}
          <p class="absolute inset-x-0 top-10 text-center text-muted-foreground">This folder is empty. Drop files here or press Upload.</p>
        {/if}
        <VirtualList items={sorted} rowHeight={52}>
          {#snippet row(e: Entry, i: number)}
            {@const on = selection.has(e.name)}
            <!-- Mouse convenience; keyboard users have the checkbox and name button. -->
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <div role="row" tabindex="-1" aria-rowindex={i + 2} aria-selected={on}
              class={cn('group flex h-[52px] cursor-default items-center gap-3 border-b border-border px-4 select-none hover:bg-accent/70', on && 'bg-primary/10 hover:bg-primary/15', e.offline && 'opacity-50')}
              onclick={(ev) => activate(e, ev)}>
              <div role="cell" class="flex w-4 justify-center">{@render checkbox(e)}</div>
              <div role="cell" class="grid size-7 place-items-center">
                {#if !e.is_dir && isImage(e.name)}
                  <img loading="lazy" src={thumbUrl(join(listedPath, e.name), e.etag)} alt="" class="size-7 rounded object-cover" />
                {:else}<FileIcon entry={e} class="size-6" />{/if}
              </div>
              <div role="cell" class="flex min-w-0 flex-1 flex-col">
                <button type="button" class="truncate rounded-sm text-left text-foreground hover:underline focus-visible:outline-2 focus-visible:outline-ring"
                  onclick={(ev) => { ev.stopPropagation(); activate(e, ev); }}>{e.name}</button>
                <span class="truncate text-xs text-muted-foreground sm:hidden">
                  {e.mount_id ? (e.offline ? 'Mount · offline' : 'Mount') : e.is_dir ? 'Folder' : bytes(e.size)} · {when(e.mtime)}
                </span>
                {#if e.mount_id}<span class="hidden text-xs text-muted-foreground sm:block">{e.offline ? 'Mount · offline' : 'Mount'}</span>{/if}
              </div>
              <div role="cell" class="hidden w-20 text-right text-sm text-muted-foreground tabular-nums sm:block">{e.is_dir ? '' : bytes(e.size)}</div>
              <div role="cell" class="hidden w-28 text-right text-sm text-muted-foreground md:block">{when(e.mtime)}</div>
              <div role="cell">{@render moreButton(e)}</div>
            </div>
          {/snippet}
        </VirtualList>
      </div>
    </div>
  {:else}
    <div class="flex h-9 shrink-0 items-center gap-3 border-b border-border px-4 text-xs font-medium text-muted-foreground">
      {@render selectAll()}
      <span class="mr-auto">{sorted.length} items</span>
      {@render sortMenu()}
    </div>
    <div class="relative min-h-0 flex-1" bind:clientWidth={gridWidth}>
      {#if !loading && entries.length === 0}
        <p class="absolute inset-x-0 top-10 text-center text-muted-foreground">This folder is empty. Drop files here or press Upload.</p>
      {/if}
      <VirtualList items={gridRows} rowHeight={196}>
        {#snippet row(items: Entry[])}
          <div class="grid gap-3 px-4 pt-3" style:grid-template-columns="repeat({cols}, minmax(0, 1fr))">
            {#each items as e (e.name)}
              {@const on = selection.has(e.name)}
              <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
              <div class={cn('group relative flex h-[184px] flex-col overflow-hidden rounded-lg border border-border bg-card select-none hover:border-ring/60', on && 'border-primary ring-2 ring-primary/40')}
                onclick={(ev) => activate(e, ev)}>
                <div class="grid min-h-0 flex-1 place-items-center bg-muted/60">
                  {#if !e.is_dir && isImage(e.name)}
                    <img loading="lazy" src={thumbUrl(join(listedPath, e.name), e.etag)} alt="" class="size-full object-cover" />
                  {:else}<FileIcon entry={e} class="size-14" />{/if}
                </div>
                <div class="flex items-center gap-1 px-2.5 py-2">
                  <div class="flex min-w-0 flex-1 flex-col">
                    <button type="button" class="truncate rounded-sm text-left text-sm font-medium text-foreground focus-visible:outline-2 focus-visible:outline-ring"
                      onclick={(ev) => { ev.stopPropagation(); activate(e, ev); }}>{e.name}</button>
                    <span class="truncate text-xs text-muted-foreground">{e.mount_id ? (e.offline ? 'Mount · offline' : 'Mount') : e.is_dir ? 'Folder' : bytes(e.size)}</span>
                  </div>
                  {@render moreButton(e, '-mr-1')}
                </div>
                {@render checkbox(e, cn('absolute top-2 left-2 size-5 rounded opacity-0 group-hover:opacity-100 focus-visible:opacity-100 [@media(hover:none)]:opacity-100', on && 'opacity-100'))}
              </div>
            {/each}
          </div>
        {/snippet}
      </VirtualList>
    </div>
  {/if}

  {#if uploads.length}
    <div class="uploads fixed right-4 z-20 grid max-h-[50vh] w-[min(380px,calc(100vw-2rem))] gap-2 overflow-auto rounded-xl border border-border bg-popover p-3 shadow-lg">
      <div class="flex items-center"><b class="flex-1 text-sm">Uploads</b>
        <Button variant="ghost" size="sm" onclick={() => (uploads = uploads.filter((u) => u.status !== 'done' && u.status !== 'error'))}>Clear</Button></div>
      {#each uploads.slice(-6) as u (u.id)}
        <div class="grid gap-1 text-sm">
          <div class="flex gap-2"><span class="min-w-0 flex-1 truncate">{u.path}</span>
            <span class="text-muted-foreground">{u.status === 'error' ? '' : u.status === 'done' ? (u.skipped ? `done, ${bytes(u.skipped)} deduped` : 'done') : u.status}</span></div>
          {#if u.status === 'error'}<div class="text-destructive">{u.error}</div>{/if}
          <progress max="1" value={u.progress} class="h-1.5 w-full accent-primary"></progress>
        </div>
      {/each}
    </div>
  {/if}

  {#if dragging}
    <div class="pointer-events-none fixed inset-0 z-40 grid place-items-center border-4 border-dashed border-primary bg-primary/15 text-xl font-medium">Drop to upload to {path || 'Home'}</div>
  {/if}
</div>

{#if detailsEntry && Details}
  <!-- Beside the list on wide screens, over it on phones. -->
  <div class="w-80 shrink-0 border-l border-border max-md:fixed max-md:inset-0 max-md:z-30 max-md:w-full max-md:border-0 max-md:pt-[env(safe-area-inset-top)] max-md:pb-[env(safe-area-inset-bottom)] lg:w-96">
    <Details
      entry={detailsEntry}
      path={join(listedPath, detailsEntry.name)}
      storage={mount ? `${mount.mode} · ${mount.connection_name}` : undefined}
      onclose={() => (details = null)}
      onshare={() => share(detailsEntry)} />
  </div>
{/if}
</div>

{#if menu}
  {@const e = menu}
  <div class="fixed inset-0 z-30 grid place-items-center bg-black/35 max-sm:items-end" role="presentation" onclick={() => (menu = null)}>
    <div role="menu" tabindex="-1" aria-label="Actions for {e.name}"
      class="grid w-[min(320px,calc(100vw-2rem))] gap-0.5 rounded-xl border border-border bg-popover p-2 text-popover-foreground shadow-lg max-sm:w-full max-sm:rounded-b-none max-sm:pb-[max(0.75rem,env(safe-area-inset-bottom))]"
      onclick={(ev) => ev.stopPropagation()} onkeydown={(ev) => { if (ev.key === 'Escape') menu = null; }}>
      <div class="flex items-center gap-2 px-2.5 pt-1.5 pb-2.5"><FileIcon entry={e} class="size-5" /><b class="truncate text-sm">{e.name}</b></div>
      <button role="menuitem" class="item" onclick={() => showDetails(e)}>Details</button>
      {#if !e.is_dir}<a role="menuitem" class="item" href={contentUrl(join(listedPath, e.name), true)}>Download</a>{/if}
      {#if !e.mount_id}
        {#if !e.is_dir && isTextName(e.name)}
          <button role="menuitem" class="item" onclick={() => { editing = join(listedPath, e.name); menu = null; }}>Edit</button>
        {/if}
        <button role="menuitem" class="item" onclick={() => rename(e)}>Rename</button>
        <button role="menuitem" class="item" onclick={() => moveTo(e)}>Move…</button>
      {/if}
      <button role="menuitem" class="item" onclick={() => moveTo(e, true)}>Copy…</button>
      <button role="menuitem" class="item" onclick={() => share(e)}>Share link…</button>
      {#if e.mount_id}
        <a role="menuitem" class="item" href="/mounts">Manage mount…</a>
      {:else}
        <button role="menuitem" class="item danger" onclick={() => remove(e)}>Delete</button>
      {/if}
    </div>
  </div>
{/if}

<!-- The viewer and editor aren't rebuilt yet: they keep the old styles. -->
{#if viewerIndex >= 0 && Viewer}
  <div class="legacy contents">
  <Viewer
    items={viewerItems}
    bind:index={viewerIndex}
    onedit={(i) => { editing = viewerItems[i].path; viewerIndex = -1; }}
    onclose={() => (viewerIndex = -1)} />
  </div>
{/if}

{#if editing && session.user}
  <div class="legacy contents">
  {#key editing}
    <Editor path={editing} userId={session.user.id} mountId={mount?.id ?? null} onclose={() => { editing = null; load(); }} />
  {/key}
  </div>
{/if}

<style>
  /* Above the tab bar on phones, clear of the corner on wider screens. */
  .uploads { bottom: 1rem; }
  @media (width < 48rem) {
    .uploads { bottom: calc(72px + env(safe-area-inset-bottom)); }
  }
  .item {
    display: block; width: 100%; padding: 0.625rem; border-radius: calc(var(--radius) - 4px);
    text-align: left; font-size: 0.875rem; color: inherit;
  }
  /* Scoped styles beat utilities, so the colour lives here. */
  .item.danger { color: var(--destructive); }
  .item:hover, .item:focus-visible { background: var(--hover); outline: none; }
</style>
