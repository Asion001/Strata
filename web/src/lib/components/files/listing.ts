// Pure helpers for the file list: sorting and multi-selection.
import { SvelteSet } from 'svelte/reactivity';
import type { Entry } from '$lib/api';

export type SortKey = 'name' | 'size' | 'mtime';
export type Sort = { key: SortKey; desc: boolean };

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });

/** Folders (and mounts) first, then by the chosen column; ties by name. */
export function sortEntries(list: Entry[], sort: Sort): Entry[] {
  const dir = sort.desc ? -1 : 1;
  // Each sort value once (big folders), unparseable dates as the oldest so
  // the order stays consistent.
  const keyed = list.map((e) => {
    const t = sort.key === 'mtime' ? Date.parse(e.mtime) : 0;
    return { e, v: sort.key === 'size' ? e.size : sort.key === 'mtime' ? (Number.isNaN(t) ? -Infinity : t) : 0 };
  });
  keyed.sort((a, b) => {
    if (a.e.is_dir !== b.e.is_dir) return a.e.is_dir ? -1 : 1;
    // Ties stay in A-to-Z order whichever way the column runs.
    if (a.v === b.v) return collator.compare(a.e.name, b.e.name) * (sort.key === 'name' ? dir : 1);
    return (a.v < b.v ? -1 : 1) * dir;
  });
  return keyed.map((k) => k.e);
}

/** Clicking a column: the same one flips direction, another starts ascending
 *  (newest first for dates, which is what people want there). */
export function nextSort(cur: Sort, key: SortKey): Sort {
  if (cur.key === key) return { key, desc: !cur.desc };
  return { key, desc: key === 'mtime' };
}

/** Selection by name within one folder, with a shift-click anchor. */
export class Selection {
  // Reactive, so the UI follows every change.
  names = new SvelteSet<string>();
  anchor: string | null = null;

  has(name: string) {
    return this.names.has(name);
  }
  get size() {
    return this.names.size;
  }

  /** Plain toggle (checkbox, Ctrl/Cmd-click). */
  toggle(name: string) {
    if (this.names.has(name)) this.names.delete(name);
    else this.names.add(name);
    this.anchor = name;
  }

  /** Shift-click: everything between the anchor and `name` in view order
   *  gets the anchor's state (selected unless the anchor was deselected). */
  range(name: string, order: string[]) {
    const a = this.anchor === null ? -1 : order.indexOf(this.anchor);
    const b = order.indexOf(name);
    if (a < 0 || b < 0) return this.toggle(name);
    const on = this.names.has(order[a]);
    const [from, to] = a < b ? [a, b] : [b, a];
    for (let i = from; i <= to; i++) {
      if (on) this.names.add(order[i]);
      else this.names.delete(order[i]);
    }
  }

  /** Select all, or clear when everything is already selected. */
  toggleAll(order: string[]) {
    if (order.length && order.every((n) => this.names.has(n))) this.clear();
    else for (const n of order) this.names.add(n);
  }

  clear() {
    this.names.clear();
    this.anchor = null;
  }

  /** Drops names that are no longer listed (deleted, renamed elsewhere). */
  prune(order: string[]) {
    const keep = new Set(order);
    for (const n of [...this.names]) if (!keep.has(n)) this.names.delete(n);
    if (this.anchor !== null && !keep.has(this.anchor)) this.anchor = null;
  }
}

/** Reads a stored preference; storage can be blocked or hold junk. */
export function loadPref<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    return allowed.includes(v as T) ? (v as T) : fallback;
  } catch {
    return fallback;
  }
}

export function savePref(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {}
}
