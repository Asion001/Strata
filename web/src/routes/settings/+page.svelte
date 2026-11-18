<script lang="ts">
  import { api } from '$lib/api';
  import { session } from '$lib/session.svelte';

  let old_password = $state('');
  let new_password = $state('');
  let msg = $state('');

  async function change(e: Event) {
    e.preventDefault();
    try { await api('POST', '/api/auth/password', { old_password, new_password }); msg = 'Password changed.'; old_password = new_password = ''; }
    catch (err) { msg = (err as Error).message; }
  }
  async function logout() {
    await api('POST', '/api/auth/logout');
    // Unsaved editor drafts hold file contents: don't leave them behind.
    const { markSignedOut, clearAllDrafts } = await import('$lib/editor/drafts');
    markSignedOut(); // stops autosave in every tab before the drafts go
    await clearAllDrafts();
    location.href = '/login';
  }
</script>

<div class="page">
  <h1>Settings</h1>
  <p>Signed in as <b>{session.user?.username}</b>. <button onclick={logout}>Sign out</button></p>
  <h2>Change password</h2>
  <form class="card stack" onsubmit={change}>
    <label>Current password <input type="password" bind:value={old_password} autocomplete="current-password" required /></label>
    <label>New password <input type="password" bind:value={new_password} minlength="8" autocomplete="new-password" required /></label>
    {#if msg}<div class="muted">{msg}</div>{/if}
    <div><button class="primary">Change password</button></div>
  </form>
  <h2>Install</h2>
  <p class="muted">On iPhone or iPad: open this site in Safari, tap Share, then “Add to Home Screen”. On desktop Chrome or Edge: use the install icon in the address bar.</p>
</div>
