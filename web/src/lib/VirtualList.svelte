<script lang="ts" generics="T">
  // Renders only the rows in view, so folders with 100k entries stay fast.
  import type { Snippet } from 'svelte';

  let { items, rowHeight = 52, row }: { items: T[]; rowHeight?: number; row: Snippet<[T, number]> } = $props();
  let scrollTop = $state(0);
  let height = $state(600);
  const overscan = 8;
  let start = $derived(Math.max(0, Math.floor(scrollTop / rowHeight) - overscan));
  let end = $derived(Math.min(items.length, Math.ceil((scrollTop + height) / rowHeight) + overscan));
</script>

<div class="viewport" bind:clientHeight={height} onscroll={(e) => (scrollTop = e.currentTarget.scrollTop)}>
  <div style="height: {items.length * rowHeight}px; position: relative;">
    {#each items.slice(start, end) as item, i (start + i)}
      <div class="slot" style="top: {(start + i) * rowHeight}px; height: {rowHeight}px">
        {@render row(item, start + i)}
      </div>
    {/each}
  </div>
</div>

<style>
  .viewport { overflow-y: auto; height: 100%; contain: strict; -webkit-overflow-scrolling: touch; }
  .slot { position: absolute; left: 0; right: 0; }
</style>
