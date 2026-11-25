// What the recent-files cache keeps: the 50 most recently opened files (of
// at most 20 MB each), within a total size cap, plus whatever is pinned
// ("Keep offline"). Pure policy; `recent.ts` applies it.

export interface Limits {
  /** Unpinned files kept (the most recently opened). */
  maxEntries: number;
  /** Largest unpinned file kept. */
  maxFile: number;
  /** Everything together, pinned files included. */
  maxBytes: number;
}

export const MB = 1024 * 1024;
export const DEFAULTS: Limits = { maxEntries: 50, maxFile: 20 * MB, maxBytes: 500 * MB };

export interface Held {
  key: string;
  size: number;
  /** When it was last opened (ms). */
  used: number;
  pinned: boolean;
}

/** Can a file of this size be kept? `others`: bytes pinned elsewhere, which
 *  eviction can't free. */
export function admits(size: number, pinned: boolean, others: number, l: Limits): boolean {
  if (size < 0 || !Number.isFinite(size)) return false;
  if (!pinned) return size <= l.maxFile && size <= l.maxBytes;
  return others + size <= l.maxBytes;
}

/** The keys to drop, least recently opened first, so that what is left fits
 *  the limits. Pinned files are never dropped. */
export function evictions(held: Held[], l: Limits): string[] {
  let total = held.reduce((a, h) => a + h.size, 0);
  const loose = held.filter((h) => !h.pinned).sort((a, b) => a.used - b.used);
  let count = loose.length;
  const out: string[] = [];
  for (const h of loose) {
    if (count <= l.maxEntries && total <= l.maxBytes && h.size <= l.maxFile) continue;
    out.push(h.key);
    count--;
    total -= h.size;
  }
  return out;
}

/** The limits, with the total cap from settings (MB), if set and sane. */
export function limits(capMb: string | null | undefined): Limits {
  const n = Number(capMb);
  if (!capMb || !Number.isFinite(n) || n < 1) return DEFAULTS;
  return { ...DEFAULTS, maxBytes: Math.floor(n) * MB };
}
