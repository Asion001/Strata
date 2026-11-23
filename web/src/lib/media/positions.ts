// Where playback stopped, per file, in this browser (localStorage). Kept
// small: the most recent entries only. Every access tolerates a missing or
// broken store (private mode, quota): it just doesn't remember then.

const KEY = 'strata-media-positions';
const MAX = 200;

type Store = Record<string, { t: number; at: number }>;

function load(): Store {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? '{}');
    return v && typeof v === 'object' ? v : {};
  } catch {
    return {};
  }
}

function save(s: Store) {
  try {
    localStorage.setItem(KEY, JSON.stringify(s));
  } catch {}
}

/** Where to resume `file` (seconds), or 0. Positions in the first or last
 *  few seconds don't count: starting over is what you'd want there. */
export function resumeAt(file: string, duration: number): number {
  const t = load()[file]?.t ?? 0;
  if (!Number.isFinite(t) || !Number.isFinite(duration) || duration <= 0) return 0;
  return t > 5 && t < duration - 5 ? t : 0;
}

/** Remember the position; the oldest entries go beyond MAX. */
export function remember(file: string, t: number, now = Date.now()) {
  if (!Number.isFinite(t) || t < 0) return;
  const s = load();
  s[file] = { t, at: now };
  const keys = Object.keys(s);
  if (keys.length > MAX) {
    keys.sort((a, b) => s[a].at - s[b].at);
    for (const k of keys.slice(0, keys.length - MAX)) delete s[k];
  }
  save(s);
}

/** Played to the end: start from the beginning next time. */
export function forget(file: string) {
  const s = load();
  if (file in s) {
    delete s[file];
    save(s);
  }
}
