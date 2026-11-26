<script lang="ts">
  import type { FilesController } from './files.svelte';

  let { c }: { c: FilesController } = $props();
  const n = $derived(c.sorted.length);
</script>

<input type="checkbox" class="size-4 cursor-pointer accent-primary" aria-label="Select all"
  checked={n > 0 && c.selection.size === n}
  indeterminate={c.selection.size > 0 && c.selection.size < n}
  disabled={!n}
  onclick={(ev) => {
    c.selection.toggleAll(c.order);
    ev.currentTarget.checked = c.selection.size === n;
    ev.currentTarget.indeterminate = false;
  }} />
