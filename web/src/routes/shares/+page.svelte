<script lang="ts">
  import { api } from '$lib/api';
  import { when } from '$lib/format';

  type Share = { id: number; token: string; path: string; is_dir: boolean; has_password: boolean; expires_at: string | null; created_at: string };
  let shares = $state<Share[]>([]);
  const load = async () => (shares = await api('GET', '/api/shares'));
  $effect(() => { load(); });
  const link = (s: Share) => `${location.origin}/s/${s.token}`;
  async function del(s: Share) {
    if (confirm('Delete this link? People who have it lose access.')) { await api('DELETE', `/api/shares/${s.id}`); await load(); }
  }
</script>

<div class="page">
  <h1>Share links</h1>
  <p class="muted">Create links from the ⋯ menu of any file or folder.</p>
  <table>
    <tbody>
      {#each shares as s}
        <tr>
          <td>{s.is_dir ? '📁' : '📄'} {s.path}
            <div class="muted small">{s.has_password ? '🔒 password · ' : ''}{s.expires_at ? `expires ${when(s.expires_at)}` : 'no expiry'}</div></td>
          <td class="right">
            <button onclick={() => navigator.clipboard.writeText(link(s))}>Copy link</button>
            <a href={link(s)} target="_blank" rel="noopener">Open</a>
            <button class="danger" onclick={() => del(s)}>Delete</button>
          </td>
        </tr>
      {:else}
        <tr><td class="muted">No share links yet.</td></tr>
      {/each}
    </tbody>
  </table>
</div>

<style>
  .small { font-size: .85em; }
  .right { text-align: right; white-space: nowrap; }
  .right > * { margin-left: 6px; }
</style>
