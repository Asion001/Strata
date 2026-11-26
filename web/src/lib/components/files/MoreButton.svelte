<script lang="ts">
  import type { Entry } from '$lib/api';
  import { cn } from '$lib/utils';
  import { buttonClass } from '$lib/components/ui/button/button.svelte';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import type { FilesController } from './files.svelte';

  let { c, e, class: extra = '' }: { c: FilesController; e: Entry; class?: string } = $props();
</script>

<button
  type="button"
  class={cn(buttonClass('ghost', 'icon'), 'size-8 text-muted-foreground', extra)}
  aria-label="Actions for {e.name}"
  onclick={(ev) => { ev.stopPropagation(); c.menu = e; }}>
  <EllipsisIcon />
</button>
