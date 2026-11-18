<script lang="ts">
  import { confirmDialog, promptDialog } from '$lib/dialogs.svelte';
  import { toast, toastError } from '$lib/toast.svelte';
  import { api, type User } from '$lib/api';
  import { session } from '$lib/session.svelte';

  let users = $state<User[]>([]);
  let form = $state({ username: '', password: '', is_admin: false });
  const load = async () => (users = await api('GET', '/api/admin/users'));
  $effect(() => { load(); });

  async function create(e: Event) {
    e.preventDefault();
    try { await api('POST', '/api/admin/users', form); form = { username: '', password: '', is_admin: false }; await load(); }
    catch (err) { toastError(err); }
  }
  async function reset(u: User) {
    const pw = await promptDialog({ title: `New password for ${u.username}`, label: 'Password', type: 'password', confirm: 'Change password', body: 'At least 8 characters.' });
    if (pw) try { await api('POST', `/api/admin/users/${u.id}/password`, { password: pw }); toast('Password changed', 'success'); } catch (e) { toastError(e); }
  }
  async function del(u: User) {
    const ok = await confirmDialog({ title: `Delete ${u.username}?`, body: 'Their mounts are removed; stored data stays on disk.', confirm: 'Delete user', danger: true });
    if (ok) try { await api('DELETE', `/api/admin/users/${u.id}`); await load(); } catch (e) { toastError(e); }
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
