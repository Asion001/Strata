<script lang="ts">
  import Button from '$lib/components/ui/button/button.svelte';
  import FolderInputIcon from '@lucide/svelte/icons/folder-input';
  import CopyIcon from '@lucide/svelte/icons/copy';
  import TrashIcon from '@lucide/svelte/icons/trash';
  import XIcon from '@lucide/svelte/icons/x';
  import InfoIcon from '@lucide/svelte/icons/info';
  import type { FilesController } from './files.svelte';

  let { c }: { c: FilesController } = $props();
  const mounts = $derived(c.picked.filter((e) => e.mount_id).length);
</script>

<div class="flex items-center gap-2 border-b border-border bg-accent/60 px-4 py-1.5 text-sm" role="toolbar" aria-label="Selected items">
  <Button variant="ghost" size="icon" class="size-8" aria-label="Clear selection" onclick={() => c.selection.clear()}><XIcon /></Button>
  <span class="mr-auto font-medium" aria-live="polite">{c.selection.size} selected</span>
  {#if c.picked.length === 1}
    <Button variant="ghost" size="sm" onclick={() => c.showDetails(c.picked[0])}><InfoIcon /><span class="max-sm:sr-only">Details</span></Button>
  {/if}
  <Button variant="ghost" size="sm" onclick={() => c.transferPicked(false)} disabled={c.busy || mounts === c.picked.length} title={mounts ? 'Mounts are moved on the Mounts page' : undefined}><FolderInputIcon /><span class="max-sm:sr-only">Move</span></Button>
  <Button variant="ghost" size="sm" onclick={() => c.transferPicked(true)} disabled={c.busy}><CopyIcon /><span class="max-sm:sr-only">Copy</span></Button>
  <Button variant="ghost" size="sm" class="text-destructive hover:text-destructive" onclick={() => c.removeItems(c.picked)} disabled={c.busy || mounts === c.picked.length} title={mounts ? 'Mounts are removed on the Mounts page' : undefined}><TrashIcon /><span class="max-sm:sr-only">Delete</span></Button>
</div>
