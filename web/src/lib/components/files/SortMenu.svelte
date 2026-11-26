<script lang="ts">
  import { cn } from '$lib/utils';
  import { SORT_CHOICES, type FilesController } from './files.svelte';

  let { c, class: extra = '' }: { c: FilesController; class?: string } = $props();
</script>

<select aria-label="Sort by" class={cn('h-7 rounded-md border border-input bg-transparent px-1.5 text-xs text-foreground', extra)}
  value={SORT_CHOICES.findIndex(([, s]) => s.key === c.sort.key && s.desc === c.sort.desc)}
  onchange={(ev) => c.setSort(SORT_CHOICES[Number(ev.currentTarget.value)][1])}>
  {#each SORT_CHOICES as [label], i (label)}<option value={i}>{label}</option>{/each}
</select>
