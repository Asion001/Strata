<script lang="ts">
  import '../app.css';
  import { page } from '$app/state';
  import { afterNavigate, goto } from '$app/navigation';
  import { api } from '$lib/api';
  import { session } from '$lib/session.svelte';
  import { cn } from '$lib/utils';
  import { markSignedIn } from '$lib/editor/drafts';
  import { reloadMounts } from '$lib/mounts.svelte';
  import AppSidebar from '$lib/components/shell/AppSidebar.svelte';
  import BottomNav from '$lib/components/shell/BottomNav.svelte';
  import { cancelAllDialogs, dialogHost } from '$lib/dialogs.svelte';
  import { toaster } from '$lib/toast.svelte';
  import { palette, paletteKey } from '$lib/palette.svelte';

  let { children } = $props();

  // A question asked on one page means nothing on the next (Back pressed
  // while it was open): withdraw it rather than act on the new page.
  afterNavigate((nav) => {
    if (nav.type !== 'enter') cancelAllDialogs();
  });
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
     were built with (see app.css). Rebuilt pages (the file list so far) drop it. -->
{#if isPublic}
  <div class="legacy contents">{@render children()}</div>
{:else if session.user}
  <div class="flex h-dvh pt-[env(safe-area-inset-top)]">
    <AppSidebar class="hidden md:flex" />
    <div class="flex min-w-0 flex-1 flex-col">
      <main class={cn(page.url.pathname !== '/' && 'legacy', 'min-h-0 flex-1 overflow-auto md:pr-[env(safe-area-inset-right)]')}>{@render children()}</main>
      <BottomNav class="md:hidden" />
    </div>
  </div>
{/if}

<svelte:window onkeydown={(e) => { if (session.user && !isPublic) paletteKey(e); }} />

{#if palette.component && session.user && !isPublic}<palette.component />{/if}
{#if dialogHost.component}<dialogHost.component />{/if}
{#if toaster.component}<toaster.component />{/if}
