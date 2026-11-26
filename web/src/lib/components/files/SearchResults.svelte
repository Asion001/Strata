<script lang="ts">
  import { bytes } from '$lib/format';
  import { fileHref } from '$lib/components/shell/nav';
  import FileIcon from './FileIcon.svelte';
  import type { FilesController, Hit } from './files.svelte';

  let { c, hits }: { c: FilesController; hits: Hit[] } = $props();
</script>

<div class="flex-1 overflow-auto">
  <p class="px-4 py-3 text-sm text-muted-foreground">{hits.length} results for “{c.query}”</p>
  {#each hits as h (h.path)}
    <a class="flex h-14 items-center gap-3 border-b border-border px-4 text-foreground hover:bg-accent"
      href={h.is_dir ? `/?p=${encodeURIComponent(h.path)}` : fileHref(h.path)} onclick={() => (c.query = '')}>
      <FileIcon entry={h} class="size-6" />
      <span class="flex min-w-0 flex-1 flex-col"><span class="truncate">{h.name}</span><small class="truncate text-xs text-muted-foreground">{h.path}</small></span>
      <span class="text-sm text-muted-foreground">{h.is_dir ? '' : bytes(h.size)}</span>
    </a>
  {/each}
</div>
