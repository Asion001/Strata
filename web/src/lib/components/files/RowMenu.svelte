<script lang="ts">
  import { contentUrl } from '$lib/api';
  import { join, isTextName } from '$lib/format';
  import FileIcon from './FileIcon.svelte';
  import type { FilesController } from './files.svelte';

  let { c }: { c: FilesController } = $props();
</script>

{#if c.menu}
  {@const e = c.menu}
  <div class="fixed inset-0 z-30 grid place-items-center bg-black/35 max-sm:items-end" role="presentation" onclick={() => (c.menu = null)}>
    <div role="menu" tabindex="-1" aria-label="Actions for {e.name}"
      class="grid w-[min(320px,calc(100vw-2rem))] gap-0.5 rounded-xl border border-border bg-popover p-2 text-popover-foreground shadow-lg max-sm:w-full max-sm:rounded-b-none max-sm:pb-[max(0.75rem,env(safe-area-inset-bottom))]"
      onclick={(ev) => ev.stopPropagation()} onkeydown={(ev) => { if (ev.key === 'Escape') c.menu = null; }}>
      <div class="flex items-center gap-2 px-2.5 pt-1.5 pb-2.5"><FileIcon entry={e} class="size-5" /><b class="truncate text-sm">{e.name}</b></div>
      <button role="menuitem" class="item" onclick={() => c.showDetails(e)}>Details</button>
      {#if !e.is_dir}<a role="menuitem" class="item" href={contentUrl(join(c.listedPath, e.name), true)}>Download</a>{/if}
      {#if !e.mount_id}
        {#if !e.is_dir && isTextName(e.name)}
          <button role="menuitem" class="item" onclick={() => c.edit(e)}>Edit</button>
        {/if}
        <button role="menuitem" class="item" onclick={() => c.rename(e)}>Rename</button>
        <button role="menuitem" class="item" onclick={() => c.moveTo(e)}>Move…</button>
      {/if}
      <button role="menuitem" class="item" onclick={() => c.moveTo(e, true)}>Copy…</button>
      <button role="menuitem" class="item" onclick={() => c.share(e)}>Share link…</button>
      {#if !e.is_dir}
        <button role="menuitemcheckbox" aria-checked={c.pinned.has(join(c.listedPath, e.name))} class="item" onclick={() => c.togglePin(e)}>Keep offline</button>
      {/if}
      {#if e.mount_id}
        <a role="menuitem" class="item" href="/mounts">Manage mount…</a>
      {:else}
        <button role="menuitem" class="item danger" onclick={() => c.remove(e)}>Delete</button>
        {#if c.mount?.trash}
          <button role="menuitem" class="item danger" onclick={() => c.remove(e, true)}>Delete permanently</button>
        {/if}
      {/if}
    </div>
  </div>
{/if}

<style>
  .item {
    display: block; width: 100%; padding: 0.625rem; border-radius: calc(var(--radius) - 4px);
    text-align: left; font-size: 0.875rem; color: inherit;
  }
  /* Scoped styles beat utilities, so the colour lives here. */
  .item.danger { color: var(--destructive); }
  .item:hover, .item:focus-visible { background: var(--hover); outline: none; }
</style>
