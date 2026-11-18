<script lang="ts">
  import { confirmDialog } from '$lib/dialogs.svelte';
  import { toastError } from '$lib/toast.svelte';
  import { api } from '$lib/api';
  import { session } from '$lib/session.svelte';
  import { cn } from '$lib/utils';
  import Button from '$lib/components/ui/button/button.svelte';
  import Input from '$lib/components/ui/input/input.svelte';
  import { kindIcon } from '$lib/components/shell/nav';
  import { badgeClass, cardClass, h1Class, labelClass, mutedClass, pageClass, selectClass } from '$lib/components/ui/styles';
  import PlusIcon from '@lucide/svelte/icons/plus';

  type Conn = {
    id: number; name: string; kind: string; config: Record<string, any>; capabilities: Record<string, any>;
    is_default: boolean; health: string; health_error: string | null; health_checked_at: string | null;
  };
  let conns = $state<Conn[]>([]);
  let error = $state('');
  let editing = $state<{ id?: number; name: string; config: Record<string, any> } | null>(null);
  let testResult = $state('');
  let busy = $state(false);
  // The editor is a native modal <dialog>, like the app's other dialogs.
  let dlg = $state<HTMLDialogElement | null>(null);
  $effect(() => {
    if (!dlg) return;
    if (editing && !dlg.open) dlg.showModal();
    else if (!editing && dlg.open) dlg.close();
  });
  const edit = (c?: Conn) => {
    testResult = '';
    editing = c
      ? { id: c.id, name: c.name, config: { ...blank(c.kind), ...Object.fromEntries(Object.entries(c.config).map(([k, v]) => [k, v ?? ''])) } }
      : { name: '', config: blank(session.user?.is_admin ? 'local' : 's3') };
  };

  const blank = (kind: string): Record<string, any> =>
    kind === 'local' ? { kind, path: '' }
    : kind === 's3' ? { kind, bucket: '', region: '', endpoint: '', access_key_id: '', secret_access_key: '', root: '' }
    : { kind, endpoint: 'ftp://', user: '', password: '', root: '' };

  async function load() {
    try { conns = await api('GET', '/api/connections'); } catch (e) { error = (e as Error).message; }
  }
  $effect(() => { load(); });

  function clean(c: Record<string, any>) {
    const o: Record<string, any> = {};
    for (const [k, v] of Object.entries(c)) o[k] = v === '' && k !== 'path' && k !== 'secret_access_key' && k !== 'password' ? null : v;
    if (o.kind === 's3') { o.bucket ??= ''; o.access_key_id ??= ''; }
    if (o.kind === 'ftp') o.endpoint ??= '';
    return o;
  }

  async function save(e: Event) {
    e.preventDefault();
    if (!editing) return;
    busy = true;
    try {
      const body = { name: editing.name, config: clean(editing.config) };
      if (editing.id) await api('PUT', `/api/connections/${editing.id}`, body);
      else await api('POST', '/api/connections', body);
      editing = null;
      await load();
    } catch (err) { toastError(err); } finally { busy = false; }
  }
  async function test() {
    if (!editing) return;
    testResult = 'testing…';
    try {
      const r = await api('POST', '/api/connections/test', { config: clean(editing.config), id: editing.id });
      testResult = r.ok ? '✓ reachable' : `✗ ${r.error}`;
    } catch (err) { testResult = `✗ ${(err as Error).message}`; }
  }
  async function probe(c: Conn) {
    try { await api('POST', `/api/connections/${c.id}/probe`); } catch (e) { toastError(e); }
    await load();
  }
  const health = (h: string) => cn(badgeClass, h === 'ok' && 'border-transparent bg-emerald-500/15 text-emerald-700 dark:text-emerald-400', h === 'error' && 'border-transparent bg-destructive/15 text-destructive');
  async function del(c: Conn) {
    const ok = await confirmDialog({ title: `Delete connection “${c.name}”?`, body: 'Unmounted hashed volumes on it lose their file index.', confirm: 'Delete', danger: true });
    if (!ok) return;
    try { await api('DELETE', `/api/connections/${c.id}`); await load(); } catch (e) { toastError(e); }
  }
  const fields: Record<string, [string, string, string?][]> = {
    local: [['path', 'Folder (relative to the server storage base)']],
    s3: [['bucket', 'Bucket'], ['endpoint', 'Endpoint (empty for AWS)'], ['region', 'Region'], ['access_key_id', 'Access key ID'], ['secret_access_key', 'Secret access key', 'password'], ['root', 'Folder inside the bucket']],
    ftp: [['endpoint', 'Endpoint (ftp://host:21 or ftps://host)'], ['user', 'User'], ['password', 'Password', 'password'], ['root', 'Folder']]
  };
</script>

<div class={pageClass}>
  <div class="flex items-center gap-3">
    <h1 class={cn(h1Class, 'flex-1')}>Connections</h1>
    <Button onclick={() => edit()}><PlusIcon />Add connection</Button>
  </div>
  <p class={cn(mutedClass, 'mt-1 mb-6')}>A connection is a storage backend with its credentials (encrypted at rest). Mount it anywhere in your tree from <a href="/mounts" class="underline underline-offset-2 hover:text-foreground">Mounts</a>.</p>
  {#if error}<p class="mb-4 text-sm text-destructive">{error}</p>{/if}
  <div class="grid gap-3">
    {#each conns as c (c.id)}
      {@const Icon = kindIcon(c.kind)}
      <div class={cn(cardClass, 'flex flex-wrap items-start gap-3 p-4')}>
        <Icon class="mt-0.5 size-5 shrink-0 text-muted-foreground" />
        <div class="min-w-0 flex-1">
          <div class="flex flex-wrap items-center gap-1.5">
            <b class="mr-1 font-medium">{c.name}</b>
            <span class={badgeClass}>{c.kind}</span>
            <span class={health(c.health)}>{c.health}</span>
            {#if c.is_default}<span class={badgeClass}>server default</span>{/if}
            {#if c.capabilities?.read_only}<span class={badgeClass}>read-only</span>{/if}
            {#if c.capabilities?.snapshots && c.capabilities.snapshots !== 'none'}<span class={badgeClass}>snapshots: {c.capabilities.snapshots}</span>{/if}
          </div>
          <div class="mt-1 truncate text-xs text-muted-foreground">
            {c.kind === 'local' ? `/${c.config.path}` : c.kind === 's3' ? `${c.config.endpoint ?? 'aws'} · ${c.config.bucket}${c.config.root ?? ''}` : c.config.endpoint}
            {#if c.capabilities?.fs_type} · {c.capabilities.fs_type}{/if}
          </div>
          {#if c.health_error}<div class="mt-1 text-xs text-destructive">{c.health_error}</div>{/if}
        </div>
        <div class="flex gap-1">
          <Button variant="ghost" size="sm" onclick={() => probe(c)}>Check</Button>
          <Button variant="ghost" size="sm" onclick={() => edit(c)}>Edit</Button>
          {#if !c.is_default}<Button variant="ghost" size="sm" class="text-destructive hover:text-destructive" onclick={() => del(c)}>Delete</Button>{/if}
        </div>
      </div>
    {/each}
  </div>
</div>

<!-- Escape and the backdrop cancel; keys stop here so page shortcuts don't run. -->
<dialog
  bind:this={dlg}
  aria-labelledby="conn-title"
  class="m-auto max-h-[90dvh] w-[calc(100%-2rem)] max-w-md overflow-y-auto rounded-xl border border-border bg-popover p-0 text-popover-foreground shadow-lg backdrop:bg-black/50"
  oncancel={(e) => { e.preventDefault(); editing = null; }}
  onclose={() => (editing = null)}
  onkeydown={(e) => e.stopPropagation()}
  onclick={(e) => { if (e.target === dlg) editing = null; }}
>
  {#if editing}
    <form class="grid gap-4 p-6" onsubmit={save}>
      <h2 id="conn-title" class="text-lg font-semibold">{editing.id ? 'Edit' : 'New'} connection</h2>
      <label class={labelClass}>Name <Input bind:value={editing.name} required /></label>
      {#if !editing.id}
        <label class={labelClass}>Type
          <select class={selectClass} value={editing.config.kind} onchange={(e) => (editing!.config = blank(e.currentTarget.value))}>
            {#if session.user?.is_admin}<option value="local">Local folder on the server</option>{/if}
            <option value="s3">S3 / S3-compatible</option>
            <option value="ftp">FTP / FTPS</option>
          </select>
        </label>
      {/if}
      {#each fields[editing.config.kind] as [k, label, type] (k)}
        <label class={labelClass}>{label} <Input type={type ?? 'text'} bind:value={editing.config[k]}
          placeholder={editing.id && type === 'password' ? 'unchanged' : ''} autocomplete="off" /></label>
      {/each}
      <div class="flex flex-wrap items-center gap-2">
        <Button variant="outline" onclick={test}>Test</Button>
        <span class="min-w-0 flex-1 text-sm text-muted-foreground" aria-live="polite">{testResult}</span>
        <Button variant="ghost" onclick={() => (editing = null)}>Cancel</Button>
        <Button type="submit" disabled={busy}>Save</Button>
      </div>
    </form>
  {/if}
</dialog>
