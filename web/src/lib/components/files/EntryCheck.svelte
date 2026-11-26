<script lang="ts">
  import type { Entry } from '$lib/api';
  import { cn } from '$lib/utils';
  import type { FilesController } from './files.svelte';

  let { c, e, class: extra = '' }: { c: FilesController; e: Entry; class?: string } = $props();
</script>

<input
  type="checkbox"
  class={cn('size-4 shrink-0 cursor-pointer accent-primary', extra)}
  checked={c.selection.has(e.name)}
  aria-label="Select {e.name}"
  onclick={(ev) => c.check(e, ev)} />
