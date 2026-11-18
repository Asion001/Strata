<script lang="ts">
  // Desktop navigation: files, share links, the user's mounts, then setup pages.
  import { page } from '$app/state';
  import { cn } from '$lib/utils';
  import { session } from '$lib/session.svelte';
  import { mounts } from '$lib/mounts.svelte';
  import SettingsIcon from '@lucide/svelte/icons/settings';
  import SearchIcon from '@lucide/svelte/icons/search';
  import { openPalette } from '$lib/palette.svelte';
  import { activeMount, folderHref, isActive, kindIcon, mainNav, manageNav, sidebarMounts } from './nav';

  let { class: className = '' }: { class?: string } = $props();

  const current = $derived(page.url.pathname === '/' ? (page.url.searchParams.get('p') ?? '') : null);
  const shown = $derived(sidebarMounts(mounts.list));
  // The root mount is All files, so only the listed mounts can be open.
  const openMount = $derived(current === null ? undefined : activeMount(shown, current));

  function active(href: string): boolean {
    if (href === '/') return current !== null && !openMount;
    return isActive(href, page.url.pathname);
  }

  const mac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform);

  const item =
    'flex h-9 items-center gap-2.5 rounded-md px-2.5 text-sm text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:outline-2 focus-visible:outline-ring';
  const on = 'bg-sidebar-accent font-medium text-sidebar-accent-foreground';
</script>

<aside class={cn('flex w-60 shrink-0 flex-col gap-1 border-r border-sidebar-border bg-sidebar px-3 py-3 pb-[calc(0.75rem+env(safe-area-inset-bottom))] pl-[calc(0.75rem+env(safe-area-inset-left))] text-sidebar-foreground', className)}>
  <a href="/" class="mb-3 flex items-center gap-2 px-2 py-1 text-[15px] font-semibold text-foreground">
    <img src="/favicon.png" alt="" width="24" height="24" class="rounded-md" />
    Strata
  </a>
  <button type="button" onclick={openPalette} class={cn(item, 'mb-2 border border-sidebar-border')} aria-keyshortcuts="Control+K Meta+K">
    <SearchIcon class="size-4 shrink-0" />Search
    <kbd class="ml-auto text-xs">{mac ? '⌘' : 'Ctrl '}K</kbd>
  </button>

  <nav aria-label="Files" class="flex flex-col gap-0.5">
    {#each mainNav as l (l.href)}
      <a href={l.href} class={cn(item, active(l.href) && on)} aria-current={active(l.href) ? 'page' : undefined}>
        <l.icon class="size-4 shrink-0" />{l.label}
      </a>
    {/each}
  </nav>

  {#if shown.length}
    <div class="mt-4 px-2.5 pb-1 text-xs font-medium text-muted-foreground">Mounts</div>
    <nav aria-label="Mounts" class="flex min-h-0 flex-col gap-0.5 overflow-y-auto">
      {#each shown as m (m.id)}
        {@const Icon = kindIcon(m.connection_kind)}
        {@const here = openMount?.id === m.id}
        <a href={folderHref(m.path)} class={cn(item, here && on)} aria-current={here ? 'page' : undefined} title="/{m.path} · {m.connection_name}">
          <Icon class="size-4 shrink-0" />
          <span class="min-w-0 flex-1 truncate">{m.label}</span>
          <span class="font-mono text-[11px] text-muted-foreground">{m.mode}</span>
        </a>
      {/each}
    </nav>
  {/if}

  <div class="mt-auto flex flex-col gap-0.5 pt-4">
    <div class="px-2.5 pb-1 text-xs font-medium text-muted-foreground">Manage</div>
    {#each manageNav.filter((l) => !l.adminOnly || session.user?.is_admin) as l (l.href)}
      <a href={l.href} class={cn(item, active(l.href) && on)} aria-current={active(l.href) ? 'page' : undefined}>
        <l.icon class="size-4 shrink-0" />{l.label}
      </a>
    {/each}
    <a href="/settings" class={cn(item, 'mt-2 border-t border-sidebar-border pt-2 h-11 rounded-none', active('/settings') && 'text-sidebar-accent-foreground')}
      aria-current={active('/settings') ? 'page' : undefined} title="Account settings">
      <span class="grid size-6 shrink-0 place-items-center rounded-full bg-muted text-[11px] font-semibold text-foreground uppercase">
        {session.user?.username.slice(0, 2)}
      </span>
      <span class="min-w-0 flex-1 truncate">{session.user?.username}</span>
      <SettingsIcon class="size-4 shrink-0" />
    </a>
  </div>
</aside>
