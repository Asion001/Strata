<script lang="ts">
  // The folder browser for Move and Copy, shown by DialogHost for a
  // pickFolder() request. Folders only; what can't be picked says why.
  import { tick } from 'svelte';
  import { api, enc, type Entry, type Schemas } from '$lib/api';
  import { join } from '$lib/format';
  import type { FolderOpts } from '$lib/dialogs.svelte';
  import Button from '$lib/components/ui/button/button.svelte';
  import Input from '$lib/components/ui/input/input.svelte';
  import FileIcon from './FileIcon.svelte';
  import { canOpen, clean, crumbs, recent, remember, whyNot } from './folder-picker';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import FolderPlusIcon from '@lucide/svelte/icons/folder-plus';
  import HistoryIcon from '@lucide/svelte/icons/history';

  let { opts, done, titleId }: { opts: FolderOpts; done: (v: string | null) => void; titleId: string } = $props();

  let path = $state('');
  // The folder the rows are from (path, once its listing is in).
  let listed = $state('');
  let folders = $state<Entry[]>([]);
  let mount = $state<Schemas['ListingMount'] | null>(null);
  let loading = $state(true);
  let error = $state('');
  let filter = $state('');
  // The name of the folder being created, while its field shows.
  let naming = $state<string | null>(null);
  let creating = $state(false);
  let createError = $state('');
  let root = $state<HTMLElement | null>(null);
  let filterEl = $state<HTMLInputElement | null>(null);

  // Picked before, and still allowed here (computed once, at open).
  const recents = recent().filter((p) => !whyNot(p, opts));
  const byName = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });

  let shown = $derived(filter ? folders.filter((f) => f.name.toLowerCase().includes(filter.trim().toLowerCase())) : folders);
  let why = $derived(error ? error : mount?.offline ? 'This storage is offline.' : whyNot(path, opts));
  const rowWhy = (e: Entry) => (e.offline ? 'This storage is offline.' : canOpen(join(listed, e.name), opts) ? null : whyNot(join(listed, e.name), opts));

  // Each listing supersedes the last: a slow answer for a folder already
  // left never shows.
  let seq = 0;
  async function open(p: string) {
    const n = ++seq;
    const at = (path = clean(p));
    filter = '';
    naming = null;
    loading = true;
    error = '';
    try {
      const r = await api<Schemas['ListingBody']>('GET', `/api/files/list?path=${enc(at)}`);
      if (n !== seq) return;
      folders = r.entries.filter((e) => e.is_dir).sort((a, b) => byName.compare(a.name, b.name));
      listed = at;
      mount = r.mount;
    } catch (e) {
      if (n !== seq) return;
      folders = [];
      listed = at;
      mount = null;
      error = `Couldn't open this folder: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      if (n === seq) loading = false;
    }
    if (n !== seq) return;
    // Focus left with the row that was opened: back into the list.
    await tick();
    if (root && !root.contains(document.activeElement)) (rows()[0] ?? filterEl)?.focus();
  }
  // svelte-ignore state_referenced_locally
  void open(opts.start);

  // A mouse or trackpad starts in the filter; a phone doesn't (its keyboard
  // would cover the list).
  $effect(() => {
    if (window.matchMedia?.('(pointer: fine)').matches) filterEl?.focus();
  });

  const rows = () => [...(root?.querySelectorAll<HTMLElement>('[data-row]') ?? [])];

  function pick() {
    if (loading || why) return;
    remember(path);
    done(path);
  }

  async function create() {
    const name = (naming ?? '').trim();
    if (!name || creating) return;
    creating = true;
    try {
      await api('POST', '/api/files/mkdir', { path: join(path, name) });
      naming = null;
      await open(join(path, name));
    } catch (e) {
      // (Shown under the field; the listing stays.)
      createError = e instanceof Error ? e.message : String(e);
    } finally {
      creating = false;
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      pick();
      return;
    }
    const target = e.target as HTMLElement;
    const all = rows();
    const i = all.indexOf(target);
    const go = (k: number) => {
      e.preventDefault();
      all[Math.max(0, Math.min(all.length - 1, k))]?.focus();
    };
    if (e.key === 'ArrowDown') go(i < 0 ? 0 : i + 1);
    else if (e.key === 'ArrowUp' && i >= 0) i === 0 ? (e.preventDefault(), filterEl?.focus()) : go(i - 1);
    else if (e.key === 'Home' && i >= 0) go(0);
    else if (e.key === 'End' && i >= 0) go(all.length - 1);
    else if ((e.key === 'ArrowLeft' || e.key === 'Backspace') && i >= 0 && path) {
      e.preventDefault();
      void open(path.split('/').slice(0, -1).join('/'));
    }
  }

  const trail = $derived(crumbs(path));
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div bind:this={root} class="flex h-full min-h-0 flex-col gap-3 p-4 sm:p-6" {onkeydown}>
  <h2 id={titleId} class="text-lg leading-none font-semibold">{opts.title}</h2>

  <nav aria-label="Folder path" class="flex min-w-0 flex-wrap items-center gap-0.5 text-sm">
    {#each trail as cr, i (cr.path)}
      {#if i}<ChevronRightIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />{/if}
      <button
        type="button"
        class="max-w-40 truncate rounded-md px-1.5 py-1 font-medium hover:bg-accent"
        aria-current={i === trail.length - 1 ? 'location' : undefined}
        onclick={() => open(cr.path)}>{cr.name}</button>
    {/each}
  </nav>

  {#if recents.length}
    <div class="flex min-w-0 items-center gap-1.5 overflow-x-auto text-sm" role="group" aria-label="Recent folders">
      <HistoryIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
      {#each recents as r (r)}
        <button type="button" class="shrink-0 truncate rounded-full border border-border px-2.5 py-0.5 hover:bg-accent" title={r || 'Home'} onclick={() => open(r)}>
          {r ? r.split('/').at(-1) : 'Home'}
        </button>
      {/each}
    </div>
  {/if}

  <div class="flex gap-2">
    <Input bind:ref={filterEl} bind:value={filter} placeholder="Filter folders" aria-label="Filter folders" autocomplete="off" />
    <Button variant="outline" onclick={() => ((naming = ''), (createError = ''))} disabled={loading || !!error || !!mount?.offline}>
      <FolderPlusIcon /><span class="max-sm:sr-only">New folder</span>
    </Button>
  </div>

  <div class="min-h-40 flex-1 overflow-y-auto rounded-md border border-border" aria-busy={loading}>
    {#if naming !== null}
      <form class="flex items-start gap-2 border-b border-border p-2" onsubmit={(e) => (e.preventDefault(), create())}>
        <div class="grid flex-1 gap-1">
          <!-- svelte-ignore a11y_autofocus -->
          <Input
            bind:value={naming}
            aria-label="New folder name"
            autocomplete="off"
            autofocus
            aria-invalid={!!createError || undefined}
            onkeydown={(e) => {
              // Escape closes the field, not the whole dialog.
              if (e.key === 'Escape') (e.preventDefault(), e.stopPropagation(), (naming = null), filterEl?.focus());
            }} />
          {#if createError}<span class="text-xs text-destructive" role="alert">{createError}</span>{/if}
        </div>
        <Button type="submit" disabled={creating || !naming.trim()}>Create</Button>
      </form>
    {/if}
    {#if error}
      <p class="p-4 text-sm text-destructive" role="alert">{error}</p>
    {:else if !loading && !shown.length}
      <p class="p-4 text-sm text-muted-foreground">{filter ? 'No folder matches.' : 'No folders here.'}</p>
    {:else}
      <ul aria-label="Folders">
        {#each shown as f (f.name)}
          {@const no = rowWhy(f)}
          <li>
            <button
              type="button"
              data-row
              class="flex w-full items-center gap-3 px-3 py-2 text-left text-sm hover:bg-accent focus-visible:bg-accent focus-visible:outline-none aria-disabled:cursor-not-allowed aria-disabled:opacity-60"
              aria-disabled={!!no || undefined}
              title={no ?? undefined}
              onclick={() => !no && open(join(listed, f.name))}>
              <FileIcon entry={f} class="size-5" />
              <span class="min-w-0 flex-1">
                <span class="block truncate">{f.name}</span>
                {#if no}<span class="block text-xs text-muted-foreground">{no}</span>{/if}
              </span>
              {#if !no}<ChevronRightIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />{/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>

  <div class="flex flex-col gap-2 sm:flex-row sm:items-center">
    <p class="min-h-5 flex-1 text-sm text-muted-foreground" aria-live="polite">{loading ? '' : (why ?? '')}</p>
    <div class="flex flex-col-reverse gap-2 sm:flex-row">
      <Button variant="outline" onclick={() => done(null)}>Cancel</Button>
      <Button onclick={pick} disabled={loading || !!why} title="Ctrl+Enter">{opts.confirm ?? 'Choose'}</Button>
    </div>
  </div>
</div>
