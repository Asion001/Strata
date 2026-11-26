<script lang="ts">
  import type { FilesController } from './files.svelte';

  let { c }: { c: FilesController } = $props();
</script>

<svelte:window
  ondragover={(e) => { e.preventDefault(); c.dragging = true; }}
  ondragleave={(e) => { if (!e.relatedTarget) c.dragging = false; }}
  ondrop={c.onDrop} />

{#if c.dragging}
  <div class="pointer-events-none fixed inset-0 z-40 grid place-items-center border-4 border-dashed border-primary bg-primary/15 text-xl font-medium">Drop to upload to {c.path || 'Home'}</div>
{/if}
