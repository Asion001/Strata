// The rules of the folder picker (Move and Copy), apart from its dialog.

export type PickRules = {
  /** Where the items are now: picking it would do nothing (or collide). */
  from?: string;
  /** Folders being moved: neither they nor anything inside can be picked. */
  exclude?: string[];
};

/** "/a//b/ " → "a/b": the paths the API uses (Home is ""). */
export function clean(p: string) {
  return p
    .trim()
    .split('/')
    .filter((s) => s !== '')
    .join('/');
}

const inside = (p: string, dir: string) => p === dir || p.startsWith(`${dir}/`);

/** Why `target` can't be picked, or null when it can. */
export function whyNot(target: string, rules: PickRules): string | null {
  const t = clean(target);
  const moved = (rules.exclude ?? []).map(clean).find((d) => inside(t, d));
  if (moved !== undefined) return t === moved ? 'This is one of the folders being moved.' : 'This is inside a folder being moved.';
  if (rules.from !== undefined && t === clean(rules.from)) return 'The items are already here.';
  return null;
}

/** Whether a folder can be opened in the picker (a moved folder can't: nothing in it can be picked). */
export function canOpen(target: string, rules: PickRules) {
  const t = clean(target);
  return !(rules.exclude ?? []).some((d) => inside(t, clean(d)));
}

const KEY = 'strata.recentFolders';
const KEEP = 5;

/** The last few folders picked, newest first (this browser only). */
export function recent(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? '[]');
    return Array.isArray(v) ? v.filter((p): p is string => typeof p === 'string').slice(0, KEEP) : [];
  } catch {
    return [];
  }
}

export function remember(path: string) {
  const p = clean(path);
  try {
    localStorage.setItem(KEY, JSON.stringify([p, ...recent().filter((r) => r !== p)].slice(0, KEEP)));
  } catch {
    // (Storage blocked or full: recents are only a convenience.)
  }
}

/** "a/b/c" → crumbs for a breadcrumb, Home first. */
export function crumbs(path: string) {
  const parts = clean(path) ? clean(path).split('/') : [];
  return [{ name: 'Home', path: '' }, ...parts.map((n, i) => ({ name: n, path: parts.slice(0, i + 1).join('/') }))];
}
