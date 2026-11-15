<script lang="ts">
  import '../app.css';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { api } from '$lib/api';
  import { session } from '$lib/session.svelte';

  let { children } = $props();
  let isPublic = $derived(page.url.pathname === '/login' || page.url.pathname.startsWith('/s/'));

  $effect(() => {
    if (isPublic || session.checked) return;
    api('GET', '/api/auth/me')
      .then((u) => (session.user = u))
      .catch(() => goto('/login?next=' + encodeURIComponent(page.url.pathname + page.url.search)))
      .finally(() => (session.checked = true));
  });

  const links = [
    { href: '/', label: 'Files' },
    { href: '/mounts', label: 'Mounts' },
    { href: '/connections', label: 'Connections' },
    { href: '/shares', label: 'Shares' }
  ];
</script>

{#if isPublic}
  {@render children()}
{:else if session.user}
  <div class="shell">
    <nav>
      <a class="brand" href="/"><img src="/favicon.png" alt="" width="22" height="22" /> Strata</a>
      <div class="links">
        {#each links as l}
          <a href={l.href} class:active={l.href === '/' ? page.url.pathname === '/' : page.url.pathname.startsWith(l.href)}>{l.label}</a>
        {/each}
        {#if session.user.is_admin}
          <a href="/admin" class:active={page.url.pathname.startsWith('/admin')}>Users</a>
        {/if}
      </div>
      <a class="me" href="/settings" title="Settings">{session.user.username}</a>
    </nav>
    <main>{@render children()}</main>
  </div>
{/if}

<style>
  .shell { display: flex; flex-direction: column; height: 100dvh; }
  nav {
    display: flex; align-items: center; gap: 16px; padding: 8px 16px;
    padding-top: max(8px, env(safe-area-inset-top));
    border-bottom: 1px solid var(--border); background: var(--panel);
  }
  .brand { display: flex; align-items: center; gap: 8px; font-weight: 700; color: var(--text); }
  .brand img { border-radius: 6px; }
  .links { display: flex; gap: 4px; overflow-x: auto; flex: 1; scrollbar-width: none; }
  .links a { color: var(--muted); padding: 6px 10px; border-radius: 8px; white-space: nowrap; }
  .links a.active, .links a:hover { color: var(--text); background: var(--hover); }
  .me { color: var(--muted); font-size: .9em; }
  main { flex: 1; min-height: 0; overflow: auto; }
  @media (max-width: 600px) {
    nav { gap: 8px; padding-left: 10px; padding-right: 10px; }
    .brand { font-size: 0; gap: 0; }
  }
</style>
