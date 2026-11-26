<script lang="ts">
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import type { FilesController } from './files.svelte';

  let { c }: { c: FilesController } = $props();
</script>

<nav aria-label="Folder path" class="flex min-w-0 flex-wrap items-center gap-1 text-sm">
  <a href="/?p=" class="rounded-md px-1.5 py-1 font-medium text-foreground hover:bg-accent">Home</a>
  {#each c.crumbs as cr, i (cr.path)}
    <ChevronRightIcon class="size-4 text-muted-foreground" aria-hidden="true" />
    <a href="/?p={encodeURIComponent(cr.path)}" class="truncate rounded-md px-1.5 py-1 font-medium text-foreground hover:bg-accent" aria-current={i === c.crumbs.length - 1 ? 'page' : undefined}>{cr.name}</a>
  {/each}
  {#if c.mount && c.mount.path !== ''}
    <span class="ml-1 rounded-full border border-border px-2 py-0.5 text-xs text-muted-foreground" title="Mounted from {c.mount.connection_name}">{c.mount.mode} · {c.mount.connection_name}{c.mount.offline ? ' · offline' : ''}</span>
  {/if}
</nav>
