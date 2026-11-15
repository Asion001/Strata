<script lang="ts">
  import { api } from '$lib/api';
  import { session } from '$lib/session.svelte';

  type Conn = {
    id: number; name: string; kind: string; config: Record<string, any>; capabilities: Record<string, any>;
    is_default: boolean; health: string; health_error: string | null; health_checked_at: string | null;
  };
  let conns = $state<Conn[]>([]);
  let error = $state('');
  let editing = $state<{ id?: number; name: string; config: Record<string, any> } | null>(null);
  let testResult = $state('');
  let busy = $state(false);

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
    } catch (err) { alert((err as Error).message); } finally { busy = false; }
  }
  async function test() {
    if (!editing) return;
    testResult = 'testing…';
    try {
      const r = await api('POST', '/api/connections/test', { config: clean(editing.config), id: editing.id });
      testResult = r.ok ? '✓ reachable' : `✗ ${r.error}`;
    } catch (err) { testResult = `✗ ${(err as Error).message}`; }
  }
  async function probe(c: Conn) { await api('POST', `/api/connections/${c.id}/probe`); await load(); }
  async function del(c: Conn) {
    if (!confirm(`Delete connection "${c.name}"? Unmounted hashed volumes on it lose their file index.`)) return;
    try { await api('DELETE', `/api/connections/${c.id}`); await load(); } catch (e) { alert((e as Error).message); }
  }
  const fields: Record<string, [string, string, string?][]> = {
    local: [['path', 'Folder (relative to the server storage base)']],
    s3: [['bucket', 'Bucket'], ['endpoint', 'Endpoint (empty for AWS)'], ['region', 'Region'], ['access_key_id', 'Access key ID'], ['secret_access_key', 'Secret access key', 'password'], ['root', 'Folder inside the bucket']],
    ftp: [['endpoint', 'Endpoint (ftp://host:21 or ftps://host)'], ['user', 'User'], ['password', 'Password', 'password'], ['root', 'Folder']]
  };
</script>

<div class="page">
  <div class="row"><h1 class="grow">Connections</h1>
    <button class="primary" onclick={() => { testResult = ''; editing = { name: '', config: blank(session.user?.is_admin ? 'local' : 's3') }; }}>Add connection</button></div>
  <p class="muted">A connection is a storage backend with its credentials (encrypted at rest). Mount it anywhere in your tree from <a href="/mounts">Mounts</a>.</p>
  {#if error}<p class="error">{error}</p>{/if}
  <div class="stack">
    {#each conns as c}
      <div class="card row">
        <div class="grow">
          <b>{c.name}</b> <span class="badge">{c.kind}</span>
          {#if c.is_default}<span class="badge">server default</span>{/if}
          <span class="badge {c.health === 'ok' ? 'ok' : c.health === 'error' ? 'err' : ''}">{c.health}</span>
          {#if c.capabilities?.snapshots && c.capabilities.snapshots !== 'none'}<span class="badge">snapshots: {c.capabilities.snapshots}</span>{/if}
          <div class="muted small">
            {c.kind === 'local' ? `/${c.config.path}` : c.kind === 's3' ? `${c.config.endpoint ?? 'aws'} · ${c.config.bucket}${c.config.root ?? ''}` : c.config.endpoint}
            {#if c.capabilities?.fs_type} · {c.capabilities.fs_type}{/if}
          </div>
          {#if c.health_error}<div class="error small">{c.health_error}</div>{/if}
        </div>
        <button onclick={() => probe(c)}>Check</button>
        <button onclick={() => { testResult = ''; editing = { id: c.id, name: c.name, config: { ...blank(c.kind), ...Object.fromEntries(Object.entries(c.config).map(([k, v]) => [k, v ?? ''])) } }; }}>Edit</button>
        {#if !c.is_default}<button class="danger" onclick={() => del(c)}>Delete</button>{/if}
      </div>
    {/each}
  </div>
</div>

{#if editing}
  <div class="modal-bg" role="presentation" onclick={() => (editing = null)}>
    <form class="card stack modal" onsubmit={save} onclick={(e) => e.stopPropagation()} onkeydown={() => {}} tabindex="-1">
      <h2>{editing.id ? 'Edit' : 'New'} connection</h2>
      <label>Name <input bind:value={editing.name} required /></label>
      {#if !editing.id}
        <label>Type
          <select value={editing.config.kind} onchange={(e) => (editing!.config = blank(e.currentTarget.value))}>
            {#if session.user?.is_admin}<option value="local">Local folder on the server</option>{/if}
            <option value="s3">S3 / S3-compatible</option>
            <option value="ftp">FTP / FTPS</option>
          </select>
        </label>
      {/if}
      {#each fields[editing.config.kind] as [k, label, type]}
        <label>{label} <input type={type ?? 'text'} bind:value={editing.config[k]}
          placeholder={editing.id && type === 'password' ? 'unchanged' : ''} autocomplete="off" /></label>
      {/each}
      <div class="row"><button type="button" onclick={test}>Test</button><span class="muted grow">{testResult}</span>
        <button type="button" onclick={() => (editing = null)}>Cancel</button><button class="primary" disabled={busy}>Save</button></div>
    </form>
  </div>
{/if}

<style>
  .small { font-size: .85em; margin-top: 4px; }
  .modal-bg { position: fixed; inset: 0; background: rgb(0 0 0 / .4); display: grid; place-items: center; z-index: 30; padding: 16px; }
  .modal { width: min(480px, 100%); max-height: 90dvh; overflow: auto; }
  h2 { margin: 0; }
</style>
