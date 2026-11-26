<script lang="ts">
  import { confirmDialog } from '$lib/dialogs.svelte';
  import { toast, toastError } from '$lib/toast.svelte';
  import { api, ApiError, type Schemas } from '$lib/api';
  import { session } from '$lib/session.svelte';
  import { cn } from '$lib/utils';
  import Button from '$lib/components/ui/button/button.svelte';
  import Input from '$lib/components/ui/input/input.svelte';
  import { kindIcon } from '$lib/components/shell/nav';
  import { badgeClass, cardClass, h1Class, labelClass, mutedClass, pageClass, selectClass } from '$lib/components/ui/styles';
  import PlusIcon from '@lucide/svelte/icons/plus';

  // (The form edits the config loosely, field by field.)
  type Conn = Omit<Schemas['ConnectionView'], 'config' | 'capabilities'> & { config: Record<string, any>; capabilities: Record<string, any> };
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
      testResult = r.ok ? (r.note ? `✓ ${r.note}` : '✓ reachable') : `✗ ${r.error ?? r.reason ?? 'failed'}`;
    } catch (err) { testResult = `✗ ${(err as Error).message}`; }
  }
  async function probe(c: Conn) {
    try { await api('POST', `/api/connections/${c.id}/probe`); } catch (e) { toastError(e); }
    await load();
  }
  const health = (h: string) => cn(badgeClass, h === 'ok' && 'border-transparent bg-emerald-500/15 text-emerald-700 dark:text-emerald-400', h === 'degraded' && 'border-transparent bg-amber-500/15 text-amber-700 dark:text-amber-400', h === 'offline' && 'border-transparent bg-destructive/15 text-destructive');
  // Snapshot support, from either the current or the older capability shape.
  const snapshots = (c: Conn): { kind: string; create: boolean } | null => {
    const s = c.capabilities?.snapshots;
    const kind = typeof s === 'string' ? s : s?.kind;
    return kind && kind !== 'none' ? { kind, create: typeof s === 'object' ? !!s.create : false } : null;
  };

  // Live: health changes are pushed over the websocket.
  $effect(() => {
    let ws: WebSocket | null = null;
    let closed = false;
    const connect = () => {
      ws = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/ws`);
      ws.onmessage = (m) => {
        try { if ('health' in JSON.parse(m.data)) load(); } catch {}
      };
      ws.onclose = () => { if (!closed) setTimeout(connect, 3000); };
    };
    connect();
    return () => { closed = true; ws?.close(); };
  });
  async function del(c: Conn) {
    const ok = await confirmDialog({ title: `Delete connection “${c.name}”?`, body: 'Unmounted hashed volumes on it lose their file index.', confirm: 'Delete', danger: true });
    if (!ok) return;
    try {
      await api('DELETE', `/api/connections/${c.id}`);
    } catch (e) {
      // Deleted items from it are still in someone's trash: say how many and
      // ask again before removing them for good.
      if (!(e instanceof ApiError && e.status === 409 && e.message.includes('purge_trash'))) return toastError(e);
      const n = e.message.match(/^\d+/)?.[0] ?? 'Some';
      const purge = await confirmDialog({
        title: 'Remove deleted items for good?',
        body: `${n} deleted item(s) from this connection are still in the trash (yours or other users’). Deleting the connection removes them permanently.`,
        confirm: 'Delete connection and items',
        danger: true
      });
      if (!purge) return;
      let r: Schemas['ConnectionDeletedBody'];
      try { r = await api('DELETE', `/api/connections/${c.id}?purge_trash=true`); } catch (e2) { return toastError(e2); }
      // (Its storage couldn't be reached: those files are still on it.)
      const left = r?.left_on_storage ?? 0;
      if (left > 0) toast(`“${c.name}” is deleted, but ${left} deleted item(s) couldn’t be removed from its storage because it can’t be reached. They stay in its .strata-trash folder; remove that folder by hand when the storage is back.`, 'error', 15000);
    }
    await load();
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
            <span class={health(c.health)} title={c.health_checked_at ? `checked ${new Date(c.health_checked_at).toLocaleString()}` : 'not checked yet'}>{c.health}</span>
            {#if c.health_latency_ms != null && c.health !== 'offline'}<span class="text-xs text-muted-foreground">{c.health_latency_ms} ms</span>{/if}
            {#if c.is_default}<span class={badgeClass}>server default</span>{/if}
            {#if c.capabilities?.read_only}<span class={badgeClass}>read-only</span>{/if}
            {#if snapshots(c)}{@const s = snapshots(c)!}<span class={badgeClass}>snapshots: {s.kind}{s.create ? '' : ' (read-only)'}</span>{/if}
          </div>
          <div class="mt-1 truncate text-xs text-muted-foreground">
            {c.kind === 'local' ? `/${c.config.path}` : c.kind === 's3' ? `${c.config.endpoint ?? 'aws'} · ${c.config.bucket}${c.config.root ?? ''}` : c.config.endpoint}
            {#if c.capabilities?.fs_type} · {c.capabilities.fs_type}{/if}
          </div>
          {#if c.health !== 'ok' && (c.health_reason || c.health_error)}
            <div class="mt-1 text-xs break-words text-destructive">{c.health_reason ?? ''}{#if c.health_error}: {c.health_error}{/if}</div>
          {/if}
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
