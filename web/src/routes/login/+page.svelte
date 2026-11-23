<script lang="ts">
  import { page } from '$app/state';
  import { api } from '$lib/api';
  import { session } from '$lib/session.svelte';
  import { cn } from '$lib/utils';
  import Button from '$lib/components/ui/button/button.svelte';
  import Input from '$lib/components/ui/input/input.svelte';
  import { cardClass, labelClass } from '$lib/components/ui/styles';

  let username = $state('');
  let password = $state('');
  let error = $state('');
  let busy = $state(false);

  async function submit(e: Event) {
    e.preventDefault();
    busy = true;
    error = '';
    try {
      // A sign-out that couldn't reach the server ends that session first.
      const offline = await import('$lib/offline/app.svelte');
      await offline.finishPendingLogout(() => api('POST', '/api/auth/logout'));
      const r = await api('POST', '/api/auth/login', { username, password });
      offline.clearPendingLogout();
      session.user = r.user;
      session.checked = true;
      // Unsaved drafts of anyone else who used this browser go away.
      const { markSignedIn, keepOnlyUser } = await import('$lib/editor/drafts');
      markSignedIn(r.user.id);
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

<div class="grid min-h-dvh place-items-center bg-background p-4 text-[15px] text-foreground">
  <form class={cn(cardClass, 'grid w-full max-w-sm gap-4 p-6 shadow-sm')} onsubmit={submit}>
    <div class="mb-2 flex flex-col items-center gap-3 text-center">
      <img src="/icons/icon-192.png" alt="" width="56" height="56" class="rounded-xl" />
      <h1 class="text-xl font-semibold">Sign in to Strata</h1>
    </div>
    <label class={labelClass}>Username <Input bind:value={username} autocomplete="username" autocapitalize="none" required /></label>
    <label class={labelClass}>Password <Input type="password" bind:value={password} autocomplete="current-password" required /></label>
    {#if error}<p class="text-sm text-destructive" role="alert">{error}</p>{/if}
    <Button type="submit" class="w-full" disabled={busy}>{busy ? 'Signing in…' : 'Sign in'}</Button>
  </form>
</div>
