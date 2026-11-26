<script lang="ts">
  // The files page: a thin route wiring the files controller to its parts.
  import { page } from '$app/state';
  import { join } from '$lib/format';
  import { staleApp } from '$lib/lazy';
  import { session } from '$lib/session.svelte';
  import Editor from '$lib/editor/Editor.svelte';
  import { FilesController } from '$lib/components/files/files.svelte';
  import Toolbar from '$lib/components/files/Toolbar.svelte';
  import FileTable from '$lib/components/files/FileTable.svelte';
  import FileGrid from '$lib/components/files/FileGrid.svelte';
  import RowMenu from '$lib/components/files/RowMenu.svelte';
  import UploadPanel from '$lib/components/files/UploadPanel.svelte';
  import DropOverlay from '$lib/components/files/DropOverlay.svelte';
  import SearchResults from '$lib/components/files/SearchResults.svelte';

  const c = new FilesController(() => page.url);
  c.start();

  // The viewer (and, through it, the PDF viewer), the details panel and the
  // selection bar load on first use.
  let Viewer = $state<typeof import('$lib/Viewer.svelte').default>();
  let Details = $state<typeof import('$lib/components/files/DetailsPanel.svelte').default>();
  let SelectionBar = $state<typeof import('$lib/components/files/SelectionBar.svelte').default>();
  $effect(() => {
    if (c.viewerIndex >= 0 && !Viewer) import('$lib/Viewer.svelte').then((m) => (Viewer = m.default), staleApp);
  });
  $effect(() => {
    if (c.details !== null && !Details) import('$lib/components/files/DetailsPanel.svelte').then((m) => (Details = m.default), staleApp);
  });
  $effect(() => {
    if (c.selection.size && !SelectionBar) import('$lib/components/files/SelectionBar.svelte').then((m) => (SelectionBar = m.default), staleApp);
  });
</script>

<svelte:window onkeydown={c.onKeys} />
<DropOverlay {c} />

<div class="flex h-full">
<div class="relative flex min-w-0 flex-1 flex-col text-[15px]">
  <Toolbar {c} />
  {#if c.offlineAt}
    <p class="border-b border-border bg-muted px-4 py-2 text-sm" role="status" data-testid="offline-banner">Offline, as of {new Date(c.offlineAt).toLocaleString()}. Files kept offline open; the rest needs the network.</p>
  {/if}
  {#if c.mount?.offline}
    <p class="border-b border-border px-4 py-2 text-sm text-muted-foreground" role="status">This storage is offline. Its files can't be opened or changed until it's back.</p>
  {/if}

  {#if c.selection.size && !c.hits && SelectionBar}
    <SelectionBar {c} />
  {/if}

  {#if c.hits}
    <SearchResults {c} hits={c.hits} />
  {:else if c.error}
    <p class="p-4 text-destructive">{c.error}</p>
  {:else if c.view === 'list'}
    <FileTable {c} />
  {:else}
    <FileGrid {c} />
  {/if}

  <UploadPanel />
</div>

{#if c.detailsEntry && Details}
  {@const e = c.detailsEntry}
  <!-- Beside the list on wide screens, over it on phones. -->
  <div class="w-80 shrink-0 border-l border-border max-md:fixed max-md:inset-0 max-md:z-30 max-md:w-full max-md:border-0 max-md:pt-[env(safe-area-inset-top)] max-md:pb-[env(safe-area-inset-bottom)] lg:w-96">
    <Details
      entry={e}
      path={join(c.listedPath, e.name)}
      storage={c.mount ? `${c.mount.mode} · ${c.mount.connection_name}` : undefined}
      onclose={() => (c.details = null)}
      onshare={() => c.share(e)} />
  </div>
{/if}
</div>

<RowMenu {c} />

<!-- The viewer and editor aren't rebuilt yet: they keep the old styles. -->
{#if c.viewerIndex >= 0 && Viewer}
  <div class="legacy contents">
  <Viewer
    items={c.viewerItems}
    bind:index={c.viewerIndex}
    onedit={(i) => { c.editing = c.viewerItems[i].path; c.viewerIndex = -1; }}
    onshare={(i) => { const e = c.byName.get(c.viewerItems[i].name); if (e) void c.share(e); }}
    ondelete={c.viewerDelete}
    onclose={() => (c.viewerIndex = -1)} />
  </div>
{/if}

{#if c.editing && session.user}
  <div class="legacy contents">
  {#key c.editing}
    <Editor path={c.editing} userId={session.user.id} mountId={c.mount?.id ?? null} onclose={() => { c.editing = null; void c.load(); }} />
  {/key}
  </div>
{/if}
