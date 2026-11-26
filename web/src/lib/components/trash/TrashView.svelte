<script lang="ts">
  // Deleted files: what the user deleted, to restore or remove for good.
  import { SvelteSet } from 'svelte/reactivity';
  import { api, type Schemas } from '$lib/api';
  import { bytes, parent, when } from '$lib/format';
  import { cn } from '$lib/utils';
  import { confirmDialog } from '$lib/dialogs.svelte';
  import { toast, toastError } from '$lib/toast.svelte';
  import { listTrash, reportRestore, restoreItems, type TrashItem } from '$lib/trash.svelte';
  import { folderHref } from '$lib/components/shell/nav';
  import Button from '$lib/components/ui/button/button.svelte';
  import FileIcon from '$lib/components/files/FileIcon.svelte';
  import VirtualList from '$lib/VirtualList.svelte';
  import { h1Class, mutedClass } from '$lib/components/ui/styles';
  import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';
  import XIcon from '@lucide/svelte/icons/x';

  let items = $state<TrashItem[] | null>(null);
  let days = $state<number | null>(null);
  let error = $state('');
  let busy = $state(false);
  const selected = new SvelteSet<number>();

  let seq = 0;
  async function load() {
    const n = ++seq;
    try {
      const r = await listTrash();
      // (An older listing that answers late is dropped.)
      if (n !== seq) return;
      items = r.items;
      days = r.days;
      error = '';
      // (What is gone can't stay selected.)
      const ids = new Set(r.items.map((i) => i.id));
      for (const id of [...selected]) if (!ids.has(id)) selected.delete(id);
    } catch (e) {
      if (n === seq) error = e instanceof Error ? e.message : String(e);
    }
  }
  $effect(() => {
    void load();
  });

  const picked = $derived(items?.filter((i) => selected.has(i.id)) ?? []);
  const allOn = $derived(!!items?.length && items.every((i) => selected.has(i.id)));
  const label = (n: number) => (n === 1 ? '1 item' : `${n} items`);
  const where = (i: TrashItem) => parent(i.original_path);

  function toggleAll() {
    if (allOn) selected.clear();
    else for (const i of items ?? []) selected.add(i.id);
  }

  async function run(fn: () => Promise<void>) {
    if (busy) return;
    busy = true;
    try {
      await fn();
    } finally {
      busy = false;
      await load();
    }
  }

  const restore = (list: TrashItem[]) =>
    run(async () => {
      const r = await restoreItems(list.map((i) => ({ id: i.id, path: i.original_path })));
      for (const x of r.restored) selected.delete(x.id);
      reportRestore(r);
    });

  async function purge(list: TrashItem[]) {
    const one = list.length === 1;
    const ok = await confirmDialog({
      title: one ? `Delete “${list[0].name}” permanently?` : `Delete ${label(list.length)} permanently?`,
      body: `${one ? 'It' : 'They'} can’t be restored after this.`,
      confirm: 'Delete permanently',
      danger: true
    });
    if (!ok) return;
    await run(async () => {
      let failed = 0;
      let lastError = '';
      for (const i of list) {
        try {
          await api('DELETE', `/api/trash/${i.id}`);
          selected.delete(i.id);
        } catch (e) {
          failed++;
          lastError = e instanceof Error ? e.message : String(e);
        }
      }
      if (failed) toast(`Couldn't delete ${failed === 1 ? '1 item' : `${failed} items`} permanently: ${lastError}`, 'error');
      else toast(one ? `“${list[0].name}” deleted permanently.` : `${label(list.length)} deleted permanently.`, 'success');
    });
  }

  async function empty() {
    const n = items?.length ?? 0;
    const ok = await confirmDialog({
      title: 'Empty Deleted files?',
      body: `${label(n)} will be deleted permanently. They can’t be restored after this.`,
      confirm: 'Empty',
      danger: true
    });
    if (!ok) return;
    await run(async () => {
      try {
        const r = await api<Schemas['PurgedBody']>('POST', '/api/trash/empty', {});
        selected.clear();
        if (r.failed) toast(`${label(r.purged)} deleted permanently; ${label(r.failed)} couldn't be (their storage is offline).`, 'error');
        else toast(`Deleted files emptied (${label(r.purged)}).`, 'success');
      } catch (e) {
        toastError(e);
      }
    });
  }
</script>

<div class="flex h-full flex-col text-[15px]">
  <header class="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 border-b border-border px-4 py-3">
    <div class="min-w-0">
      <h1 class={h1Class}>Deleted files</h1>
      {#if days}
        <p class={mutedClass} data-testid="retention">Items are removed for good after {days} days.</p>
      {:else if days === 0 && items?.length}
        <p class={mutedClass} data-testid="retention">Deleted files are turned off on this server. These items can still be restored until they expire.</p>
      {/if}
    </div>
    {#if items?.length}
      <Button variant="outline" class="text-destructive hover:text-destructive" onclick={empty} disabled={busy}><Trash2Icon />Empty</Button>
    {/if}
  </header>

  {#if selected.size}
    <div class="flex items-center gap-2 border-b border-border bg-accent/60 px-4 py-1.5 text-sm" role="toolbar" aria-label="Selected items">
      <Button variant="ghost" size="icon" class="size-8" aria-label="Clear selection" onclick={() => selected.clear()}><XIcon /></Button>
      <span class="mr-auto font-medium" aria-live="polite">{selected.size} selected</span>
      <Button variant="ghost" size="sm" onclick={() => restore(picked)} disabled={busy}><RotateCcwIcon /><span class="max-sm:sr-only">Restore</span></Button>
      <Button variant="ghost" size="sm" class="text-destructive hover:text-destructive" onclick={() => purge(picked)} disabled={busy}><Trash2Icon /><span class="max-sm:sr-only">Delete permanently</span></Button>
    </div>
  {/if}

  {#if error}
    <p class="p-4 text-destructive" role="alert">{error}</p>
  {:else if items === null}
    <p class="p-4 {mutedClass}">Loading…</p>
  {:else if days === 0 && !items.length}
    <p class="p-4 {mutedClass}" data-testid="trash-off">Deleted files are turned off on this server: deleting removes files for good.</p>
  {:else if !items.length}
    <div class="px-4 py-10 text-center" data-testid="trash-empty">
      <p class="font-medium">Nothing here</p>
      <p class={mutedClass}>Deleted files and folders show up here{days ? ` for ${days} days` : ''}, and you can restore them.</p>
    </div>
  {:else}
    <div class="flex min-h-0 flex-1 flex-col" role="table" aria-label="Deleted files" aria-rowcount={items.length + 1}>
      <div role="row" aria-rowindex={1} class="flex h-9 shrink-0 items-center gap-3 border-b border-border px-4 text-xs font-medium text-muted-foreground">
        <div role="columnheader" class="flex w-4 justify-center">
          <input type="checkbox" class="size-4" aria-label="Select all" checked={allOn} onchange={toggleAll} />
        </div>
        <div class="w-6" aria-hidden="true"></div>
        <div role="columnheader" class="flex-1">Name</div>
        <div role="columnheader" class="hidden w-48 md:block">Original location</div>
        <div role="columnheader" class="hidden w-28 text-right sm:block">Deleted</div>
        <div role="columnheader" class="hidden w-20 text-right sm:block">Size</div>
        <div class="w-[4.5rem]" aria-hidden="true"></div>
      </div>
      <div class="min-h-0 flex-1">
        <VirtualList {items} rowHeight={52}>
          {#snippet row(i: TrashItem, n: number)}
            {@const on = selected.has(i.id)}
            <div role="row" aria-rowindex={n + 2} aria-selected={on}
              class={cn('flex h-[52px] items-center gap-3 border-b border-border px-4 hover:bg-accent/70', on && 'bg-primary/10 hover:bg-primary/15')}>
              <div role="cell" class="flex w-4 justify-center">
                <input type="checkbox" class="size-4" aria-label="Select {i.name}" checked={on}
                  onchange={() => (on ? selected.delete(i.id) : selected.add(i.id))} />
              </div>
              <div role="cell" class="grid size-6 place-items-center"><FileIcon entry={i} class="size-6" /></div>
              <div role="cell" class="flex min-w-0 flex-1 flex-col">
                <span class="truncate" title={i.original_path}>{i.name}</span>
                <span class="truncate text-xs text-muted-foreground md:hidden">{where(i) || 'Home'} · {when(i.deleted_at)}</span>
              </div>
              <div role="cell" class="hidden w-48 truncate text-sm md:block">
                <a class="text-muted-foreground hover:underline" href={folderHref(where(i))} title={where(i) || 'Home'}>{where(i) || 'Home'}</a>
              </div>
              <div role="cell" class="hidden w-28 text-right text-sm text-muted-foreground sm:block">{when(i.deleted_at)}</div>
              <div role="cell" class="hidden w-20 text-right text-sm text-muted-foreground tabular-nums sm:block">{i.is_dir ? '' : bytes(i.size)}</div>
              <div role="cell" class="flex w-[4.5rem] justify-end gap-1">
                <Button variant="ghost" size="icon" class="size-8" aria-label="Restore {i.name}" title="Restore" onclick={() => restore([i])} disabled={busy}><RotateCcwIcon /></Button>
                <Button variant="ghost" size="icon" class="size-8 text-destructive hover:text-destructive" aria-label="Delete {i.name} permanently" title="Delete permanently" onclick={() => purge([i])} disabled={busy}><Trash2Icon /></Button>
              </div>
            </div>
          {/snippet}
        </VirtualList>
      </div>
    </div>
  {/if}
</div>
