<script lang="ts">
  import { bytes } from '$lib/format';
  import Button from '$lib/components/ui/button/button.svelte';
  import { cancelUpload, clearFinished, net, resolveUpload, retryUpload, uploads } from '$lib/offline/app.svelte';
  import { isActive, progress } from '$lib/offline/jobs';
  import { toast } from '$lib/toast.svelte';
</script>

{#if uploads.jobs.length}
  <section aria-label="Uploads" class="uploads fixed right-4 z-20 grid max-h-[50vh] w-[min(380px,calc(100vw-2rem))] gap-2 overflow-auto rounded-xl border border-border bg-popover p-3 shadow-lg">
    <div class="flex items-center"><b class="flex-1 text-sm">Uploads{#if !net.online && uploads.jobs.some(isActive)} <span class="font-normal text-muted-foreground">· waiting for the network</span>{/if}</b>
      {#if uploads.jobs.some((u) => !isActive(u))}<Button variant="ghost" size="sm" onclick={clearFinished}>Clear</Button>{/if}</div>
    {#each uploads.jobs.slice(-6) as u (u.id)}
      <div class="grid gap-1 text-sm" data-testid="upload">
        <div class="flex items-center gap-2"><span class="min-w-0 flex-1 truncate">{u.path}</span>
          <span class="text-muted-foreground">{u.state === 'failed' ? '' : u.state === 'decide' ? 'name taken' : u.state === 'done' ? (u.skipped ? `done, ${bytes(u.skipped)} deduped` : 'done') : u.state}</span>
          {#if isActive(u) && u.state !== 'committing'}<Button variant="ghost" size="sm" class="h-6 px-2" onclick={async () => { if (!(await cancelUpload(u.id))) toast('Too late to cancel: the file is being saved.', 'info'); }} aria-label="Cancel upload of {u.path}">Cancel</Button>{/if}
          {#if u.state === 'failed'}<Button variant="ghost" size="sm" class="h-6 px-2" onclick={async () => { if (!(await retryUpload(u.id))) toast('The file is no longer available here; add it again.', 'error'); }}>Retry</Button>{/if}</div>
        {#if u.state === 'failed'}<div class="text-destructive">{u.error}</div>{/if}
        {#if u.state === 'decide'}
          <!-- Cancel (above) skips it. -->
          <div class="flex flex-wrap items-center gap-1" data-testid="upload-decide">
            <span class="flex-1 text-muted-foreground">A {u.existing?.is_dir ? 'folder' : 'file'} with this name is already there.</span>
            <Button variant="outline" size="sm" class="h-6 px-2" onclick={() => resolveUpload(u.id, 'rename')}>Keep both</Button>
            {#if !u.existing?.is_dir}<Button variant="outline" size="sm" class="h-6 px-2" onclick={() => resolveUpload(u.id, 'replace')}>Replace</Button>{/if}
          </div>
        {/if}
        <progress max="1" value={progress(u)} class="h-1.5 w-full accent-primary"></progress>
      </div>
    {/each}
  </section>
{/if}

<style>
  /* Above the tab bar on phones, clear of the corner on wider screens. */
  .uploads { bottom: 1rem; }
  @media (width < 48rem) {
    .uploads { bottom: calc(72px + env(safe-area-inset-bottom)); }
  }
</style>
