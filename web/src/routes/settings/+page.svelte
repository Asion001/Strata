<script lang="ts">
  import { api } from '$lib/api';
  import { session } from '$lib/session.svelte';
  import { cn } from '$lib/utils';
  import { toast, toastError } from '$lib/toast.svelte';
  import Button from '$lib/components/ui/button/button.svelte';
  import Input from '$lib/components/ui/input/input.svelte';
  import { manageNav } from '$lib/components/shell/nav';
  import { cardClass, h1Class, h2Class, labelClass, listClass, mutedClass, pageClass } from '$lib/components/ui/styles';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import LogOutIcon from '@lucide/svelte/icons/log-out';

  let old_password = $state('');
  let new_password = $state('');
  let busy = $state(false);

  async function change(e: Event) {
    e.preventDefault();
    busy = true;
    try {
      await api('POST', '/api/auth/password', { old_password, new_password });
      toast('Password changed.', 'success');
      old_password = new_password = '';
    } catch (err) { toastError(err); } finally { busy = false; }
  }
  async function logout() {
    // Unsaved editor drafts hold file contents: don't leave them behind.
    const { markSignedOut, clearAllDrafts } = await import('$lib/editor/drafts');
    const { signOut } = await import('$lib/offline/app.svelte');
    markSignedOut(); // stops autosave in every tab before the drafts go
    // Queued uploads are cancelled (with their server sessions), then the
    // session ends, then every cached file, listing and queued upload goes
    // (offline too: then only the local part).
    await signOut(session.user?.id ?? null, () => api('POST', '/api/auth/logout'));
    await clearAllDrafts();
    location.href = '/login';
  }
</script>

<div class={pageClass}>
  <h1 class={cn(h1Class, 'mb-6')}>Settings</h1>
  <div class={cn(cardClass, 'flex items-center gap-3 p-4')}>
    <span class="grid size-10 shrink-0 place-items-center rounded-full bg-accent text-sm font-semibold uppercase">{session.user?.username.slice(0, 2)}</span>
    <div class="min-w-0 flex-1">
      <div class="truncate font-medium">{session.user?.username}</div>
      <div class={mutedClass}>{session.user?.is_admin ? 'Administrator' : 'User'}</div>
    </div>
    <Button variant="outline" onclick={logout}><LogOutIcon />Sign out</Button>
  </div>

  <!-- On phones the sidebar is hidden, so setup pages are reached from here. -->
  <nav aria-label="Manage" class="md:hidden">
    <h2 class={h2Class}>Manage</h2>
    <ul class={listClass}>
      {#each manageNav.filter((l) => !l.adminOnly || session.user?.is_admin) as l (l.href)}
        <li><a href={l.href} class="flex items-center gap-3 px-4 py-3"><l.icon class="size-4 text-muted-foreground" /><span class="flex-1">{l.label}</span><ChevronRightIcon class="size-4 text-muted-foreground" /></a></li>
      {/each}
    </ul>
  </nav>

  <h2 class={h2Class}>Change password</h2>
  <form class={cn(cardClass, 'grid gap-4 p-4')} onsubmit={change}>
    <label class={labelClass}>Current password <Input type="password" bind:value={old_password} autocomplete="current-password" required /></label>
    <label class={labelClass}>New password <Input type="password" bind:value={new_password} minlength={8} autocomplete="new-password" required /></label>
    <div><Button type="submit" disabled={busy}>Change password</Button></div>
  </form>

  <h2 class={h2Class}>Install the app</h2>
  <p class={mutedClass}>On iPhone or iPad: open this site in Safari, tap Share, then “Add to Home Screen”. On desktop Chrome or Edge: use the install icon in the address bar.</p>
</div>
