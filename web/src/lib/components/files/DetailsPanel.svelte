<script lang="ts">
  // The right-hand details panel for one file or folder: what it is, where it
  // lives, and its share links. Loaded on first use.
  import { api, thumbUrl, type Entry, type Schemas } from '$lib/api';
  import { bytes, isImage, kindOf, when } from '$lib/format';
  import { cn } from '$lib/utils';
  import { alertDialog, confirmDialog } from '$lib/dialogs.svelte';
  import { toast, toastError } from '$lib/toast.svelte';
  import Button from '$lib/components/ui/button/button.svelte';
  import FileIcon from './FileIcon.svelte';
  import XIcon from '@lucide/svelte/icons/x';
  import LinkIcon from '@lucide/svelte/icons/link';
  import LockIcon from '@lucide/svelte/icons/lock';
  import CopyIcon from '@lucide/svelte/icons/copy';
  import TrashIcon from '@lucide/svelte/icons/trash';

  type Share = Schemas['Share'];

  let {
    entry,
    path,
    storage,
    onclose,
    onshare
  }: {
    entry: Entry;
    /** Full path of the entry. */
    path: string;
    /** Where the folder's files are stored, e.g. "raw · photos-bucket". */
    storage?: string;
    onclose: () => void;
    /** Asks for and creates a share link; resolves once it's done or cancelled. */
    onshare: () => Promise<unknown>;
  } = $props();

  let tab = $state<'details' | 'sharing'>('details');
  let shares = $state<Share[] | null>(null);
  let seq = 0;

  async function loadShares() {
    const my = ++seq;
    const p = path;
    try {
      const all: Share[] = await api('GET', '/api/shares');
      if (my === seq) shares = all.filter((s) => s.path === p);
    } catch (e) {
      if (my === seq) shares = [];
      toastError(e);
    }
  }
  // Fresh links whenever the panel shows another entry.
  $effect(() => {
    void path;
    shares = null;
    loadShares();
  });

  const KIND_LABEL: Record<string, string> = {
    mount: 'Mount', dir: 'Folder', image: 'Image', video: 'Video', audio: 'Audio',
    archive: 'Archive', pdf: 'PDF document', text: 'Text', file: 'File'
  };
  const kind = $derived(kindOf(entry));
  const location = $derived(path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : 'Home');
  const link = (s: Share) => `${window.location.origin}/s/${s.token}`;

  async function copy(s: Share) {
    try {
      await navigator.clipboard.writeText(link(s));
      toast('Link copied.', 'success');
    } catch {
      await alertDialog({ title: 'Share link', copy: link(s), ok: 'Done' });
    }
  }
  async function del(s: Share) {
    const ok = await confirmDialog({ title: 'Delete this link?', body: 'People who have it lose access.', confirm: 'Delete link', danger: true });
    if (!ok) return;
    try {
      await api('DELETE', `/api/shares/${s.id}`);
    } catch (e) {
      toastError(e);
    }
    await loadShares();
  }
  async function create() {
    await onshare();
    await loadShares();
  }

  const tabClass = (on: boolean) =>
    cn('-mb-px border-b-2 px-1 pb-2 text-sm font-medium transition-colors', on ? 'border-primary text-foreground' : 'border-transparent text-muted-foreground hover:text-foreground');
</script>

<aside aria-label="Details for {entry.name}" class="flex h-full flex-col bg-background">
  <div class="flex items-start gap-3 border-b border-border p-4">
    <div class="grid size-12 shrink-0 place-items-center overflow-hidden rounded-lg bg-muted">
      {#if isImage(entry.name) && !entry.is_dir}
        <img src={thumbUrl(path, entry.etag, 96)} alt="" class="size-full object-cover" />
      {:else}
        <FileIcon {entry} class="size-6" />
      {/if}
    </div>
    <div class="min-w-0 flex-1">
      <h2 class="truncate font-semibold" title={entry.name}>{entry.name}</h2>
      <p class="text-sm text-muted-foreground">{KIND_LABEL[kind]}{entry.is_dir ? '' : ` · ${bytes(entry.size)}`}</p>
    </div>
    <Button variant="ghost" size="icon" class="-mt-1 -mr-2 size-8" aria-label="Close details" onclick={onclose}><XIcon /></Button>
  </div>

  <div class="flex gap-4 border-b border-border px-4 pt-3" role="tablist" aria-label="Details sections">
    <button type="button" role="tab" aria-selected={tab === 'details'} class={tabClass(tab === 'details')} onclick={() => (tab = 'details')}>Details</button>
    <button type="button" role="tab" aria-selected={tab === 'sharing'} class={tabClass(tab === 'sharing')} onclick={() => (tab = 'sharing')}>
      Sharing{#if shares?.length}<span class="ml-1.5 rounded-full bg-accent px-1.5 text-xs">{shares.length}</span>{/if}
    </button>
  </div>

  <div class="min-h-0 flex-1 overflow-y-auto p-4" role="tabpanel">
    {#if tab === 'details'}
      <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-3 text-sm">
        <dt class="text-muted-foreground">Type</dt><dd>{KIND_LABEL[kind]}</dd>
        {#if !entry.is_dir}<dt class="text-muted-foreground">Size</dt><dd>{bytes(entry.size)}</dd>{/if}
        <dt class="text-muted-foreground">Modified</dt><dd title={entry.mtime}>{when(entry.mtime)}</dd>
        <dt class="text-muted-foreground">Location</dt><dd class="break-all">{location}</dd>
        {#if storage}<dt class="text-muted-foreground">Storage</dt><dd>{storage}</dd>{/if}
      </dl>
    {:else}
      <Button variant="outline" class="w-full" onclick={create}><LinkIcon />Create share link</Button>
      {#if shares === null}
        <p class="mt-4 text-sm text-muted-foreground">Loading…</p>
      {:else if !shares.length}
        <p class="mt-4 text-sm text-muted-foreground">No share links yet. Anyone with a link can open {entry.is_dir ? 'this folder' : 'this file'}.</p>
      {:else}
        <ul class="mt-4 grid gap-2">
          {#each shares as s (s.id)}
            <li class="flex items-center gap-2 rounded-lg border border-border p-2.5 text-sm">
              <div class="min-w-0 flex-1">
                <div class="flex items-center gap-1.5 font-medium">
                  {#if s.has_password}<LockIcon class="size-3.5" aria-label="Password protected" />{/if}
                  <span class="truncate">…/s/{s.token.slice(0, 8)}</span>
                </div>
                <div class="text-xs text-muted-foreground">{s.expires_at ? `Expires ${when(s.expires_at)}` : 'Never expires'} · created {when(s.created_at)}</div>
              </div>
              <Button variant="ghost" size="icon" class="size-8" aria-label="Copy link" onclick={() => copy(s)}><CopyIcon /></Button>
              <Button variant="ghost" size="icon" class="size-8 text-destructive hover:text-destructive" aria-label="Delete link" onclick={() => del(s)}><TrashIcon /></Button>
            </li>
          {/each}
        </ul>
      {/if}
    {/if}
  </div>
</aside>
