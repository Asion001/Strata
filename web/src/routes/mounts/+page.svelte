<script lang="ts">
  import { confirmDialog } from '$lib/dialogs.svelte';
  import { toast, toastError } from '$lib/toast.svelte';
  import { api } from '$lib/api';
  import { reloadMounts } from '$lib/mounts.svelte';
  import { cn } from '$lib/utils';
  import Button from '$lib/components/ui/button/button.svelte';
  import Input from '$lib/components/ui/input/input.svelte';
  import { folderHref, kindIcon } from '$lib/components/shell/nav';
  import { badgeClass, cardClass, h1Class, h2Class, labelClass, listClass, mutedClass, pageClass, rowClass, selectClass } from '$lib/components/ui/styles';

  type Mount = { id: number; path: string; mode: string; connection_id: number; connection_name: string; connection_kind: string; base_path: string };
  let mounts = $state<Mount[]>([]);
  let conns = $state<{ id: number; name: string; kind: string; capabilities?: { read_only?: boolean } }[]>([]);
  let form = $state({ path: '', connection_id: 0, base_path: '', mode: 'raw' });
  let readOnly = $derived(!!conns.find((c) => c.id === form.connection_id)?.capabilities?.read_only);
  $effect(() => { if (readOnly && form.mode === 'hashed') form.mode = 'raw'; });
  let error = $state('');
  let busy = $state(false);

  async function load() {
    try {
      [mounts, conns] = await Promise.all([api('GET', '/api/mounts'), api('GET', '/api/connections')]);
      if (!form.connection_id && conns.length) form.connection_id = conns[0].id;
      error = '';
    } catch (e) { error = (e as Error).message; }
  }
  $effect(() => { load(); });

  async function create(e: Event) {
    e.preventDefault();
    busy = true;
    try {
      const m = await api('POST', '/api/mounts', form);
      if (m.overlaps?.length)
        toast(`Heads up: ${m.overlaps.map((p: string) => '/' + p).join(', ')} shows an overlapping folder of the same connection. Changes appear in both places.`, 'info', 12000);
      else toast(`Mounted at /${form.path}.`, 'success');
      form.path = ''; form.base_path = ''; await load(); reloadMounts();
    } catch (err) { toastError(err); } finally { busy = false; }
  }
  async function unmount(m: Mount) {
    const ok = await confirmDialog({ title: `Unmount “${m.path}”?`, body: 'Files stay on the backend; remounting the same folder brings them back.', confirm: 'Unmount' });
    if (!ok) return;
    try { await api('DELETE', `/api/mounts/${m.id}`); await load(); reloadMounts(); } catch (e) { toastError(e); }
  }
  async function scan(m: Mount) {
    try { await api('POST', `/api/mounts/${m.id}/scan`); toast(`Rescanning /${m.path} in the background.`); } catch (e) { toastError(e); }
  }
</script>

<div class={pageClass}>
  <h1 class={h1Class}>Mounts</h1>
  <p class={cn(mutedClass, 'mt-1 mb-6')}>Show any connection at any folder of your tree, even inside another mount.
    <b class="font-medium text-foreground">raw</b> keeps the backend's folders as they are (changes made outside Strata are picked up by scanning);
    <b class="font-medium text-foreground">hashed</b> stores deduplicated, content-addressed blobs and keeps the tree in the database.</p>
  {#if error}<p class="mb-4 text-sm text-destructive">{error}</p>{/if}

  <ul class={listClass}>
    {#each mounts as m (m.id)}
      {@const Icon = kindIcon(m.connection_kind)}
      <li class={cn(rowClass, 'flex-wrap')}>
        <Icon class="size-5 shrink-0 text-muted-foreground" />
        <div class="min-w-0 flex-1 basis-40">
          <a href={folderHref(m.path)} class="block truncate font-medium hover:underline">/{m.path}</a>
          <div class="truncate text-xs text-muted-foreground">{m.connection_name}{m.base_path ? ` : /${m.base_path}` : ''}</div>
        </div>
        <span class={badgeClass}>{m.mode}</span>
        {#if m.mode === 'raw'}<Button variant="ghost" size="sm" onclick={() => scan(m)}>Rescan</Button>{/if}
        {#if m.path}
          <Button variant="ghost" size="sm" class="text-destructive hover:text-destructive" onclick={() => unmount(m)}>Unmount</Button>
        {:else}
          <span class="px-3 text-xs text-muted-foreground">your root</span>
        {/if}
      </li>
    {/each}
  </ul>

  <h2 class={h2Class}>New mount</h2>
  <form class={cn(cardClass, 'grid gap-4 p-4')} onsubmit={create}>
    <label class={labelClass}>Folder in your tree <Input bind:value={form.path} placeholder="e.g. Photos/NAS" required /></label>
    <label class={labelClass}>Connection
      <select bind:value={form.connection_id} class={selectClass}>
        {#each conns as c (c.id)}<option value={c.id}>{c.name} ({c.kind}{c.capabilities?.read_only ? ', read-only' : ''})</option>{/each}
      </select>
    </label>
    <label class={labelClass}>Folder inside the connection <Input bind:value={form.base_path} placeholder="(top level)" /></label>
    <label class={labelClass}>Mode
      <select bind:value={form.mode} class={selectClass}>
        <option value="raw">raw: plain files and folders</option>
        <option value="hashed" disabled={readOnly}>hashed: deduplicated blob store{readOnly ? ' (needs a writable connection)' : ''}</option>
      </select>
    </label>
    <div><Button type="submit" disabled={busy}>Mount</Button></div>
  </form>
</div>
