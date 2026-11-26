<script lang="ts">
  import type { Entry } from '$lib/api';
  import { thumbUrl } from '$lib/api';
  import { bytes, when, join, isImage } from '$lib/format';
  import { cn } from '$lib/utils';
  import VirtualList from '$lib/VirtualList.svelte';
  import ArrowUpIcon from '@lucide/svelte/icons/arrow-up';
  import ArrowDownIcon from '@lucide/svelte/icons/arrow-down';
  import FileIcon from './FileIcon.svelte';
  import SelectAll from './SelectAll.svelte';
  import SortMenu from './SortMenu.svelte';
  import EntryCheck from './EntryCheck.svelte';
  import MoreButton from './MoreButton.svelte';
  import type { SortKey } from './listing';
  import type { FilesController } from './files.svelte';

  let { c }: { c: FilesController } = $props();
</script>

{#snippet sortHead(key: SortKey, label: string, cls: string)}
  <div role="columnheader" class={cls} aria-sort={c.sort.key === key ? (c.sort.desc ? 'descending' : 'ascending') : 'none'}>
    <button type="button" class="inline-flex items-center gap-1 rounded-sm hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring" onclick={() => c.sortBy(key)}>
      {label}
      {#if c.sort.key === key}
        {#if c.sort.desc}<ArrowDownIcon class="size-3.5" />{:else}<ArrowUpIcon class="size-3.5" />{/if}
      {/if}
    </button>
  </div>
{/snippet}

<div class="flex min-h-0 flex-1 flex-col" role="table" aria-label="Files" aria-rowcount={c.sorted.length + 1}>
  <div role="row" aria-rowindex={1} class="flex h-9 shrink-0 items-center gap-3 border-b border-border px-4 text-xs font-medium text-muted-foreground">
    <div role="columnheader" class="flex w-4 justify-center"><SelectAll {c} /></div>
    <div class="w-7" aria-hidden="true"></div>
    {@render sortHead('name', 'Name', 'flex-1')}
    {@render sortHead('size', 'Size', 'hidden w-20 justify-end text-right sm:flex')}
    {@render sortHead('mtime', 'Modified', 'hidden w-28 justify-end text-right md:flex')}
    <SortMenu {c} class="sm:hidden" />
    <div class="w-8 max-sm:hidden" aria-hidden="true"></div>
  </div>
  <div class="relative min-h-0 flex-1">
    {#if !c.loading && c.entries.length === 0}
      <p class="absolute inset-x-0 top-10 text-center text-muted-foreground">This folder is empty. Drop files here or press Upload.</p>
    {/if}
    <VirtualList items={c.sorted} rowHeight={52}>
      {#snippet row(e: Entry, i: number)}
        {@const on = c.selection.has(e.name)}
        <!-- Mouse convenience; keyboard users have the checkbox and name button. -->
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div role="row" tabindex="-1" aria-rowindex={i + 2} aria-selected={on}
          class={cn('group flex h-[52px] cursor-default items-center gap-3 border-b border-border px-4 select-none hover:bg-accent/70', on && 'bg-primary/10 hover:bg-primary/15', e.offline && 'opacity-50')}
          onclick={(ev) => c.activate(e, ev)}>
          <div role="cell" class="flex w-4 justify-center"><EntryCheck {c} {e} /></div>
          <div role="cell" class="grid size-7 place-items-center">
            {#if !e.is_dir && isImage(e.name)}
              <img loading="lazy" src={thumbUrl(join(c.listedPath, e.name), e.etag)} alt="" class="size-7 rounded object-cover" />
            {:else}<FileIcon entry={e} class="size-6" />{/if}
          </div>
          <div role="cell" class="flex min-w-0 flex-1 flex-col">
            <button type="button" class="truncate rounded-sm text-left text-foreground hover:underline focus-visible:outline-2 focus-visible:outline-ring"
              onclick={(ev) => { ev.stopPropagation(); c.activate(e, ev); }}>{e.name}</button>
            <span class="truncate text-xs text-muted-foreground sm:hidden">
              {e.mount_id ? (e.offline ? 'Mount · offline' : 'Mount') : e.is_dir ? 'Folder' : bytes(e.size)} · {when(e.mtime)}
            </span>
            {#if e.mount_id}<span class="hidden text-xs text-muted-foreground sm:block">{e.offline ? 'Mount · offline' : 'Mount'}</span>{/if}
          </div>
          <div role="cell" class="hidden w-20 text-right text-sm text-muted-foreground tabular-nums sm:block">{e.is_dir ? '' : bytes(e.size)}</div>
          <div role="cell" class="hidden w-28 text-right text-sm text-muted-foreground md:block">{when(e.mtime)}</div>
          <div role="cell"><MoreButton {c} {e} /></div>
        </div>
      {/snippet}
    </VirtualList>
  </div>
</div>
