<script lang="ts">
  import '../app.css';
  import { page } from '$app/state';
  import { afterNavigate, goto } from '$app/navigation';
  import { api } from '$lib/api';
  import { session } from '$lib/session.svelte';
  import { markSignedIn } from '$lib/editor/drafts';
  import { reloadMounts } from '$lib/mounts.svelte';
  import AppSidebar from '$lib/components/shell/AppSidebar.svelte';
  import BottomNav from '$lib/components/shell/BottomNav.svelte';
  import { cancelAllDialogs, dialogHost } from '$lib/dialogs.svelte';
  import { toaster } from '$lib/toast.svelte';
  import { palette, paletteKey } from '$lib/palette.svelte';
  import { ApiError } from '$lib/api';
  import { applyUpdate, finishPendingLogout, lastUser, net, SIGNED_OUT, signedIn, startQueue, update, watchUpdates } from '$lib/offline/app.svelte';
  import { onMount } from 'svelte';

  let { children } = $props();

  // A question asked on one page means nothing on the next (Back pressed
  // while it was open): withdraw it rather than act on the new page.
  afterNavigate((nav) => {
    if (nav.type !== 'enter') cancelAllDialogs();
  });
  let isPublic = $derived(page.url.pathname === '/login' || page.url.pathname.startsWith('/s/'));

  $effect(() => {
    if (isPublic || session.checked) return;
    // A sign-out that couldn't reach the server ends that session first.
    finishPendingLogout(() => api('POST', '/api/auth/logout')).then((pending) => {
      if (pending !== 'none') {
        session.checked = true;
        return goto('/login');
      }
      return checkSession();
    });
  });
  function checkSession() {
    return api('GET', '/api/auth/me')
      .then((u) => {
        clearTimeout(recheck);
        session.user = u;
        markSignedIn(u.id); // lets the editor's autosave run in every tab
        reloadMounts();
      })
      .catch((e) => {
        // Offline (no answer at all): carry on as whoever was signed in
        // here last, with what is cached. Anything else: sign in again.
        // (A gateway answering for an unreachable server counts as no answer.)
        const unreachable = !(e instanceof ApiError) || e.status >= 502;
        const last = unreachable ? lastUser() : null;
        if (last) {
          // (Unchanged: no reassigning, which would reload every page.)
          if (session.user?.id !== last.id) session.user = last;
          // Check the session for real again (it may have expired): when the
          // network returns, and every half minute meanwhile (a server that
          // was down doesn't change navigator.onLine).
          if (!recheckOnline) {
            recheckOnline = true;
            window.addEventListener('online', () => void checkSession());
          }
          clearTimeout(recheck);
          recheck = setTimeout(() => void checkSession(), 30_000);
        }
        else goto('/login?next=' + encodeURIComponent(page.url.pathname + page.url.search));
      })
      .finally(() => (session.checked = true));
  }

  let recheck: ReturnType<typeof setTimeout> | undefined;
  let recheckOnline = false; // (one listener, however often it fails)
  onMount(() => void watchUpdates());
  // Signed out in another tab: this one leaves too.
  onMount(() => {
    const left = (e: StorageEvent) => {
      if (e.key === SIGNED_OUT && !isPublic) location.href = '/login';
    };
    window.addEventListener('storage', left);
    return () => window.removeEventListener('storage', left);
  });
  // Whoever is signed in (checked here, or just signed in on the sign-in
  // page): offline caches are per user, so first say who, then resume
  // their queued uploads.
  let queueFor: number | null = null;
  $effect(() => {
    const u = session.user;
    if (!u || isPublic || queueFor === u.id) return;
    queueFor = u.id;
    void signedIn(u).then(() => startQueue(u.id, uploadDone));
  });
  // A queued upload finished: the folder it went to refreshes (if shown).
  const uploadDone = (j: { path: string }) => window.dispatchEvent(new CustomEvent('strata-upload-done', { detail: j.path }));

</script>

{#if isPublic}
  {@render children()}
{:else if session.user}
  <div class="flex h-dvh pt-[env(safe-area-inset-top)]">
    <AppSidebar class="hidden md:flex" />
    <div class="flex min-w-0 flex-1 flex-col">
      <main class="min-h-0 flex-1 overflow-auto md:pr-[env(safe-area-inset-right)]">{@render children()}</main>
      <BottomNav class="md:hidden" />
    </div>
  </div>
{/if}

<svelte:window onkeydown={(e) => { if (session.user && !isPublic) paletteKey(e); }} />

{#if update.ready && !isPublic}
  <div role="status" class="fixed inset-x-0 top-[env(safe-area-inset-top)] z-[60] mx-auto mt-2 flex w-fit max-w-[calc(100vw-2rem)] items-center gap-3 rounded-full border border-border bg-popover py-1.5 pr-1.5 pl-4 text-sm shadow-lg">
    New version available
    <button class="rounded-full bg-primary px-3 py-1 font-medium text-primary-foreground" onclick={applyUpdate}>Reload</button>
  </div>
{:else if !net.online && session.user && !isPublic}
  <div role="status" class="fixed inset-x-0 top-[env(safe-area-inset-top)] z-[60] mx-auto mt-2 w-fit rounded-full border border-border bg-popover px-4 py-1.5 text-sm shadow-lg">Offline</div>
{/if}

{#if palette.component && session.user && !isPublic}<palette.component />{/if}
{#if dialogHost.component}<dialogHost.component />{/if}
{#if toaster.component}<toaster.component />{/if}
