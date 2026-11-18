import type { Component } from 'svelte';
import FolderIcon from '@lucide/svelte/icons/folder';
import LinkIcon from '@lucide/svelte/icons/link';
import FolderTreeIcon from '@lucide/svelte/icons/folder-tree';
import PlugIcon from '@lucide/svelte/icons/plug';
import UsersIcon from '@lucide/svelte/icons/users';
import CloudIcon from '@lucide/svelte/icons/cloud';
import ServerIcon from '@lucide/svelte/icons/server';
import HardDriveIcon from '@lucide/svelte/icons/hard-drive';

export type NavItem = { href: string; label: string; icon: Component; adminOnly?: boolean };

export const mainNav: NavItem[] = [
  { href: '/', label: 'All files', icon: FolderIcon },
  { href: '/shares', label: 'Share links', icon: LinkIcon }
];

export const manageNav: NavItem[] = [
  { href: '/mounts', label: 'Mounts', icon: FolderTreeIcon },
  { href: '/connections', label: 'Connections', icon: PlugIcon },
  { href: '/admin', label: 'Users', icon: UsersIcon, adminOnly: true }
];

/** Icon for a connection kind (s3, ftp, local). */
export function kindIcon(kind: string): Component {
  if (kind === 's3') return CloudIcon;
  if (kind === 'local') return HardDriveIcon;
  return ServerIcon;
}

/**
 * Whether a nav link is the current page. "/" (All files) is current only on
 * the file browser outside a mount; the mount links cover the mounts.
 */
export function isActive(href: string, pathname: string): boolean {
  if (href === '/') return pathname === '/';
  return pathname === href || pathname.startsWith(href + '/');
}

/** Link to a folder in the file browser. */
export function folderHref(path: string): string {
  return `/?p=${encodeURIComponent(path)}`;
}

/** Whether `current` (the browser's ?p=) is `mountPath` or inside it. */
export function inMount(mountPath: string, current: string): boolean {
  return current === mountPath || current.startsWith(mountPath + '/');
}

/** The mount that contains `current`, preferring the deepest (mounts nest). */
export function activeMount<T extends { path: string }>(list: T[], current: string): T | undefined {
  let best: T | undefined;
  for (const m of list) if (inMount(m.path, current) && (!best || m.path.length > best.path.length)) best = m;
  return best;
}
