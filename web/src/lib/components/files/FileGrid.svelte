<script lang="ts">
  import type { Entry } from '$lib/api';
  import { thumbUrl } from '$lib/api';
  import { bytes, join, isImage } from '$lib/format';
  import { cn } from '$lib/utils';
  import VirtualList from '$lib/VirtualList.svelte';
  import FileIcon from './FileIcon.svelte';
  import SelectAll from './SelectAll.svelte';
  import SortMenu from './SortMenu.svelte';
  import EntryCheck from './EntryCheck.svelte';
  import MoreButton from './MoreButton.svelte';
  import type { FilesController } from './files.svelte';

  let { c }: { c: FilesController } = $props();
  // As many columns as fit, rendered as virtual rows.
  let width = $state(0);
  const cols = $derived(Math.max(2, Math.floor((width - 16) / 172)));
  const rows = $derived(Array.from({ length: Math.ceil(c.sorted.length / cols) }, (_, i) => c.sorted.slice(i * cols, i * cols + cols)));
</script>

<div class="flex h-9 shrink-0 items-center gap-3 border-b border-border px-4 text-xs font-medium text-muted-foreground">
  <SelectAll {c} />
  <span class="mr-auto">{c.sorted.length} items</span>
  <SortMenu {c} />
</div>
<div class="relative min-h-0 flex-1" bind:clientWidth={width}>
  {#if !c.loading && c.entries.length === 0}
    <p class="absolute inset-x-0 top-10 text-center text-muted-foreground">This folder is empty. Drop files here or press Upload.</p>
  {/if}
  <VirtualList items={rows} rowHeight={196}>
    {#snippet row(items: Entry[])}
      <div class="grid gap-3 px-4 pt-3" style:grid-template-columns="repeat({cols}, minmax(0, 1fr))">
        {#each items as e (e.name)}
          {@const on = c.selection.has(e.name)}
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
          <div class={cn('group relative flex h-[184px] flex-col overflow-hidden rounded-lg border border-border bg-card select-none hover:border-ring/60', on && 'border-primary ring-2 ring-primary/40')}
            onclick={(ev) => c.activate(e, ev)}>
            <div class="grid min-h-0 flex-1 place-items-center bg-muted/60">
              {#if !e.is_dir && isImage(e.name)}
                <img loading="lazy" src={thumbUrl(join(c.listedPath, e.name), e.etag)} alt="" class="size-full object-cover" />
              {:else}<FileIcon entry={e} class="size-14" />{/if}
            </div>
            <div class="flex items-center gap-1 px-2.5 py-2">
              <div class="flex min-w-0 flex-1 flex-col">
                <button type="button" class="truncate rounded-sm text-left text-sm font-medium text-foreground focus-visible:outline-2 focus-visible:outline-ring"
                  onclick={(ev) => { ev.stopPropagation(); c.activate(e, ev); }}>{e.name}</button>
                <span class="truncate text-xs text-muted-foreground">{e.mount_id ? (e.offline ? 'Mount · offline' : 'Mount') : e.is_dir ? 'Folder' : bytes(e.size)}</span>
              </div>
              <MoreButton {c} {e} class="-mr-1" />
            </div>
            <EntryCheck {c} {e} class={cn('absolute top-2 left-2 size-5 rounded opacity-0 group-hover:opacity-100 focus-visible:opacity-100 [@media(hover:none)]:opacity-100', on && 'opacity-100')} />
          </div>
        {/each}
      </div>
    {/snippet}
  </VirtualList>
</div>
