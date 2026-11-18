<script lang="ts">
  import { confirmDialog, promptDialog } from '$lib/dialogs.svelte';
  import { toast, toastError } from '$lib/toast.svelte';
  import { api, type User } from '$lib/api';
  import { session } from '$lib/session.svelte';
  import { cn } from '$lib/utils';
  import Button from '$lib/components/ui/button/button.svelte';
  import Input from '$lib/components/ui/input/input.svelte';
  import { badgeClass, cardClass, h1Class, h2Class, labelClass, listClass, pageClass, rowClass } from '$lib/components/ui/styles';

  let users = $state<User[]>([]);
  let form = $state({ username: '', password: '', is_admin: false });
  let busy = $state(false);
  const load = async () => {
    try { users = await api('GET', '/api/admin/users'); } catch (e) { toastError(e); }
  };
  $effect(() => { load(); });

  async function create(e: Event) {
    e.preventDefault();
    busy = true;
    try {
      await api('POST', '/api/admin/users', form);
      toast(`${form.username} can sign in now.`, 'success');
      form = { username: '', password: '', is_admin: false };
      await load();
    } catch (err) { toastError(err); } finally { busy = false; }
  }
  async function reset(u: User) {
    const pw = await promptDialog({ title: `New password for ${u.username}`, label: 'Password', type: 'password', confirm: 'Change password', body: 'At least 8 characters.' });
    if (pw) try { await api('POST', `/api/admin/users/${u.id}/password`, { password: pw }); toast('Password changed.', 'success'); } catch (e) { toastError(e); }
  }
  async function del(u: User) {
    const ok = await confirmDialog({ title: `Delete ${u.username}?`, body: 'Their mounts are removed; stored data stays on disk.', confirm: 'Delete user', danger: true });
    if (ok) try { await api('DELETE', `/api/admin/users/${u.id}`); await load(); } catch (e) { toastError(e); }
  }
</script>

<div class={pageClass}>
  <h1 class={cn(h1Class, 'mb-6')}>Users</h1>
  <ul class={listClass}>
    {#each users as u (u.id)}
      <li class={cn(rowClass, 'flex-wrap')}>
        <span class="grid size-8 shrink-0 place-items-center rounded-full bg-accent text-xs font-semibold uppercase">{u.username.slice(0, 2)}</span>
        <span class="min-w-0 flex-1 truncate font-medium">{u.username}</span>
        {#if u.is_admin}<span class={badgeClass}>admin</span>{/if}
        <Button variant="ghost" size="sm" onclick={() => reset(u)}>Reset password</Button>
        {#if u.id !== session.user?.id}<Button variant="ghost" size="sm" class="text-destructive hover:text-destructive" onclick={() => del(u)}>Delete</Button>{/if}
      </li>
    {/each}
  </ul>

  <h2 class={h2Class}>Add user</h2>
  <form class={cn(cardClass, 'grid gap-4 p-4')} onsubmit={create}>
    <label class={labelClass}>Username <Input bind:value={form.username} autocapitalize="none" autocomplete="off" required /></label>
    <label class={labelClass}>Password <Input type="password" bind:value={form.password} minlength={8} autocomplete="new-password" required /></label>
    <label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={form.is_admin} class="size-4 accent-primary" /> Administrator</label>
    <div><Button type="submit" disabled={busy}>Create user</Button></div>
  </form>
</div>
