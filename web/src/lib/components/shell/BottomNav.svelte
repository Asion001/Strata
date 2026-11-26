<script lang="ts">
  // Phone navigation: a tab bar at the bottom, clear of the home indicator.
  import { page } from '$app/state';
  import { cn } from '$lib/utils';
  import FolderIcon from '@lucide/svelte/icons/folder';
  import LinkIcon from '@lucide/svelte/icons/link';
  import FolderTreeIcon from '@lucide/svelte/icons/folder-tree';
  import SettingsIcon from '@lucide/svelte/icons/settings';
  import { isActive } from './nav';

  let { class: className = '' }: { class?: string } = $props();

  // Settings also covers the setup pages it links to on phones.
  const tabs = [
    { href: '/', label: 'Files', icon: FolderIcon, match: ['/'] },
    { href: '/shares', label: 'Shared', icon: LinkIcon, match: ['/shares'] },
    { href: '/mounts', label: 'Mounts', icon: FolderTreeIcon, match: ['/mounts'] },
    { href: '/settings', label: 'Settings', icon: SettingsIcon, match: ['/settings', '/connections', '/admin', '/trash'] }
  ];
</script>

<nav aria-label="Sections"
  class={cn('flex border-t border-border bg-card pb-[env(safe-area-inset-bottom)] pl-[env(safe-area-inset-left)] pr-[env(safe-area-inset-right)]', className)}>
  {#each tabs as t (t.href)}
    {@const on = t.match.some((h) => isActive(h, page.url.pathname))}
    <a href={t.href} aria-current={on ? 'page' : undefined}
      class={cn('flex min-h-14 flex-1 flex-col items-center justify-center gap-0.5 text-[11px] text-muted-foreground', on && 'font-semibold text-foreground')}>
      <t.icon class="size-5" />{t.label}
    </a>
  {/each}
</nav>
