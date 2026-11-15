<script lang="ts">
  import { api, type User } from '$lib/api';
  import { session } from '$lib/session.svelte';

  let users = $state<User[]>([]);
  let form = $state({ username: '', password: '', is_admin: false });
  const load = async () => (users = await api('GET', '/api/admin/users'));
  $effect(() => { load(); });

  async function create(e: Event) {
    e.preventDefault();
    try { await api('POST', '/api/admin/users', form); form = { username: '', password: '', is_admin: false }; await load(); }
    catch (err) { alert((err as Error).message); }
  }
  async function reset(u: User) {
    const pw = prompt(`New password for ${u.username} (8+ characters)`);
    if (pw) try { await api('POST', `/api/admin/users/${u.id}/password`, { password: pw }); alert('Password changed'); } catch (e) { alert((e as Error).message); }
  }
  async function del(u: User) {
    if (confirm(`Delete ${u.username}? Their mounts are removed; stored data stays on disk.`))
      try { await api('DELETE', `/api/admin/users/${u.id}`); await load(); } catch (e) { alert((e as Error).message); }
  }
</script>

<div class="page">
  <h1>Users</h1>
  <table>
    <tbody>
      {#each users as u}
        <tr>
          <td>{u.username} {#if u.is_admin}<span class="badge">admin</span>{/if}</td>
          <td class="right">
            <button onclick={() => reset(u)}>Reset password</button>
            {#if u.id !== session.user?.id}<button class="danger" onclick={() => del(u)}>Delete</button>{/if}
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
  <h2>Add user</h2>
  <form class="card stack" onsubmit={create}>
    <label>Username <input bind:value={form.username} autocapitalize="none" required /></label>
    <label>Password <input type="password" bind:value={form.password} minlength="8" autocomplete="new-password" required /></label>
    <label class="check"><input type="checkbox" bind:checked={form.is_admin} /> Administrator</label>
    <div><button class="primary">Create user</button></div>
  </form>
</div>

<style>
  .right { text-align: right; }
  .check { display: flex; flex-direction: row; align-items: center; gap: 8px; }
  .check input { width: auto; }
</style>
