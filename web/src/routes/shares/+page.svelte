<script lang="ts">
  import { alertDialog, confirmDialog } from '$lib/dialogs.svelte';
  import { toast, toastError } from '$lib/toast.svelte';
  import { api } from '$lib/api';
  import { when } from '$lib/format';
  import { cn } from '$lib/utils';
  import Button from '$lib/components/ui/button/button.svelte';
  import FileIcon from '$lib/components/files/FileIcon.svelte';
  import { fileHref, folderHref } from '$lib/components/shell/nav';
  import { h1Class, listClass, mutedClass, pageClass, rowClass } from '$lib/components/ui/styles';
  import CopyIcon from '@lucide/svelte/icons/copy';
  import ExternalLinkIcon from '@lucide/svelte/icons/external-link';
  import LockIcon from '@lucide/svelte/icons/lock';
  import TrashIcon from '@lucide/svelte/icons/trash';

  type Share = { id: number; token: string; path: string; is_dir: boolean; has_password: boolean; expires_at: string | null; created_at: string };
  let shares = $state<Share[] | null>(null);
  const load = async () => {
    try { shares = await api('GET', '/api/shares'); } catch (e) { shares ??= []; toastError(e); }
  };
  $effect(() => { load(); });
  const link = (s: Share) => `${location.origin}/s/${s.token}`;
  const name = (s: Share) => s.path.slice(s.path.lastIndexOf('/') + 1) || s.path;
  async function copy(s: Share) {
    try { await navigator.clipboard.writeText(link(s)); toast('Link copied.', 'success'); }
    catch { await alertDialog({ title: 'Share link', copy: link(s), ok: 'Done' }); }
  }
  async function del(s: Share) {
    const ok = await confirmDialog({ title: 'Delete this link?', body: 'People who have it lose access.', confirm: 'Delete link', danger: true });
    if (!ok) return;
    try { await api('DELETE', `/api/shares/${s.id}`); } catch (e) { toastError(e); }
    await load();
  }
</script>

<div class={pageClass}>
  <h1 class={h1Class}>Share links</h1>
  <p class={cn(mutedClass, 'mt-1 mb-6')}>Create links from the ⋯ menu or the details panel of any file or folder.</p>
  {#if shares === null}
    <p class={mutedClass}>Loading…</p>
  {:else if !shares.length}
    <p class={mutedClass}>No share links yet.</p>
  {:else}
    <ul class={listClass}>
      {#each shares as s (s.id)}
        <li class={rowClass}>
          <FileIcon entry={{ name: s.path, is_dir: s.is_dir }} class="size-5 shrink-0" />
          <div class="min-w-0 flex-1">
            <a href={s.is_dir ? folderHref(s.path) : fileHref(s.path)} class="block truncate font-medium hover:underline" title={s.path}>{name(s)}</a>
            <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
              {#if s.has_password}<LockIcon class="size-3" aria-label="Password protected" />{/if}
              <span class="truncate">{s.path} · {s.expires_at ? `expires ${when(s.expires_at)}` : 'never expires'}</span>
            </div>
          </div>
          <Button variant="ghost" size="icon" class="size-8" aria-label="Copy link to {name(s)}" onclick={() => copy(s)}><CopyIcon /></Button>
          <a href={link(s)} target="_blank" rel="noopener" class="grid size-8 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground" aria-label="Open link to {name(s)}"><ExternalLinkIcon class="size-4" /></a>
          <Button variant="ghost" size="icon" class="size-8 text-destructive hover:text-destructive" aria-label="Delete link to {name(s)}" onclick={() => del(s)}><TrashIcon /></Button>
        </li>
      {/each}
    </ul>
  {/if}
</div>
