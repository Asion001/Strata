// Unsaved editor drafts in IndexedDB, so a crash or closed tab loses nothing.
import { clearPositions } from '../media/positions';

export interface Draft {
  key: string;
  /** ETag of the version the draft was based on. */
  baseEtag: string | null;
  /** BLAKE3 (hex) of the bytes the draft was based on. */
  baseHash?: string | null;
  text: string;
  savedAt: number;
  /** The editor instance (tab) that wrote it: a tab only clears its own. */
  tab?: string;
}

const DB = 'strata-editor';
const STORE = 'drafts';

/** The file a draft belongs to: user, mount and path. */
export function draftKey(userId: number, mountId: number | null, path: string): string {
  return `${userId}|${mountId ?? ''}|${path}`;
}

// Each editor tab keeps its own entry per file: `<file key>\u0001<tab>`.
// (\u0001 can't occur in a path: control characters are refused in names.)
const SEP = '\u0001';
const entryKey = (fileKey: string, tab: string) => `${fileKey}${SEP}${tab}`;
const fileRange = (fileKey: string) => IDBKeyRange.bound(`${fileKey}${SEP}`, `${fileKey}\u0002`, false, true);

/** Save this tab's draft of a file. */
export const saveTabDraft = (fileKey: string, tab: string, d: Omit<Draft, 'key' | 'tab'>) =>
  saveDraft({ ...d, key: entryKey(fileKey, tab), tab });

/** Every tab's draft of a file, newest first. */
export async function listDrafts(fileKey: string): Promise<Draft[]> {
  const all = await tx<Draft[]>('readonly', (s) => s.getAll(fileRange(fileKey)) as IDBRequest<Draft[]>);
  return all.sort((a, b) => b.savedAt - a.savedAt);
}

/** Is this tab's draft of a file stored? Only this tab writes its entry,
 *  and only through `saveTabDraft`, so it holds the last text it stored. */
export async function hasTabDraft(fileKey: string, tab: string): Promise<boolean> {
  return (await tx<number>('readonly', (s) => s.count(entryKey(fileKey, tab)))) > 0;
}

// When each tab's draft was last known to belong to an open tab, outside
// IndexedDB (a touch must stay cheap; the record may hold megabytes).
// Expiry counts from the later of this and the draft's edit time.
const ALIVE_KEY = 'strata-draft-alive';

function aliveMap(): Record<string, number> {
  try {
    return JSON.parse(localStorage.getItem(ALIVE_KEY) ?? '{}');
  } catch {
    return {};
  }
}

/** This tab is still open: its draft of a file doesn't expire meanwhile. */
export function touchTabDraft(fileKey: string, tab: string, now = Date.now()) {
  try {
    const m = aliveMap();
    m[entryKey(fileKey, tab)] = now;
    localStorage.setItem(ALIVE_KEY, JSON.stringify(m));
  } catch {}
}

/** Forget this tab's draft of a file. */
export const clearTabDraft = (fileKey: string, tab: string) => clearDraft(entryKey(fileKey, tab));

/** Delete every draft in `range` (all if omitted) for which `pred` holds,
 *  in one transaction. Settles on complete, error and abort alike. */
async function deleteWhere(pred: (d: Draft) => boolean, range?: IDBKeyRange): Promise<void> {
  const db = await open();
  try {
    await new Promise<void>((resolve, reject) => {
      const t = db.transaction(STORE, 'readwrite');
      const req = t.objectStore(STORE).openCursor(range);
      req.onsuccess = () => {
        const c = req.result;
        if (!c) return;
        if (pred(c.value as Draft)) c.delete();
        c.continue();
      };
      t.oncomplete = () => resolve();
      t.onerror = () => reject(t.error);
      t.onabort = () => reject(t.error ?? new Error('draft transaction aborted'));
    });
  } finally {
    db.close();
  }
}

/** After a save: drop this tab's draft and any other tab's draft that holds
 *  exactly the saved text (it is obsolete); different unsaved work stays. */
export const clearAfterSave = (fileKey: string, tab: string, saved: string) =>
  deleteWhere((d) => d.tab === tab || d.text === saved, fileRange(fileKey));

// Offers declined or already restored here, remembered per draft version
// (key + savedAt) outside IndexedDB: another tab's entry is never written or
// deleted from this tab, since it may be that tab's live work.
const SEEN_KEY = 'strata-draft-offers-seen';

function seenMap(): Record<string, { savedAt: number; at: number }> {
  try {
    return JSON.parse(localStorage.getItem(SEEN_KEY) ?? '{}');
  } catch {
    return {};
  }
}

/** Don't offer this draft version again (a newer autosave of it is offered). */
export function markOfferSeen(d: Draft) {
  try {
    const m = seenMap();
    const now = Date.now();
    m[d.key] = { savedAt: d.savedAt, at: now };
    localStorage.setItem(SEEN_KEY, JSON.stringify(m));
  } catch {}
}

export function offerSeen(d: Draft): boolean {
  return (seenMap()[d.key]?.savedAt ?? -1) >= d.savedAt;
}

function open(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    // Schema version 1 forever: a bump would lock out (and silently stop
    // the autosave of) tabs still running an older build.
    const req = indexedDB.open(DB, 1);
    req.onupgradeneeded = () => {
      // No one signed in: the drafts were wiped on sign-out (or never
      // existed). A late writer from another tab must not bring the
      // database back, so it isn't created.
      if (signedInId() === null) {
        req.transaction?.abort();
        return;
      }
      req.result.createObjectStore(STORE, { keyPath: 'key' });
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

async function tx<T>(mode: IDBTransactionMode, fn: (s: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  const db = await open();
  try {
    return await new Promise<T>((resolve, reject) => {
      const t = db.transaction(STORE, mode);
      const req = fn(t.objectStore(STORE));
      t.oncomplete = () => resolve(req.result);
      t.onerror = () => reject(t.error);
      t.onabort = () => reject(t.error);
    });
  } finally {
    db.close();
  }
}

export const saveDraft = (d: Draft) => tx('readwrite', (s) => s.put(d)).then(() => undefined);
export const getDraft = (key: string) =>
  tx<Draft | undefined>('readonly', (s) => s.get(key) as IDBRequest<Draft | undefined>);
export const clearDraft = (key: string) => tx('readwrite', (s) => s.delete(key)).then(() => undefined);


/** Drafts are kept for 30 days at most. */
export const DRAFT_TTL_MS = 30 * 24 * 3600 * 1000;

/** Remove drafts older than the TTL (one transaction, one draft in memory
 *  at a time). */
export async function expireDrafts(now = Date.now()): Promise<void> {
  const alive = aliveMap();
  await deleteWhere((d) => now - Math.max(d.savedAt, alive[d.key] ?? 0) > DRAFT_TTL_MS);
  // Marks of drafts that are gone are pointless: keep both maps small (and
  // a declined offer stays declined for as long as its draft exists).
  const keys = new Set((await tx<IDBValidKey[]>('readonly', (s) => s.getAllKeys())).map(String));
  pruneMap(ALIVE_KEY, keys);
  pruneMap(SEEN_KEY, keys);
}

function pruneMap(name: string, keep: Set<string>) {
  try {
    const m = JSON.parse(localStorage.getItem(name) ?? '{}');
    for (const k of Object.keys(m)) if (!keep.has(k)) delete m[k];
    localStorage.setItem(name, JSON.stringify(m));
  } catch {}
}

/** How often an open tab marks its draft alive: well inside the TTL, so an
 *  open tab's draft never expires. */
export const DRAFT_TOUCH_MS = 60 * 60 * 1000;

// Who is signed in, shared by every tab (localStorage is synchronous and
// cross-tab). Autosave only writes while it names the draft's user.
export const AUTH_KEY = 'strata-signed-in';

/** Recent Move/Copy destinations (folder-picker.ts): private paths. */
const RECENT_FOLDERS = 'strata-recent-folders';

export function markSignedIn(userId: number) {
  try {
    // Another account, signed in without a sign-out first: not its paths.
    const before = localStorage.getItem(AUTH_KEY);
    if (before !== null && before !== String(userId)) localStorage.removeItem(RECENT_FOLDERS);
    localStorage.setItem(AUTH_KEY, String(userId));
  } catch {}
}

export function markSignedOut() {
  try {
    localStorage.removeItem(AUTH_KEY);
    localStorage.removeItem(SEEN_KEY);
    localStorage.removeItem(ALIVE_KEY);
    localStorage.removeItem(RECENT_FOLDERS);
  } catch {}
  // Playback positions name private paths too.
  clearPositions();
}

/** Who is signed in here (null: no one; undefined: no storage at all, so
 *  drafts can't outlive the tab anyway and any user counts). */
function signedInId(): string | null | undefined {
  try {
    return localStorage.getItem(AUTH_KEY);
  } catch {
    return undefined;
  }
}

export function isSignedIn(userId: number): boolean {
  const id = signedInId();
  return id === undefined || id === String(userId);
}

/** Forget every draft in this browser (on sign-out: drafts hold file
 *  contents). Waits for the deletion; gives up after a few seconds if an
 *  open connection somewhere keeps it blocked. */
export function clearAllDrafts(): Promise<void> {
  return new Promise((resolve) => {
    const req = indexedDB.deleteDatabase(DB);
    const timer = setTimeout(resolve, 3000);
    req.onsuccess = req.onerror = () => {
      clearTimeout(timer);
      resolve();
    };
  });
}

/** Drop drafts (and declined-offer marks) of other users, when someone
 *  signs in here. */
export async function keepOnlyUser(userId: number): Promise<void> {
  const mine = (k: string) => k.startsWith(`${userId}|`);
  for (const name of [SEEN_KEY, ALIVE_KEY]) {
    try {
      const m = JSON.parse(localStorage.getItem(name) ?? '{}');
      for (const k of Object.keys(m)) if (!mine(k)) delete m[k];
      localStorage.setItem(name, JSON.stringify(m));
    } catch {}
  }
  const keys = await tx<IDBValidKey[]>('readonly', (s) => s.getAllKeys());
  for (const k of keys) if (!mine(String(k))) await clearDraft(String(k));
}
