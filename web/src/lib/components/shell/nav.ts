import type { Component } from 'svelte';
import FolderIcon from '@lucide/svelte/icons/folder';
import LinkIcon from '@lucide/svelte/icons/link';
import FolderTreeIcon from '@lucide/svelte/icons/folder-tree';
import PlugIcon from '@lucide/svelte/icons/plug';
import UsersIcon from '@lucide/svelte/icons/users';
import CloudIcon from '@lucide/svelte/icons/cloud';
import ServerIcon from '@lucide/svelte/icons/server';
import HardDriveIcon from '@lucide/svelte/icons/hard-drive';
import Trash2Icon from '@lucide/svelte/icons/trash-2';

export type NavItem = { href: string; label: string; icon: Component; adminOnly?: boolean; needsTrash?: boolean };

/** Deleted files: shown only while the server keeps a trash. */
export const trashNav: NavItem = { href: '/trash', label: 'Deleted files', icon: Trash2Icon, needsTrash: true };

export const mainNav: NavItem[] = [
  { href: '/', label: 'All files', icon: FolderIcon },
  { href: '/shares', label: 'Share links', icon: LinkIcon },
  trashNav
];

/** The items this user sees: admin pages for admins, Deleted files while
 *  there is a trash. */
export function visibleNav(items: NavItem[], who: { admin: boolean; trash: boolean }): NavItem[] {
  return items.filter((l) => (!l.adminOnly || who.admin) && (!l.needsTrash || who.trash));
}

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

/**
 * Mounts to list in the sidebar, each with a label. The root mount ('') is
 * "All files", so it is left out. A label is the last folder name, or the
 * whole path when two mounts end in the same name.
 */
export function sidebarMounts<T extends { path: string }>(list: T[]): (T & { label: string })[] {
  const shown = list.filter((m) => m.path !== '');
  const last = (p: string) => p.split('/').pop() ?? p;
  const count = new Map<string, number>();
  for (const m of shown) count.set(last(m.path), (count.get(last(m.path)) ?? 0) + 1);
  return shown.map((m) => ({ ...m, label: count.get(last(m.path))! > 1 ? m.path : last(m.path) }));
}

/** Link to a file: its folder, with the file's details open. */
export function fileHref(path: string): string {
  const i = path.lastIndexOf('/');
  return `/?p=${encodeURIComponent(i < 0 ? '' : path.slice(0, i))}&f=${encodeURIComponent(path.slice(i + 1))}`;
}
