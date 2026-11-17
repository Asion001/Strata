<script lang="ts">
  import { page } from '$app/state';
  import { api } from '$lib/api';
  import { session } from '$lib/session.svelte';

  let username = $state('');
  let password = $state('');
  let error = $state('');
  let busy = $state(false);

  async function submit(e: Event) {
    e.preventDefault();
    busy = true;
    error = '';
    try {
      const r = await api('POST', '/api/auth/login', { username, password });
      session.user = r.user;
      session.checked = true;
      // Unsaved drafts of anyone else who used this browser go away.
      const { keepOnlyUser } = await import('$lib/editor/drafts');
      await keepOnlyUser(r.user.id).catch(() => {});
      const next = page.url.searchParams.get('next');
      location.href = next && next.startsWith('/') && !next.startsWith('//') ? next : '/';
    } catch (err) {
      error = (err as Error).message;
    } finally {
      busy = false;
    }
  }
</script>

<div class="wrap">
  <form class="card stack" onsubmit={submit}>
    <div class="logo"><img src="/icons/icon-192.png" alt="" width="56" height="56" /><h1>Strata</h1></div>
    <label>Username <input bind:value={username} autocomplete="username" autocapitalize="none" required /></label>
    <label>Password <input type="password" bind:value={password} autocomplete="current-password" required /></label>
    {#if error}<div class="error">{error}</div>{/if}
    <button class="primary" disabled={busy}>{busy ? 'Signing in…' : 'Sign in'}</button>
  </form>
</div>

<style>
  .wrap { min-height: 100dvh; display: grid; place-items: center; padding: 16px; }
  form { width: min(360px, 100%); }
  .logo { display: flex; align-items: center; gap: 12px; }
  .logo h1 { margin: 0; }
  .logo img { border-radius: 12px; }
</style>
