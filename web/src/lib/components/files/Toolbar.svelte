<script lang="ts">
  import { cn } from '$lib/utils';
  import { buttonClass } from '$lib/components/ui/button/button.svelte';
  import Button from '$lib/components/ui/button/button.svelte';
  import LayoutGridIcon from '@lucide/svelte/icons/layout-grid';
  import ListIcon from '@lucide/svelte/icons/list';
  import UploadIcon from '@lucide/svelte/icons/upload';
  import FolderUpIcon from '@lucide/svelte/icons/folder-up';
  import FolderPlusIcon from '@lucide/svelte/icons/folder-plus';
  import SearchIcon from '@lucide/svelte/icons/search';
  import Breadcrumb from './Breadcrumb.svelte';
  import type { FilesController } from './files.svelte';

  let { c }: { c: FilesController } = $props();
  let fileInput: HTMLInputElement;
  let folderInput: HTMLInputElement;
</script>

<header class="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 border-b border-border px-4 py-2.5">
  <Breadcrumb {c} />
  <div class="flex w-full items-center gap-2 sm:w-auto">
    <label class="relative min-w-0 flex-1 sm:w-52 sm:flex-none">
      <span class="sr-only">Search by name</span>
      <SearchIcon class="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
      <input type="search" placeholder="Search by name" bind:value={c.query}
        class="h-9 w-full rounded-md border border-input bg-transparent pr-3 pl-8 text-sm outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50" />
    </label>
    <div class="flex rounded-md border border-border p-0.5" role="group" aria-label="View">
      <button type="button" class={cn(buttonClass('ghost', 'icon'), 'size-7', c.view === 'list' && 'bg-accent text-foreground')} aria-pressed={c.view === 'list'} aria-label="List view" onclick={() => c.setView('list')}><ListIcon /></button>
      <button type="button" class={cn(buttonClass('ghost', 'icon'), 'size-7', c.view === 'grid' && 'bg-accent text-foreground')} aria-pressed={c.view === 'grid'} aria-label="Grid view" onclick={() => c.setView('grid')}><LayoutGridIcon /></button>
    </div>
    <Button variant="outline" size="icon" aria-label="New folder" title="New folder" onclick={c.newFolder}><FolderPlusIcon /></Button>
    <Button variant="outline" size="icon" aria-label="Upload a folder" title="Upload a folder" onclick={() => folderInput.click()}><FolderUpIcon /></Button>
    <Button onclick={() => fileInput.click()}><UploadIcon /><span class="max-sm:sr-only">Upload</span></Button>
  </div>
  <input bind:this={fileInput} type="file" multiple hidden onchange={(e) => { c.enqueue(e.currentTarget.files!); e.currentTarget.value = ''; }} />
  <input bind:this={folderInput} type="file" webkitdirectory hidden onchange={(e) => { c.enqueue(e.currentTarget.files!); e.currentTarget.value = ''; }} />
</header>
