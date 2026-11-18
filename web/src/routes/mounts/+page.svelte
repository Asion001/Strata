<script lang="ts">
  import { confirmDialog } from '$lib/dialogs.svelte';
  import { toast, toastError } from '$lib/toast.svelte';
  import { api } from '$lib/api';
  import { reloadMounts } from '$lib/mounts.svelte';

  type Mount = { id: number; path: string; mode: string; connection_id: number; connection_name: string; connection_kind: string; base_path: string };
  let mounts = $state<Mount[]>([]);
  let conns = $state<{ id: number; name: string; kind: string }[]>([]);
  let form = $state({ path: '', connection_id: 0, base_path: '', mode: 'raw' });
  let error = $state('');
  let msg = $state('');

  async function load() {
    try {
      [mounts, conns] = await Promise.all([api('GET', '/api/mounts'), api('GET', '/api/connections')]);
      if (!form.connection_id && conns.length) form.connection_id = conns[0].id;
    } catch (e) { error = (e as Error).message; }
  }
  $effect(() => { load(); });

  async function create(e: Event) {
    e.preventDefault();
    try {
      const m = await api('POST', '/api/mounts', form);
      if (m.overlaps?.length)
        toast(`Heads up: ${m.overlaps.map((p: string) => '/' + p).join(', ')} shows an overlapping folder of the same connection. Changes appear in both places.`, 'info', 12000);
      form.path = ''; form.base_path = ''; await load(); reloadMounts();
    }
    catch (err) { toastError(err); }
  }
  async function unmount(m: Mount) {
    const ok = await confirmDialog({ title: `Unmount “${m.path}”?`, body: 'Files stay on the backend; remounting the same folder brings them back.', confirm: 'Unmount' });
    if (!ok) return;
    try { await api('DELETE', `/api/mounts/${m.id}`); await load(); reloadMounts(); } catch (e) { toastError(e); }
  }
  async function scan(m: Mount) {
    await api('POST', `/api/mounts/${m.id}/scan`);
    msg = `Rescanning ${m.path} in the background…`;
  }
</script>

<div class="page">
  <h1>Mounts</h1>
  <p class="muted">Show any connection at any folder of your tree, even inside another mount.
    <b>raw</b> keeps the backend's folders as they are (changes made outside Strata are picked up by scanning);
    <b>hashed</b> stores deduplicated, content-addressed blobs and keeps the tree in the database.</p>
  {#if error}<p class="error">{error}</p>{/if}
  {#if msg}<p class="muted">{msg}</p>{/if}
  <table>
    <thead><tr><th>Folder</th><th>Connection</th><th>Mode</th><th></th></tr></thead>
    <tbody>
      {#each mounts as m}
        <tr>
          <td><a href="/?p={encodeURIComponent(m.path)}">/{m.path}</a></td>
          <td>{m.connection_name}<span class="muted">{m.base_path ? ` : /${m.base_path}` : ''}</span></td>
          <td><span class="badge">{m.mode}</span></td>
          <td class="right">
            {#if m.mode === 'raw'}<button onclick={() => scan(m)}>Rescan</button>{/if}
            {#if m.path}<button class="danger" onclick={() => unmount(m)}>Unmount</button>{:else}<span class="muted">your root</span>{/if}
          </td>
        </tr>
      {/each}
    </tbody>
  </table>

  <h2>New mount</h2>
  <form class="card stack" onsubmit={create}>
    <label>Folder in your tree <input bind:value={form.path} placeholder="e.g. Photos/NAS" required /></label>
    <label>Connection
      <select bind:value={form.connection_id}>
        {#each conns as c}<option value={c.id}>{c.name} ({c.kind})</option>{/each}
      </select>
    </label>
    <label>Folder inside the connection <input bind:value={form.base_path} placeholder="(top level)" /></label>
    <label>Mode
      <select bind:value={form.mode}>
        <option value="raw">raw: plain files and folders</option>
        <option value="hashed">hashed: deduplicated blob store</option>
      </select>
    </label>
    <div><button class="primary">Mount</button></div>
  </form>
</div>

<style>
  .right { text-align: right; white-space: nowrap; }
  td { vertical-align: middle; }
  @media (max-width: 600px) { th:nth-child(2), td:nth-child(2) { display: none; } }
</style>
