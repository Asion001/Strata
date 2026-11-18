<script lang="ts">
  import '../app.css';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { api } from '$lib/api';
  import { session } from '$lib/session.svelte';
  import { markSignedIn } from '$lib/editor/drafts';
  import { reloadMounts } from '$lib/mounts.svelte';
  import AppSidebar from '$lib/components/shell/AppSidebar.svelte';
  import BottomNav from '$lib/components/shell/BottomNav.svelte';
  import { dialogHost } from '$lib/dialogs.svelte';
  import { toaster } from '$lib/toast.svelte';

  let { children } = $props();
  let isPublic = $derived(page.url.pathname === '/login' || page.url.pathname.startsWith('/s/'));

  $effect(() => {
    if (isPublic || session.checked) return;
    api('GET', '/api/auth/me')
      .then((u) => {
        session.user = u;
        markSignedIn(u.id); // lets the editor's autosave run in every tab
        reloadMounts();
      })
      .catch(() => goto('/login?next=' + encodeURIComponent(page.url.pathname + page.url.search)))
      .finally(() => (session.checked = true));
  });

</script>

<!-- Old pages sit inside .legacy, which turns on the global element styles they
     were built with (see app.css). Rebuilt pages drop it. -->
{#if isPublic}
  <div class="legacy contents">{@render children()}</div>
{:else if session.user}
  <div class="flex h-dvh pt-[env(safe-area-inset-top)]">
    <AppSidebar class="hidden md:flex" />
    <div class="flex min-w-0 flex-1 flex-col">
      <main class="legacy min-h-0 flex-1 overflow-auto md:pr-[env(safe-area-inset-right)]">{@render children()}</main>
      <BottomNav class="md:hidden" />
    </div>
  </div>
{/if}

{#if dialogHost.component}<dialogHost.component />{/if}
{#if toaster.component}<toaster.component />{/if}
